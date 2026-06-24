//! HomeIllustration Spine 动画提取模块
//!
//! 从 `Prefabs/UI/HomeIllustration/hi_*.ab` 中提取 Spine 动画资源，
//! 并生成可直接用于 Web/Godot 的清理目录。
//!
//! # 输出结构
//!
//! ```text
//! exports/home-illustration/
//!   hi_1001/
//!     spine_hi_1001.skel
//!     spine_hi_1001.atlas
//!     spine_hi_1001.png
//!     bg_hi_1001.png
//!     config.json            ← 交互参数 + 角色名 + 语音引用
//!     fx/                    ← 特效贴图（可选）
//!       ef_dust001.png
//!       ...
//!     voice/                 ← 语音文件（可选，需先运行 wbu audio 提取）
//!       Play_dx_home_1001_1.wav
//!       Play_dx_home_1001_2.wav
//!       ...
//!   hi_1002/
//!     ...
//! ```

use anyhow::{Context, bail};
use indicatif::{ProgressBar, ProgressStyle};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

// ============================================================================
// 常量
// ============================================================================

const UNITY_VERSION: &str = "2022.3.62f2";
const HOME_ILLUST_DIR: &str = "Prefabs/UI/HomeIllustration";
const HOME_ILLUST_CONFIG_VERSION: u32 = 5;

/// 需要跳过的非插画资源
const SKIP_NAMES: &[&str] = &["HomeIllustBG", "UIHomeIllustMessageWindow"];

// ============================================================================
// 数据结构
// ============================================================================

#[derive(Debug, Default)]
pub struct HomeIllustStats {
    pub processed: usize,
    pub skipped: usize,
    pub failed: usize,
}

#[derive(Debug, serde::Serialize)]
struct AspectLayout {
    x: f64,
    y: f64,
    scale_x: f64,
    scale_y: f64,
}

#[derive(Debug, Clone, Copy, Default, serde::Serialize)]
struct Vec3 {
    x: f64,
    y: f64,
    z: f64,
}

#[derive(Debug, Clone, Copy, Default, serde::Serialize)]
struct HomePosition {
    x: f64,
    y: f64,
}

#[derive(Debug, Clone, Default, serde::Serialize)]
struct PrefabTransformNode {
    name: String,
    local_position: Vec3,
    local_scale: Vec3,
    world_position: Vec3,
    world_scale: Vec3,
}

#[derive(Debug, Clone, Default, serde::Serialize)]
struct PrefabTransforms {
    prefab_scale: f64,
    nodes: BTreeMap<String, PrefabTransformNode>,
}

#[derive(Debug, Clone, serde::Serialize)]
struct LayoutDebugTransform {
    key: String,
    name: String,
    path_id: i64,
    parent_path_id: i64,
    local_position: Vec3,
    local_scale: Vec3,
    world_position: Vec3,
    world_scale: Vec3,
}

#[derive(Debug, Clone, serde::Serialize)]
struct LayoutDebugAspectDefine {
    label: String,
    aspect: f64,
    local_position: HomePosition,
    local_scale: HomePosition,
}

#[derive(Debug, Clone, serde::Serialize)]
struct LayoutDebug {
    version: u32,
    notes: Vec<String>,
    skeleton_scale: f64,
    home_position: Option<HomePosition>,
    aspect_defines: Vec<LayoutDebugAspectDefine>,
    root_chain: Vec<LayoutDebugTransform>,
    spine_chain: Vec<LayoutDebugTransform>,
    background_chain: Vec<LayoutDebugTransform>,
    background_quad_world_scale: Option<Vec3>,
    prefab_scale: f64,
}

/// 从主数据预加载的映射表（仅用于角色名查找）
struct HomeIllustMeta {
    /// leader_skin_id → 角色名（日文，来自 LeaderSkinMaster）
    leader_names: HashMap<i64, String>,
    /// card_style_id → name_label（来自 CardText col 1）
    card_name_labels: HashMap<i64, String>,
    /// variant → (label → 本地化文本)
    text_labels: HashMap<String, HashMap<String, String>>,
    /// hi_id → HomeIllustrationMaster 坐标。Unity UI 坐标以画面中心为原点。
    home_positions: HashMap<i64, HomePosition>,
}

/// 最终输出的 config.json
#[derive(Debug, serde::Serialize)]
struct IllustConfig {
    config_version: u32,
    /// hi_ ID 字符串
    id: String,
    /// 源 AssetBundle 的 SHA256，用于可更新增量导出。
    source_hash: String,
    /// 角色名（日文/通用）
    character_name: Option<String>,
    /// 各语言本地化名 { "chs": "...", "eng": "...", ... }
    character_names: BTreeMap<String, String>,
    /// "home" | "battle"
    illust_type: String,
    /// Wwise voice_prefix（dx_home_{id}）
    voice_prefix: Option<String>,
    /// 推荐语音文件（相对于 voice/ 子目录）
    voice_files: Vec<String>,
    /// 默认循环动画名
    idle_animation: String,
    tap_animations: Vec<String>,
    blend_times: Vec<f32>,
    default_mix: f32,
    #[serde(rename = "loop")]
    r#loop: bool,
    physics_position_factor: [f32; 2],
    physics_rotation_factor: f32,
    has_screen_blend: bool,
    skeleton_scale: f64,
    home_position: Option<HomePosition>,
    prefab_transforms: PrefabTransforms,
    aspect_layouts: BTreeMap<String, AspectLayout>,
    #[serde(skip_serializing_if = "Option::is_none")]
    layout_debug: Option<LayoutDebug>,
    bg_textures: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    switch_targets: Vec<String>,
    has_effects: bool,
}

/// 提取上下文
struct ExtractCtx<'a> {
    meta: &'a HomeIllustMeta,
    asset_studio_path: &'a Path,
    layout_debug: bool,
}

// ============================================================================
// 公共 API
// ============================================================================

/// 批量提取 HomeIllustration Spine 动画
pub fn process_home_illustrations(
    data_dir: &Path,
    asset_studio_path: &Path,
    vgmstream_path: &Path,
    copy_voices: bool,
    layout_debug: bool,
) -> anyhow::Result<HomeIllustStats> {
    // 0. 预加载主数据映射
    let meta = load_master_data(data_dir)?;

    let decrypted_dir = data_dir
        .join("variants")
        .join("Chs")
        .join("decrypted")
        .join(HOME_ILLUST_DIR);

    if !decrypted_dir.exists() {
        bail!(
            "HomeIllustration 目录不存在: {}（请先运行 wbu asset batch -v Chs）",
            decrypted_dir.display()
        );
    }

    let output_root = data_dir.join("exports").join("home-illustration");
    fs::create_dir_all(&output_root)?;

    // 收集所有 hi_*.ab 文件
    let mut ab_files: Vec<PathBuf> = Vec::new();
    for entry in fs::read_dir(&decrypted_dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if name_str.ends_with(".ab") && name_str.starts_with("hi_") {
            let stem = name_str.trim_end_matches(".ab").to_string();
            if !SKIP_NAMES.contains(&stem.as_str()) {
                ab_files.push(entry.path());
            }
        }
    }
    ab_files.sort();

    if ab_files.is_empty() {
        println!("没有找到 HomeIllustration hi_*.ab 文件");
        return Ok(HomeIllustStats::default());
    }

    let pb = ProgressBar::new(ab_files.len() as u64);
    pb.set_style(
        ProgressStyle::with_template(
            "{spinner:.green} [{elapsed_precise}] [{bar:30.cyan/blue}] {pos}/{len} {msg}",
        )
        .unwrap()
        .progress_chars("=> "),
    );

    let ctx = ExtractCtx {
        meta: &meta,
        asset_studio_path,
        layout_debug,
    };

    let mut stats = HomeIllustStats::default();

    for ab_path in &ab_files {
        let stem = ab_path.file_stem().unwrap().to_string_lossy().to_string();
        let output_dir = output_root.join(&stem);

        pb.set_message(stem.clone());

        let source_hash = sha256_file(ab_path)?;

        // 增量跳过：只有源 AssetBundle hash 未变化时才跳过。
        if output_dir.join("config.json").exists()
            && config_source_hash(&output_dir.join("config.json")).as_deref() == Some(&source_hash)
            && config_version(&output_dir.join("config.json")) == Some(HOME_ILLUST_CONFIG_VERSION)
            && (!layout_debug || config_has_layout_debug(&output_dir.join("config.json")))
        {
            stats.skipped += 1;
            pb.inc(1);
            continue;
        }

        match extract_one(ab_path, &output_dir, &ctx, source_hash) {
            Ok(()) => stats.processed += 1,
            Err(e) => {
                tracing::error!("{stem}: {e}");
                stats.failed += 1;
                let _ = fs::remove_dir_all(&output_dir);
            }
        }
        pb.inc(1);
    }

    pb.finish_and_clear();

    // 后处理：复制语音
    if copy_voices {
        let mut voice_copied = 0usize;
        for ab_path in &ab_files {
            let stem = ab_path.file_stem().unwrap().to_string_lossy().to_string();
            let output_dir = output_root.join(&stem);
            if copy_voice_files(&stem, &output_dir, data_dir, vgmstream_path).unwrap_or(0) > 0 {
                voice_copied += 1;
            }
        }
        if voice_copied > 0 {
            println!("语音文件已复制到 {} 个目录", voice_copied);
        }
    }

    Ok(stats)
}

// ============================================================================
// 主数据加载
// ============================================================================

/// 从导出的主数据 JSON 中加载角色名映射
fn load_master_data(data_dir: &Path) -> anyhow::Result<HomeIllustMeta> {
    let master_root = data_dir.join("exports").join("master-data");

    // 1. LeaderSkinMaster → leader_skin_id → name (any variant, all have Japanese)
    let mut leader_names = HashMap::new();
    let ls_path = master_root.join("Chs").join("LeaderSkinMaster.json");
    if ls_path.exists() {
        let raw: Vec<serde_json::Value> = serde_json::from_str(
            &fs::read_to_string(&ls_path)
                .with_context(|| format!("无法读取 {}", ls_path.display()))?,
        )?;
        for row in &raw {
            if let (Some(id), Some(name)) = (
                row.get(0).and_then(|v| v.as_i64()),
                row.get(1).and_then(|v| v.as_str()),
            ) {
                leader_names.insert(id, name.to_string());
            }
        }
    }

    // 2. CardText → card_style_id → name_label (same labels across all variants)
    let mut card_name_labels = HashMap::new();
    let ct_path = master_root.join("Chs").join("CardText.json");
    if ct_path.exists() {
        let raw: Vec<serde_json::Value> = serde_json::from_str(
            &fs::read_to_string(&ct_path)
                .with_context(|| format!("无法读取 {}", ct_path.display()))?,
        )?;
        for row in &raw {
            if let (Some(style_id), Some(label)) = (
                row.get(0).and_then(|v| v.as_i64()),
                row.get(1).and_then(|v| v.as_str()),
            ) {
                card_name_labels.insert(style_id, label.to_string());
            }
        }
    }

    // 3. MasterTextLabel from all 5 variants
    let mut text_labels: HashMap<String, HashMap<String, String>> = HashMap::new();
    for variant in &["Chs", "Cht", "Eng", "Jpn", "Kor"] {
        let vkey = match *variant {
            "Chs" => "chs",
            "Cht" => "cht",
            "Eng" => "eng",
            "Jpn" => "jpn",
            "Kor" => "kor",
            _ => continue,
        };
        let mtl_path = master_root.join(variant).join("MasterTextLabel.json");
        if !mtl_path.exists() {
            continue;
        }
        let raw: Vec<serde_json::Value> = serde_json::from_str(
            &fs::read_to_string(&mtl_path)
                .with_context(|| format!("无法读取 {}", mtl_path.display()))?,
        )?;
        let mut map = HashMap::new();
        for row in &raw {
            if let (Some(label), Some(text)) = (
                row.get(0).and_then(|v| v.as_str()),
                row.get(1).and_then(|v| v.as_str()),
            ) {
                map.insert(label.to_string(), text.to_string());
            }
        }
        text_labels.insert(vkey.to_string(), map);
    }

    // 4. HomeIllustrationMaster stores the in-game UI position in the
    // HomeIllustration window coordinate space. The window center is (0, 0).
    let mut home_positions = HashMap::new();
    let hi_path = master_root.join("Chs").join("HomeIllustrationMaster.json");
    if hi_path.exists() {
        let raw: Vec<serde_json::Value> = serde_json::from_str(
            &fs::read_to_string(&hi_path)
                .with_context(|| format!("无法读取 {}", hi_path.display()))?,
        )?;
        for row in &raw {
            let Some(id) = row.get(0).and_then(|v| v.as_i64()) else {
                continue;
            };
            let x = row.get(2).and_then(|v| v.as_f64()).unwrap_or(0.0);
            let y = row.get(3).and_then(|v| v.as_f64()).unwrap_or(0.0);
            home_positions.insert(id, HomePosition { x, y });
        }
    }

    Ok(HomeIllustMeta {
        leader_names,
        card_name_labels,
        text_labels,
        home_positions,
    })
}

/// 解析 hi_ ID 为整数
fn parse_hi_id(stem: &str) -> Option<i64> {
    stem.strip_prefix("hi_")?.parse().ok()
}

fn parse_bg_hi_id(name: &str) -> Option<i64> {
    name.strip_prefix("bg_hi_")?
        .strip_suffix(".png")?
        .parse()
        .ok()
}

fn select_backgrounds(
    hi_id: Option<i64>,
    spine_skel: &Option<PathBuf>,
    candidates: &[(String, PathBuf)],
) -> Vec<String> {
    let mut names: Vec<String> = candidates.iter().map(|(name, _)| name.clone()).collect();
    names.sort();
    names.dedup();

    if let Some(id) = hi_id {
        let own_name = format!("bg_hi_{id}.png");
        if names.iter().any(|name| name == &own_name) {
            return vec![own_name];
        }
    }

    if spine_skel.is_none() {
        return names;
    }

    Vec::new()
}

/// 判断是否为战斗背景（2001-2108 范围）
fn is_battle_background(hi_id: i64) -> bool {
    (2001..=2108).contains(&hi_id)
}

/// 战斗背景 hi_id → leader_skin_id 映射
fn battle_to_leader_id(hi_id: i64) -> Option<i64> {
    match hi_id {
        2001..=2008 => Some(hi_id - 900), // 200x → 110x
        2101..=2108 => Some(hi_id - 900), // 210x → 120x
        _ => None,
    }
}

/// 从主数据查找角色名（日文）和各语言本地化名
fn lookup_character_names(
    meta: &HomeIllustMeta,
    hi_id: i64,
) -> (Option<String>, BTreeMap<String, String>) {
    let jp_name = meta.leader_names.get(&hi_id).cloned();
    let mut names = BTreeMap::new();

    // 1. HomeIllustrationName_{hi_id}（主页展示名，优先用于 Home Illustration）
    let home_label = format!("HomeIllustrationName_{hi_id}");
    for (variant, map) in &meta.text_labels {
        if let Some(text) = map.get(&home_label) {
            names.insert(variant.clone(), text.clone());
        }
    }

    // 2. CardText → MasterTextLabel（完整卡名，仅补齐缺失语言）
    if let Some(label) = meta.card_name_labels.get(&hi_id) {
        for (variant, map) in &meta.text_labels {
            if let Some(text) = map.get(label) {
                names.entry(variant.clone()).or_insert_with(|| text.clone());
            }
        }
    }

    (jp_name, names)
}

// ============================================================================
// 单文件提取
// ============================================================================

fn extract_one(
    ab_path: &Path,
    output_dir: &Path,
    ctx: &ExtractCtx,
    source_hash: String,
) -> anyhow::Result<()> {
    let stem = ab_path.file_stem().unwrap().to_string_lossy().to_string();

    // 清理旧数据
    if output_dir.exists() {
        fs::remove_dir_all(output_dir)?;
    }

    // 临时目录
    let temp_dir = output_dir.with_file_name(format!("_tmp_{}", stem));
    if temp_dir.exists() {
        fs::remove_dir_all(&temp_dir)?;
    }
    fs::create_dir_all(&temp_dir)?;

    // 1. AssetStudio 导出全部资产
    run_asset_studio_single(ab_path, &temp_dir, ctx.asset_studio_path)?;
    run_asset_studio_dump_single(ab_path, &temp_dir, ctx.asset_studio_path)?;

    // 2. 找到导出的子目录
    let export_subdirs: Vec<PathBuf> = fs::read_dir(&temp_dir)?
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false))
        .map(|e| e.path())
        .collect();

    if export_subdirs.is_empty() {
        bail!("AssetStudio 未生成任何导出文件");
    }

    // 收集所有导出文件
    let mut all_files = Vec::new();
    for subdir in &export_subdirs {
        let cab_dir = find_single_subdir(subdir)?;
        collect_files(&cab_dir, &mut all_files);
    }

    let prefab_transforms = parse_prefab_transforms(&stem, &all_files);

    // 3. 分类文件
    fs::create_dir_all(output_dir)?;
    let fx_dir = output_dir.join("fx");

    let mut spine_skel: Option<PathBuf> = None;
    let mut spine_atlas: Option<PathBuf> = None;
    let mut spine_png: Option<PathBuf> = None;
    let mut bg_candidates: Vec<(String, PathBuf)> = Vec::new();
    let mut has_effects = false;

    // Unity JSON 数据
    let mut leader_skin_setting: Option<serde_json::Value> = None;
    let mut skeleton_anim: Option<serde_json::Value> = None;
    let mut transform_adjuster: Option<serde_json::Value> = None;
    let mut skeleton_data: Option<serde_json::Value> = None;

    for file in &all_files {
        let name = file.file_name().unwrap().to_string_lossy().to_string();
        let spine_prefix = format!("spine_{stem}");

        if name.ends_with(".skel")
            && (name.starts_with(&spine_prefix) || name == format!("{stem}.skel"))
        {
            spine_skel = Some(file.clone());
        } else if name.ends_with(".atlas")
            && (name.starts_with(&spine_prefix) || name == format!("{stem}.atlas"))
        {
            spine_atlas = Some(file.clone());
        } else if name.ends_with(".png")
            && (name.starts_with(&spine_prefix) || name == format!("{stem}.png"))
        {
            spine_png = Some(file.clone());
        } else if name.ends_with(".png") && name.starts_with("bg_hi_") {
            bg_candidates.push((name.clone(), file.clone()));
        } else if name.ends_with(".png") && name.starts_with("ef_") {
            has_effects = true;
            fs::create_dir_all(&fx_dir)?;
            fs::copy(file, fx_dir.join(&name))?;
        } else if name.ends_with(".png")
            && (name.starts_with("sp_") || name.starts_with("tex_eff_"))
        {
            has_effects = true;
            fs::create_dir_all(&fx_dir)?;
            fs::copy(file, fx_dir.join(&name))?;
        } else if name == "LeaderSkinSetting.json" {
            leader_skin_setting = Some(serde_json::from_str(&fs::read_to_string(file)?)?);
        } else if name == "SkeletonAnimation.json" {
            skeleton_anim = Some(serde_json::from_str(&fs::read_to_string(file)?)?);
        } else if name == "HomeIllustTransformAdjuster.json" {
            transform_adjuster = Some(serde_json::from_str(&fs::read_to_string(file)?)?);
        } else if name.ends_with("_SkeletonData.json") {
            skeleton_data = Some(serde_json::from_str(&fs::read_to_string(file)?)?);
        }
    }

    let hi_id = parse_hi_id(&stem);
    let bg_pngs = select_backgrounds(hi_id, &spine_skel, &bg_candidates);
    for name in &bg_pngs {
        if let Some((_, path)) = bg_candidates
            .iter()
            .find(|(candidate, _)| candidate == name)
        {
            fs::copy(path, output_dir.join(name))?;
        }
    }

    // 4. 复制核心 Spine 文件
    if let Some(ref path) = spine_skel {
        fs::copy(path, output_dir.join(format!("spine_{stem}.skel")))?;
    }
    if let Some(ref path) = spine_atlas {
        copy_atlas_with_page_name(
            path,
            &output_dir.join(format!("spine_{stem}.atlas")),
            spine_png.as_deref(),
            &format!("spine_{stem}.png"),
        )?;
    }
    if let Some(ref path) = spine_png {
        fs::copy(path, output_dir.join(format!("spine_{stem}.png")))?;
    }

    // 5. 生成 config.json
    let config = build_config(
        &stem,
        source_hash,
        hi_id,
        ctx.meta,
        &leader_skin_setting,
        &skeleton_anim,
        &transform_adjuster,
        skeleton_data.as_ref(),
        prefab_transforms,
        if ctx.layout_debug {
            let skeleton_scale = skeleton_scale_from_data(skeleton_data.as_ref());
            let home_position = hi_id.and_then(|id| ctx.meta.home_positions.get(&id).copied());
            Some(build_layout_debug(
                &stem,
                &all_files,
                &transform_adjuster,
                skeleton_scale,
                home_position,
            ))
        } else {
            None
        },
        &bg_pngs,
        spine_skel.is_some(),
        has_effects,
    );

    let config_json = serde_json::to_string_pretty(&config)?;
    fs::write(output_dir.join("config.json"), config_json)?;

    // 6. 清理临时目录
    let _ = fs::remove_dir_all(&temp_dir);

    // 7. 如果 fx 目录为空，删除
    if has_effects && fx_dir.exists() {
        let count = fs::read_dir(&fx_dir)?.count();
        if count == 0 {
            let _ = fs::remove_dir(&fx_dir);
        }
    }

    Ok(())
}

// ============================================================================
// 辅助函数
// ============================================================================

fn run_asset_studio_single(
    ab_path: &Path,
    output_dir: &Path,
    asset_studio_path: &Path,
) -> anyhow::Result<()> {
    let status = Command::new(asset_studio_path)
        .arg(ab_path)
        .args([
            "-t",
            "all",
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
        ])
        .status()
        .with_context(|| format!("无法启动 AssetStudio: {}", asset_studio_path.display()))?;

    if !status.success() {
        bail!("AssetStudio 退出码: {:?}", status.code());
    }
    Ok(())
}

fn run_asset_studio_dump_single(
    ab_path: &Path,
    output_dir: &Path,
    asset_studio_path: &Path,
) -> anyhow::Result<()> {
    let status = Command::new(asset_studio_path)
        .arg(ab_path)
        .args([
            "-m",
            "dump",
            "--load-all",
            "-g",
            "fileName",
            "-f",
            "assetName_pathID",
            "-o",
            &output_dir.to_string_lossy(),
            "--unity-version",
            UNITY_VERSION,
            "--log-level",
            "warning",
        ])
        .status()
        .with_context(|| format!("无法启动 AssetStudio dump: {}", asset_studio_path.display()))?;

    if !status.success() {
        bail!("AssetStudio dump 退出码: {:?}", status.code());
    }
    Ok(())
}

fn find_single_subdir(dir: &Path) -> anyhow::Result<PathBuf> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            return Ok(entry.path());
        }
    }
    bail!("目录为空: {}", dir.display())
}

fn collect_files(dir: &Path, result: &mut Vec<PathBuf>) {
    if let Ok(entries) = fs::read_dir(dir) {
        let mut sortable: Vec<_> = entries.flatten().collect();
        sortable.sort_by_key(|e| e.file_name());
        for entry in sortable {
            let path = entry.path();
            if path.is_dir() {
                collect_files(&path, result);
            } else {
                result.push(path);
            }
        }
    }
}

fn sha256_file(path: &Path) -> anyhow::Result<String> {
    let data = fs::read(path).with_context(|| format!("无法读取 {}", path.display()))?;
    let mut hasher = Sha256::new();
    hasher.update(&data);
    Ok(format!("{:x}", hasher.finalize()))
}

fn config_source_hash(path: &Path) -> Option<String> {
    let text = fs::read_to_string(path).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    value
        .get("source_hash")
        .and_then(|v| v.as_str())
        .map(ToString::to_string)
}

fn config_version(path: &Path) -> Option<u32> {
    let text = fs::read_to_string(path).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    value
        .get("config_version")
        .and_then(|v| v.as_u64())
        .and_then(|v| u32::try_from(v).ok())
}

fn config_has_layout_debug(path: &Path) -> bool {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(_) => return false,
    };
    let value: serde_json::Value = match serde_json::from_str(&text) {
        Ok(value) => value,
        Err(_) => return false,
    };
    value.get("layout_debug").is_some()
}

fn copy_atlas_with_page_name(
    src: &Path,
    dst: &Path,
    src_page: Option<&Path>,
    dst_page_name: &str,
) -> anyhow::Result<()> {
    let mut atlas =
        fs::read_to_string(src).with_context(|| format!("读取 atlas 失败: {}", src.display()))?;

    if let Some(page) = src_page
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
    {
        atlas = atlas
            .lines()
            .map(|line| {
                if line.trim() == page {
                    dst_page_name
                } else {
                    line
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        atlas.push('\n');
    }

    fs::write(dst, atlas).with_context(|| format!("写入 atlas 失败: {}", dst.display()))?;
    Ok(())
}

#[derive(Debug, Default)]
struct DumpGameObject {
    name: String,
    transform_id: i64,
}

#[derive(Debug, Default)]
struct DumpTransform {
    game_object_id: i64,
    father_id: i64,
    children: Vec<i64>,
    local_position: Vec3,
    local_scale: Vec3,
}

#[derive(Debug, Default)]
struct DumpScene {
    game_objects: HashMap<i64, DumpGameObject>,
    transforms: HashMap<i64, DumpTransform>,
}

fn parse_dump_scene(files: &[PathBuf]) -> DumpScene {
    let mut game_objects: HashMap<i64, DumpGameObject> = HashMap::new();
    let mut transforms: HashMap<i64, DumpTransform> = HashMap::new();

    for file in files {
        if file.extension().and_then(|s| s.to_str()) != Some("txt") {
            continue;
        }
        let Ok(text) = fs::read_to_string(file) else {
            continue;
        };
        let path_id = path_id_from_name(file).unwrap_or_default();
        if text.contains("GameObject Base") {
            let name = string_after(&text, "m_Name").unwrap_or_default();
            let transform_id = first_file_id_after(&text, "m_Component").unwrap_or_default();
            if path_id != 0 {
                game_objects.insert(path_id, DumpGameObject { name, transform_id });
            }
        } else if text.contains("Transform Base") || text.contains("RectTransform Base") {
            let game_object_id = first_file_id_after(&text, "m_GameObject").unwrap_or_default();
            let father_id = first_file_id_after(&text, "m_Father").unwrap_or_default();
            let children = file_ids_after(&text, "m_Children");
            let local_position = vec3_after(&text, "m_LocalPosition").unwrap_or_default();
            let local_scale = vec3_after(&text, "m_LocalScale").unwrap_or(Vec3 {
                x: 1.0,
                y: 1.0,
                z: 1.0,
            });
            if path_id != 0 {
                transforms.insert(
                    path_id,
                    DumpTransform {
                        game_object_id,
                        father_id,
                        children,
                        local_position,
                        local_scale,
                    },
                );
            }
        }
    }

    DumpScene {
        game_objects,
        transforms,
    }
}

fn parse_prefab_transforms(stem: &str, files: &[PathBuf]) -> PrefabTransforms {
    let scene = parse_dump_scene(files);
    let game_objects = &scene.game_objects;
    let transforms = &scene.transforms;

    let mut nodes = BTreeMap::new();
    let root_transform = game_objects
        .iter()
        .filter(|(_, go)| go.name == stem)
        .map(|(id, _)| *id)
        .min();
    let character_transform = transform_by_go_name(&game_objects, "Character");
    let spine_root_transform = transform_by_go_name(&game_objects, &format!("spine_{stem}"));
    let spine_object_transform = game_objects
        .iter()
        .filter(|(_, go)| go.name.starts_with("Spine GameObject"))
        .map(|(id, _)| *id)
        .filter(|id| Some(*id) != root_transform)
        .min()
        .or(spine_root_transform);
    let bg_transform = transform_by_go_name(&game_objects, &format!("bg_{stem}"))
        .or_else(|| transform_by_go_name(&game_objects, "BG"));

    add_transform_node(
        "root",
        root_transform,
        &game_objects,
        &transforms,
        &mut nodes,
    );
    add_transform_node(
        "character",
        character_transform,
        &game_objects,
        &transforms,
        &mut nodes,
    );
    add_transform_node(
        "spineRoot",
        spine_root_transform,
        &game_objects,
        &transforms,
        &mut nodes,
    );
    add_transform_node(
        "spineObject",
        spine_object_transform,
        &game_objects,
        &transforms,
        &mut nodes,
    );
    add_transform_node(
        "background",
        bg_transform,
        &game_objects,
        &transforms,
        &mut nodes,
    );
    if let Some(bg) = bg_transform.and_then(|id| transforms.get(&id)) {
        if let Some(child_id) = bg.children.first().copied() {
            add_transform_node(
                "backgroundQuad",
                Some(child_id),
                &game_objects,
                &transforms,
                &mut nodes,
            );
        }
    }

    let prefab_scale = spine_object_transform
        .map(|id| world_scale(id, &transforms).x)
        .or_else(|| spine_root_transform.map(|id| world_scale(id, &transforms).x))
        .unwrap_or(1.0);

    PrefabTransforms {
        prefab_scale,
        nodes,
    }
}

fn transform_by_go_name(game_objects: &HashMap<i64, DumpGameObject>, name: &str) -> Option<i64> {
    game_objects
        .iter()
        .filter(|(_, go)| go.name == name)
        .map(|(id, _)| *id)
        .min()
}

fn transform_by_go_prefix(
    game_objects: &HashMap<i64, DumpGameObject>,
    prefix: &str,
) -> Option<i64> {
    game_objects
        .iter()
        .filter(|(_, go)| go.name.starts_with(prefix))
        .map(|(id, _)| *id)
        .min()
}

fn add_transform_node(
    key: &str,
    transform_id: Option<i64>,
    game_objects: &HashMap<i64, DumpGameObject>,
    transforms: &HashMap<i64, DumpTransform>,
    nodes: &mut BTreeMap<String, PrefabTransformNode>,
) {
    let Some(id) = transform_id else {
        return;
    };
    let Some(transform) = transforms.get(&id) else {
        return;
    };
    let name = game_objects
        .get(&transform.game_object_id)
        .map(|go| go.name.clone())
        .unwrap_or_else(|| key.to_string());
    nodes.insert(
        key.to_string(),
        PrefabTransformNode {
            name,
            local_position: transform.local_position,
            local_scale: transform.local_scale,
            world_position: world_position(id, transforms),
            world_scale: world_scale(id, transforms),
        },
    );
}

fn world_position(id: i64, transforms: &HashMap<i64, DumpTransform>) -> Vec3 {
    let Some(t) = transforms.get(&id) else {
        return Vec3::default();
    };
    if t.father_id == 0 {
        return t.local_position;
    }
    let parent_pos = world_position(t.father_id, transforms);
    let parent_scale = world_scale(t.father_id, transforms);
    Vec3 {
        x: parent_pos.x + t.local_position.x * parent_scale.x,
        y: parent_pos.y + t.local_position.y * parent_scale.y,
        z: parent_pos.z + t.local_position.z * parent_scale.z,
    }
}

fn build_layout_debug(
    stem: &str,
    files: &[PathBuf],
    transform_adjuster: &Option<serde_json::Value>,
    skeleton_scale: f64,
    home_position: Option<HomePosition>,
) -> LayoutDebug {
    let scene = parse_dump_scene(files);
    let game_objects = &scene.game_objects;
    let transforms = &scene.transforms;
    let root_transform = transform_by_go_name(game_objects, stem);
    let character_transform = transform_by_go_name(game_objects, "Character");
    let spine_root_transform = transform_by_go_name(game_objects, &format!("spine_{stem}"));
    let spine_object_transform = game_objects
        .iter()
        .filter(|(_, go)| go.name.starts_with("Spine GameObject"))
        .map(|(id, _)| *id)
        .filter(|id| Some(*id) != root_transform)
        .min()
        .or(spine_root_transform);
    let bg_transform = transform_by_go_name(game_objects, &format!("bg_{stem}"))
        .or_else(|| transform_by_go_name(game_objects, "BG"));
    let background_quad_transform = bg_transform
        .and_then(|id| transforms.get(&id))
        .and_then(|t| t.children.first().copied());

    let mut root_chain = Vec::new();
    if let Some(id) = root_transform {
        push_layout_node("root", id, game_objects, transforms, &mut root_chain);
    }

    let mut spine_chain = Vec::new();
    for (key, id) in [
        ("root", root_transform),
        ("character", character_transform),
        ("spineRoot", spine_root_transform),
        ("spineObject", spine_object_transform),
    ] {
        if let Some(id) = id {
            push_layout_node(key, id, game_objects, transforms, &mut spine_chain);
        }
    }

    let mut background_chain = Vec::new();
    for (key, id) in [
        ("root", root_transform),
        ("background", bg_transform),
        ("backgroundQuad", background_quad_transform),
    ] {
        if let Some(id) = id {
            push_layout_node(key, id, game_objects, transforms, &mut background_chain);
        }
    }

    let background_quad_world_scale =
        background_quad_transform.map(|id| world_scale(id, transforms));
    let prefab_scale = spine_object_transform
        .map(|id| world_scale(id, transforms).x)
        .or_else(|| spine_root_transform.map(|id| world_scale(id, transforms).x))
        .unwrap_or(1.0);

    LayoutDebug {
        version: 1,
        notes: vec![
            "Dumped from Unity Transform and HomeIllustTransformAdjuster data.".to_string(),
            "HomeIllustrationMaster position is preserved as source pixels; web conversion must be derived from the actual game window/camera.".to_string(),
            "backgroundQuad is the in-prefab background mesh transform and can be used to infer visible world extents.".to_string(),
        ],
        skeleton_scale,
        home_position,
        aspect_defines: parse_aspect_defines(transform_adjuster),
        root_chain,
        spine_chain,
        background_chain,
        background_quad_world_scale,
        prefab_scale,
    }
}

fn push_layout_node(
    key: &str,
    transform_id: i64,
    game_objects: &HashMap<i64, DumpGameObject>,
    transforms: &HashMap<i64, DumpTransform>,
    out: &mut Vec<LayoutDebugTransform>,
) {
    let Some(transform) = transforms.get(&transform_id) else {
        return;
    };
    let name = game_objects
        .get(&transform.game_object_id)
        .map(|go| go.name.clone())
        .unwrap_or_else(|| key.to_string());
    out.push(LayoutDebugTransform {
        key: key.to_string(),
        name,
        path_id: transform_id,
        parent_path_id: transform.father_id,
        local_position: transform.local_position,
        local_scale: transform.local_scale,
        world_position: world_position(transform_id, transforms),
        world_scale: world_scale(transform_id, transforms),
    });
}

fn parse_aspect_defines(
    transform_adjuster: &Option<serde_json::Value>,
) -> Vec<LayoutDebugAspectDefine> {
    let mut result = Vec::new();
    let Some(defines) = transform_adjuster
        .as_ref()
        .and_then(|v| v.get("_aspectDefines"))
        .and_then(|v| v.as_array())
    else {
        return result;
    };
    for def in defines {
        let aspect = def.get("_aspect").and_then(|v| v.as_f64()).unwrap_or(1.78);
        let pos = def
            .get("_localPosition")
            .unwrap_or(&serde_json::Value::Null);
        let scale = def.get("_localScale").unwrap_or(&serde_json::Value::Null);
        result.push(LayoutDebugAspectDefine {
            label: aspect_label(aspect).to_string(),
            aspect,
            local_position: HomePosition {
                x: pos.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0),
                y: pos.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0),
            },
            local_scale: HomePosition {
                x: scale.get("x").and_then(|v| v.as_f64()).unwrap_or(1.0),
                y: scale.get("y").and_then(|v| v.as_f64()).unwrap_or(1.0),
            },
        });
    }
    result.sort_by(|a, b| {
        a.aspect
            .partial_cmp(&b.aspect)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    result
}

fn aspect_label(aspect: f64) -> &'static str {
    if (aspect - 1.33).abs() < 0.01 {
        "4:3"
    } else if (aspect - 1.78).abs() < 0.02 {
        "16:9"
    } else if (aspect - 2.17).abs() < 0.01 {
        "21:9"
    } else {
        "21:10"
    }
}

fn world_scale(id: i64, transforms: &HashMap<i64, DumpTransform>) -> Vec3 {
    let Some(t) = transforms.get(&id) else {
        return Vec3 {
            x: 1.0,
            y: 1.0,
            z: 1.0,
        };
    };
    if t.father_id == 0 {
        return t.local_scale;
    }
    let parent = world_scale(t.father_id, transforms);
    Vec3 {
        x: parent.x * t.local_scale.x,
        y: parent.y * t.local_scale.y,
        z: parent.z * t.local_scale.z,
    }
}

fn path_id_from_name(path: &Path) -> Option<i64> {
    let stem = path.file_stem()?.to_string_lossy();
    let (_, id) = stem.rsplit_once('@')?;
    id.trim().parse().ok()
}

fn string_after(text: &str, key: &str) -> Option<String> {
    let pos = text.find(key)?;
    let rest = &text[pos..];
    let first_quote = rest.find('"')?;
    let rest = &rest[first_quote + 1..];
    let second_quote = rest.find('"')?;
    Some(rest[..second_quote].to_string())
}

fn first_file_id_after(text: &str, key: &str) -> Option<i64> {
    let pos = text.find(key)?;
    let rest = &text[pos..];
    let marker = "m_PathID = ";
    let id_pos = rest.find(marker)?;
    let rest = &rest[id_pos + marker.len()..];
    parse_i64_prefix(rest)
}

fn file_ids_after(text: &str, key: &str) -> Vec<i64> {
    let Some(pos) = text.find(key) else {
        return Vec::new();
    };
    let rest = &text[pos..];
    let end = rest.find("\n  ").unwrap_or(rest.len());
    let section = &rest[..end];
    let marker = "m_PathID = ";
    let mut ids = Vec::new();
    let mut cursor = 0;
    while let Some(rel) = section[cursor..].find(marker) {
        cursor += rel + marker.len();
        if let Some(id) = parse_i64_prefix(&section[cursor..]) {
            ids.push(id);
        }
    }
    ids
}

fn vec3_after(text: &str, key: &str) -> Option<Vec3> {
    let pos = text.find(key)?;
    let rest = &text[pos..];
    let mut x = None;
    let mut y = None;
    let mut z = None;
    for line in rest.lines().skip(1) {
        let trimmed = line.trim();
        if let Some(value) = vector_component_value(trimmed, "x") {
            x = parse_f64_prefix(value);
        } else if let Some(value) = vector_component_value(trimmed, "y") {
            y = parse_f64_prefix(value);
        } else if let Some(value) = vector_component_value(trimmed, "z") {
            z = parse_f64_prefix(value);
        } else if x.is_some() || y.is_some() || z.is_some() {
            break;
        }
        if x.is_some() && y.is_some() && z.is_some() {
            break;
        }
    }
    Some(Vec3 {
        x: x?,
        y: y?,
        z: z?,
    })
}

fn vector_component_value<'a>(line: &'a str, axis: &str) -> Option<&'a str> {
    line.strip_prefix(&format!("{axis} = "))
        .or_else(|| line.strip_prefix(&format!("float {axis} = ")))
}

fn parse_i64_prefix(text: &str) -> Option<i64> {
    let mut end = 0;
    for (idx, ch) in text.char_indices() {
        if idx == 0 && ch == '-' {
            end = ch.len_utf8();
            continue;
        }
        if ch.is_ascii_digit() {
            end = idx + ch.len_utf8();
        } else {
            break;
        }
    }
    if end == 0 {
        return None;
    }
    text[..end].parse().ok()
}

fn parse_f64_prefix(text: &str) -> Option<f64> {
    let mut end = 0;
    for (idx, ch) in text.char_indices() {
        if ch.is_ascii_digit() || matches!(ch, '-' | '+' | '.' | 'e' | 'E') {
            end = idx + ch.len_utf8();
        } else {
            break;
        }
    }
    if end == 0 {
        return None;
    }
    text[..end].parse().ok()
}

fn copy_voice_files(
    stem: &str,
    output_dir: &Path,
    data_dir: &Path,
    vgmstream_path: &Path,
) -> anyhow::Result<usize> {
    let hi_id = match parse_hi_id(stem) {
        Some(id) => id,
        None => return Ok(0),
    };

    let voice_root = output_dir.join("voice");
    let prefix = format!("Play_dx_home_{}_", hi_id);
    let mut total = 0;

    let audio_dir = data_dir.join("exports").join("audio").join("jpn");
    let lang_dir = voice_root.join("jpn");
    if audio_dir.exists() {
        for entry in fs::read_dir(&audio_dir)? {
            let entry = entry?;
            let name_str = entry.file_name().to_string_lossy().to_string();
            if name_str.starts_with(&prefix) && name_str.ends_with(".wav") {
                fs::create_dir_all(&lang_dir)?;
                let dest = lang_dir.join(&name_str);
                if !dest.exists() {
                    fs::copy(entry.path(), &dest)?;
                    total += 1;
                }
            }
        }
    }

    let needed = (1..=4).any(|n| {
        !lang_dir
            .join(format!("Play_dx_home_{hi_id}_{n}.wav"))
            .exists()
    });
    if needed {
        total += extract_home_voice_pck(hi_id, data_dir, &lang_dir, vgmstream_path)?;
    }

    if !voice_root.join("jpn").exists() {
        let _ = fs::remove_dir(&voice_root);
    }
    Ok(total)
}

fn extract_home_voice_pck(
    hi_id: i64,
    data_dir: &Path,
    output_dir: &Path,
    vgmstream_path: &Path,
) -> anyhow::Result<usize> {
    let pck_path = data_dir
        .join("variants")
        .join("Chs")
        .join("raw-assets")
        .join("sound")
        .join("Windows")
        .join("d")
        .join("Japanese(JP)")
        .join(format!("dx_home_{hi_id}.pck"));
    if !pck_path.exists() {
        return Ok(0);
    }

    let data = fs::read(&pck_path).with_context(|| format!("无法读取 {}", pck_path.display()))?;
    let mut entries: Vec<(u32, u32)> = audio::parse_akpk(&data).into_iter().collect();
    entries.sort_by_key(|(_, offset)| *offset);
    if entries.is_empty() {
        return Ok(0);
    }

    fs::create_dir_all(output_dir)?;
    let mut total = 0;
    for (idx, (_wem_id, offset)) in entries.iter().enumerate() {
        let out_path = output_dir.join(format!("Play_dx_home_{hi_id}_{}.wav", idx + 1));
        if out_path.exists() {
            continue;
        }
        let Some(wem_data) = audio::extract_wem(&data, *offset) else {
            continue;
        };
        audio::wem_to_wav(wem_data, &out_path, vgmstream_path)?;
        total += 1;
    }
    Ok(total)
}

fn build_config(
    stem: &str,
    source_hash: String,
    hi_id: Option<i64>,
    meta: &HomeIllustMeta,
    leader_skin: &Option<serde_json::Value>,
    skeleton_anim: &Option<serde_json::Value>,
    transform: &Option<serde_json::Value>,
    skeleton_data: Option<&serde_json::Value>,
    prefab_transforms: PrefabTransforms,
    layout_debug: Option<LayoutDebug>,
    bg_textures: &[String],
    has_spine: bool,
    has_effects: bool,
) -> IllustConfig {
    let switch_targets: Vec<String> = if !has_spine && bg_textures.len() > 1 {
        let mut targets: Vec<String> = bg_textures
            .iter()
            .filter_map(|name| parse_bg_hi_id(name).map(|id| format!("hi_{id}")))
            .collect();
        targets.sort();
        targets.dedup();
        targets
    } else {
        Vec::new()
    };

    let (illust_type, character_name, character_names, voice_prefix, voice_files) = match hi_id {
        Some(id) if is_battle_background(id) => {
            let (jp_name, names) = lookup_character_names(meta, id);
            ("battle".to_string(), jp_name, names, None, vec![])
        }
        Some(id) if !switch_targets.is_empty() => {
            let (jp_name, names) = lookup_character_names(meta, id);
            let primary_name = jp_name.or_else(|| names.get("jpn").cloned());
            let vp = Some(format!("dx_home_{id}"));
            let vf = (1..=4)
                .map(|n| format!("Play_dx_home_{id}_{n}.wav"))
                .collect();
            ("switch".to_string(), primary_name, names, vp, vf)
        }
        Some(id) => {
            let (jp_name, names) = lookup_character_names(meta, id);
            let primary_name = jp_name.or_else(|| names.get("jpn").cloned());
            let vp = Some(format!("dx_home_{id}"));
            let vf = (1..=4)
                .map(|n| format!("Play_dx_home_{id}_{n}.wav"))
                .collect();
            ("home".to_string(), primary_name, names, vp, vf)
        }
        None => ("home".to_string(), None, BTreeMap::new(), None, vec![]),
    };

    let tap_animations: Vec<String> = leader_skin
        .as_ref()
        .and_then(|v| v.get("_homeEmoteList"))
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect()
        })
        .unwrap_or_default();

    let blend_times: Vec<f32> = leader_skin
        .as_ref()
        .and_then(|v| v.get("BlendTimes"))
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_f64().map(|f| f as f32))
                .collect()
        })
        .unwrap_or_else(|| vec![0.2]);

    let default_mix = skeleton_anim
        .as_ref()
        .and_then(|v| v.get("defaultMix"))
        .or_else(|| skeleton_data.and_then(|v| v.get("defaultMix")))
        .and_then(|v| v.as_f64())
        .unwrap_or(0.2) as f32;

    let loop_val = skeleton_anim
        .as_ref()
        .and_then(|v| v.get("loop"))
        .and_then(|v| v.as_i64())
        .unwrap_or(1)
        != 0;

    let phys_pos = skeleton_anim
        .as_ref()
        .and_then(|v| v.get("physicsPositionInheritanceFactor"))
        .and_then(|v| Some([v.get("x")?.as_f64()? as f32, v.get("y")?.as_f64()? as f32]))
        .unwrap_or([1.0, 1.0]);

    let phys_rot = skeleton_anim
        .as_ref()
        .and_then(|v| v.get("physicsRotationInheritanceFactor"))
        .and_then(|v| v.as_f64())
        .unwrap_or(1.0) as f32;

    let has_screen_blend = skeleton_data
        .and_then(|v| v.get("blendModeMaterials"))
        .and_then(|v| v.get("requiresBlendModeMaterials"))
        .and_then(|v| v.as_i64())
        .unwrap_or(0)
        != 0;

    let skeleton_scale = skeleton_scale_from_data(skeleton_data);

    let home_position = hi_id.and_then(|id| meta.home_positions.get(&id).copied());

    let mut aspect_layouts = BTreeMap::new();
    if let Some(t) = transform {
        if let Some(defines) = t.get("_aspectDefines").and_then(|v| v.as_array()) {
            for def in defines {
                let aspect = def.get("_aspect").and_then(|v| v.as_f64()).unwrap_or(1.78);
                let pos = def.get("_localPosition").unwrap();
                let scale = def.get("_localScale").unwrap();
                let label = aspect_label(aspect);
                aspect_layouts.insert(
                    label.to_string(),
                    AspectLayout {
                        x: pos.get("x").and_then(|v| v.as_f64()).unwrap_or(0.0),
                        y: pos.get("y").and_then(|v| v.as_f64()).unwrap_or(0.0),
                        scale_x: scale.get("x").and_then(|v| v.as_f64()).unwrap_or(1.0),
                        scale_y: scale.get("y").and_then(|v| v.as_f64()).unwrap_or(1.0),
                    },
                );
            }
        }
    }

    IllustConfig {
        config_version: HOME_ILLUST_CONFIG_VERSION,
        id: stem.to_string(),
        source_hash,
        character_name,
        character_names,
        illust_type,
        voice_prefix,
        voice_files,
        idle_animation: "idle".to_string(),
        tap_animations,
        blend_times,
        default_mix,
        r#loop: loop_val,
        physics_position_factor: phys_pos,
        physics_rotation_factor: phys_rot,
        has_screen_blend,
        skeleton_scale,
        home_position,
        prefab_transforms,
        aspect_layouts,
        layout_debug,
        bg_textures: bg_textures.to_vec(),
        switch_targets,
        has_effects,
    }
}

fn skeleton_scale_from_data(skeleton_data: Option<&serde_json::Value>) -> f64 {
    skeleton_data
        .and_then(|v| v.get("scale"))
        .and_then(|v| v.as_f64())
        .unwrap_or(0.01)
}
