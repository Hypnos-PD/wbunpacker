//! Wwise WEM 映射子模块
//!
//! # 概述
//!
//! 从游戏下载的音频资源中，建立 Wwise 内部的 WEM ID → 事件名称的映射。
//! 这是整个音频提取管线的**前置步骤**——没有这个映射，
//! extract_card_audio 就无法知道哪个 WEM ID 对应哪个卡牌的哪个语音槽。
//!
//! # 数据流
//!
//! ```text
//! ┌── 第一步：解密 Wwise 事件表
//! │
//! │   WwiseIdMapping.bytes  (AES-256-CBC + PKCS7 加密)
//! │         │
//! │         ├── key  = 文件前 0x20 字节
//! │         ├── iv   = 文件 0x20-0x30 字节
//! │         ├── ciphertext = 文件 0x30 之后
//! │         │
//! │         └── 解密 → (event_count: u32, (event_id: u32, name_len: u32, name: utf-8)[])
//! │                    得到: event_id → "Play_dx_10001110_1" 映射
//! │
//! └── 第二步：从 .pck 中提取 Wwise SoundBank
//!     │
//!     │   dx_*.pck 文件
//!     │         │
//!     │         ├── 扫描 BKHD 标记，提取独立的 .bnk 文件
//!     │         │    (一个 .pck 内可能包含多个 Bank)
//!     │         │
//!     │         └── 用 wwiser 解析 .bnk 的 HIRC 层级数据：
//!     │               CAkSound → (sound_id, wem_id)
//!     │               CAkActionPlay → (action_id, sound_id)
//!     │               CAkEvent → (event_id, action_id)
//!     │                     ↓
//!     │              最终得到: wem_id → event_name 映射
//!     │
//!     └── 合并所有卡牌的映射 → wem_mapping.json
//! ```
//!
//! # 外部依赖
//!
//! - **wwiser** — Python 库，解析 Wwise SoundBank (.bnk) 的 HIRC 层级。
//!   需要本地 clone: https://github.com/bnnm/wwiser
//!   Rust 侧通过 `std::process::Command` 调用 wwiser 的 Python 脚本，
//!   或直接内嵌 wwiser 的解析逻辑。
//!
//! # 加密细节
//!
//! WwiseIdMapping.bytes 的加密格式：
//! - 算法: AES-256-CBC + PKCS7 padding
//! - 密钥存储方式: 密钥和 IV 直接存储在文件头（非标准）
//!   这要求配置文件中的密钥信息保持机密
//!
//! # 与 W2AU 的对应
//!
//! 本模块替代 Python 的 `wwise_wem_mapping.py`。
//! 区别：原版用 Python 调用 wwiser（Python 生态），
//! Rust 版可以选择用 `std::process::Command` 调 Python
//! 或完全重写 .bnk HIRC 解析。

use anyhow::Context;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

// ============================================================================
// 数据结构
// ============================================================================

/// Wwise 事件表中的单条记录。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WwiseEvent {
    /// Wwise 内部事件 ID（32 位）
    pub event_id: u32,
    /// 事件名称，如 "Play_dx_10001110_1"
    pub name: String,
}

/// WEM ID → 事件名称的映射条目。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WemMapping {
    /// Wwise 内部的 WEM ID（音频文件标识符）
    pub wem_id: u32,
    /// 对应的游戏事件名
    pub event_name: String,
}

// ============================================================================
// 公共 API
// ============================================================================

/// 解密 WwiseIdMapping.bytes 文件，提取 Wwise 事件表。
///
/// # 加密格式
///
/// ```text
/// [0x00..0x20]  AES-256 密钥
/// [0x20..0x30]  AES-CBC IV
/// [0x30..EOF]   密文（PKCS7 padded）
/// ```
///
/// 解密后得到 msgpack 格式的事件表：
/// ```text
/// count: u32 LE
/// [event_id: u32 LE, name_len: u32 LE, name: utf-8]{count}
/// ```
///
/// # 依赖
/// 需要 `aes` + `cbc` crate（或 `openssl`）进行 AES-CBC 解密。
pub fn decrypt_wwise_event_table(mapping_path: &Path) -> anyhow::Result<BTreeMap<u32, String>> {
    todo!("AES-CBC 解密 WwiseIdMapping.bytes")
}

/// 从 .pck 文件中提取 Wwise SoundBank (.bnk)。
///
/// # 算法
///
/// 1. 在 .pck 数据中搜索 BKHD (0x424B4844) 标记
/// 2. 验证 size field ≤ 0x10000（SoundBank header 合理范围）
/// 3. 顺序遍历后续 chunk，累计 total_size
/// 4. 遇到下一个 BKHD 或 STID 时停止
/// 5. 提取 [bkhd_pos .. bkhd_pos+total_size] → 独立 .bnk 文件
///
/// # 返回
/// 提取出的 .bnk 文件临时路径列表
pub fn extract_banks_from_pck(pck_data: &[u8], _output_dir: &Path, _prefix: &str) -> anyhow::Result<Vec<std::path::PathBuf>> {
    todo!(".pck → .bnk 提取")
}

/// 使用 wwiser 解析 .bnk 文件的 HIRC 数据，
/// 建立 wem_id → event_name 映射。
///
/// # HIRC 层级
///
/// Wwise SoundBank 的 HIRC (Hierarchy) 包含三种关键节点：
///
/// - **CAkSound**: 音频对象，关联 WEM 文件
///   - `sid`: sound ID
///   - `AkMediaInformation.tid`: WEM ID
///
/// - **CAkActionPlay**: 播放动作
///   - `sid`: action ID
///   - `ActionInitialValues.idExt`: sound ID (引用 CAkSound)
///
/// - **CAkEvent**: 事件
///   - `sid`: event ID
///   - `Action.tid`: action ID (引用 CAkActionPlay)
///
/// 映射链: wem_id → sound_id → action_id → event_id → event_name
pub fn build_wem_mapping(
    _bank_path: &Path,
    _event_to_name: &BTreeMap<u32, String>,
    _wwiser_path: &Path,
) -> anyhow::Result<BTreeMap<u32, String>> {
    todo!("wwiser HIRC 解析 → wem 映射")
}

/// 完整管线：解密事件表 → 提取 Bank → 解析 HIRC → 输出映射。
///
/// 这是 `wwise_wem_mapping.py` 的 Rust 等效实现。
///
/// # 参数
/// - `mapping_path`: WwiseIdMapping.bytes 路径
/// - `pck_root`: .pck 文件目录
/// - `wwiser_path`: wwiser 本地 clone 路径
/// - `output_path`: wem_mapping.json 输出路径
pub fn generate_wem_mapping(
    _mapping_path: &Path,
    _pck_root: &Path,
    _wwiser_path: &Path,
    _output_path: &Path,
) -> anyhow::Result<()> {
    todo!("wwise wem mapping 完整管线")
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// 验证 WwiseEvent 结构序列化
    #[test]
    fn test_wwise_event_serde() {
        let event = WwiseEvent {
            event_id: 12345,
            name: "Play_dx_10001110_1".into(),
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("12345"));
        assert!(json.contains("Play_dx_10001110_1"));
    }
}
