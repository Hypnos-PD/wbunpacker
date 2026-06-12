//! Wwise 音频提取模块
//!
//! # 概述
//!
//! 从游戏 CDN 下载的 .pck (AKPK 容器) 文件中提取 Wwise WEM 音频，
//! 并通过 vgmstream + ffmpeg 转码为 MP3。
//!
//! # 管线
//!
//! `	ext
//! sound/WwiseIdMapping.bytes  (AES-256-CBC)
//!     │   decrypt_wwise_event_table()
//!     ▼
//!   event_id → event_name 映射
//!
//! sound/Windows/*/*.pck  (AKPK 容器)
//!     │   parse_akpk()  → wem_id → (pck内偏移)
//!     │   extract_wem() → WEM RIFF 数据
//!     ▼
//!   .wem 临时文件
//!     │   wem_to_wav()  (vgmstream)
//!     ▼
//!   .wav 临时文件
//!     │   wav_to_mp3()  (ffmpeg)
//!     ▼
//!   .mp3  ← 按 event_name 组织输出
//! `

use anyhow::Context;
use std::collections::HashMap;
use std::path::Path;


pub mod wwise;

// ============================================================================
// 常量
// ============================================================================

/// AKPK 容器魔数
const AKPK_MAGIC: &[u8; 4] = b"AKPK";
/// BKHD (Wwise Bank Header) 魔数
const BKHD_MAGIC: &[u8; 4] = b"BKHD";
/// RIFF 容器魔数
const RIFF_MAGIC: &[u8; 4] = b"RIFF";
/// WEM 条目在 AKPK 中的大小（字节）
const WEM_ENTRY_SIZE: usize = 20;

// ============================================================================
// 数据结构
// ============================================================================

/// 单条 WEM 提取结果
#[derive(Debug)]
pub struct WemExtractResult {
    pub wem_id: u32,
    pub event_name: Option<String>,
    pub output_path: String,
}

/// 批量提取统计
#[derive(Debug, Default)]
pub struct AudioExtractStats {
    pub pck_files: usize,
    pub wem_extracted: usize,
    pub wem_converted: usize,
    pub failed: usize,
}

// ============================================================================
// AKPK 解析
// ============================================================================

/// 解析 AKPK 文件，提取所有 WEM 条目的 wem_id → (pck内偏移) 映射。
///
/// # 算法
///
/// 1. 验证 AKPK magic
/// 2. 读取 header size（偏移 4，u32 LE）
/// 3. 搜索 BKHD magic
/// 4. 从 BKHD 向前反向扫描 20 字节 WEM 条目
/// 5. 验证条目有效性（wem_id > 0x100000, flag1/flag2 == 1, offset 在合法范围内）
pub fn parse_akpk(data: &[u8]) -> HashMap<u32, u32> {
    if data.len() < 8 || &data[..4] != AKPK_MAGIC {
        return HashMap::new();
    }

    let hdr_size = u32::from_le_bytes([data[4], data[5], data[6], data[7]]) as usize;
    let file_size = data.len();

    let bkhd_pos = data[hdr_size.saturating_sub(4)..(hdr_size + 16).min(file_size)]
        .windows(4)
        .position(|w| w == BKHD_MAGIC)
        .map(|p| hdr_size.saturating_sub(4) + p);

    let bkhd_pos = match bkhd_pos {
        Some(p) => p,
        None => return HashMap::new(),
    };

    let mut pos = bkhd_pos;
    while pos > hdr_size.saturating_sub(200) {
        let v = u32::from_le_bytes([data[pos - 4], data[pos - 3], data[pos - 2], data[pos - 1]]);
        if v != 0 {
            break;
        }
        pos -= 4;
    }

    let mut entries = HashMap::new();
    while pos > 0x54 {
        pos = pos.saturating_sub(WEM_ENTRY_SIZE);
        if pos < 0x54 {
            break;
        }
        if pos + WEM_ENTRY_SIZE > data.len() {
            break;
        }

        let wem_id = u32::from_le_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]]);
        let flag1 = u32::from_le_bytes([data[pos + 4], data[pos + 5], data[pos + 6], data[pos + 7]]);
        let offset = u32::from_le_bytes([data[pos + 12], data[pos + 13], data[pos + 14], data[pos + 15]]);
        let flag2 = u32::from_le_bytes([data[pos + 16], data[pos + 17], data[pos + 18], data[pos + 19]]);

        if wem_id > 0x100000
            && flag1 == 1
            && offset as usize > hdr_size
            && (offset as usize) < file_size
            && flag2 == 1
        {
            entries.insert(wem_id, offset);
        } else {
            break;
        }
    }

    entries
}

// ============================================================================
// WEM 提取
// ============================================================================

/// 从 .pck 文件中提取指定偏移处的 WEM 数据。
///
/// WEM 在 pck 中存储为 RIFF 容器：RIFF { chunk_size, "WAVE", ...sub_chunks }
pub fn extract_wem(data: &[u8], offset: u32) -> Option<&[u8]> {
    let off = offset as usize;
    if off + 8 > data.len() || &data[off..off + 4] != RIFF_MAGIC {
        return None;
    }
    let chunk_size = u32::from_le_bytes([data[off + 4], data[off + 5], data[off + 6], data[off + 7]])
        as usize;
    let end = off + 8 + chunk_size;
    if end > data.len() {
        return None;
    }
    Some(&data[off..end])
}

// ============================================================================
// 转码
// ============================================================================

/// 调用 vgmstream 将 WEM 数据转码为 WAV。
///
/// vgmstream 是一个专门的游戏音频解码库，支持 Wwise WEM、ADPCM、Vorbis 等格式。
pub fn wem_to_wav(
    wem_data: &[u8],
    output: &Path,
    vgmstream_path: &Path,
) -> anyhow::Result<()> {
    let tmp = std::env::temp_dir().join(format!("wbu_{}.wem", std::process::id()));
    std::fs::write(&tmp, wem_data)?;

    let status = std::process::Command::new(vgmstream_path)
        .arg("-o")
        .arg(output)
        .arg(&tmp)
        .status()
        .with_context(|| format!("无法执行 vgmstream: {}", vgmstream_path.display()))?;

    let _ = std::fs::remove_file(&tmp);

    if !status.success() {
        return Err(anyhow::anyhow!("vgmstream 转码失败，退出码: {:?}", status.code()));
    }
    Ok(())
}

/// 调用 ffmpeg 将 WAV 转码为 MP3。
pub fn wav_to_mp3(wav_path: &Path, mp3_path: &Path, ffmpeg_path: &str) -> anyhow::Result<()> {
    let status = std::process::Command::new(ffmpeg_path)
        .args([
            "-y",
            "-i",
            &wav_path.to_string_lossy(),
            "-codec:a",
            "libmp3lame",
            "-b:a",
            "128k",
            &mp3_path.to_string_lossy(),
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .with_context(|| "无法执行 ffmpeg，请确认已安装并在 PATH 中")?;

    if !status.success() {
        return Err(anyhow::anyhow!("ffmpeg 转码失败，退出码: {:?}", status.code()));
    }
    Ok(())
}

// ============================================================================
// 批量提取
// ============================================================================

/// 扫描 pck 目录，提取所有 WEM 并转码为 MP3。
///
/// # 参数
/// - pck_dir: 包含 .pck 文件的目录（递归扫描）
/// - output_dir: MP3 输出根目录
/// - vent_map: Wwise event_id → event_name 映射（来自 WwiseIdMapping.bytes）
/// - gmstream_path: vgmstream-cli.exe 的完整路径
///
/// # 输出结构
/// `	ext
/// {output_dir}/
///     Play_fx_smn_10244120_1.mp3
///     Play_fx_sty_010101_timeshift_rewind_small.mp3
///     ...
///     _unmapped/
///         {wem_id}.mp3  ← 未能匹配到事件名的 WEM
/// `
pub fn extract_all(
    pck_dir: &Path,
    output_dir: &Path,
    event_map: &std::collections::BTreeMap<u32, String>,
    vgmstream_path: &Path,
    ffmpeg_path: &str,
) -> anyhow::Result<AudioExtractStats> {
    std::fs::create_dir_all(output_dir)?;
    let unmapped_dir = output_dir.join("_unmapped");

    let mut stats = AudioExtractStats::default();
    let mut pck_files: Vec<std::path::PathBuf> = Vec::new();

    // 递归扫描 .pck 文件
    for entry in walkdir::WalkDir::new(pck_dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().map(|x| x == "pck").unwrap_or(false))
    {
        pck_files.push(entry.path().to_path_buf());
    }

    stats.pck_files = pck_files.len();
    tracing::info!("扫描到 {} 个 .pck 文件", pck_files.len());

    for pck_path in &pck_files {
        let data = std::fs::read(pck_path)
            .with_context(|| format!("无法读取: {}", pck_path.display()))?;

        let entries = parse_akpk(&data);
        if entries.is_empty() {
            continue;
        }

        for (wem_id, offset) in &entries {
            let wem_data = match extract_wem(&data, *offset) {
                Some(d) => d,
                None => {
                    stats.failed += 1;
                    continue;
                }
            };

            let event_name = event_map.get(wem_id);

            let out_path = if let Some(name) = event_name {
                output_dir.join(format!("{}.mp3", name))
            } else {
                std::fs::create_dir_all(&unmapped_dir)?;
                unmapped_dir.join(format!("{}.mp3", wem_id))
            };

            if out_path.exists() {
                stats.wem_extracted += 1;
                continue;
            }

            // WEM → WAV
            let wav_tmp = std::env::temp_dir().join(format!("wbu_{}.wav", wem_id));
            match wem_to_wav(wem_data, &wav_tmp, vgmstream_path) {
                Ok(()) => {}
                Err(e) => {
                    tracing::error!("WEM→WAV 失败 {}: {e}", wem_id);
                    stats.failed += 1;
                    continue;
                }
            }

            // WAV → MP3
            match wav_to_mp3(&wav_tmp, &out_path, ffmpeg_path) {
                Ok(()) => {
                    stats.wem_converted += 1;
                }
                Err(e) => {
                    tracing::error!("WAV→MP3 失败 {}: {e}", wem_id);
                    stats.failed += 1;
                }
            }

            let _ = std::fs::remove_file(&wav_tmp);
            stats.wem_extracted += 1;
        }
    }

    tracing::info!(
        "音频提取完成: {} pck, {} WEM 提取, {} MP3 转换, {} 失败",
        stats.pck_files,
        stats.wem_extracted,
        stats.wem_converted,
        stats.failed
    );

    Ok(stats)
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_akpk_invalid_magic() {
        let data = b"NOT_AKPK_FILE\x00\x00\x00\x00";
        let entries = parse_akpk(data);
        assert!(entries.is_empty());
    }

    #[test]
    fn test_akpk_magic_value() {
        assert_eq!(AKPK_MAGIC, b"AKPK");
        assert_eq!(BKHD_MAGIC, b"BKHD");
    }
}
