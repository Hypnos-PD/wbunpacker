//! AssetBundle 下载与解密模块
//!
//! # 概述
//!
//! 本模块负责从游戏 CDN 下载加密的 AssetBundle，
//! 并使用 XOR 流解密将其还原为 Unity 可直接读取的格式。
//!
//! # 存储架构
//!
//! 采用 blob + 硬链接 双层存储：
//!
//! `	ext
//! blobs/raw/{hash[..2]}/{hash}          ← 下载的加密文件（按 hash 存，天然去重）
//! blobs/decrypted/{hash[..2]}/{hash}     ← 解密后的文件
//!
//! variants/{variant}/raw/{name}           ← ──硬链接→ blobs/raw/...
//! variants/{variant}/decrypted/{name}.ab  ← ──硬链接→ blobs/decrypted/...
//! variants/{variant}/raw-assets/{name}    ← ──硬链接→ blobs/raw/...
//! `
//!
//! 游戏更新时同名文件 hash/checksum 变化 → 新 blob 自动下载，
//! 旧 blob 保留不删，硬链接自动更新指向新 blob。
//!
//! # 跳过策略
//!
//! AssetBundle: size 快速预筛 → CRC64 精确校验 → 一致才跳过
//! RawAsset（无 checksum 字段）: size 比对

use anyhow::Context;
use base64::Engine;
use std::io::{Read, Seek};
use std::path::Path;

// ============================================================================
// 常量
// ============================================================================

const HEADER_SKIP_BYTES: u64 = 0x100;

// ============================================================================
// XOR 解密流
// ============================================================================

pub struct AssetBundleDecryptor<R: Read + Seek> {
    inner: R,
    keystream: Vec<u8>,
    position: u64,
}

impl<R: Read + Seek> AssetBundleDecryptor<R> {
    pub fn new(inner: R, base_keys_b64: &str, asset_key: i64) -> anyhow::Result<Self> {
        let base_keys = base64::engine::general_purpose::STANDARD
            .decode(base_keys_b64)
            .context("BaseKeys Base64 解码失败")?;

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

    pub fn decrypt_to(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let bytes_read = self.inner.read(buf)?;
        if bytes_read == 0 {
            return Ok(0);
        }

        let read_start = self.position;
        let mut decrypt_offset = 0usize;

        if read_start < HEADER_SKIP_BYTES {
            let skip_in_buf = (HEADER_SKIP_BYTES - read_start) as usize;
            if skip_in_buf < bytes_read {
                decrypt_offset = skip_in_buf;
            }
        }

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
// 数据结构
// ============================================================================

#[derive(Debug)]
pub struct DownloadResult {
    pub path: String,
    pub size: u64,
}

#[derive(Debug, Default)]
pub struct BatchStats {
    pub done: usize,
    pub skipped: usize,
    pub failed: usize,
    pub downloaded_bytes: u64,
    pub hardlinks: usize,
}

// ============================================================================
// 工具函数
// ============================================================================

/// 根据 hash 构建 CDN 下载 URL
pub fn build_download_url(hash: &str, cdn_template: &str) -> String {
    let dir = if hash.len() >= 2 { &hash[..2] } else { hash };
    cdn_template
        .replace("{hash_dir}", dir)
        .replace("{hash}", hash)
}

/// blob 存储路径: blobs/{category}/{hash[..2]}/{hash}
pub fn blob_path(blobs_dir: &Path, category: &str, hash: &str) -> std::path::PathBuf {
    let dir = if hash.len() >= 2 { &hash[..2] } else { hash };
    blobs_dir.join(category).join(dir).join(hash)
}

/// 创建硬链接。
///
/// - 目标不存在 → 创建链接，返回 	rue
/// - 目标存在且文件大小与源一致 → 跳过，返回 alse
/// - 目标存在但大小不一致（游戏更新导致内容变化）→ 删除旧链接后重建，返回 	rue
pub fn hardlink_or_skip(src: &Path, dst: &Path) -> std::io::Result<bool> {
    if dst.exists() {
        if let (Ok(s_meta), Ok(d_meta)) = (std::fs::metadata(src), std::fs::metadata(dst)) {
            if s_meta.len() == d_meta.len() {
                return Ok(false);
            }
        }
        std::fs::remove_file(dst)?;
    }
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::hard_link(src, dst)?;
    Ok(true)
}

/// 计算文件 CRC-64/ECMA-182 校验和
fn crc64_file(path: &Path) -> std::io::Result<u64> {
    use crc::{CRC_64_ECMA_182, Crc};
    let crc64 = Crc::<u64>::new(&CRC_64_ECMA_182);
    let mut digest = crc64.digest();
    let mut file = std::fs::File::open(path)?;
    let mut buf = [0u8; 65536];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        digest.update(&buf[..n]);
    }
    Ok(digest.finalize())
}

// ============================================================================
// 下载与解密
// ============================================================================

/// 下载单个加密 AssetBundle 到指定路径，若已存在则跳过
pub async fn download_asset(
    hash: &str,
    cdn_template: &str,
    dest_path: &Path,
) -> anyhow::Result<DownloadResult> {
    if dest_path.exists() {
        let size = std::fs::metadata(dest_path).map(|m| m.len()).unwrap_or(0);
        return Ok(DownloadResult {
            path: dest_path.display().to_string(),
            size,
        });
    }

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

    let bytes = response.bytes().await.context("读取响应体失败")?;

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

/// 解密加密的 AssetBundle，输出到指定路径
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

// ============================================================================
// 批量下载
// ============================================================================

/// 批量下载并解密所有 AssetBundle。
///
/// 流程：
/// 1. 下载到 blobs/raw/{hash[..2]}/{hash}
/// 2. 硬链接到 variants/{variant}/raw/{name}
/// 3. 解密到 blobs/decrypted/{hash[..2]}/{hash}
/// 4. 硬链接到 variants/{variant}/decrypted/{name}.ab
/// 5. RawAsset: 下载到 blob → 硬链接
///
/// AssetBundle 跳过判断: size 快速预筛 → CRC64 精确校验。
/// RawAsset（无 checksum）: size 比对。
pub async fn batch_download(
    m: &manifest::Manifest,
    cdn_template: &str,
    base_keys_b64: &str,
    concurrency: usize,
    blobs_dir: &Path,
    variant_dir: &Path,
) -> anyhow::Result<BatchStats> {
    use indicatif::{ProgressBar, ProgressStyle};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
    use tokio::sync::Semaphore;

    let semaphore = Arc::new(Semaphore::new(concurrency));
    let done = Arc::new(AtomicUsize::new(0));
    let skipped = Arc::new(AtomicUsize::new(0));
    let failed = Arc::new(AtomicUsize::new(0));
    let dl_bytes = Arc::new(AtomicU64::new(0));
    let hardlinks = Arc::new(AtomicUsize::new(0));

    let total = m.assets.len() + m.raw_assets.len();
    let pb = ProgressBar::new(total as u64);
    pb.set_style(
        ProgressStyle::with_template(
            "{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} ({eta}) {msg}",
        )
        .unwrap()
        .progress_chars("##-"),
    );

    tracing::info!(
        "批量处理: {} AssetBundle + {} RawAsset, 共 {} 个, 并发 {}",
        m.assets.len(),
        m.raw_assets.len(),
        total,
        concurrency
    );

    let blobs_raw = blobs_dir.join("raw");
    let blobs_decrypted = blobs_dir.join("decrypted");

    let mut tasks = Vec::with_capacity(total);

    // AssetBundle: 下载 → 硬链接 raw → 解密 → 硬链接 decrypted
    for asset in &m.assets {
        let hash = asset.hash.clone();
        let key = asset.key;
        let name = asset.name.clone();
        let asset_size = asset.size as u64;
        let checksum = asset.checksum;
        let cdn = cdn_template.to_string();
        let b64 = base_keys_b64.to_string();
        let blob_raw_path = blob_path(&blobs_raw, "", &hash);
        let blob_dec_path = blob_path(&blobs_decrypted, "", &hash);
        let link_raw = variant_dir.join("raw").join(&name);
        let link_dec = variant_dir
            .join("decrypted")
            .join(&name)
            .with_extension("ab");
        let sem = semaphore.clone();
        let d = done.clone();
        let s = skipped.clone();
        let f = failed.clone();
        let db = dl_bytes.clone();
        let hl = hardlinks.clone();
        let pb = pb.clone();

        tasks.push(tokio::spawn(async move {
            // 跳过判断：size 预筛 + CRC64 精确校验
            if link_dec.exists()
                && std::fs::metadata(&link_dec).map(|m| m.len()).unwrap_or(0) == asset_size
            {
                if crc64_file(&link_dec).ok() == Some(checksum) {
                    s.fetch_add(1, Ordering::Relaxed);
                    pb.inc(1);
                    return;
                }
            }

            let _permit = sem.acquire().await.unwrap();
            pb.set_message(name.clone());

            if !blob_raw_path.exists() {
                match download_asset(&hash, &cdn, &blob_raw_path).await {
                    Ok(r) => {
                        db.fetch_add(r.size, Ordering::Relaxed);
                    }
                    Err(e) => {
                        tracing::error!("下载失败 {}: {e}", name);
                        f.fetch_add(1, Ordering::Relaxed);
                        pb.inc(1);
                        return;
                    }
                }
            }

            if let Err(e) = hardlink_or_skip(&blob_raw_path, &link_raw) {
                tracing::error!("硬链接失败 raw: {} — {e}", name);
            }

            if !blob_dec_path.exists() {
                match decrypt_file(&blob_raw_path, &blob_dec_path, &b64, key) {
                    Ok(()) => {}
                    Err(e) => {
                        tracing::error!("解密失败 {}: {e}", name);
                        f.fetch_add(1, Ordering::Relaxed);
                        pb.inc(1);
                        return;
                    }
                }
            }

            let _ = hardlink_or_skip(&blob_dec_path, &link_dec);

            d.fetch_add(1, Ordering::Relaxed);
            pb.inc(1);
        }));
    }

    // RawAsset: 下载到 blob → 硬链接（无 checksum，仅 size 比对）
    for raw in &m.raw_assets {
        let hash = raw.hash.clone();
        let name = raw.name.clone();
        let raw_size = raw.size as u64;
        let cdn = cdn_template.to_string();
        let blob_raw_path = blob_path(&blobs_raw, "", &hash);
        let link_raw_asset = variant_dir.join("raw-assets").join(&name);
        let sem = semaphore.clone();
        let d = done.clone();
        let s = skipped.clone();
        let f = failed.clone();
        let db = dl_bytes.clone();
        let hl = hardlinks.clone();
        let pb = pb.clone();

        tasks.push(tokio::spawn(async move {
            if link_raw_asset.exists()
                && std::fs::metadata(&link_raw_asset)
                    .map(|m| m.len())
                    .unwrap_or(0)
                    == raw_size
            {
                s.fetch_add(1, Ordering::Relaxed);
                pb.inc(1);
                return;
            }

            let _permit = sem.acquire().await.unwrap();
            pb.set_message(name.clone());

            if !blob_raw_path.exists() {
                match download_asset(&hash, &cdn, &blob_raw_path).await {
                    Ok(r) => {
                        db.fetch_add(r.size, Ordering::Relaxed);
                    }
                    Err(e) => {
                        tracing::error!("RawAsset 下载失败 {}: {e}", name);
                        f.fetch_add(1, Ordering::Relaxed);
                        pb.inc(1);
                        return;
                    }
                }
            }

            let _ = hardlink_or_skip(&blob_raw_path, &link_raw_asset);

            d.fetch_add(1, Ordering::Relaxed);
            pb.inc(1);
        }));
    }

    for task in tasks {
        let _ = task.await;
    }
    pb.finish_and_clear();

    Ok(BatchStats {
        done: done.load(Ordering::Relaxed),
        skipped: skipped.load(Ordering::Relaxed),
        failed: failed.load(Ordering::Relaxed),
        downloaded_bytes: dl_bytes.load(Ordering::Relaxed),
        hardlinks: hardlinks.load(Ordering::Relaxed),
    })
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn test_decrypt_basic() {
        let mut encrypted = vec![0u8; 512];
        for i in 256..512 {
            encrypted[i] = (i as u8) ^ 0x42;
        }

        let base_keys_b64 = base64::engine::general_purpose::STANDARD.encode(&[0x42u8]);
        let reader = Cursor::new(encrypted.clone());
        let mut decryptor = AssetBundleDecryptor::new(reader, &base_keys_b64, 0).unwrap();

        let mut decrypted = vec![0u8; 512];
        decryptor.decrypt_to(&mut decrypted).unwrap();

        assert_eq!(decrypted[0..256], encrypted[0..256]);
        for i in 256..512 {
            assert_eq!(decrypted[i], i as u8, "mismatch at byte {i}");
        }
    }
}
