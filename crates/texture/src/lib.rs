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

/// 卡牌职业图标资源目录。
const CARD_CLASS_ICON_SOURCE_FILE: &str = "Atlas/Card2D.ab";

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

/// 使用自定义资源渲染卡牌（不依赖 master data）。
pub fn render_custom_card(
    data_dir: &Path,
    image_path: &Path,
    card_name: &str,
    kind: &str,
    cost: Option<i64>,
    attack: Option<i64>,
    life: Option<i64>,
    class: Option<i64>,
    rarity: Option<&str>,
    variant: &str,
    name_font_path: Option<&Path>,
    number_font_path: Option<&Path>,
) -> anyhow::Result<PathBuf> {
    if !["follower", "spell", "amulet"].contains(&kind) {
        anyhow::bail!("--type 必须是 follower / spell / amulet，当前值为 {kind}");
    }
    if !image_path.exists() {
        anyhow::bail!("资源图片不存在: {}", image_path.display());
    }

    // 边框
    let rarity_name = rarity.and_then(|r| rarity_name_from_str(r).ok()).unwrap_or("bronze");
    let frame_path = data_dir
        .join("exports")
        .join("card-frames")
        .join(format!("frame2d_{kind}_{rarity_name}.png"));
    if !frame_path.exists() {
        anyhow::bail!(
            "边框不存在: {}（请先运行 wbu texture card-frames）",
            frame_path.display()
        );
    }

    // 职业图标
    let class_icon_path = if let Some(cls) = class {
        if cls > 7 {
            anyhow::bail!("--class 必须是 0-7，当前值为 {cls}");
        }
        let p = data_dir
            .join("exports")
            .join("card-class-icons")
            .join(format!("card2d_class_icon_{cls}.png"));
        if !p.exists() {
            anyhow::bail!(
                "职业图标不存在: {}（请先运行 wbu texture card-frames）",
                p.display()
            );
        }
        Some(p)
    } else {
        None
    };

    let config = load_render_config()?;
    let layout = resolve_render_config(&config, kind);
    let mut canvas = RgbaImage::from_pixel(
        config.canvas.width,
        config.canvas.height,
        Rgba(config.canvas.background),
    );

    // 卡图
    let art = image::open(image_path)
        .with_context(|| format!("无法打开卡图: {}", image_path.display()))?
        .to_rgba8();
    let art_crop = crop_configured_art(&art, &layout.art)?;
    let art_resized = DynamicImage::ImageRgba8(art_crop)
        .resize_exact(
            layout.art.width,
            layout.art.height,
            image::imageops::FilterType::Lanczos3,
        )
        .to_rgba8();
    image::imageops::overlay(&mut canvas, &art_resized, layout.art.x, layout.art.y);

    // 边框
    let frame = image::open(&frame_path)
        .with_context(|| format!("无法打开边框: {}", frame_path.display()))?
        .resize_exact(
            layout.frame.width,
            layout.frame.height,
            image::imageops::FilterType::Lanczos3,
        )
        .to_rgba8();
    image::imageops::overlay(&mut canvas, &frame, layout.frame.x, layout.frame.y);

    // 职业图标
    if let (Some(icon_config), Some(icon_path)) = (&layout.class_icon, &class_icon_path) {
        let class_icon = image::open(icon_path)
            .with_context(|| format!("无法打开职业图标: {}", icon_path.display()))?
            .resize_exact(
                icon_config.width,
                icon_config.height,
                image::imageops::FilterType::Lanczos3,
            )
            .to_rgba8();
        image::imageops::overlay(&mut canvas, &class_icon, icon_config.x, icon_config.y);
    }

    // 卡名
    let name_font = load_name_font(name_font_path)?;
    draw_label_text(
        &mut canvas,
        &name_font,
        card_name,
        layout.name.center_x,
        layout.name.center_y,
        layout.name.font_size,
        layout.name.max_width.unwrap_or(i32::MAX as u32) as i32,
    );

    // cost
    if let Some(c) = cost {
        let number_font = load_number_font(number_font_path)?;
        draw_label_text(
            &mut canvas,
            &number_font,
            &c.to_string(),
            layout.cost.center_x,
            layout.cost.center_y,
            layout.cost.font_size,
            layout.cost.max_width.unwrap_or(i32::MAX as u32) as i32,
        );
    }

    // attack / life（仅 follower）
    if kind == "follower" {
        let number_font = load_number_font(number_font_path)?;
        if let Some(atk) = attack {
            draw_label_text(
                &mut canvas,
                &number_font,
                &atk.to_string(),
                layout.attack.center_x,
                layout.attack.center_y,
                layout.attack.font_size,
                layout.attack.max_width.unwrap_or(i32::MAX as u32) as i32,
            );
        }
        if let Some(lf) = life {
            draw_label_text(
                &mut canvas,
                &number_font,
                &lf.to_string(),
                layout.defense.center_x,
                layout.defense.center_y,
                layout.defense.font_size,
                layout.defense.max_width.unwrap_or(i32::MAX as u32) as i32,
            );
        }
    }

    // 输出
    let out_dir = data_dir
        .join("exports")
        .join("card-renders")
        .join(variant)
        .join("Custom");
    std::fs::create_dir_all(&out_dir)?;
    // 用文件名 stem 作为输出名
    let stem = image_path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "custom".to_string());
    let out_path = out_dir.join(format!("{stem}.png"));
    canvas.save(&out_path)?;
    Ok(out_path)
}

/// 以 card_id 读取 master data 为底，用命令行参数覆盖任意字段后渲染。
/// 只要传了 --res 就输出到 Custom/ 目录。
pub fn render_card_with_overrides(
    data_dir: &Path,
    card_id: i64,
    res: Option<&str>,
    name: Option<&str>,
    kind_override: Option<&str>,
    cost_override: Option<i64>,
    attack_override: Option<i64>,
    life_override: Option<i64>,
    class_override: Option<i64>,
    rarity_override: Option<&str>,
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

    if card.is_evolution {
        anyhow::bail!("暂不渲染进化派生卡");
    }

    let kind = kind_override.unwrap_or_else(|| {
        card_kind(card.card_id).unwrap_or("follower")
    });
    let rarity = if let Some(r) = rarity_override {
        rarity_name_from_str(r)?
    } else {
        rarity_name(card.rarity)?
    };
    let art_path = if let Some(p) = res {
        std::path::PathBuf::from(p)
    } else {
        card_texture_path(data_dir, card.card_style_id)
    };
    let frame_path = data_dir
        .join("exports")
        .join("card-frames")
        .join(format!("frame2d_{kind}_{rarity}.png"));
    let class_icon_path = if let Some(cls) = class_override {
        if cls > 7 {
            anyhow::bail!("--class 必须是 0-7，当前值为 {cls}");
        }
        let p = data_dir
            .join("exports")
            .join("card-class-icons")
            .join(format!("card2d_class_icon_{cls}.png"));
        if !p.exists() {
            anyhow::bail!("职业图标不存在: {}", p.display());
        }
        Some(p)
    } else {
        let p = class_icon_path(data_dir, card.card_id)?;
        p.exists().then_some(p)
    };

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
    let layout = resolve_render_config(&config, kind);
    let mut canvas = RgbaImage::from_pixel(
        config.canvas.width,
        config.canvas.height,
        Rgba(config.canvas.background),
    );

    let art = image::open(&art_path)
        .with_context(|| format!("无法打开卡图: {}", art_path.display()))?
        .to_rgba8();
    let art_crop = crop_configured_art(&art, &layout.art)?;
    let art_resized = DynamicImage::ImageRgba8(art_crop)
        .resize_exact(
            layout.art.width,
            layout.art.height,
            image::imageops::FilterType::Lanczos3,
        )
        .to_rgba8();
    image::imageops::overlay(&mut canvas, &art_resized, layout.art.x, layout.art.y);

    let frame = image::open(&frame_path)
        .with_context(|| format!("无法打开边框: {}", frame_path.display()))?
        .resize_exact(
            layout.frame.width,
            layout.frame.height,
            image::imageops::FilterType::Lanczos3,
        )
        .to_rgba8();
    image::imageops::overlay(&mut canvas, &frame, layout.frame.x, layout.frame.y);

    if let (Some(icon_config), Some(icon_path)) = (&layout.class_icon, &class_icon_path) {
        let class_icon = image::open(icon_path)
            .with_context(|| format!("无法打开职业图标: {}", icon_path.display()))?
            .resize_exact(
                icon_config.width,
                icon_config.height,
                image::imageops::FilterType::Lanczos3,
            )
            .to_rgba8();
        image::imageops::overlay(&mut canvas, &class_icon, icon_config.x, icon_config.y);
    }

    let name_font = load_name_font(name_font_path)?;
    let display_name = name.unwrap_or(&card.name_chs);
    draw_label_text(
        &mut canvas,
        &name_font,
        display_name,
        layout.name.center_x,
        layout.name.center_y,
        layout.name.font_size,
        layout.name.max_width.unwrap_or(i32::MAX as u32) as i32,
    );

    let number_font = load_number_font(number_font_path)?;
    let cost_val = cost_override.or(card.cost);
    if let Some(c) = cost_val {
        draw_label_text(
            &mut canvas,
            &number_font,
            &c.to_string(),
            layout.cost.center_x,
            layout.cost.center_y,
            layout.cost.font_size,
            layout.cost.max_width.unwrap_or(i32::MAX as u32) as i32,
        );
    }

    if kind == "follower" {
        let stats = base_stats.get(&card.card_id);
        let atk = attack_override.or_else(|| stats.and_then(|s| s.attack));
        let def = life_override.or_else(|| stats.and_then(|s| s.defense));
        if let Some(a) = atk {
            draw_label_text(
                &mut canvas,
                &number_font,
                &a.to_string(),
                layout.attack.center_x,
                layout.attack.center_y,
                layout.attack.font_size,
                layout.attack.max_width.unwrap_or(i32::MAX as u32) as i32,
            );
        }
        if let Some(d) = def {
            draw_label_text(
                &mut canvas,
                &number_font,
                &d.to_string(),
                layout.defense.center_x,
                layout.defense.center_y,
                layout.defense.font_size,
                layout.defense.max_width.unwrap_or(i32::MAX as u32) as i32,
            );
        }
    }

    // 输出：有任意覆盖参数都走 Custom/，避免覆盖正常渲染结果
    let out_dir = {
        data_dir
            .join("exports")
            .join("card-renders")
            .join(variant)
            .join("Custom")
    };
    std::fs::create_dir_all(&out_dir)?;
    let out_path = out_dir.join(format!("{card_id}.png"));
    canvas.save(&out_path)?;
    Ok(out_path)
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
        return extract_card_class_icons(data_dir, asset_studio_path);
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
    extract_card_class_icons(data_dir, asset_studio_path)?;
    Ok(())
}
/// 从 Atlas/Card2D.ab (Sprite 图集) 中提取职业图标 PNG（含普通和 _high_premium 两版本）。
fn extract_card_class_icons(data_dir: &Path, asset_studio_path: &Path) -> anyhow::Result<()> {
    let source_file = data_dir
        .join("variants")
        .join("Chs")
        .join("decrypted")
        .join(CARD_CLASS_ICON_SOURCE_FILE);

    if !source_file.exists() {
        anyhow::bail!(
            "职业图标源文件不存在: {}（请先运行 wbu asset batch -v Chs）",
            source_file.display()
        );
    }

    let output_dir = data_dir.join("exports").join("card-class-icons");
    std::fs::create_dir_all(&output_dir)?;

    // 检查是否已有全部 8 个图标（含两版本）
    let all_exist = (0..=7).all(|idx| {
        output_dir.join(format!("card2d_class_icon_{idx}.png")).exists()
            && output_dir
                .join(format!("card2d_class_icon_{idx}_high_premium.png"))
                .exists()
    });

    if all_exist {
        println!("card-class-icons 全部已存在（8×2 个），跳过");
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

    std::fs::copy(&source_file, temp_input.join("Card2D.ab"))?;

// 导出 Sprite（图集中的各个精灵），而不是 Texture2D
    {
        let spinner = ProgressBar::new_spinner();
        spinner.set_style(
            ProgressStyle::with_template("{spinner} AssetStudio 导出 Sprite 中... {elapsed}").unwrap(),
        );
        spinner.enable_steady_tick(std::time::Duration::from_millis(100));

        let status = Command::new(asset_studio_path)
            .arg(&temp_input)
            .args([
                "-t",
                "sprite",
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
    }

    let mut exported = 0usize;
    for class_idx in 0..=7 {
        // 两个版本分别提取
        let variants = [
            ("", "card2d_class_icon"),
            ("_high_premium", "card2d_class_icon"),
        ];
        for (suffix, prefix) in &variants {
            let src_name = format!("{prefix}_{class_idx}{suffix}.png");
            let dst_name = format!("card2d_class_icon_{class_idx}{suffix}.png");
            let src = temp_output.join("Atlas").join(&src_name);
            if src.exists() {
                std::fs::rename(&src, output_dir.join(&dst_name))?;
                exported += 1;
            }
        }
    }

    let _ = std::fs::remove_dir_all(&temp_input);
    let _ = std::fs::remove_dir_all(&temp_output);
    println!("card-class-icons 提取完成: {} 个", exported);
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
    class: i64,
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
    class_icon: Option<RectConfig>,
    name: TextConfig,
    cost: TextConfig,
    attack: TextConfig,
    defense: TextConfig,
    #[serde(default)]
    follower: KindRenderConfig,
    #[serde(default)]
    spell: KindRenderConfig,
    #[serde(default)]
    amulet: KindRenderConfig,
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

#[derive(Debug, Clone, Default, serde::Deserialize)]
struct KindRenderConfig {
    art: Option<ArtConfig>,
    frame: Option<RectConfig>,
    class_icon: Option<RectConfig>,
    name: Option<TextConfig>,
    cost: Option<TextConfig>,
    attack: Option<TextConfig>,
    defense: Option<TextConfig>,
}

#[derive(Debug, Clone)]
struct ResolvedRenderConfig {
    art: ArtConfig,
    frame: RectConfig,
    class_icon: Option<RectConfig>,
    name: TextConfig,
    cost: TextConfig,
    attack: TextConfig,
    defense: TextConfig,
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

fn resolve_render_config(config: &RenderConfig, kind: &str) -> ResolvedRenderConfig {
    let override_config = match kind {
        "follower" => &config.follower,
        "spell" => &config.spell,
        "amulet" => &config.amulet,
        _ => &config.follower,
    };

    ResolvedRenderConfig {
        art: override_config
            .art
            .clone()
            .unwrap_or_else(|| config.art.clone()),
        frame: override_config
            .frame
            .clone()
            .unwrap_or_else(|| config.frame.clone()),
        class_icon: override_config
            .class_icon
            .clone()
            .or_else(|| config.class_icon.clone()),
        name: override_config
            .name
            .clone()
            .unwrap_or_else(|| config.name.clone()),
        cost: override_config
            .cost
            .clone()
            .unwrap_or_else(|| config.cost.clone()),
        attack: override_config
            .attack
            .clone()
            .unwrap_or_else(|| config.attack.clone()),
        defense: override_config
            .defense
            .clone()
            .unwrap_or_else(|| config.defense.clone()),
    }
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

    let kind = card_kind(card.card_id)?;
    let rarity = rarity_name(card.rarity)?;
    let art_path = card_texture_path(data_dir, card.card_style_id);
    let frame_path = data_dir
        .join("exports")
        .join("card-frames")
        .join(format!("frame2d_{kind}_{rarity}.png"));
    let class_icon_path = class_icon_path(data_dir, card.card_id)?;

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
    let layout = resolve_render_config(&config, kind);
    let mut canvas = RgbaImage::from_pixel(
        config.canvas.width,
        config.canvas.height,
        Rgba(config.canvas.background),
    );

    let art = image::open(&art_path)
        .with_context(|| format!("无法打开卡图: {}", art_path.display()))?
        .to_rgba8();
    let art_crop = crop_configured_art(&art, &layout.art)?;
    let art_resized = DynamicImage::ImageRgba8(art_crop)
        .resize_exact(
            layout.art.width,
            layout.art.height,
            image::imageops::FilterType::Lanczos3,
        )
        .to_rgba8();
    image::imageops::overlay(&mut canvas, &art_resized, layout.art.x, layout.art.y);

    let frame = image::open(&frame_path)
        .with_context(|| format!("无法打开边框: {}", frame_path.display()))?
        .resize_exact(
            layout.frame.width,
            layout.frame.height,
            image::imageops::FilterType::Lanczos3,
        )
        .to_rgba8();
    image::imageops::overlay(&mut canvas, &frame, layout.frame.x, layout.frame.y);

    if let Some(class_icon_config) = &layout.class_icon {
        if !class_icon_path.exists() {
            anyhow::bail!(
                "职业图标不存在: {}（请先运行 wbu texture card-frames）",
                class_icon_path.display()
            );
        }
        let class_icon = image::open(&class_icon_path)
            .with_context(|| format!("无法打开职业图标: {}", class_icon_path.display()))?
            .resize_exact(
                class_icon_config.width,
                class_icon_config.height,
                image::imageops::FilterType::Lanczos3,
            )
            .to_rgba8();
        image::imageops::overlay(
            &mut canvas,
            &class_icon,
            class_icon_config.x,
            class_icon_config.y,
        );
    }

    let name_font = load_name_font(name_font_path)?;
    let number_font = load_number_font(number_font_path)?;
    draw_label_text(
        &mut canvas,
        &name_font,
        &card.name_chs,
        layout.name.center_x,
        layout.name.center_y,
        layout.name.font_size,
        layout.name.max_width.unwrap_or(i32::MAX as u32) as i32,
    );
    if let Some(cost) = card.cost {
        draw_label_text(
            &mut canvas,
            &number_font,
            &cost.to_string(),
            layout.cost.center_x,
            layout.cost.center_y,
            layout.cost.font_size,
            layout.cost.max_width.unwrap_or(i32::MAX as u32) as i32,
        );
    }
    if kind == "follower" {
        let stats = base_stats.get(&card.card_id);
        if let Some(attack) = stats.and_then(|s| s.attack) {
            draw_label_text(
                &mut canvas,
                &number_font,
                &attack.to_string(),
                layout.attack.center_x,
                layout.attack.center_y,
                layout.attack.font_size,
                layout.attack.max_width.unwrap_or(i32::MAX as u32) as i32,
            );
        }
        if let Some(defense) = stats.and_then(|s| s.defense) {
            draw_label_text(
                &mut canvas,
                &number_font,
                &defense.to_string(),
                layout.defense.center_x,
                layout.defense.center_y,
                layout.defense.font_size,
                layout.defense.max_width.unwrap_or(i32::MAX as u32) as i32,
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

/// 从 card_id 第 6 位（0-indexed 位置 5）推断卡牌种类。
/// 参考 WBArts: 1 = follower, 2 = amulet, 3 = spell.
fn card_kind(card_id: i64) -> anyhow::Result<&'static str> {
    let s = card_id.to_string();
    let digit = s.as_bytes().get(5).map(|b| b - b'0').unwrap_or(0);
    match digit {
        1 => Ok("follower"),
        2 => Ok("amulet"),
        3 => Ok("spell"),
        d => anyhow::bail!("不支持的 card_id[5]={d} (card_id={card_id})"),
    }
}

/// 从 card_id 第 4 位（0-indexed 位置 3）取职业图标编号。
/// 参考 WBArts: cls_digit = int(cid_str[3]).
fn class_icon_path(data_dir: &Path, card_id: i64) -> anyhow::Result<PathBuf> {
    let s = card_id.to_string();
    let class_idx = s.as_bytes().get(3).map(|b| b - b'0').unwrap_or(0);
    if class_idx > 7 {
        anyhow::bail!("不支持的 card_id[3]={class_idx} (card_id={card_id})");
    }
    Ok(data_dir
        .join("exports")
        .join("card-class-icons")
        .join(format!("card2d_class_icon_{class_idx}.png")))
}

fn rarity_name_from_str(rarity: &str) -> anyhow::Result<&'static str> {
    match rarity {
        "bronze" => Ok("bronze"),
        "silver" => Ok("silver"),
        "gold" => Ok("gold"),
        "legend" => Ok("legend"),
        r => anyhow::bail!("不支持的 rarity={r}，可选: bronze / silver / gold / legend"),
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

    #[test]
    fn test_card_kind_mapping() {
        // card_id[5] = 1 -> follower, 2 -> amulet, 3 -> spell
        assert_eq!(card_kind(10001110).unwrap(), "follower");  // 不屈的剑斗士
        assert_eq!(card_kind(10001210).unwrap(), "amulet");    // 侦探的放大镜
        assert_eq!(card_kind(10012310).unwrap(), "spell");     // 昆虫的忠告
        assert!(card_kind(65401010).is_err()); // cid[5]=0 = leader
        assert!(card_kind(65044910).is_err()); // cid[5]=9 = token
    }
}
