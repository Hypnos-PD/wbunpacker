//! Wwise 事件表解密与 WEM 映射子模块
//!
//! # 概述
//!
//! 负责两件事：
//! 1. 解密 WwiseIdMapping.bytes → event_id → event_name
//! 2. 从 .pck 提取 SoundBank，解析 HIRC → wem_id → event_id
//!
//! 合并后得到 wem_id → event_name，供音频提取命名使用。
//!
//! # 完整管线
//!
//! ```text
//! WwiseIdMapping.bytes  (AES-256-CBC)
//!     │   decrypt_wwise_event_table()
//!     ▼
//!   event_id → event_name
//!
//! .pck 文件
//!     │   extract_banks_from_pck()  → .bnk 数据
//!     │   parse_bank_hirc()         → wem_id → event_id
//!     ▼
//!   wem_id → event_id
//!
//! 合并:
//!     build_wem_mapping_for_pck()   → wem_id → event_name
//! ```

use anyhow::Context;
use std::collections::BTreeMap;

use aes::cipher::{BlockDecryptMut, KeyIvInit};
type Aes256CbcDec = cbc::Decryptor<aes::Aes256>;

// ============================================================================
// 第一部分: WwiseIdMapping.bytes 解密
// ============================================================================

/// 解密 WwiseIdMapping.bytes，返回 event_id → event_name 的映射。
///
/// # 加密格式
///
/// ```text
/// [0x00..0x20]  AES-256 密钥（32 字节）
/// [0x20..0x30]  AES-CBC IV（16 字节）
/// [0x30..EOF]   密文（PKCS7 padded）
/// ```
///
/// # 明文格式
///
/// ```text
/// count: u32 LE
/// [event_id: u32 LE, name_len: u32 LE, name: utf-8]{count}
/// ```
pub fn decrypt_wwise_event_table(data: &[u8]) -> anyhow::Result<BTreeMap<u32, String>> {
    if data.len() < 0x30 {
        return Err(anyhow::anyhow!(
            "WwiseIdMapping.bytes 太短: {} 字节（需要至少 0x30）",
            data.len()
        ));
    }

    let key: &[u8; 32] = data[0x00..0x20]
        .try_into()
        .context("密钥切片失败")?;
    let iv: &[u8; 16] = data[0x20..0x30]
        .try_into()
        .context("IV 切片失败")?;
    let ciphertext = &data[0x30..];

    let cipher = Aes256CbcDec::new(key.into(), iv.into());
    let mut buf = ciphertext.to_vec();
    let plain = cipher
        .decrypt_padded_mut::<aes::cipher::block_padding::Pkcs7>(&mut buf)
        .map_err(|e| anyhow::anyhow!("AES-CBC 解密失败: {e}"))?;

    if plain.len() < 4 {
        return Err(anyhow::anyhow!("解密后数据太短: {} 字节", plain.len()));
    }

    let count = u32::from_le_bytes([plain[0], plain[1], plain[2], plain[3]]) as usize;
    let mut map = BTreeMap::new();
    let mut offset = 4usize;

    for _ in 0..count {
        if offset + 8 > plain.len() {
            break;
        }
        let event_id = u32::from_le_bytes([
            plain[offset], plain[offset + 1], plain[offset + 2], plain[offset + 3],
        ]);
        let name_len = u32::from_le_bytes([
            plain[offset + 4], plain[offset + 5], plain[offset + 6], plain[offset + 7],
        ]) as usize;
        offset += 8;

        if offset + name_len > plain.len() {
            break;
        }
        let name = String::from_utf8_lossy(&plain[offset..offset + name_len]).into_owned();
        offset += name_len;

        map.insert(event_id, name);
    }

    tracing::info!("Wwise 事件表解密完成: {} 个事件", map.len());
    Ok(map)
}

// ============================================================================
// 第二部分: SoundBank 提取与 HIRC 解析
// ============================================================================

/// Wwise SoundBank chunk ID（大端序）。对照 W2AU Python 版 `CHUNK_IDS`。
mod chunk_id {
    pub const BKHD: u32 = 0x424B4844;
    pub const DIDX: u32 = 0x44494458;
    pub const DATA: u32 = 0x44415441;
    pub const HIRC: u32 = 0x48495243;
    pub const STID: u32 = 0x53544944;

    pub const ALL: &[u32] = &[
        0x424B4844, // BKHD
        0x44494458, // DIDX
        0x44415441, // DATA
        0x48495243, // HIRC
        0x53544944, // STID
        0x46585052, // FXPR
        0x454E5653, // ENVS
        0x53544D47, // STMG
        0x504C4154, // PLAT
        0x494E4954, // INIT
    ];
}

/// 从 .pck 数据中提取所有独立 SoundBank（.bnk）的原始字节。
///
/// 算法（对照 W2AU `extract_banks`）：
/// 1. 搜索 "BKHD" 标记
/// 2. 验证 BKHD size ≤ 0x10000
/// 3. 从 BKHD 起顺序遍历 chunk，遇到下一个 BKHD 或 STID 停止
/// 4. 累计 total_size，切出完整 bank
pub fn extract_banks_from_pck(pck_data: &[u8]) -> Vec<Vec<u8>> {
    let limit = pck_data.len();
    let mut banks: Vec<Vec<u8>> = Vec::new();
    let mut pos = 0usize;

    while pos < limit {
        // 1. 搜索 BKHD
        let idx = match pck_data[pos..].windows(4).position(|w| w == b"BKHD") {
            Some(i) => pos + i,
            None => break,
        };

        if idx + 16 > limit {
            pos = idx + 4;
            continue;
        }

        let size_field = u32::from_le_bytes([
            pck_data[idx + 4], pck_data[idx + 5], pck_data[idx + 6], pck_data[idx + 7],
        ]) as usize;

        // 2. BKHD size 合理性检查
        if size_field > 0x10000 {
            pos = idx + 4;
            continue;
        }

        // 3. 遍历后续 chunk
        let mut total = 0usize;
        let mut first_block = true;
        let mut sub = idx;

        while sub + 8 <= limit {
            let cid = u32::from_be_bytes([
                pck_data[sub], pck_data[sub + 1], pck_data[sub + 2], pck_data[sub + 3],
            ]);
            let csize = u32::from_le_bytes([
                pck_data[sub + 4], pck_data[sub + 5], pck_data[sub + 6], pck_data[sub + 7],
            ]) as usize;

            if !first_block && (cid == chunk_id::BKHD || cid == chunk_id::STID) {
                break;
            }
            first_block = false;

            if !chunk_id::ALL.contains(&cid) {
                break;
            }

            total += csize + 8;
            sub += csize + 8;
        }

        // 4. 切出 bank
        if total > 0 && idx + total <= limit {
            banks.push(pck_data[idx..idx + total].to_vec());
        }

        pos = idx + total.max(4);
    }

    banks
}

/// 解析 bank 的 HIRC 段，构建 wem_id → event_id 映射链。
///
/// HIRC 对象类型（Wwise 2022）：
/// - 0x02: CAkSound — AkMediaInformation.tid = wem_id
/// - 0x03: CAkEvent — Action.tid = action_id
/// - 0x04: CAkActionPlay — idExt = sound_id
///
/// 映射链: wem_id → sound_id → action_id → event_id
pub fn parse_bank_hirc(bank_data: &[u8]) -> BTreeMap<u32, u32> {
    // 查找 HIRC chunk
    let hirc_off = match find_chunk(bank_data, chunk_id::HIRC) {
        Some(o) => o,
        None => return BTreeMap::new(),
    };

    if hirc_off + 12 > bank_data.len() {
        return BTreeMap::new();
    }

    let count = u32::from_le_bytes([
        bank_data[hirc_off + 8], bank_data[hirc_off + 9],
        bank_data[hirc_off + 10], bank_data[hirc_off + 11],
    ]) as usize;

    let mut offset = hirc_off + 12;
    let mut wem_to_sound: BTreeMap<u32, u32> = BTreeMap::new();
    let mut sound_to_action: BTreeMap<u32, u32> = BTreeMap::new();
    let mut action_to_event: BTreeMap<u32, u32> = BTreeMap::new();

    for _ in 0..count {
        if offset + 5 > bank_data.len() {
            break;
        }

        let obj_type = bank_data[offset];
        let obj_len = u32::from_le_bytes([
            bank_data[offset + 1], bank_data[offset + 2],
            bank_data[offset + 3], bank_data[offset + 4],
        ]) as usize;

        let obj_id = if offset + 9 <= bank_data.len() {
            u32::from_le_bytes([
                bank_data[offset + 5], bank_data[offset + 6],
                bank_data[offset + 7], bank_data[offset + 8],
            ])
        } else {
            break;
        };

        let data_start = offset + 9;
        let data_end = (data_start + obj_len.saturating_sub(4)).min(bank_data.len());

        match obj_type {
            0x02 => {
                // CAkSound: 搜索 AkMediaInformation → tid (wem_id)
                if let Some(wem_id) = find_nested_u32_at(&bank_data[data_start..data_end], b"tid ") {
                    wem_to_sound.insert(wem_id, obj_id);
                }
            }
            0x03 => {
                // CAkEvent: 搜索 Action → tid (action_id)
                if let Some(action_id) = find_nested_u32_at(&bank_data[data_start..data_end], b"tid ") {
                    action_to_event.insert(action_id, obj_id);
                }
            }
            0x04 => {
                // CAkActionPlay: 搜索 idExt (sound_id)
                if let Some(sound_id) = find_nested_u32_at(&bank_data[data_start..data_end], b"idEx") {
                    sound_to_action.insert(sound_id, obj_id);
                }
            }
            _ => {}
        }

        offset = data_end;
    }

    // 组装: wem_id → sound_id → action_id → event_id
    let mut result = BTreeMap::new();
    for (wem_id, sound_id) in &wem_to_sound {
        if let Some(action_id) = sound_to_action.get(sound_id) {
            if let Some(event_id) = action_to_event.get(action_id) {
                result.insert(*wem_id, *event_id);
            }
        }
    }

    result
}

/// 在 data 中搜索 marker 后紧跟的 u32 LE 值。
///
/// wwiser 的二进制格式中，字段由 type(1) + size(2) + data 组成。
/// 这里简化为裸字节搜索，对我们需要的那几个字段足够。
fn find_nested_u32_at(data: &[u8], marker: &[u8]) -> Option<u32> {
    data.windows(marker.len() + 4)
        .find(|w| &w[..marker.len()] == marker)
        .map(|w| {
            u32::from_le_bytes([
                w[marker.len()], w[marker.len() + 1],
                w[marker.len() + 2], w[marker.len() + 3],
            ])
        })
}

/// 在字节切片中搜索 chunk ID（大端序 4 字节）。
fn find_chunk(data: &[u8], id: u32) -> Option<usize> {
    let be = id.to_be_bytes();
    data.windows(4).position(|w| w == be)
}

/// 完整管线：解密事件表 → 从 pck 提取 bank → 解析 HIRC → wem_id → event_name。
///
/// # 参数
/// - `mapping_data`: WwiseIdMapping.bytes 的原始加密数据
/// - `pck_data`: 单个 .pck 文件内容
pub fn build_wem_mapping_for_pck(
    mapping_data: &[u8],
    pck_data: &[u8],
) -> anyhow::Result<BTreeMap<u32, String>> {
    let event_table = decrypt_wwise_event_table(mapping_data)?;
    let banks = extract_banks_from_pck(pck_data);

    let mut result = BTreeMap::new();
    for bank in &banks {
        let wem_to_event = parse_bank_hirc(bank);
        for (wem_id, event_id) in &wem_to_event {
            if let Some(name) = event_table.get(event_id) {
                result.insert(*wem_id, name.clone());
            }
        }
    }

    Ok(result)
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// 用 Python 验证过的真实数据做回归测试
    #[test]
    fn test_decrypt_wwise_mapping() {
        let path = "D:/WBUnpacker/blobs/raw/CZ/CZ6HQ5ZWKIQP6JOXC6REDJS3VQ";
        let data = std::fs::read(path).expect("请先下载 WwiseIdMapping.bytes");
        let map = decrypt_wwise_event_table(&data).expect("解密失败");
        assert!(map.len() > 10000, "事件数量过少: {}", map.len());
        // 验证一个已知条目
        assert_eq!(
            map.get(&3661058280),
            Some(&"Play_fx_smn_10244120_1".to_string()),
            "已知事件 ID 不匹配"
        );
    }
}