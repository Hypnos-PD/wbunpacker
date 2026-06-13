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
//! `text
//! exports/card-textures/
//!   _raw/       ← AssetStudio 原始导出
//!   Main/       ← 1xxxx 主卡 ({id}.png)
//!   Special/    ← 8xxxx 特殊卡
//!   Token/      ← 9xxxx Token
//! exports/card-textures-resized/
//!   Main/       ← 缩放后（保持子目录结构）
//!   Special/
//!   Token/
//! `

use ab_glyph::{FontArc, PxScale};
use anyhow::Context;
use image::{DynamicImage, Rgba, RgbaImage};
use imageproc::drawing::{draw_text_mut, text_size};
use indicatif::{ProgressBar, ProgressStyle};
use manifest::Manifest;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use tracing::debug;

// ============================================================================
// 常量
// ============================================================================

/// Unity 版本（用于 AssetStudio 解析）
const UNITY_VERSION: &str = "2022.3.62f2";

/// Card/Textures 在 manifest 中的路径前缀
const CARD_TEXTURES_PREFIX: &str = "Assets/_Wizard2Resources/Card/Textures/";

/// Card2D 边框资源目录。
const CARD_FRAME_SOURCE_DIR: &str = "UI/Card2D";

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
        println!(
            "跳过 AssetStudio 导出（已有 {} 个目录，预期 {}）",
            existing, expected_count
        );
    } else {
        println!("AssetStudio 导出（预期 {} 个卡图）...", expected_count);
        run_asset_studio(data_dir, &raw_dir, asset_studio_path)?;
    }

    // 第二步: 分类
    let png_count = count_categorized(&output_dir);
    if png_count >= expected_count {
        println!(
            "跳过分类（已有 {} 个 PNG，预期 {}）",
            png_count, expected_count
        );
    } else {
        println!("分类到 Main/Special/Token...");
        let r = categorize(&raw_dir, &output_dir)?;
        println!(
            "   Main={} Special={} Token={} (共 {} PNG)",
            r.by_main, r.by_special, r.by_token, r.png_count
        );
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

/// 提取卡包UI图标: 从解密 AssetBundle 中提取 utx_ic_item_10000~10007 → PNG。
///
/// 增量: 已存在的 PNG 跳过。
pub fn process_pack_icons(data_dir: &Path, asset_studio_path: &Path) -> anyhow::Result<()> {
    extract_pack_icons(data_dir, asset_studio_path)
}

/// 提取 Card2D 卡牌边框 PNG。
pub fn process_card_frames(data_dir: &Path, asset_studio_path: &Path) -> anyhow::Result<()> {
    extract_card_frames(data_dir, asset_studio_path)
}

/// 渲染单张完整卡牌图。
pub fn render_card_image(
    data_dir: &Path,
    card_id: i64,
    variant: &str,
    name_font_path: Option<&Path>,
    number_font_path: Option<&Path>,
) -> anyhow::Result<PathBuf> {
    let cards = load_cards(data_dir)?;
    let base_stats = load_base_stats(data_dir)?;
    let card = cards
        .iter()
        .find(|c| c.card_id == card_id)
        .ok_or_else(|| anyhow::anyhow!("cards_full.json 中未找到 card_id={card_id}"))?;
    render_one_card(
        data_dir,
        card,
        &base_stats,
        variant,
        name_font_path,
        number_font_path,
    )
}

/// 批量渲染 cards_full.json 中当前支持的卡牌。
pub fn render_all_card_images(
    data_dir: &Path,
    variant: &str,
    name_font_path: Option<&Path>,
    number_font_path: Option<&Path>,
) -> anyhow::Result<RenderStats> {
    let cards = load_cards(data_dir)?;
    let base_stats = load_base_stats(data_dir)?;
    let pb = ProgressBar::new(cards.len() as u64);
    pb.set_style(
        ProgressStyle::with_template("{spinner} [{bar:30}] {pos}/{len} 渲染 {msg}")
            .unwrap()
            .progress_chars("=> "),
    );

    let mut stats = RenderStats::default();
    for card in &cards {
        pb.set_message(card.card_id.to_string());
        match render_one_card(
            data_dir,
            card,
            &base_stats,
            variant,
            name_font_path,
            number_font_path,
        ) {
            Ok(_) => stats.rendered += 1,
            Err(e) => {
                debug!("跳过 card_id={}: {e}", card.card_id);
                stats.skipped += 1;
            }
        }
        pb.inc(1);
    }
    pb.finish_and_clear();
    Ok(stats)
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
    Ok(m.assets
        .iter()
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
    ["Main", "Special", "Token"]
        .iter()
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
        ProgressStyle::with_template("{spinner} AssetStudio 导出中... {elapsed}").unwrap(),
    );
    spinner.enable_steady_tick(std::time::Duration::from_millis(100));

    let status = Command::new(asset_studio_path)
        .arg(&decrypted_dir)
        .args([
            "-t",
            "tex2d",
            "-g",
            "fileName",
            "-f",
            "assetName",
            "-o",
            &output_dir.to_string_lossy(),
            "-r",
            "--unity-version",
            UNITY_VERSION,
            "--log-level",
            "warning",
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
            .progress_chars("=> "),
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
            _ => {
                pb.inc(1);
                continue;
            }
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
            .progress_chars("=> "),
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
    let img = image::open(input).with_context(|| format!("无法打开图片: {}", input.display()))?;

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
// pack-icons 图标提取
// ============================================================================

/// 从解密 AssetBundle 中提取卡包 pack-icons 图标。
///
/// 源: variants/Chs/decrypted/UI/IconItem/utx_ic_item_{id}.ab
/// 输出: exports/pack-icons/{id}.png
///
/// 增量: 已存在的 PNG 跳过。
fn extract_pack_icons(data_dir: &Path, asset_studio_path: &Path) -> anyhow::Result<()> {
    use sha2::Digest;
    use std::collections::BTreeMap;
    use std::io::Read;

    let icon_source_dir = data_dir
        .join("variants")
        .join("Chs")
        .join("decrypted")
        .join("UI")
        .join("IconItem");

    if !icon_source_dir.exists() {
        anyhow::bail!(
            "图标源目录不存在: {}（请先运行 wbu asset batch -v Chs）",
            icon_source_dir.display()
        );
    }

    let output_dir = data_dir.join("exports").join("pack-icons");
    std::fs::create_dir_all(&output_dir)?;

    // 加载哈希缓存文件（id → sha256）
    let hash_cache_path = output_dir.join(".hashes.json");
    let hash_cache: BTreeMap<String, String> = if hash_cache_path.exists() {
        let raw = std::fs::read_to_string(&hash_cache_path)?;
        serde_json::from_str(&raw).unwrap_or_default()
    } else {
        BTreeMap::new()
    };

    // 动态扫描卡包图标: utx_ic_item_100\d{2}.ab（如 10000~10008）
    let icon_bundles: Vec<_> = std::fs::read_dir(&icon_source_dir)?
        .filter_map(|e| e.ok())
        .filter(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            // 卡包图标格式: utx_ic_item_100 + 两位数字 + .ab
            name.starts_with("utx_ic_item_100")
                && name.ends_with(".ab")
                && name.len() == "utx_ic_item_10000.ab".len()
        })
        .collect();

    if icon_bundles.is_empty() {
        println!("UI/IconItem 目录为空，跳过");
        return Ok(());
    }

    // 检查增量: 对每个 bundle 计算 SHA256，与缓存比较
    let mut stale_ids: Vec<String> = Vec::new();
    let mut current_hashes: BTreeMap<String, String> = BTreeMap::new();

    for entry in &icon_bundles {
        let name = entry.file_name().to_string_lossy().to_string();
        let id = name
            .strip_prefix("utx_ic_item_")
            .unwrap_or(&name)
            .strip_suffix(".ab")
            .unwrap_or(&name)
            .to_string();

        // 计算源 bundle 的 SHA256
        let mut file = std::fs::File::open(entry.path())?;
        let mut hasher = sha2::Sha256::new();
        let mut buf = [0u8; 8192];
        loop {
            let n = file.read(&mut buf)?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
        }
        let hash = format!("{:x}", hasher.finalize());

        current_hashes.insert(id.clone(), hash.clone());

        // 检查: PNG 是否存在 且 哈希与缓存一致
        let png_path = output_dir.join(format!("{}.png", &id));
        if !png_path.exists() {
            stale_ids.push(id);
        } else if hash_cache.get(&id) != Some(&hash) {
            // 哈希变了 → 源文件更新 → 需要重新提取
            stale_ids.push(id);
        }
    }

    if stale_ids.is_empty() {
        println!(
            "pack-icons 图标全部为最新（{} 个），跳过",
            icon_bundles.len()
        );
        // 更新哈希缓存（可能有新增的条目）
        let json = serde_json::to_string_pretty(&current_hashes)?;
        std::fs::write(&hash_cache_path, json)?;
        return Ok(());
    }

    println!(
        "需要更新 {} 个图标（共 {} 个，{} 个已是最新）",
        stale_ids.len(),
        icon_bundles.len(),
        icon_bundles.len() - stale_ids.len()
    );

    // 创建临时工作目录
    let temp_input = data_dir
        .join("exports")
        .join("pack-icons")
        .join(".temp_input");
    let temp_output = data_dir
        .join("exports")
        .join("pack-icons")
        .join(".temp_output");

    // 清理旧临时目录
    if temp_input.exists() {
        std::fs::remove_dir_all(&temp_input)?;
    }
    if temp_output.exists() {
        std::fs::remove_dir_all(&temp_output)?;
    }
    std::fs::create_dir_all(&temp_input)?;
    std::fs::create_dir_all(&temp_output)?;

    // 只复制需要更新的 bundle
    for entry in &icon_bundles {
        let name = entry.file_name().to_string_lossy().to_string();
        let id = name
            .strip_prefix("utx_ic_item_")
            .unwrap_or(&name)
            .strip_suffix(".ab")
            .unwrap_or(&name);
        if stale_ids.contains(&id.to_string()) {
            let src = entry.path();
            let dst = temp_input.join(entry.file_name());
            std::fs::copy(&src, &dst)?;
        }
    }

    // 运行 AssetStudio
    let spinner = ProgressBar::new_spinner();
    spinner.set_style(
        ProgressStyle::with_template("{spinner} AssetStudio 导出中... {elapsed}").unwrap(),
    );
    spinner.enable_steady_tick(std::time::Duration::from_millis(100));

    let status = Command::new(asset_studio_path)
        .arg(&temp_input)
        .args([
            "-t",
            "tex2d",
            "-g",
            "fileName",
            "-f",
            "assetName",
            "-o",
            &temp_output.to_string_lossy(),
            "--unity-version",
            UNITY_VERSION,
            "--log-level",
            "warning",
        ])
        .status()
        .with_context(|| format!("无法启动 AssetStudio: {}", asset_studio_path.display()))?;

    spinner.finish_and_clear();

    if !status.success() {
        anyhow::bail!("AssetStudio 退出码: {:?}", status.code());
    }

    // 从 AssetStudio 输出中收集 PNG
    for entry in &icon_bundles {
        let name = entry.file_name().to_string_lossy().to_string();
        let id = name
            .strip_prefix("utx_ic_item_")
            .unwrap_or(&name)
            .strip_suffix(".ab")
            .unwrap_or(&name);

        if !stale_ids.contains(&id.to_string()) {
            continue; // 未变化，跳过
        }

        let dest = output_dir.join(format!("{}.png", id));

        // AssetStudio 输出: .temp_output/{bundle_name}.ab_export/CAB-{hash}/{texture_name}.png
        let export_dir = temp_output.join(format!("{}_export", name));
        if let Ok(pngs) = find_pngs(&export_dir) {
            if let Some(png_path) = pngs.first() {
                std::fs::rename(png_path, &dest)?;
                debug!("pack-icons 图标更新: {} → {}", id, dest.display());
            }
        }
    }

    // 清理临时目录
    let _ = std::fs::remove_dir_all(&temp_input);
    let _ = std::fs::remove_dir_all(&temp_output);

    // 更新哈希缓存
    let json = serde_json::to_string_pretty(&current_hashes)?;
    std::fs::write(&hash_cache_path, json)?;

    println!("pack-icons 图标提取完成: 更新 {} 个", stale_ids.len());

    Ok(())
}

// ============================================================================
// card-frames 提取
// ============================================================================

/// 从 UI/Card2D 中提取 frame2d_*.ab 为 PNG。
fn extract_card_frames(data_dir: &Path, asset_studio_path: &Path) -> anyhow::Result<()> {
    let source_dir = data_dir
        .join("variants")
        .join("Chs")
        .join("decrypted")
        .join(CARD_FRAME_SOURCE_DIR);

    if !source_dir.exists() {
        anyhow::bail!(
            "边框源目录不存在: {}（请先运行 wbu asset batch -v Chs）",
            source_dir.display()
        );
    }

    let output_dir = data_dir.join("exports").join("card-frames");
    std::fs::create_dir_all(&output_dir)?;

    let bundles: Vec<_> = std::fs::read_dir(&source_dir)?
        .filter_map(|e| e.ok())
        .filter(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            name.starts_with("frame2d_") && name.ends_with(".ab")
        })
        .collect();

    if bundles.is_empty() {
        println!("UI/Card2D 中未找到 frame2d_*.ab，跳过");
        return Ok(());
    }

    let stale: Vec<_> = bundles
        .iter()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().to_string();
            let id = name.strip_suffix(".ab").unwrap_or(&name);
            let out = output_dir.join(format!("{id}.png"));
            (!out.exists()).then(|| entry.path())
        })
        .collect();

    if stale.is_empty() {
        println!("card-frames 全部已存在（{} 个），跳过", bundles.len());
        return Ok(());
    }

    let temp_input = output_dir.join(".temp_input");
    let temp_output = output_dir.join(".temp_output");
    if temp_input.exists() {
        std::fs::remove_dir_all(&temp_input)?;
    }
    if temp_output.exists() {
        std::fs::remove_dir_all(&temp_output)?;
    }
    std::fs::create_dir_all(&temp_input)?;
    std::fs::create_dir_all(&temp_output)?;

    for src in &stale {
        let dst = temp_input.join(src.file_name().unwrap());
        std::fs::copy(src, dst)?;
    }

    run_asset_studio_on_dir(&temp_input, &temp_output, asset_studio_path, false)?;

    let mut exported = 0usize;
    for src in &stale {
        let file_name = src.file_name().unwrap().to_string_lossy().to_string();
        let id = file_name.strip_suffix(".ab").unwrap_or(&file_name);
        let export_dir = temp_output.join(format!("{}_export", file_name));
        if let Ok(pngs) = find_pngs(&export_dir) {
            if let Some(png_path) = pngs.first() {
                std::fs::rename(png_path, output_dir.join(format!("{id}.png")))?;
                exported += 1;
            }
        }
    }

    let _ = std::fs::remove_dir_all(&temp_input);
    let _ = std::fs::remove_dir_all(&temp_output);

    println!("card-frames 提取完成: {} 个", exported);
    Ok(())
}

fn run_asset_studio_on_dir(
    input_dir: &Path,
    output_dir: &Path,
    asset_studio_path: &Path,
    recursive: bool,
) -> anyhow::Result<()> {
    let spinner = ProgressBar::new_spinner();
    spinner.set_style(
        ProgressStyle::with_template("{spinner} AssetStudio 导出中... {elapsed}").unwrap(),
    );
    spinner.enable_steady_tick(std::time::Duration::from_millis(100));

    let mut cmd = Command::new(asset_studio_path);
    cmd.arg(input_dir).args([
        "-t",
        "tex2d",
        "-g",
        "fileName",
        "-f",
        "assetName",
        "-o",
        &output_dir.to_string_lossy(),
        "--unity-version",
        UNITY_VERSION,
        "--log-level",
        "warning",
    ]);
    if recursive {
        cmd.arg("-r");
    }

    let status = cmd
        .status()
        .with_context(|| format!("无法启动 AssetStudio: {}", asset_studio_path.display()))?;

    spinner.finish_and_clear();

    if !status.success() {
        anyhow::bail!("AssetStudio 退出码: {:?}", status.code());
    }
    Ok(())
}

// ============================================================================
// card render
// ============================================================================

#[derive(Debug, Default)]
pub struct RenderStats {
    pub rendered: usize,
    pub skipped: usize,
}

#[derive(Debug, serde::Deserialize)]
struct CardRenderEntry {
    card_id: i64,
    card_style_id: i64,
    cost: Option<i64>,
    rarity: Option<i64>,
    type_flags: Option<i64>,
    is_evolution: bool,
    name_chs: String,
}

#[derive(Debug, Default)]
struct BaseStats {
    attack: Option<i64>,
    defense: Option<i64>,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct RenderConfig {
    canvas: CanvasConfig,
    art: ArtConfig,
    frame: RectConfig,
    name: TextConfig,
    cost: TextConfig,
    attack: TextConfig,
    defense: TextConfig,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct CanvasConfig {
    width: u32,
    height: u32,
    background: [u8; 4],
}

#[derive(Debug, Clone, serde::Deserialize)]
struct ArtConfig {
    x: i64,
    y: i64,
    width: u32,
    height: u32,
    crop_x: u32,
    crop_y: u32,
    crop_width: u32,
    crop_height: u32,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct RectConfig {
    x: i64,
    y: i64,
    width: u32,
    height: u32,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct TextConfig {
    center_x: i32,
    center_y: i32,
    font_size: f32,
    max_width: Option<u32>,
}

fn load_cards(data_dir: &Path) -> anyhow::Result<Vec<CardRenderEntry>> {
    let path = data_dir
        .join("exports")
        .join("analysis")
        .join("cards_full.json");
    let raw =
        std::fs::read_to_string(&path).with_context(|| format!("无法读取: {}", path.display()))?;
    serde_json::from_str(&raw).with_context(|| format!("JSON 解析失败: {}", path.display()))
}

fn load_base_stats(data_dir: &Path) -> anyhow::Result<HashMap<i64, BaseStats>> {
    let path = data_dir
        .join("exports")
        .join("master-data")
        .join("Chs")
        .join("BaseCardMaster.json");
    let raw =
        std::fs::read_to_string(&path).with_context(|| format!("无法读取: {}", path.display()))?;
    let rows: Vec<Vec<serde_json::Value>> =
        serde_json::from_str(&raw).with_context(|| format!("JSON 解析失败: {}", path.display()))?;

    let mut result = HashMap::with_capacity(rows.len());
    for row in rows {
        if let Some(card_id) = row.first().and_then(|v| v.as_i64()) {
            result.insert(
                card_id,
                BaseStats {
                    attack: row.get(5).and_then(|v| v.as_i64()),
                    defense: row.get(6).and_then(|v| v.as_i64()),
                },
            );
        }
    }
    Ok(result)
}

fn load_render_config() -> anyhow::Result<RenderConfig> {
    let path = workspace_config_path("render.toml");
    let raw =
        std::fs::read_to_string(&path).with_context(|| format!("无法读取: {}", path.display()))?;
    toml::from_str(&raw).with_context(|| format!("TOML 解析失败: {}", path.display()))
}

fn render_one_card(
    data_dir: &Path,
    card: &CardRenderEntry,
    base_stats: &HashMap<i64, BaseStats>,
    variant: &str,
    name_font_path: Option<&Path>,
    number_font_path: Option<&Path>,
) -> anyhow::Result<PathBuf> {
    if card.is_evolution {
        anyhow::bail!("暂不渲染进化派生卡");
    }

    let kind = card_kind(card.type_flags)?;
    let rarity = rarity_name(card.rarity)?;
    let art_path = card_texture_path(data_dir, card.card_style_id);
    let frame_path = data_dir
        .join("exports")
        .join("card-frames")
        .join(format!("frame2d_{kind}_{rarity}.png"));

    if !art_path.exists() {
        anyhow::bail!("卡图不存在: {}", art_path.display());
    }
    if !frame_path.exists() {
        anyhow::bail!(
            "边框不存在: {}（请先运行 wbu texture card-frames）",
            frame_path.display()
        );
    }

    let config = load_render_config()?;
    let mut canvas = RgbaImage::from_pixel(
        config.canvas.width,
        config.canvas.height,
        Rgba(config.canvas.background),
    );

    let art = image::open(&art_path)
        .with_context(|| format!("无法打开卡图: {}", art_path.display()))?
        .to_rgba8();
    let art_crop = crop_configured_art(&art, &config.art)?;
    let art_resized = DynamicImage::ImageRgba8(art_crop)
        .resize_exact(
            config.art.width,
            config.art.height,
            image::imageops::FilterType::Lanczos3,
        )
        .to_rgba8();
    image::imageops::overlay(&mut canvas, &art_resized, config.art.x, config.art.y);

    let frame = image::open(&frame_path)
        .with_context(|| format!("无法打开边框: {}", frame_path.display()))?
        .resize_exact(
            config.frame.width,
            config.frame.height,
            image::imageops::FilterType::Lanczos3,
        )
        .to_rgba8();
    image::imageops::overlay(&mut canvas, &frame, config.frame.x, config.frame.y);

    let name_font = load_name_font(name_font_path)?;
    let number_font = load_number_font(number_font_path)?;
    draw_label_text(
        &mut canvas,
        &name_font,
        &card.name_chs,
        config.name.center_x,
        config.name.center_y,
        config.name.font_size,
        config.name.max_width.unwrap_or(i32::MAX as u32) as i32,
    );
    if let Some(cost) = card.cost {
        draw_centered_text(
            &mut canvas,
            &number_font,
            &cost.to_string(),
            config.cost.center_x,
            config.cost.center_y,
            config.cost.font_size,
            Rgba([255, 255, 255, 255]),
        );
    }
    if kind == "follower" {
        let stats = base_stats.get(&card.card_id);
        if let Some(attack) = stats.and_then(|s| s.attack) {
            draw_centered_text(
                &mut canvas,
                &number_font,
                &attack.to_string(),
                config.attack.center_x,
                config.attack.center_y,
                config.attack.font_size,
                Rgba([255, 255, 255, 255]),
            );
        }
        if let Some(defense) = stats.and_then(|s| s.defense) {
            draw_centered_text(
                &mut canvas,
                &number_font,
                &defense.to_string(),
                config.defense.center_x,
                config.defense.center_y,
                config.defense.font_size,
                Rgba([255, 255, 255, 255]),
            );
        }
    }

    let output_dir = data_dir
        .join("exports")
        .join("card-renders")
        .join(variant)
        .join(card_texture_category(card.card_style_id));
    std::fs::create_dir_all(&output_dir)?;
    let out = output_dir.join(format!("{}.png", card.card_id));
    DynamicImage::ImageRgba8(canvas).save(&out)?;
    Ok(out)
}

fn crop_configured_art(image: &RgbaImage, config: &ArtConfig) -> anyhow::Result<RgbaImage> {
    let x = config.crop_x.min(image.width().saturating_sub(1));
    let y = config.crop_y.min(image.height().saturating_sub(1));
    let width = config.crop_width.min(image.width().saturating_sub(x));
    let height = config.crop_height.min(image.height().saturating_sub(y));
    if width == 0 || height == 0 {
        anyhow::bail!("render.toml 的 art crop 区域为空");
    }
    Ok(image::imageops::crop_imm(image, x, y, width, height).to_image())
}

fn card_kind(type_flags: Option<i64>) -> anyhow::Result<&'static str> {
    let flags = type_flags.unwrap_or(0);
    if flags & 1 != 0 {
        Ok("follower")
    } else if flags & 2 != 0 {
        Ok("spell")
    } else if flags & 4 != 0 {
        Ok("amulet")
    } else {
        anyhow::bail!("不支持的 type_flags={flags}")
    }
}

fn rarity_name(rarity: Option<i64>) -> anyhow::Result<&'static str> {
    match rarity.unwrap_or(0) {
        1 => Ok("bronze"),
        2 => Ok("silver"),
        3 => Ok("gold"),
        4 => Ok("legend"),
        r => anyhow::bail!("不支持的 rarity={r}"),
    }
}

fn card_texture_path(data_dir: &Path, card_style_id: i64) -> PathBuf {
    data_dir
        .join("exports")
        .join("card-textures-resized")
        .join(card_texture_category(card_style_id))
        .join(format!("{card_style_id}.png"))
}

fn card_texture_category(card_style_id: i64) -> &'static str {
    match card_style_id.to_string().chars().next() {
        Some('8') => "Special",
        Some('9') => "Token",
        _ => "Main",
    }
}

fn load_name_font(font_path: Option<&Path>) -> anyhow::Result<FontArc> {
    if let Some(path) = font_path {
        return load_font_file(path)
            .with_context(|| format!("无法加载卡名字体: {}", path.display()));
    }
    load_first_font(&[
        None,
        Some(workspace_font_path("dfweibeiw7-gb.ttc")),
        Some(PathBuf::from(r"C:\Windows\Fonts\NotoSansSC-VF.ttf")),
        Some(PathBuf::from(r"C:\Windows\Fonts\simhei.ttf")),
        Some(PathBuf::from(r"C:\Windows\Fonts\msyhbd.ttc")),
    ])
    .context("无法加载卡名字体，请用 --font 指定 ttf/otf 字体")
}

fn load_number_font(font_path: Option<&Path>) -> anyhow::Result<FontArc> {
    if let Some(path) = font_path {
        return load_font_file(path)
            .with_context(|| format!("无法加载数字字体: {}", path.display()));
    }
    load_first_font(&[
        None,
        Some(workspace_font_path("Junicode-Bold.ttf")),
        Some(PathBuf::from(r"C:\Windows\Fonts\seguisb.ttf")),
        Some(PathBuf::from(r"C:\Windows\Fonts\arial.ttf")),
    ])
    .context("无法加载数字字体，请用 --number-font 指定 ttf/otf 字体")
}

fn workspace_font_path(file_name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("assets")
        .join("fonts")
        .join(file_name)
}

fn workspace_config_path(file_name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("config")
        .join(file_name)
}

fn load_first_font(candidates: &[Option<PathBuf>]) -> anyhow::Result<FontArc> {
    for path in candidates.iter().flatten() {
        if !path.exists() {
            continue;
        }
        if let Ok(font) = load_font_file(path) {
            return Ok(font);
        }
    }
    anyhow::bail!("没有可用字体候选")
}

fn load_font_file(path: &Path) -> anyhow::Result<FontArc> {
    let bytes = std::fs::read(path)?;
    FontArc::try_from_vec(bytes)
        .map_err(|_| anyhow::anyhow!("字体格式不受支持或文件损坏: {}", path.display()))
}

fn draw_label_text(
    image: &mut RgbaImage,
    font: &FontArc,
    text: &str,
    center_x: i32,
    baseline_y: i32,
    mut size: f32,
    max_width: i32,
) {
    let clean = strip_ruby_tags(text);
    while size > 24.0 {
        let (w, _) = text_size(PxScale::from(size), font, &clean);
        if w <= max_width as u32 {
            break;
        }
        size -= 2.0;
    }
    draw_centered_text(
        image,
        font,
        &clean,
        center_x,
        baseline_y,
        size,
        Rgba([255, 255, 255, 255]),
    );
}

fn draw_centered_text(
    image: &mut RgbaImage,
    font: &FontArc,
    text: &str,
    center_x: i32,
    center_y: i32,
    size: f32,
    color: Rgba<u8>,
) {
    let scale = PxScale::from(size);
    let (w, h) = text_size(scale, font, text);
    let x = center_x - (w as i32 / 2);
    let y = center_y - (h as i32 / 2);
    draw_text_with_shadow(image, font, text, x, y, size, color);
}

fn draw_text_with_shadow(
    image: &mut RgbaImage,
    font: &FontArc,
    text: &str,
    x: i32,
    y: i32,
    size: f32,
    color: Rgba<u8>,
) {
    let scale = PxScale::from(size);
    let shadow = Rgba([0, 0, 0, 210]);
    for (dx, dy) in [(-2, 0), (2, 0), (0, -2), (0, 2), (2, 2)] {
        draw_text_mut(image, shadow, x + dx, y + dy, scale, font, text);
    }
    draw_text_mut(image, color, x, y, scale, font, text);
}

fn strip_ruby_tags(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_tag = false;
    for ch in text.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    out
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
