//! Wwise 事件表解密与 WEM 映射子模块
//!
//! # 概述
//!
//! sound/WwiseIdMapping.bytes 是 Wwise 的事件 ID 到名称的映射表，
//! 使用 AES-256-CBC + PKCS7 加密。
//!
//! # 加密格式
//!
//! `	ext
//! [0x00..0x20]  AES-256 密钥（32 字节）
//! [0x20..0x30]  AES-CBC IV（16 字节）
//! [0x30..EOF]   密文（PKCS7 padded）
//! `
//!
//! # 明文格式
//!
//! `	ext
//! count: u32 LE
//! [event_id: u32 LE, name_len: u32 LE, name: utf-8]{count}
//! `

use anyhow::Context;
use std::collections::BTreeMap;

use aes::cipher::{BlockDecryptMut, KeyIvInit};
type Aes256CbcDec = cbc::Decryptor<aes::Aes256>;

// ============================================================================
// 解密
// ============================================================================

/// 解密 WwiseIdMapping.bytes，返回 event_id → event_name 的映射。
///
/// # 格式
///
/// 密钥和 IV 直接存储在文件头（非标准做法）。
/// 明文是简单的二进制表：u32 计数 + (u32 id, u32 名称长度, UTF-8 名称)*N。
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

    // 解析明文: count (u32 LE) + entries
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
            plain[offset],
            plain[offset + 1],
            plain[offset + 2],
            plain[offset + 3],
        ]);
        let name_len = u32::from_le_bytes([
            plain[offset + 4],
            plain[offset + 5],
            plain[offset + 6],
            plain[offset + 7],
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
        assert!(
            map.len() > 10000,
            "事件数量过少: {}",
            map.len()
        );
        // 验证一个已知的条目
        assert_eq!(
            map.get(&3661058280),
            Some(&"Play_fx_smn_10244120_1".to_string()),
            "已知事件 ID 不匹配"
        );
    }
}
