//! 卡图纹理处理模块
//!
//! # 概述
//!
//! 从 Unity AssetBundle 中导出卡牌插画，并处理为 WBArts 所需的格式。
//!
//! # 管线的两步
//!
//! ## 第一步：AssetStudio 导出
//!
//! 调用外部工具 AssetStudio CLI 从解密后的 AssetBundle 中提取 Texture2D。
//! AssetStudio 是一个专门解析 Unity SerializedFile 格式的工具，
//! 这部分不需要自己实现（Unity 的序列化格式极其复杂）。
//!
//! ```powershell
//! AssetStudioModCLI.exe `
//!   --input data/decrypted/assetbundles/... `
//!   --output data/exports/card-textures/raw/ `
//!   --unity-version 2022.3.62f2
//! ```
//!
//! ## 第二步：缩放与命名
//!
//! 导出得到的原始 PNG 尺寸不统一（通常为正方形纹理），
//! 需要缩放到 848×1024（竖卡比例 5:7）并重命名。
//!
//! 本模块负责第二步，以及封装 AssetStudio 的命令行调用。
//!
//! # 输出目录结构
//!
//! ```text
//! data/exports/card-textures/
//!   raw/        ← AssetStudio 直接导出的原始 1:1 PNG
//!   resized/    ← 缩放至 848×1024 的 PNG
//!   named/      ← STSVWB 命名规则的最终卡图（按变体分目录）
//! ```
//!
//! # 卡图命名规则 (STSVWB)
//!
//! 以英文名为基础，去除标点，连字符替换为空格，每个单词首字母大写：
//!
//! | 原始英文名 | 处理后的 STSVWB 名 |
//! |---|---|
//! | `Achim, Lord of Despair` | `Achim Lord Of Despair` |
//! | `Anthuria, Toe-Tapping Torch` | `Anthuria Toe Tapping Torch` |
//! | `Adventurers' Guild` | `Adventurers Guild` |
//!
//! 去除的标点：`, . ' " : ; ! ? ( ) &`
//! 替换为空格：`-` `–` `—`
//!
//! # 变体分类
//!
//! 根据 card_style_id 的末位数字决定输出子目录：
//!
//! | 末位 | 文件夹 | 含义 |
//! |---|---|---|
//! | 0 | (根目录) | 基础卡图 |
//! | 1 | Evo/ | 进化 |
//! | 2 | Skin/ | 异画 |
//! | 3 | SkinEvo/ | 异画进化 |
//! | ... | 依此类推 | ... |
//!
//! 但 WBArts 实际上只消费 `resized/` 目录，
//! STSVWB 命名规则主要用于备用的手动分发场景。

use anyhow::Context;
use image::DynamicImage;
use std::path::{Path, PathBuf};
use std::process::Command;

// ============================================================================
// 常量
// ============================================================================

/// 目标卡图宽度（像素）
const TARGET_WIDTH: u32 = 848;

/// 目标卡图高度（像素）
const TARGET_HEIGHT: u32 = 1024;

/// AssetStudio CLI 默认路径
const DEFAULT_ASSET_STUDIO_PATH: &str =
    r"D:\Tools\AssetStudioModCLI_net9_win64\AssetStudioModCLI.exe";

/// Unity 版本（用于 AssetStudio 解析）
const UNITY_VERSION: &str = "2022.3.62f2";

/// STSVWB 命名规则中需要去除的标点
const STRIP_CHARS: &[char] = &[',', '.', '\'', '"', ':', ';', '!', '?', '(', ')', '&'];

/// STSVWB 命名规则中需要替换为空格的字符
const REPLACE_WITH_SPACE: &[char] = &['-', '\u{2013}', '\u{2014}']; // hyphen, en-dash, em-dash

// ============================================================================
// 数据结构
// ============================================================================

/// 纹理处理结果。
pub struct TextureResult {
    /// card_style_id
    pub cs_id: u64,
    /// 输出文件路径
    pub output_path: PathBuf,
    /// 是否为新文件（true: 新增, false: 已存在，跳过）
    pub is_new: bool,
}

// ============================================================================
// 公共 API
// ============================================================================

/// 调用 AssetStudio CLI 导出原始纹理。
///
/// 遍历解密后的 AssetBundle 目录，提取所有 Texture2D 为 PNG。
///
/// # 参数
/// - `input_dir`: 解密后的 AssetBundle 目录
/// - `output_dir`: 原始 PNG 输出目录
/// - `asset_studio_path`: AssetStudio CLI 可执行文件路径
///
/// # 注意
/// AssetStudio 的导出可能非常耗时（数十分钟），
/// 取决于 AssetBundle 的总大小和数量。
pub fn export_raw(
    input_dir: &Path,
    output_dir: &Path,
    asset_studio_path: Option<&Path>,
) -> anyhow::Result<Vec<PathBuf>> {
    todo!("AssetStudio 调用封装")
}

/// 将原始 PNG 缩放至 848×1024 的卡图尺寸。
///
/// 使用 Lanczos3 算法进行高质量缩放。
/// 已存在的文件自动跳过（增量处理）。
///
/// # 参数
/// - `raw_dir`: 原始 PNG 目录
/// - `resized_dir`: 缩放后输出目录
pub fn resize_textures(raw_dir: &Path, resized_dir: &Path) -> anyhow::Result<Vec<TextureResult>> {
    todo!("图片缩放实现")
}

/// 按 STSVWB 规则重命名卡图并分类到变体目录。
///
/// 读取 cards_full.json 获取卡牌的英文名和 card_style_id，
/// 应用 STSVWB 命名规则后复制/重命名到 named/ 目录。
///
/// # 参数
/// - `resized_dir`: 缩放后的 PNG 目录
/// - `named_dir`: STSVWB 命名输出目录
/// - `cards_full_path`: cards_full.json 路径（用于卡名和 card_style_id 映射）
pub fn rename_textures(
    resized_dir: &Path,
    named_dir: &Path,
    cards_full_path: &Path,
) -> anyhow::Result<Vec<TextureResult>> {
    todo!("STSVWB 重命名实现")
}

// ============================================================================
// 内部函数
// ============================================================================

/// 将单张图片缩放至目标尺寸。
///
/// 使用 `image` crate 的 Lanczos3 重采样算法，
/// 这是视觉质量最高的缩放算法之一。
///
/// 如果原始图片已经是目标尺寸则直接复制（不做缩放），
/// 避免重复采样损失画质。
fn resize_single(input: &Path, output: &Path) -> anyhow::Result<()> {
    let img = image::open(input)
        .with_context(|| format!("无法打开图片: {}", input.display()))?;

    if img.width() == TARGET_WIDTH && img.height() == TARGET_HEIGHT {
        // 尺寸已匹配，直接复制
        if let Some(parent) = output.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(input, output)?;
        return Ok(());
    }

    let resized = img.resize_exact(TARGET_WIDTH, TARGET_HEIGHT, image::imageops::FilterType::Lanczos3);

    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent)?;
    }
    resized.save(output)?;

    Ok(())
}

/// 应用 STSVWB 命名规则处理单个卡名。
///
/// 规则：
/// 1. 去除指定标点字符
/// 2. 将连字符/破折号替换为空格
/// 3. 每个单词首字母大写，其余小写
/// 4. 合并多余空格
///
/// # 示例
/// ```
/// "Achim, Lord of Despair" → "Achim Lord Of Despair"
/// "Anthuria, Toe-Tapping Torch" → "Anthuria Toe Tapping Torch"
/// ```
fn stsvwb_name(english_name: &str) -> String {
    let mut name = english_name.to_string();

    // 第一步：去除标点
    for ch in STRIP_CHARS {
        name = name.replace(*ch, "");
    }

    // 第二步：连字符替换为空格
    for ch in REPLACE_WITH_SPACE {
        name = name.replace(*ch, " ");
    }

    // 第三步：每个单词首字母大写
    let words: Vec<String> = name
        .split_whitespace()
        .filter(|s| !s.is_empty())
        .map(|w| {
            let mut chars = w.chars();
            match chars.next() {
                None => String::new(),
                Some(first) => {
                    let mut result = first.to_uppercase().to_string();
                    result.extend(chars.flat_map(|c| c.to_lowercase()));
                    result
                }
            }
        })
        .collect();

    words.join(" ")
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// 验证 STSVWB 命名规则
    #[test]
    fn test_stsvwb_name_basic() {
        assert_eq!(
            stsvwb_name("Achim, Lord of Despair"),
            "Achim Lord Of Despair"
        );
    }

    #[test]
    fn test_stsvwb_name_hyphen() {
        assert_eq!(
            stsvwb_name("Anthuria, Toe-Tapping Torch"),
            "Anthuria Toe Tapping Torch"
        );
    }

    #[test]
    fn test_stsvwb_name_apostrophe() {
        assert_eq!(
            stsvwb_name("Adventurers' Guild"),
            "Adventurers Guild"
        );
    }

    #[test]
    fn test_stsvwb_name_simple() {
        // 无特殊字符的名字保持不变
        assert_eq!(stsvwb_name("Arisa"), "Arisa");
    }
}
