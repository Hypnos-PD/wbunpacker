//! Wwise 音频提取与转码模块
//!
//! 从 .pck (AKPK 容器) 中提取 Wwise WEM 音频并转码为 WAV。
//! 如需 MP3，使用 `convert_dir_to_mp3()` 作为独立后处理步骤。
//!
//! # 默认管线
//!
//! ```text
//! sound/WwiseIdMapping.bytes  (AES-256-CBC)
//!     │   decrypt_wwise_event_table()
//!     ▼
//!   event_id → event_name 映射（全局，不分语言）
//!
//! sound/Windows/*/*.pck  (AKPK 容器)
//!     │   parse_akpk()  → wem_id → (pck内偏移)
//!     │   extract_wem() → WEM RIFF 数据
//!     │   wem_to_wav()  (vgmstream)
//!     ▼
//!   {output_dir}/{event_name}.wav
//!           │
//!           │  （可选）convert_dir_to_mp3()
//!           ▼
//!   {output_dir}/{event_name}.mp3
//! ```

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

/// 批量提取统计
#[derive(Debug, Default)]
pub struct AudioExtractStats {
    /// 扫描到的 .pck 文件数
    pub pck_files: usize,
    /// 成功输出的 WAV 文件数
    pub wav_output: usize,
    /// 被跳过的（已存在）
    pub skipped: usize,
    /// 失败数
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

    // 从 BKHD 向前跳过零值填充
    let mut pos = bkhd_pos;
    while pos > hdr_size.saturating_sub(200) {
        let v = u32::from_le_bytes([data[pos - 4], data[pos - 3], data[pos - 2], data[pos - 1]]);
        if v != 0 {
            break;
        }
        pos -= 4;
    }

    // 反向读取 20 字节 WEM 条目
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
            // 遇到无效条目 → 列表结束
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

/// 调用 vgmstream 将内存中的 WEM 数据转码为 WAV 文件。
///
/// vgmstream 是一个专门的游戏音频解码库，支持 Wwise WEM、ADPCM、Vorbis 等格式。
pub fn wem_to_wav(
    wem_data: &[u8],
    output: &Path,
    vgmstream_path: &Path,
) -> anyhow::Result<()> {
    // vgmstream 不支持 stdin，需要临时文件
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

/// 调用 ffmpeg 将单个 WAV 文件转码为 MP3。
///
/// 参数：`-b:a 128k`，剥离元数据，覆盖已存在文件。
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
            "-map_metadata",
            "-1",
            &mp3_path.to_string_lossy(),
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .with_context(|| format!("无法执行 ffmpeg: {ffmpeg_path}"))?;

    if !status.success() {
        return Err(anyhow::anyhow!("ffmpeg 转码失败，退出码: {:?}", status.code()));
    }
    Ok(())
}

// ============================================================================
// 批量提取（默认管线：WEM → WAV）
// ============================================================================

/// 扫描 pck 目录，提取所有 WEM 并转码为 WAV。
///
/// # 参数
/// - `pck_dir`: 包含 .pck 文件的目录（递归扫描）
/// - `output_dir`: WAV 输出根目录
/// - `event_map`: Wwise event_id → event_name 映射（来自 WwiseIdMapping.bytes）
/// - `vgmstream_path`: vgmstream-cli.exe 的完整路径
///
/// # 输出结构
///
/// ```text
/// {output_dir}/
///     Play_fx_smn_10244120_1.wav
///     Play_fx_sty_010101_timeshift_rewind_small.wav
///     ...
///     _unmapped/
///         {wem_id}.wav  ← 未能匹配到事件名的 WEM
/// ```
pub fn extract_all(
    pck_dir: &Path,
    output_dir: &Path,
    event_map: &std::collections::BTreeMap<u32, String>,
    vgmstream_path: &Path,
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
                output_dir.join(format!("{}.wav", name))
            } else {
                std::fs::create_dir_all(&unmapped_dir)?;
                unmapped_dir.join(format!("{}.wav", wem_id))
            };

            // 跳过已存在的文件
            if out_path.exists() {
                stats.skipped += 1;
                continue;
            }

            match wem_to_wav(wem_data, &out_path, vgmstream_path) {
                Ok(()) => {
                    stats.wav_output += 1;
                }
                Err(e) => {
                    tracing::error!("WEM→WAV 失败 {}: {e}", wem_id);
                    stats.failed += 1;
                }
            }
        }
    }

    tracing::info!(
        "音频提取完成: {} pck, {} WAV 输出, {} 跳过, {} 失败",
        stats.pck_files,
        stats.wav_output,
        stats.skipped,
        stats.failed
    );

    Ok(stats)
}

/// 批量将目录中的 WAV 文件转码为 MP3（可选后处理）。
///
/// 扫描 `wav_dir` 下所有 .wav 文件，转码到 `mp3_dir`，保持相对目录结构。
pub fn convert_dir_to_mp3(
    wav_dir: &Path,
    mp3_dir: &Path,
    ffmpeg_path: &str,
) -> anyhow::Result<usize> {
    std::fs::create_dir_all(mp3_dir)?;

    let mut converted = 0usize;
    for entry in walkdir::WalkDir::new(wav_dir)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().map(|x| x == "wav").unwrap_or(false))
    {
        let rel = entry.path().strip_prefix(wav_dir)?;
        let mp3_path = mp3_dir.join(rel).with_extension("mp3");

        if mp3_path.exists() {
            continue;
        }
        if let Some(parent) = mp3_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        wav_to_mp3(entry.path(), &mp3_path, ffmpeg_path)?;
        converted += 1;
    }

    tracing::info!("MP3 转码完成: {} 个文件", converted);
    Ok(converted)
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