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
use std::io::{Read, Seek};
use std::path::Path;

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

/// 下载结果。
#[derive(Debug)]
pub struct DownloadResult {
    pub path: String,
    pub size: u64,
}

/// 批量下载、解密统计。
#[derive(Debug, Default)]
pub struct BatchStats {
    pub done: usize,
    pub skipped: usize,
    pub failed: usize,
    pub downloaded_bytes: u64,
}

/// 根据 manifest 中的 hash 构建 CDN 下载 URL。
///
/// URL 模板占位符: {hash_dir} = hash 前 2 位, {hash} = 完整 hash
pub fn build_download_url(hash: &str, cdn_template: &str) -> String {
    let dir = if hash.len() >= 2 { &hash[..2] } else { hash };
    cdn_template
        .replace("{hash_dir}", dir)
        .replace("{hash}", hash)
}

/// 下载单个加密 AssetBundle 到本地。
pub async fn download_asset(
    hash: &str,
    cdn_template: &str,
    dest_path: &Path,
) -> anyhow::Result<DownloadResult> {
    let url = build_download_url(hash, cdn_template);
    tracing::debug!("下载: {url}");

    let response = reqwest::get(&url)
        .await
        .with_context(|| format!("下载请求失败: {url}"))?;

    if !response.status().is_success() {
        return Err(anyhow::anyhow!(
            "下载失败: HTTP {} — {url}",
            response.status().as_u16()
        ));
    }

    let bytes = response
        .bytes()
        .await
        .with_context(|| "读取响应体失败")?;

    if let Some(parent) = dest_path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("无法创建目录: {}", parent.display()))?;
    }

    std::fs::write(dest_path, &bytes)
        .with_context(|| format!("无法写入文件: {}", dest_path.display()))?;

    Ok(DownloadResult {
        path: dest_path.display().to_string(),
        size: bytes.len() as u64,
    })
}

/// 使用 XOR 流解密加密的 AssetBundle 文件，写入输出路径。
///
/// 前 256 字节透传（Unity header），后续字节按 keystream XOR。
pub fn decrypt_file(
    input_path: &Path,
    output_path: &Path,
    base_keys_b64: &str,
    asset_key: i64,
) -> anyhow::Result<()> {
    let file = std::fs::File::open(input_path)
        .with_context(|| format!("无法打开加密文件: {}", input_path.display()))?;

    let mut decryptor = AssetBundleDecryptor::new(file, base_keys_b64, asset_key)?;

    let mut buffer = [0u8; 8192];
    if let Some(parent) = output_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut out = std::fs::File::create(output_path)?;

    loop {
        let n = decryptor.decrypt_to(&mut buffer)?;
        if n == 0 {
            break;
        }
        std::io::Write::write_all(&mut out, &buffer[..n])?;
    }

    Ok(())
}

/// 批量下载并解密所有 AssetBundle。
///
/// - 已存在的解密文件自动跳过
/// - 使用 Semaphore 控制并发
/// - RawAsset 仅下载不解密
pub async fn batch_download(
    m: &manifest::Manifest,
    cdn_template: &str,
    base_keys_b64: &str,
    concurrency: usize,
    dest_raw: &Path,
    dest_decrypted: &Path,
    dest_raw_assets: &Path,
) -> anyhow::Result<BatchStats> {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, AtomicU64, Ordering};
    use tokio::sync::Semaphore;

    let semaphore = Arc::new(Semaphore::new(concurrency));
    let done = Arc::new(AtomicUsize::new(0));
    let skipped = Arc::new(AtomicUsize::new(0));
    let failed = Arc::new(AtomicUsize::new(0));
    let dl_bytes = Arc::new(AtomicU64::new(0));

    let total = m.assets.len() + m.raw_assets.len();
    tracing::info!(
        "批量处理: {} AssetBundle + {} RawAsset, 共 {} 个, 并发 {}",
        m.assets.len(),
        m.raw_assets.len(),
        total,
        concurrency
    );

    let mut tasks = Vec::with_capacity(total);

    for asset in &m.assets {
        let hash = asset.hash.clone();
        let key = asset.key;
        let name = asset.name.clone();
        let cdn = cdn_template.to_string();
        let b64 = base_keys_b64.to_string();
        let raw_dir = dest_raw.to_path_buf();
        let dec_dir = dest_decrypted.to_path_buf();
        let sem = semaphore.clone();
        let d = done.clone();
        let s = skipped.clone();
        let f = failed.clone();
        let db = dl_bytes.clone();

        tasks.push(tokio::spawn(async move {
            let dec_path = dec_dir.join(&name).with_extension("ab");
            if dec_path.exists() {
                s.fetch_add(1, Ordering::Relaxed);
                return;
            }

            let _permit = sem.acquire().await.unwrap();
            let raw_path = raw_dir.join(&name);

            if !raw_path.exists() {
                match download_asset(&hash, &cdn, &raw_path).await {
                    Ok(r) => { db.fetch_add(r.size, Ordering::Relaxed); }
                    Err(e) => {
                        tracing::error!("下载失败 {}: {e}", name);
                        f.fetch_add(1, Ordering::Relaxed);
                        return;
                    }
                }
            }

            match decrypt_file(&raw_path, &dec_path, &b64, key) {
                Ok(()) => { d.fetch_add(1, Ordering::Relaxed); }
                Err(e) => {
                    tracing::error!("解密失败 {}: {e}", name);
                    f.fetch_add(1, Ordering::Relaxed);
                }
            }
        }));
    }

    for raw in &m.raw_assets {
        let hash = raw.hash.clone();
        let name = raw.name.clone();
        let cdn = cdn_template.to_string();
        let raw_asset_dir = dest_raw_assets.to_path_buf();
        let sem = semaphore.clone();
        let d = done.clone();
        let s = skipped.clone();
        let f = failed.clone();
        let db = dl_bytes.clone();

        tasks.push(tokio::spawn(async move {
            let raw_path = raw_asset_dir.join(&name);
            if raw_path.exists() {
                s.fetch_add(1, Ordering::Relaxed);
                return;
            }

            let _permit = sem.acquire().await.unwrap();

            match download_asset(&hash, &cdn, &raw_path).await {
                Ok(r) => {
                    db.fetch_add(r.size, Ordering::Relaxed);
                    d.fetch_add(1, Ordering::Relaxed);
                }
                Err(e) => {
                    tracing::error!("RawAsset 下载失败 {}: {e}", name);
                    f.fetch_add(1, Ordering::Relaxed);
                }
            }
        }));
    }

    for task in tasks {
        let _ = task.await;
    }

    Ok(BatchStats {
        done: done.load(Ordering::Relaxed),
        skipped: skipped.load(Ordering::Relaxed),
        failed: failed.load(Ordering::Relaxed),
        downloaded_bytes: dl_bytes.load(Ordering::Relaxed),
    })
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
