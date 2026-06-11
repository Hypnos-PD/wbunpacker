//! 卡牌语音提取模块
//!
//! # 概述
//!
//! 从游戏客户端目录下的 .pck (Wwise SoundBank) 文件中
//! 提取卡牌语音并转换为 MP3 格式，供 WBArts 浏览器播放。
//!
//! 这是整个解包管线中最复杂的模块，涉及多层二进制解析和外部工具调用。
//!
//! # 整体管线
//!
//! ```text
//! Sound/Windows/d/{lang}/
//!   dx_10001110.pck    ← Wwise SoundBank（AKPK 容器）
//!         │
//!         ├── 1. 解析 AKPK header → 提取 WEM 文件偏移
//!         │         (AKPK 是一种 RIFF 风格的自定义容器格式)
//!         │
//!         ├── 2. 在 pck 中找到 BKHD 块，反向扫描 20 字节条目的 WEM 列表
//!         │         每个条目: [wem_id: u32, flag: u32, ?, offset: u32, flag: u32]
//!         │
//!         ├── 3. 提取 WEM 数据（RIFF 格式的 Wwise 编码音频）
//!         │
//!         ├── 4. wem → wav (via vgmstream-cli)
//!         │         vgmstream 是专门解码 Wwise WEM 的工具
//!         │
//!         └── 5. wav → mp3 (via ffmpeg)
//!                   WAV 体积大，转码为 MP3 供浏览器播放
//! ```
//!
//! # AKPK 格式分析
//!
//! AKPK 是 Cygames 自定义的音频容器格式，类似于 Wwise 的 .pck 文件，
//! 但结构有所不同：
//!
//! ```text
//! [AKPK header: 8+ bytes]
//!   ├── magic: "AKPK" (4 bytes)
//!   ├── header_size: u32 (4 bytes)
//!   └── ... (其他 header 字段)
//!
//! [BKHD 块 (Wwise Bank Header)]
//!   ├── magic: "BKHD" (4 bytes)
//!   └── ... (Wwise 标准 header)
//!
//! [WEM 条目列表 (BKHD 之前，反向存储)]
//!   每个条目 20 字节:
//!   ├── wem_id:    u32 (4 bytes, offset 0)
//!   ├── flag1:     u32 (4 bytes, offset 4) ← 必须为 1
//!   ├── ???:       u32 (4 bytes, offset 8)
//!   ├── offset:    u32 (4 bytes, offset 12) ← WEM 数据在文件中的偏移
//!   └── flag2:     u32 (4 bytes, offset 16) ← 必须为 1
//! ```
//!
//! 解析策略：找到 BKHD 标记 → 向前搜索空字节边界 →
//! 从 AKPK header 结束位置开始反向读取 20 字节条目，
//! 直到遇到无效条目（wem_id ≤ 0x100000 或 flags ≠ 1）为止。
//!
//! # 语音事件映射
//!
//! CardResourceMaster 表定义了卡牌→语音事件的关联：
//!
//! ```text
//! CardStyleId → prefix (如 "dx_10001110")
//!            → voice_event_43 (列为 "登场" 事件名, "Play_dx_10001110_1,...")
//!            → voice_event_45 (列为 "攻击" 事件名, ...)
//!            ...
//! ```
//!
//! WwiseIdMapping 表定义了 Wwise 事件ID → 事件名的映射。
//!
//! # 依赖的外部工具
//!
//! - **vgmstream-cli**: WEM → WAV 解码
//!   下载地址: https://github.com/vgmstream/vgmstream
//!   默认路径: `D:\Tools\vgmstream-win64\vgmstream-cli.exe`
//!
//! - **ffmpeg**: WAV → MP3 编码
//!   需要加入 PATH 环境变量

use anyhow::Context;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;

// ============================================================================
// 常量
// ============================================================================

/// AKPK 容器魔数
const AKPK_MAGIC: &[u8; 4] = b"AKPK";

/// BKHD (Wwise Bank Header) 魔数
const BKHD_MAGIC: &[u8; 4] = b"BKHD";

/// RIFF (Wwise WEM 原始音频) 魔数
const RIFF_MAGIC: &[u8; 4] = b"RIFF";

/// WEM 条目在 AKPK 中的大小（字节）
const WEM_ENTRY_SIZE: usize = 20;

/// vgmstream 可执行文件默认路径
const DEFAULT_VGMSTREAM_PATH: &str = r"D:\Tools\vgmstream-win64\vgmstream-cli.exe";

/// MP3 编码默认码率
const DEFAULT_MP3_BITRATE: &str = "128k";

// ============================================================================
// 数据结构
// ============================================================================

/// AKPK 文件中的 WEM 条目。
#[derive(Debug, Clone)]
struct WemEntry {
    /// Wwise 内部的 WEM ID
    wem_id: u32,
    /// WEM 数据在 .pck 文件中的偏移
    offset: u32,
}

/// 卡牌语音槽位的标签（5 语言）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VoiceLabel {
    /// 简体中文
    pub chs: String,
    /// 英文
    pub eng: String,
    /// 日文
    pub jpn: String,
    /// 韩文
    pub kor: String,
    /// 繁体中文
    pub cht: String,
}

/// 语音索引文件 (voice_index.json) 的顶层结构。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VoiceIndex {
    /// 槽位名 → 五语言标签
    pub labels: BTreeMap<String, VoiceLabel>,
    /// 语言 → (prefix_num → (槽位名 → 相对路径))
    pub cards: BTreeMap<String, BTreeMap<String, BTreeMap<String, String>>>,
}

/// 语音事件类型与对应槽位的映射。
///
/// CardResourceMaster 表中的第 43-52 列分别对应不同的语音事件：
/// 登场 / 攻击 / 进化攻击 / 进化 / 破坏 / 进化破坏 / 技能 / 进化技能 / 行动
const VOICE_INDICES: &[(usize, &str)] = &[
    (43, "play"),
    (45, "attack"),
    (46, "evo_attack"),
    (47, "evolve"),
    (48, "destroy"),
    (49, "evo_destroy"),
    (50, "skill"),
    (51, "evo_skill"),
    (52, "act"),
];

// ============================================================================
// 公共 API
// ============================================================================

/// 从 .pck 文件中提取卡牌语音。
///
/// # 处理流程
/// 1. 加载 CardResourceMaster → 卡牌语音事件映射
/// 2. 加载 wem_mapping → Wwise 事件名 → WEM ID 映射
/// 3. 遍历 .pck 文件，对每个文件：
///    a. 解析 AKPK → WEM 偏移表
///    b. 匹配 WEM ID → 语音槽位
///    c. 提取 WEM RIFF 数据
///    d. vgmstream: WEM → WAV
///    e. ffmpeg: WAV → MP3
/// 4. 生成 voice_index.json
///
/// # 参数
/// - `pck_root`: .pck 文件所在根目录
/// - `card_resource_path`: CardResourceMaster.json 路径
/// - `wem_mapping_path`: wem_mapping.json 路径
/// - `output_dir`: MP3 输出目录
/// - `force`: 是否强制覆盖已有文件
pub fn extract_audio(
    pck_root: &Path,
    card_resource_path: &Path,
    wem_mapping_path: &Path,
    output_dir: &Path,
    force: bool,
) -> anyhow::Result<VoiceIndex> {
    todo!("实现语音提取完整管线")
}

// ============================================================================
// 内部函数
// ============================================================================

/// 解析 AKPK 文件，提取所有 WEM 条目的偏移表。
///
/// 算法：
/// 1. 检查 AKPK magic（文件头 4 字节）
/// 2. 读取 header size（偏移 4，u32 LE）
/// 3. 在 header 附近搜索 BKHD magic
/// 4. 找到 BKHD 后反向扫描，跳过 0x00000000 填充
/// 5. 反向读取 20 字节条目，过滤出有效 WEM entry
///
/// # WEM 条目验证条件
/// - wem_id > 0x100000（Wwise 的 WEM ID 范围）
/// - flag1 == 1 && flag2 == 1（条目有效标记）
/// - offset > header_size 且 < file_size（偏移在合理范围内）
///
/// # 返回
/// wem_id → （pck 内偏移）的映射表
fn parse_akpk(data: &[u8]) -> HashMap<u32, u32> {
    if data.len() < 8 || &data[..4] != AKPK_MAGIC {
        return HashMap::new();
    }

    let hdr_size = u32::from_le_bytes([data[4], data[5], data[6], data[7]]) as usize;
    let file_size = data.len();

    // 在 header 区域搜索 BKHD
    let bkhd_pos = data[hdr_size.saturating_sub(4)..(hdr_size + 16).min(file_size)]
        .windows(4)
        .position(|w| w == BKHD_MAGIC)
        .map(|p| hdr_size.saturating_sub(4) + p);

    let bkhd_pos = match bkhd_pos {
        Some(p) => p,
        None => return HashMap::new(),
    };

    // 从 BKHD 位置向前扫描，跳过零值填充
    let mut pos = bkhd_pos;
    while pos > hdr_size.saturating_sub(200) {
        let v = u32::from_le_bytes([
            data[pos - 4],
            data[pos - 3],
            data[pos - 2],
            data[pos - 1],
        ]);
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
        let flag1 = u32::from_le_bytes([
            data[pos + 4],
            data[pos + 5],
            data[pos + 6],
            data[pos + 7],
        ]);
        let offset = u32::from_le_bytes([
            data[pos + 12],
            data[pos + 13],
            data[pos + 14],
            data[pos + 15],
        ]);
        let flag2 = u32::from_le_bytes([
            data[pos + 16],
            data[pos + 17],
            data[pos + 18],
            data[pos + 19],
        ]);

        // 验证条目的有效性
        if wem_id > 0x100000
            && flag1 == 1
            && offset as usize > hdr_size
            && (offset as usize) < file_size
            && flag2 == 1
        {
            entries.insert(wem_id, offset);
        } else {
            // 遇到无效条目 → 列表在此结束
            break;
        }
    }

    entries
}

/// 从 .pck 文件中提取指定偏移处的 WEM 数据。
///
/// WEM 在 pck 中的存储格式为 RIFF 容器：
/// ```text
/// RIFF { chunk_size: u32, format: "WAVE", ...sub_chunks }
/// ```
fn extract_wem(data: &[u8], offset: u32) -> Option<&[u8]> {
    let off = offset as usize;
    if off + 8 > data.len() || &data[off..off + 4] != RIFF_MAGIC {
        return None;
    }
    let chunk_size = u32::from_le_bytes([
        data[off + 4],
        data[off + 5],
        data[off + 6],
        data[off + 7],
    ]) as usize;
    let end = off + 8 + chunk_size;
    if end > data.len() {
        return None;
    }
    Some(&data[off..end])
}

/// 调用 vgmstream 将 WEM 转码为 WAV。
///
/// vgmstream 是一个专门解码游戏音频格式的库，
/// 支持 Wwise WEM、ADPCM、Vorbis 等多种编码。
///
/// # 返回
/// 转换成功则返回 true
fn wem_to_wav(wem_data: &[u8], output: &Path, vgmstream_path: &Path) -> anyhow::Result<bool> {
    // 将 WEM 数据写入临时文件
    let tmp = std::env::temp_dir().join(format!("wbu_{}.wem", std::process::id()));
    std::fs::write(&tmp, wem_data)?;

    let result = Command::new(vgmstream_path)
        .arg(&tmp)
        .arg("-o")
        .arg(output)
        .output();

    // 清理临时文件
    let _ = std::fs::remove_file(&tmp);

    match result {
        Ok(out) => Ok(out.status.success() && output.exists()),
        Err(_) => Ok(false),
    }
}

/// 调用 ffmpeg 将 WAV 编码为 MP3。
///
/// 参数说明：
/// - `-y`: 覆盖已有文件
/// - `-b:a 128k`: 输出码率 128 kbps
/// - `-map_metadata -1`: 剥离元数据（减小体积）
fn wav_to_mp3(wav_path: &Path, mp3_path: &Path) -> anyhow::Result<bool> {
    if let Some(parent) = mp3_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let output = Command::new("ffmpeg")
        .args([
            "-y",
            "-i",
            &wav_path.to_string_lossy(),
            "-b:a",
            DEFAULT_MP3_BITRATE,
            "-map_metadata",
            "-1",
            &mp3_path.to_string_lossy(),
        ])
        .output()?;

    Ok(output.status.success() && mp3_path.exists())
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// 验证 AKPK 解析在非 AKPK 数据上安全返回空
    #[test]
    fn test_parse_akpk_invalid_magic() {
        let data = b"NOT_AKPK_FILE\x00\x00\x00\x00";
        let entries = parse_akpk(data);
        assert!(entries.is_empty(), "非 AKPK 文件应返回空映射");
    }

    /// 验证 AKPK magic 检测
    #[test]
    fn test_akpk_magic_value() {
        assert_eq!(AKPK_MAGIC, b"AKPK");
        assert_eq!(BKHD_MAGIC, b"BKHD");
        assert_eq!(RIFF_MAGIC, b"RIFF");
    }
}

pub mod wwise;