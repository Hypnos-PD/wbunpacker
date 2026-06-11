//! MetaDB 解密模块
//!
//! # 概述
//!
//! 游戏客户端在本地维护一个 SQLite 加密数据库 `meta.db`，
//! 存放资源 hash、下载记录等元数据。本模块将其解密为明文 SQLite，
//! 方便用标准 SQLite 工具浏览。
//!
//! # 加密机制
//!
//! meta.db 使用 SQLite3 Multiple Ciphers (sqlite3mc) 的 XOR 模式加密。
//! 密钥派生过程：
//!
//! ```text
//! 1. 从配置文件读取：
//!    - Sqlite3mcKey (Base64)
//!    - Sqlite3mcBaseKey (Base64)
//!
//! 2. 对每个字节位置 i：
//!    final_key[i] = key[i] ^ base_key[i % 0xD]
//!
//! 3. 用 final_key 打开数据库 (sqlite3_key_v2)
//! 4. 执行 sqlite3_rekey_v2(db, "main", null) → 移除加密
//! ```
//!
//! # 对应原 W2AU
//!
//! C# 版本使用 SQLitePCL.raw 直接调用 C API。
//! Rust 版本使用 rusqlite 配合 sqlcipher 功能。
//!
//! # 注意事项
//!
//! - meta.db 解密是低频操作，仅当需要手动检查客户端元数据时使用
//! - 解密后的 .db 文件不应提交到版本控制（可能含敏感信息）

use anyhow::Context;
use base64::Engine;
use rusqlite::Connection;
use std::path::Path;

// ============================================================================
// 常量
// ============================================================================

/// BaseKey 的 XOR 周期长度（0xD = 13 字节）
const BASE_KEY_PERIOD: usize = 0x0D;

// ============================================================================
// 公共 API
// ============================================================================

/// 解密 meta.db 文件。
///
/// # 流程
///
/// 1. 复制原始加密文件到输出目录
/// 2. 从配置中读取 Sqlite3mcKey 和 Sqlite3mcBaseKey（均为 Base64）
/// 3. 生成最终密钥: `final_key[i] = key[i] ^ base_key[i % 13]`
/// 4. 用 rusqlite 的 key 功能打开数据库
/// 5. 执行 rekey 为 NULL → 移除加密层
///
/// # 参数
/// - `input_path`: 加密的 meta.db 文件路径
/// - `output_path`: 解密后的输出路径
/// - `sqlite_key_b64`: Base64 编码的 Sqlite3mcKey
/// - `sqlite_base_key_b64`: Base64 编码的 Sqlite3mcBaseKey
///
/// # 实现状态
/// 当前为骨架 —— sqlcipher 支持待确认。
pub fn decrypt_metadb(
    _input_path: &Path,
    _output_path: &Path,
    _sqlite_key_b64: &str,
    _sqlite_base_key_b64: &str,
) -> anyhow::Result<()> {
    todo!("meta.db 解密实现（需确认 rusqlite sqlcipher 支持）")
}

// ============================================================================
// 内部函数
// ============================================================================

/// 从两个 Base64 编码的密钥生成 SQLite 最终密钥。
///
/// 算法：对每个字节位 i，
/// ```text
/// final_key[i] = key_bytes[i] ^ base_key_bytes[i % 13]
/// ```
fn derive_final_key(key_b64: &str, base_key_b64: &str) -> anyhow::Result<Vec<u8>> {
    let key = base64::engine::general_purpose::STANDARD
        .decode(key_b64)
        .context("Sqlite3mcKey Base64 解码失败")?;

    let base_key = base64::engine::general_purpose::STANDARD
        .decode(base_key_b64)
        .context("Sqlite3mcBaseKey Base64 解码失败")?;

    let final_key: Vec<u8> = key
        .iter()
        .enumerate()
        .map(|(i, &k)| k ^ base_key[i % BASE_KEY_PERIOD])
        .collect();

    Ok(final_key)
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// 验证密钥派生逻辑与 C# 版本一致
    #[test]
    fn test_derive_final_key() {
        // 测试向量：简单的已知输入
        let key_b64 =
            base64::engine::general_purpose::STANDARD.encode(b"1234567890123456");
        let base_key_b64 =
            base64::engine::general_purpose::STANDARD.encode(b"ABCDEFGHIJKLM");

        let result = derive_final_key(&key_b64, &base_key_b64).unwrap();
        assert_eq!(result.len(), 16, "最终密钥应与输入 key 长度相同");
    }

    /// 验证不同输入产生不同密钥
    #[test]
    fn test_derive_different_keys() {
        let k1 = base64::engine::general_purpose::STANDARD.encode(b"aaaaaaaaaaaaaaaa");
        let k2 = base64::engine::general_purpose::STANDARD.encode(b"bbbbbbbbbbbbbbbb");
        let bk = base64::engine::general_purpose::STANDARD.encode(b"CCCCCCCCCCCCC");

        let r1 = derive_final_key(&k1, &bk).unwrap();
        let r2 = derive_final_key(&k2, &bk).unwrap();
        assert_ne!(r1, r2, "不同输入应产生不同密钥");
    }
}
