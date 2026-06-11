//! AssetBundle 下载与解密模块
//!
//! # 概述
//!
//! 本模块负责从游戏 CDN 下载加密的 AssetBundle，
//! 并使用 XOR 流解密将其还原为 Unity 可直接读取的格式。
//!
//! # 加密机制
//!
//! 游戏使用分层 XOR 加密保护 AssetBundle：
//!
//! 1. **BaseKeys**: 配置文件中固定的 Base64 密钥串（由 ConeShell 算法生成）
//! 2. **Per-Asset Key**: 每个 AssetBundle 在 manifest 中有独立的 64 位整数 key
//! 3. **Keystream 生成**: `byte = BaseKeys[i] XOR per_asset_key_bytes[i % 8]`
//!    每个 BaseKeys 字节被扩展为 8 字节（XOR 上 per-asset key 的 8 个位置）
//! 4. **解密方式**: 文件前 256 字节（0x100）保留不解密（Unity header），
//!    之后的字节与 keystream 逐字节 XOR
//!
//! 对应原 W2AU 中的 `AssetBundleStream` 类。
//!
//! # 下载流程
//!
//! 1. 从 manifest 查找 CDN 的 base URL（ManifestConfig 中的 cdn_base_url）
//! 2. 拼接资源路径 → 完整下载 URL
//! 3. 带自定义 header（X-Version, X-Device 等认证信息）发送 HTTP GET
//! 4. 加密的 AssetBundle → data/downloads/raw/
//! 5. RawAsset（无需解密）→ data/downloads/raw-assets/
//!
//! # 并发控制
//!
//! 批量下载使用 tokio::sync::Semaphore 控制并发数，
//! 已存在的解密文件自动跳过（支持断点续传）。

use anyhow::Context;
use base64::Engine;
use std::io::{Read, Seek, Write};
use std::path::Path;
use tokio::io::AsyncReadExt;

// ============================================================================
// 常量
// ============================================================================

/// AssetBundle header 保留不解密的字节数。
///
/// Unity AssetBundle 的前 256 字节是文件头，
/// 包含类型签名、版本号等元数据，这部分不需要加密。
const HEADER_SKIP_BYTES: u64 = 0x100;

// ============================================================================
// XOR 解密流
// ============================================================================

/// AssetBundle XOR 解密读取器。
///
/// 包装一个加密文件的 reader，透明地对内容做 XOR 解密。
///
/// # 工作原理
///
/// 1. 从配置文件读取 Base64 编码的 `BaseKeys`
/// 2. 将 BaseKeys 展开：每个原始字节 × 8，然后 XOR 上 per-asset key 的 8 个字节
/// 3. 得到完整的 keystream 数组
/// 4. 读取文件时，跳过头 256 字节，后续字节与 keystream 做 XOR
///
/// ```text
/// keystream[i] = BaseKey[floor(i/8)] XOR per_asset_key_bytes[i % 8]
/// ```
pub struct AssetBundleDecryptor<R: Read + Seek> {
    /// 内部 reader（加密文件）
    inner: R,
    /// 解密用的 keystream（从 BaseKeys + per-asset key 生成）
    keystream: Vec<u8>,
    /// 累积读取位置（用于定位 keystream 索引）
    position: u64,
}

impl<R: Read + Seek> AssetBundleDecryptor<R> {
    /// 创建新的解密读取器。
    ///
    /// # 参数
    /// - `inner`: 加密文件的 reader
    /// - `base_keys_b64`: Base64 编码的 BaseKeys 字符串（来自配置文件）
    /// - `asset_key`: manifest 中该资源的 per-asset key
    pub fn new(inner: R, base_keys_b64: &str, asset_key: i64) -> anyhow::Result<Self> {
        // 解码 Base64 → 原始字节
        let base_keys = base64::engine::general_purpose::STANDARD
            .decode(base_keys_b64)
            .context("BaseKeys Base64 解码失败")?;

        // 生成 keystream：每个 BaseKey 字节扩展为 8 字节后 XOR per-asset key
        let asset_key_bytes = asset_key.to_le_bytes();
        let mut keystream = vec![0u8; base_keys.len() * 8];

        for (i, &base_byte) in base_keys.iter().enumerate() {
            let base_offset = i * 8;
            for j in 0..8 {
                keystream[base_offset + j] = base_byte ^ asset_key_bytes[j];
            }
        }

        Ok(Self {
            inner,
            keystream,
            position: 0,
        })
    }

    /// 将加密数据解密到提供的 buffer 中。
    ///
    /// 前 HEADER_SKIP_BYTES 字节直接透传不解密。
    /// 后续字节与 keystream 做 XOR。
    pub fn decrypt_to(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let bytes_read = self.inner.read(buf)?;
        if bytes_read == 0 {
            return Ok(0);
        }

        // 计算当前读取位置（read 之前的位置）
        let read_start = self.position;
        let mut decrypt_offset = 0usize;

        // 如果读取范围与 header 保护区有重叠，调整起始位置
        if read_start < HEADER_SKIP_BYTES {
            let skip_in_buf = (HEADER_SKIP_BYTES - read_start) as usize;
            if skip_in_buf < bytes_read {
                decrypt_offset = skip_in_buf;
            }
        }

        // 对 header 之后的部分做 XOR 解密
        for i in decrypt_offset..bytes_read {
            let abs_pos = read_start + i as u64;
            if abs_pos >= HEADER_SKIP_BYTES {
                let key_idx = (abs_pos as usize) % self.keystream.len();
                buf[i] ^= self.keystream[key_idx];
            }
        }

        self.position += bytes_read as u64;
        Ok(bytes_read)
    }
}

// ============================================================================
// 下载功能
// ============================================================================

/// 单个 AssetBundle 或 RawAsset 的下载结果。
pub struct DownloadResult {
    /// 保存到本地的文件路径
    pub path: String,
    /// 下载的字节数
    pub size: u64,
    /// 是否为 RawAsset（不需要解密）
    pub is_raw: bool,
}

/// 从 CDN 下载单个资源。
///
/// # 参数
/// - `asset_path`: 资源在 manifest 中的路径
/// - `cdn_base`: CDN 基础地址
/// - `dest_dir`: 本地保存目录
/// - `headers`: 自定义 HTTP header（含认证信息）
pub async fn download_asset(
    asset_path: &str,
    cdn_base: &str,
    dest_dir: &Path,
    headers: &std::collections::HashMap<String, String>,
) -> anyhow::Result<DownloadResult> {
    todo!("单个资源下载实现")
}

/// 批量下载并解密 manifest 中的所有资源。
///
/// 使用 tokio Semaphore 控制并发数。
/// 已存在的解密文件自动跳过（断点续传）。
///
/// # 参数
/// - `manifest`: 已解析的资源清单
/// - `cdn_base`: CDN 基础地址
/// - `concurrency`: 最大并发下载数
/// - `dest_downloads`: 加密文件保存目录
/// - `dest_decrypted`: 解密文件保存目录
pub async fn batch_download(
    manifest: &manifest::Manifest,
    cdn_base: &str,
    concurrency: usize,
    dest_downloads: &Path,
    dest_decrypted: &Path,
) -> anyhow::Result<()> {
    todo!("批量下载 + 解密实现")
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    /// 验证 XOR 解密的正确性：
    /// 前 256 字节透传，之后与 keystream 做 XOR
    #[test]
    fn test_decrypt_basic() {
        // 准备测试数据：512 字节 = 256 header + 256 body
        let mut encrypted = vec![0u8; 512];
        // 在 body 部分写入可验证的数据
        for i in 256..512 {
            encrypted[i] = (i as u8) ^ 0x42; // 预先 XOR 上 0x42
        }

        // 用 Base64 编码一个简单的 BaseKey（单字节 0x42 → keystream 全为 0x42）
        // asset_key = 0 → XOR 不做改变，keystream 直接等于 BaseKey 重复
        let base_keys_b64 = base64::engine::general_purpose::STANDARD.encode(&[0x42u8]);

        let reader = Cursor::new(encrypted.clone());
        let mut decryptor =
            AssetBundleDecryptor::new(reader, &base_keys_b64, 0).unwrap();

        let mut decrypted = vec![0u8; 512];
        decryptor.decrypt_to(&mut decrypted).unwrap();

        // header 区域应保持原样
        assert_eq!(decrypted[0..256], encrypted[0..256]);
        // body 区域被 XOR 后应还原为 (i as u8)
        for i in 256..512 {
            assert_eq!(decrypted[i], i as u8, "mismatch at byte {i}");
        }
    }
}
