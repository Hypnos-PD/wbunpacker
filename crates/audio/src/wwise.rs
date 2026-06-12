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

            // 只检查 chunk size 合理性，不再用白名单过滤未知 chunk 类型
            // （Wwise bank 可能包含不在白名单中的 chunk，过滤会导致 bank 被截断）
            if csize == 0 || csize > 0x100_0000 {
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
/// HIRC 对象类型（Wwise 2022, v154）：
/// - 0x02: CAkSound — sourceID (wem_id)
/// - 0x03: CAkAction — idExt (sound_id)
/// - 0x04: CAkEvent — action_id 列表
///
/// 布局均来自对照 wwiser 源码的字段顺序读取，非标记匹配。
///
/// 映射链: wem_id → sound_id → action_id → event_id
pub fn parse_bank_hirc(bank_data: &[u8]) -> BTreeMap<u32, u32> {
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

    let mut pos = hirc_off + 12;
    let mut wem_to_sound: BTreeMap<u32, u32> = BTreeMap::new();
    let mut sound_to_action: BTreeMap<u32, u32> = BTreeMap::new();
    let mut action_to_event: BTreeMap<u32, u32> = BTreeMap::new();

    for _ in 0..count {
        if pos + 9 > bank_data.len() {
            break;
        }

        let obj_type = bank_data[pos];
        let dw_section_size = u32::from_le_bytes([
            bank_data[pos + 1], bank_data[pos + 2],
            bank_data[pos + 3], bank_data[pos + 4],
        ]) as usize;
        let _obj_id = u32::from_le_bytes([
            bank_data[pos + 5], bank_data[pos + 6],
            bank_data[pos + 7], bank_data[pos + 8],
        ]);

        // 类型特定数据: 从 obj_id 之后 (pos+9)，长度 = dwSectionSize - 4
        let data_start = pos + 9;
        let data_len = dw_section_size.saturating_sub(4);
        let data_end = (data_start + data_len).min(bank_data.len());

        match obj_type {
            0x02 => {
                // CAkSound (v154):
                //   [0..4): plugin_id (u32)
                //   [4]:    StreamType (u8)
                //   [5..9): sourceID / wem_id (u32)
                if data_len >= 9 {
                    let wem_id = read_u32_le(&bank_data[data_start + 5..]);
                    wem_to_sound.insert(wem_id, _obj_id);
                }
            }
            0x03 => {
                // CAkAction (v154):
                //   [0..4): ulID / action_id (u32)
                //   [4..6): ulActionType (u16)
                //   [6..10): idExt / sound_id (u32)
                if data_len >= 10 {
                    let action_id = read_u32_le(&bank_data[data_start..]);
                    let sound_id = read_u32_le(&bank_data[data_start + 6..]);
                    sound_to_action.insert(sound_id, action_id);
                }
            }
            0x04 => {
                // CAkEvent (v154):
                //   [0..4): ulID / event_id (u32)
                //   [4..):  var(ulActionListSize) + action_ids (u32 each)
                if data_len >= 4 {
                    let event_id = read_u32_le(&bank_data[data_start..]);
                    // 读取 var 编码的 action 列表长度
                    let (action_count, var_bytes) = read_var_u32(&bank_data[data_start + 4..]);
                    let list_start = data_start + 4 + var_bytes;
                    for i in 0..action_count as usize {
                        let off = list_start + i * 4;
                        if off + 4 <= data_end {
                            let action_id = read_u32_le(&bank_data[off..]);
                            action_to_event.insert(action_id, _obj_id);
                        }
                    }
                }
            }
            _ => {}
        }

        pos = data_end;
    }

    // 组装映射链: wem_id → sound_id → action_id → event_id
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

/// 从字节切片读取 u32 LE，不足 4 字节返回 0。
fn read_u32_le(data: &[u8]) -> u32 {
    if data.len() < 4 {
        return 0;
    }
    u32::from_le_bytes([data[0], data[1], data[2], data[3]])
}

/// Wwise 可变长度整数编码（对照 wwiser TYPE_VAR）：
/// - 读 u8，取低 7 位
/// - 若高位为 1，继续读下一个 u8，value = (value << 7) | (next & 0x7F)
/// 返回 (解码值, 消耗字节数)
fn read_var_u32(data: &[u8]) -> (u32, usize) {
    let mut value = 0u32;
    let mut bytes = 0usize;
    loop {
        if bytes >= data.len() || bytes >= 10 {
            break;
        }
        let cur = data[bytes];
        bytes += 1;
        value = (value << 7) | ((cur & 0x7F) as u32);
        if cur & 0x80 == 0 {
            break;
        }
    }
    (value, bytes)
}

/// 在字节切片中搜索 chunk ID（大端序 4 字节）。
fn find_chunk(data: &[u8], id: u32) -> Option<usize> {
    let be = id.to_be_bytes();
    data.windows(4).position(|w| w == be)
}

/// 完整管线：从多个 pck 文件构建全局 wem_id → event_name 映射。
///
/// 因为 HIRC 的 Sound/Action/Event 对象分散在不同 bank 甚至不同 pck 中，
/// 必须先全局收集所有 bank 的 wem→sound、sound→action、action→event 映射，
/// 再统一关联。
///
/// # 参数
/// - `mapping_data`: WwiseIdMapping.bytes 的原始加密数据
/// - `pck_data_list`: 多个 .pck 文件的 (路径, 数据)
pub fn build_global_wem_mapping(
    mapping_data: &[u8],
    pck_data_list: &[(&std::path::Path, &[u8])],
) -> anyhow::Result<BTreeMap<u32, String>> {
    let event_table = decrypt_wwise_event_table(mapping_data)?;

    // 全局收集
    let mut wem_to_sound: BTreeMap<u32, u32> = BTreeMap::new();
    let mut sound_to_action: BTreeMap<u32, u32> = BTreeMap::new();
    let mut action_to_event: BTreeMap<u32, u32> = BTreeMap::new();

    let mut total_banks = 0usize;
    let mut banks_with_hirc = 0usize;
    for (_pck_path, pck_data) in pck_data_list {
        let banks = extract_banks_from_pck(pck_data);
        total_banks += banks.len();
        for bank in &banks {
            if find_chunk(bank, chunk_id::HIRC).is_some() {
                banks_with_hirc += 1;
            }
            collect_hirc_mappings(
                bank,
                &mut wem_to_sound,
                &mut sound_to_action,
                &mut action_to_event,
            );
        }
    }
    // bank 统计完成

    // 全局关联: wem_id → sound_id → action_id → event_id → event_name
    let mut result = BTreeMap::new();
    for (wem_id, sound_id) in &wem_to_sound {
        if let Some(action_id) = sound_to_action.get(sound_id) {
            if let Some(event_id) = action_to_event.get(action_id) {
                if let Some(name) = event_table.get(event_id) {
                    result.insert(*wem_id, name.clone());
                }
            }
        }
    }

    tracing::info!(
        "全局映射: {} wem→sound, {} sound→action, {} action→event → {} wem→name",
        wem_to_sound.len(),
        sound_to_action.len(),
        action_to_event.len(),
        result.len()
    );

    Ok(result)
}

/// 从单个 bank 提取 HIRC 映射，追加到全局集合。
///
/// 对照 wwiser 解析 Shadowverse WB 实测结果：
///
/// CAkSound (type 0x02):
///   sid      = HIRC ulID
///   wem_id   = type_data[+5..+9] （AkMediaInformation.sourceID，跳过 ulPluginID:u32 + StreamType:u8）
///
/// CAkActionPlay (type 0x03, ulActionType == 0x0403):
///   sid      = HIRC ulID
///   idExt    = type_data[+2..+6] （ActionInitialValues.idExt，跳过 ulActionType:u16）
///
/// CAkEvent (type 0x04):
///   sid      = HIRC ulID
///   type_data 以 var(ulActionListSize) 开头，后跟 ulActionID[]（指向 Action 的 sid）
pub fn collect_hirc_mappings(
    bank_data: &[u8],
    wem_to_sound: &mut BTreeMap<u32, u32>,
    sound_to_action: &mut BTreeMap<u32, u32>,
    action_to_event: &mut BTreeMap<u32, u32>,
) {
    let hirc_off = match find_chunk(bank_data, chunk_id::HIRC) {
        Some(o) => o,
        None => return,
    };

    if hirc_off + 12 > bank_data.len() {
        return;
    }

    let count = u32::from_le_bytes([
        bank_data[hirc_off + 8], bank_data[hirc_off + 9],
        bank_data[hirc_off + 10], bank_data[hirc_off + 11],
    ]) as usize;

    let mut pos = hirc_off + 12;

    for _ in 0..count {
        if pos + 9 > bank_data.len() {
            break;
        }

        let obj_type = bank_data[pos];
        let dw_section_size = u32::from_le_bytes([
            bank_data[pos + 1], bank_data[pos + 2],
            bank_data[pos + 3], bank_data[pos + 4],
        ]) as usize;
        let obj_sid = u32::from_le_bytes([
            bank_data[pos + 5], bank_data[pos + 6],
            bank_data[pos + 7], bank_data[pos + 8],
        ]);

        let data_start = pos + 9;
        let data_len = dw_section_size.saturating_sub(4);
        let data_end = (data_start + data_len).min(bank_data.len());

        match obj_type {
            0x02 => {
                // CAkSound: sourceID(wem_id) 在 +5（跳过 ulPluginID:u32 + StreamType:u8）
                if data_len >= 9 {
                    let wem_id = read_u32_le(&bank_data[data_start + 5..]);
                    wem_to_sound.insert(wem_id, obj_sid);
                }
            }
            0x03 => {
                // CAkAction: ulActionType:u16 在 +0, idExt:u32 在 +2
                // 只取 CAkActionPlay (ulActionType == 0x0403)
                if data_len >= 6 {
                    let action_type = u16::from_le_bytes([
                        bank_data[data_start], bank_data[data_start + 1],
                    ]);
                    if action_type == 0x0403 {
                        let sound_sid = read_u32_le(&bank_data[data_start + 2..]);
                        if sound_sid != 0 {
                            sound_to_action.insert(sound_sid, obj_sid);
                        }
                    }
                }
            }
            0x04 => {
                // CAkEvent: type_data 以 var(ulActionListSize) 开头，后跟 ulActionID[]
                if data_len >= 1 {
                    let (action_count, var_bytes) =
                        read_var_u32(&bank_data[data_start..]);
                    if action_count > 0 {
                        let list_start = data_start + var_bytes;
                        let available = data_end.saturating_sub(list_start) / 4;
                        let limit = action_count.min(available as u32);
                        for i in 0..limit as usize {
                            let off = list_start + i * 4;
                            if off + 4 <= data_end {
                                let action_sid = read_u32_le(&bank_data[off..]);
                                action_to_event.insert(action_sid, obj_sid);
                            }
                        }
                    }
                }
            }
            _ => {}
        }

        pos = data_end;
    }
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

    /// 验证 CAkSound 解析能从真实数据读出 wem_id
    #[test]
    fn test_parse_caksound_wem_id() {
        let pck_path = "D:/WBUnpacker/variants/Chs/raw-assets/sound/Windows/d/English(US)/dx_10001110.pck";
        let pck_data = std::fs::read(pck_path).expect("请先下载 pck 文件");
        let banks = extract_banks_from_pck(&pck_data);
        assert!(!banks.is_empty());

        // 银行里应该有 CAkSound (type 0x02)，wem_id > 0x100000
        let mut found_wem = false;
        for bank in &banks {
            let hirc = find_chunk(bank, chunk_id::HIRC).unwrap();
            let count = u32::from_le_bytes([bank[hirc+8], bank[hirc+9], bank[hirc+10], bank[hirc+11]]) as usize;
            let mut pos = hirc + 12;
            for _ in 0..count {
                let t = bank[pos];
                let len = u32::from_le_bytes([bank[pos+1], bank[pos+2], bank[pos+3], bank[pos+4]]) as usize;
                if t == 0x02 && len >= 13 {
                    let wem_id = u32::from_le_bytes([bank[pos+14], bank[pos+15], bank[pos+16], bank[pos+17]]);
                    assert!(wem_id > 0x100000, "wem_id 应在 Wwise 范围内: {}", wem_id);
                    found_wem = true;
                }
                pos += 9 + len.saturating_sub(4);
            }
        }
        assert!(found_wem, "未找到 CAkSound 的 wem_id");
    }

    /// 用 mx_*.pck（含完整 Sound+Action+Event 链）验证全管线
    #[test]
    fn test_hirc_full_chain() {
        let mapping_path = "D:/WBUnpacker/blobs/raw/CZ/CZ6HQ5ZWKIQP6JOXC6REDJS3VQ";
        let mapping_data = std::fs::read(mapping_path).expect("请先下载 WwiseIdMapping.bytes");

        // mx_M60.pck 含 0x02+0x03+0x04 各 1 个
        let pck_path = "D:/WBUnpacker/variants/Chs/raw-assets/sound/Windows/m/mx_M60.pck";
        let pck_data = std::fs::read(pck_path).expect("请先下载 pck 文件");

        let banks = extract_banks_from_pck(&pck_data);
        assert!(!banks.is_empty(), "应提取出 bank");

        let mut total = 0usize;
        for bank in &banks {
            let wem_to_event = parse_bank_hirc(bank);
            total += wem_to_event.len();
        }
        assert!(total > 0, "应从 mx pck 解析出 wem→event 映射");

        // 完整管线
        let mapping = build_wem_mapping_for_pck(&mapping_data, &pck_data).unwrap();
        assert!(!mapping.is_empty(), "完整管线应产出映射");
        println!("完整管线产出 {} 个 wem→event 映射", mapping.len());
        for (wem_id, name) in mapping.iter().take(3) {
            println!("  {} → {}", wem_id, name);
        }
    }
}
