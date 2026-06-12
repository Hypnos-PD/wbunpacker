//! 卡图纹理处理模块
//!
//! # 管线
//!
//! 1. export   — 调 AssetStudio CLI 导出 Texture2D → PNG（增量：已存在则跳过）
//! 2. categorize — 按前缀分到 Main/Special/Token（增量：已存在则跳过）
//! 3. resize   — Lanczos3 缩放至 848x1024（增量：已存在则跳过）
//!
//! # 资源 ID 分类
//!
//! | 前缀 | 目录    | 说明     |
//! |------|---------|----------|
//! | 1xxx | Main/   | 主卡牌   |
//! | 8xxx | Special/| 特殊卡   |
//! | 9xxx | Token/  | Token 卡 |
//! | 7xxx | —       | 跳过（Spine 动画背景）|
//!
//! # 输出目录结构
//!
//! ```text
//! exports/card-textures/
//!   _raw/       ← AssetStudio 原始导出
//!   Main/       ← 1xxxx 主卡 ({id}.png)
//!   Special/    ← 8xxxx 特殊卡
//!   Token/      ← 9xxxx Token
//! exports/card-textures-resized/
//!   Main/       ← 缩放后（保持子目录结构）
//!   Special/
//!   Token/
//! ```

use anyhow::Context;
use indicatif::{ProgressBar, ProgressStyle};
use manifest::Manifest;
use std::path::{Path, PathBuf};
use std::process::Command;
use tracing::{debug, info};

// ============================================================================
// 常量
// ============================================================================

/// Unity 版本（用于 AssetStudio 解析）
const UNITY_VERSION: &str = "2022.3.62f2";

/// Card/Textures 在 manifest 中的路径前缀
const CARD_TEXTURES_PREFIX: &str = "Assets/_Wizard2Resources/Card/Textures/";

/// 需要跳过的子路径
const SKIP_PATTERNS: &[&str] = &["HighFoil"];

// ============================================================================
// 公共 API
// ============================================================================

/// 完整的卡图处理管线: 导出 → 分类 → 缩放。
///
/// 每步都有增量跳过逻辑。
pub fn process_card_textures(
    data_dir: &Path,
    asset_studio_path: &Path,
    no_resize: bool,
) -> anyhow::Result<()> {
    let output_dir = data_dir.join("exports").join("card-textures");
    let raw_dir = output_dir.join("_raw");

    // 第一步: AssetStudio 导出
    let expected_count = count_card_entries(data_dir)?;
    let existing = count_raw_dirs(&raw_dir);
    if existing >= expected_count {
        println!("跳过 AssetStudio 导出（已有 {} 个目录，预期 {}）", existing, expected_count);
    } else {
        println!("AssetStudio 导出（预期 {} 个卡图）...", expected_count);
        run_asset_studio(data_dir, &raw_dir, asset_studio_path)?;
    }

    // 第二步: 分类
    let png_count = count_categorized(&output_dir);
    if png_count >= expected_count {
        println!("跳过分类（已有 {} 个 PNG，预期 {}）", png_count, expected_count);
    } else {
        println!("分类到 Main/Special/Token...");
        let r = categorize(&raw_dir, &output_dir)?;
        println!("   Main={} Special={} Token={} (共 {} PNG)",
            r.by_main, r.by_special, r.by_token, r.png_count);
        // 清理 _raw 目录（已分类完成）
        let _ = std::fs::remove_dir_all(&raw_dir);
    }

    // 第三步: 缩放
    if !no_resize {
        let resized_dir = data_dir.join("exports").join("card-textures-resized");
        let resized_count = count_pngs_recursive(&resized_dir);
        let total_pngs = count_pngs_recursive(&output_dir);
        if resized_count >= total_pngs {
            println!("跳过缩放（已有 {} 个，预期 {}）", resized_count, total_pngs);
        } else {
            println!("缩放至 848x1024 -> {} ...", resized_dir.display());
            let rr = resize_textures(&output_dir, &resized_dir)?;
            println!("   缩放: {} | 跳过: {}", rr.resized, rr.skipped);
        }
    }

    Ok(())
}

// ============================================================================
// 计数（增量跳过用）
// ============================================================================

/// 从 manifest 读取预期的 Card/Textures 条目数。
fn count_card_entries(data_dir: &Path) -> anyhow::Result<usize> {
    let manifest_path = data_dir
        .join("manifests")
        .join("json")
        .join("assetbundle.Chs.manifest.json");
    let json = std::fs::read_to_string(&manifest_path)
        .context("请先运行: wbu manifest -v Chs --format json")?;
    let m: Manifest = serde_json::from_str(&json)?;
    Ok(m.assets.iter()
        .filter(|a| a.name.starts_with(CARD_TEXTURES_PREFIX))
        .filter(|a| !SKIP_PATTERNS.iter().any(|p| a.name.contains(p)))
        .count())
}

/// 统计 _raw 目录下的 .ab_export 子目录数。
fn count_raw_dirs(raw_dir: &Path) -> usize {
    std::fs::read_dir(raw_dir)
        .map(|entries| {
            entries
                .filter_map(|e| e.ok())
                .filter(|e| e.file_type().map_or(false, |t| t.is_dir()))
                .count()
        })
        .unwrap_or(0)
}

/// 统计 Main/Special/Token 目录下的 PNG 总数。
fn count_categorized(output_dir: &Path) -> usize {
    ["Main", "Special", "Token"].iter()
        .map(|d| count_pngs_recursive(&output_dir.join(d)))
        .sum()
}

/// 递归统计目录中的 PNG 文件数。
fn count_pngs_recursive(dir: &Path) -> usize {
    let mut count = 0;
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if entry.file_type().map_or(false, |t| t.is_dir()) {
                count += count_pngs_recursive(&path);
            } else if path.extension().map_or(false, |e| e == "png") {
                count += 1;
            }
        }
    }
    count
}

// ============================================================================
// AssetStudio 导出
// ============================================================================

/// 运行 AssetStudio CLI，导出 Texture2D → PNG。
fn run_asset_studio(
    data_dir: &Path,
    output_dir: &Path,
    asset_studio_path: &Path,
) -> anyhow::Result<()> {
    let decrypted_dir = data_dir
        .join("variants")
        .join("Chs")
        .join("decrypted")
        .join(CARD_TEXTURES_PREFIX);

    if !decrypted_dir.exists() {
        anyhow::bail!(
            "解密目录不存在: {}（请先运行 wbu asset batch -v Chs）",
            decrypted_dir.display()
        );
    }

    std::fs::create_dir_all(output_dir)?;

    // Spinner（AssetStudio 是外部进程，无法获取实时进度）
    let spinner = ProgressBar::new_spinner();
    spinner.set_style(
        ProgressStyle::with_template("{spinner} AssetStudio 导出中... {elapsed}")
            .unwrap()
    );
    spinner.enable_steady_tick(std::time::Duration::from_millis(100));

    let status = Command::new(asset_studio_path)
        .arg(&decrypted_dir)
        .args([
            "-t", "tex2d",
            "-g", "fileName",
            "-f", "assetName",
            "-o", &output_dir.to_string_lossy(),
            "-r",
            "--unity-version", UNITY_VERSION,
            "--log-level", "warning",
        ])
        .status()
        .with_context(|| format!("无法启动 AssetStudio: {}", asset_studio_path.display()))?;

    spinner.finish_and_clear();

    if !status.success() {
        anyhow::bail!("AssetStudio 退出码: {:?}", status.code());
    }

    Ok(())
}

// ============================================================================
// 分类
// ============================================================================

/// 分类结果
#[derive(Debug, Default)]
pub struct CategorizeResult {
    pub by_main: usize,
    pub by_special: usize,
    pub by_token: usize,
    pub png_count: usize,
}

/// 将 AssetStudio 原始输出按 ID 前缀分类到 Main/Special/Token。
///
/// AssetStudio 输出结构: _raw/{id}.ab_export/CAB-{hash}/{id}.png
fn categorize(raw_dir: &Path, output_dir: &Path) -> anyhow::Result<CategorizeResult> {
    let mut result = CategorizeResult::default();

    let main_dir = output_dir.join("Main");
    let special_dir = output_dir.join("Special");
    let token_dir = output_dir.join("Token");
    std::fs::create_dir_all(&main_dir)?;
    std::fs::create_dir_all(&special_dir)?;
    std::fs::create_dir_all(&token_dir)?;

    // 收集所有待处理的目录
    let dirs: Vec<_> = std::fs::read_dir(raw_dir)?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map_or(false, |t| t.is_dir()))
        .collect();

    let pb = ProgressBar::new(dirs.len() as u64);
    pb.set_style(
        ProgressStyle::with_template("{spinner} [{bar:30}] {pos}/{len} {msg}")
            .unwrap()
            .progress_chars("=> ")
    );

    for entry in dirs {
        let dir_str = entry.file_name().to_string_lossy().to_string();
        let resource_id = dir_str.strip_suffix(".ab_export").unwrap_or(&dir_str);

        if resource_id.len() != 9 {
            pb.inc(1);
            continue;
        }

        let target_dir = match resource_id.chars().next() {
            Some('1') => &main_dir,
            Some('8') => &special_dir,
            Some('9') => &token_dir,
            _ => { pb.inc(1); continue; }
        };

        // 检查是否已分类
        let dest = target_dir.join(format!("{}.png", resource_id));
        if dest.exists() {
            pb.inc(1);
            result.png_count += 1;
            continue;
        }

        // 递归查找 PNG 并移动
        if let Ok(pngs) = find_pngs(&entry.path()) {
            if let Some(png_path) = pngs.first() {
                pb.set_message(format!("{}", resource_id));
                std::fs::rename(png_path, &dest)?;
                result.png_count += 1;
            }
        }
        pb.inc(1);
    }

    pb.finish_and_clear();

    result.by_main = count_pngs_recursive(&main_dir);
    result.by_special = count_pngs_recursive(&special_dir);
    result.by_token = count_pngs_recursive(&token_dir);

    Ok(result)
}

/// 递归查找目录中的所有 PNG 文件。
fn find_pngs(dir: &Path) -> anyhow::Result<Vec<PathBuf>> {
    let mut result = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            result.extend(find_pngs(&path)?);
        } else if path.extension().map_or(false, |e| e == "png") {
            result.push(path);
        }
    }
    Ok(result)
}

// ============================================================================
// 缩放
// ============================================================================

/// 缩放结果
#[derive(Debug, Default)]
pub struct ResizeResult {
    pub resized: usize,
    pub skipped: usize,
}

/// 批量缩放目录树中所有 PNG 到 848x1024。
fn resize_textures(input_dir: &Path, output_dir: &Path) -> anyhow::Result<ResizeResult> {
    let mut result = ResizeResult::default();
    let pngs = find_pngs(input_dir)?;

    let pb = ProgressBar::new(pngs.len() as u64);
    pb.set_style(
        ProgressStyle::with_template("{spinner} [{bar:30}] {pos}/{len} 缩放 {msg}")
            .unwrap()
            .progress_chars("=> ")
    );

    for png_path in &pngs {
        let rel = png_path.strip_prefix(input_dir).unwrap_or(png_path);
        let out = output_dir.join(rel);
        if out.exists() {
            result.skipped += 1;
        } else {
            if let Some(parent) = out.parent() {
                std::fs::create_dir_all(parent)?;
            }
            if let Some(name) = png_path.file_stem() {
                pb.set_message(name.to_string_lossy().to_string());
            }
            resize_single_848x1024(png_path, &out)?;
            result.resized += 1;
        }
        pb.inc(1);
    }

    pb.finish_and_clear();
    Ok(result)
}

/// 将单张 PNG 缩放至 848x1024（Lanczos3）。
fn resize_single_848x1024(input: &Path, output: &Path) -> anyhow::Result<()> {
    let img = image::open(input)
        .with_context(|| format!("无法打开图片: {}", input.display()))?;

    if img.width() == 848 && img.height() == 1024 {
        std::fs::copy(input, output)?;
        return Ok(());
    }

    let resized = img.resize_exact(848, 1024, image::imageops::FilterType::Lanczos3);
    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent)?;
    }
    resized.save(output)?;
    Ok(())
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resource_id_routing() {
        assert!(CARD_TEXTURES_PREFIX.starts_with("Assets/_Wizard2Resources/Card/Textures/"));
        assert!(SKIP_PATTERNS.contains(&"HighFoil"));
    }
}
