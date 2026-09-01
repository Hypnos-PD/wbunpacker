//! Export regular premium card materials and their referenced textures for WebGL renderers.
//!
//! Collectible `7*` cards use a separate Spine pipeline and are intentionally excluded.

use anyhow::{Context, bail};
use indicatif::{ProgressBar, ProgressStyle};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use tempfile::TempDir;
use walkdir::WalkDir;

const UNITY_VERSION: &str = "2022.3.62f2";
const CONFIG_VERSION: u32 = 6;
const BATCH_SIZE: usize = 200;

#[derive(Debug, Default)]
pub struct FoilStats {
    pub processed: usize,
    pub skipped: usize,
    pub failed: usize,
    pub textures: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TextureEnv {
    pub texture: Option<String>,
    pub file_id: i32,
    pub path_id: i64,
    pub scale: [f32; 2],
    pub offset: [f32; 2],
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FoilMaterial {
    pub config_version: u32,
    pub id: String,
    pub card_style_id: i64,
    pub material_id: i64,
    pub source_hash: String,
    pub keywords: Vec<String>,
    pub textures: BTreeMap<String, TextureEnv>,
    pub floats: BTreeMap<String, f32>,
    pub colors: BTreeMap<String, [f32; 4]>,
    pub presentation: Option<CardPresentation>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CardPresentation {
    pub card_id: i64,
    pub name: String,
    pub kind: String,
    pub rarity: String,
    pub class: u8,
    pub cost: Option<i64>,
    pub attack: Option<i64>,
    pub defense: Option<i64>,
    pub is_evolution: bool,
    pub frame: String,
    pub class_icon: String,
}

#[derive(Debug, Deserialize)]
struct CardMetadata {
    card_id: i64,
    cost: Option<i64>,
    rarity: Option<i64>,
    name_chs: String,
}

#[derive(Debug, Deserialize)]
struct ManifestDocument {
    assets: Vec<ManifestAsset>,
}

#[derive(Debug, Deserialize)]
struct ManifestAsset {
    name: String,
    asset_id: i64,
    all_dependencies: Vec<i64>,
}

#[derive(Debug, Clone, Serialize)]
struct FoilIndexItem {
    id: String,
    card_style_id: i64,
    material_id: i64,
    config: String,
    preview: Option<String>,
}

#[derive(Debug, Serialize)]
struct FoilIndex {
    config_version: u32,
    variant: String,
    items: Vec<FoilIndexItem>,
}

#[derive(Debug, Clone, Deserialize)]
struct SleeveMetadata {
    sleeve_id: i64,
    resource_name: String,
    is_premium: bool,
    parent_sleeve_id: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SleeveFoilMaterial {
    config_version: u32,
    id: String,
    sleeve_id: i64,
    parent_sleeve_id: Option<i64>,
    source_hash: String,
    keywords: Vec<String>,
    textures: BTreeMap<String, TextureEnv>,
    floats: BTreeMap<String, f32>,
    colors: BTreeMap<String, [f32; 4]>,
}

#[derive(Debug, Clone, Serialize)]
struct SleeveFoilIndexItem {
    id: String,
    sleeve_id: i64,
    parent_sleeve_id: Option<i64>,
    config: String,
    preview: Option<String>,
}

#[derive(Debug, Serialize)]
struct SleeveFoilIndex {
    config_version: u32,
    variant: String,
    items: Vec<SleeveFoilIndexItem>,
}

#[derive(Debug, Default)]
struct RawTextureEnv {
    file_id: i32,
    path_id: i64,
    scale: [f32; 2],
    offset: [f32; 2],
}

#[derive(Debug, Default)]
struct ParsedMaterial {
    keywords: Vec<String>,
    textures: BTreeMap<String, RawTextureEnv>,
    floats: BTreeMap<String, f32>,
    colors: BTreeMap<String, [f32; 4]>,
}

/// Export regular premium `*_M.ab` materials into a browser-friendly directory.
pub fn process_foil_materials(
    data_dir: &Path,
    asset_studio_path: &Path,
    variant: &str,
    output: Option<&Path>,
    only_id: Option<i64>,
    force: bool,
) -> anyhow::Result<FoilStats> {
    let decrypted = data_dir.join("variants").join(variant).join("decrypted");
    let material_dir = decrypted.join("Card/Materials");
    if !material_dir.exists() {
        bail!("闪卡材质目录不存在: {}", material_dir.display());
    }

    let output_root = output
        .map(Path::to_path_buf)
        .unwrap_or_else(|| data_dir.join("exports/foil-materials"));
    let config_dir = output_root.join("materials");
    let texture_dir = output_root.join("textures");
    fs::create_dir_all(&config_dir)?;
    fs::create_dir_all(&texture_dir)?;

    let mut bundles = collect_material_bundles(&material_dir)?;
    bundles.sort();
    if bundles.is_empty() {
        bail!("没有找到普通卡闪卡材质");
    }

    if let Some(only_id) = only_id {
        bundles
            .retain(|path| material_id_from_path(path).is_some_and(|id| id_matches(id, only_id)));
        if bundles.is_empty() {
            bail!("没有找到 card/card_style/material id={only_id} 的普通闪卡材质");
        }
    }

    let pending: Vec<(i64, PathBuf, String)> = bundles
        .iter()
        .filter_map(|path| {
            let material_id = material_id_from_path(path)?;
            let source_hash = sha256_file(path).ok()?;
            if force {
                return Some((material_id, path.clone(), source_hash));
            }
            let config_path = config_dir.join(format!("{}.json", material_id - 10));
            (!config_hash_matches(&config_path, &source_hash)).then_some((
                material_id,
                path.clone(),
                source_hash,
            ))
        })
        .collect();
    let presentations = load_presentations(data_dir)?;
    let mut stats = FoilStats::default();
    stats.skipped = bundles.len().saturating_sub(pending.len());

    if !pending.is_empty() {
        let material_ids: Vec<i64> = pending.iter().map(|(id, _, _)| *id).collect();
        let (source_maps, dependency_bundles) = build_texture_source_maps(
            data_dir,
            &decrypted,
            asset_studio_path,
            variant,
            &material_ids,
        )?;
        let material_bundles: Vec<PathBuf> =
            pending.iter().map(|(_, path, _)| path.clone()).collect();

        let before_textures = count_png_files(&texture_dir);
        batch_export_textures(
            &dependency_bundles,
            &texture_dir,
            asset_studio_path,
            "导出特效纹理",
        )?;
        batch_export_textures(
            &material_bundles,
            &texture_dir,
            asset_studio_path,
            "导出遮罩纹理",
        )?;
        validate_exported_textures(&texture_dir)?;
        stats.textures = count_png_files(&texture_dir).saturating_sub(before_textures);

        process_material_batches(
            &pending,
            &source_maps,
            &presentations,
            &config_dir,
            asset_studio_path,
            &mut stats,
        )?;
    }

    let mut items = collect_index_items(&config_dir);
    items.sort_by_key(|item| item.card_style_id);
    let index = FoilIndex {
        config_version: CONFIG_VERSION,
        variant: variant.to_string(),
        items,
    };
    fs::write(
        output_root.join("index.json"),
        serde_json::to_string_pretty(&index)? + "\n",
    )?;
    Ok(stats)
}

/// Export premium sleeve materials into a browser-friendly directory.
pub fn process_foil_sleeves(
    data_dir: &Path,
    asset_studio_path: &Path,
    variant: &str,
    output: Option<&Path>,
    only_id: Option<i64>,
    force: bool,
) -> anyhow::Result<FoilStats> {
    let decrypted = data_dir.join("variants").join(variant).join("decrypted");
    let material_dir = decrypted.join("Sleeve/Materials");
    if !material_dir.exists() {
        bail!("闪背材质目录不存在: {}", material_dir.display());
    }

    let metadata_path = data_dir.join("exports/analysis/sleeves_full.json");
    let mut sleeves: Vec<SleeveMetadata> =
        serde_json::from_str(&fs::read_to_string(&metadata_path).with_context(|| {
            format!(
                "无法读取 {}，请先运行 wbu master sleeves",
                metadata_path.display()
            )
        })?)?;
    sleeves.retain(|sleeve| sleeve.is_premium);
    if sleeves.is_empty()
        || only_id.is_some_and(|id| !sleeves.iter().any(|sleeve| sleeve.sleeve_id == id))
    {
        bail!("没有找到符合条件的 premium 卡背");
    }

    let dependency_prefixes = [
        "Card/Common/Foil/Textures/",
        "Assets/_Wizard2Resources/Sleeve/Textures/",
    ];
    let manifest_path = data_dir
        .join("manifests/json")
        .join(format!("assetbundle.{variant}.manifest.json"));
    let manifest: ManifestDocument = serde_json::from_str(
        &fs::read_to_string(&manifest_path)
            .with_context(|| format!("无法读取 {}", manifest_path.display()))?,
    )?;
    let manifest_by_id: HashMap<i64, &ManifestAsset> = manifest
        .assets
        .iter()
        .map(|asset| (asset.asset_id, asset))
        .collect();
    let manifest_by_name: HashMap<&str, &ManifestAsset> = manifest
        .assets
        .iter()
        .map(|asset| (asset.name.as_str(), asset))
        .collect();

    let output_root = output
        .map(Path::to_path_buf)
        .unwrap_or_else(|| data_dir.join("exports/foil-sleeves"));
    let config_dir = output_root.join("materials");
    let texture_dir = output_root.join("textures");
    fs::create_dir_all(&config_dir)?;
    fs::create_dir_all(&texture_dir)?;

    let mut pending = Vec::new();
    let mut expected_hashes = HashMap::new();
    let mut total = 0usize;
    for sleeve in &sleeves {
        let bundle = material_dir.join(format!("{}_M.ab", sleeve.resource_name));
        if !bundle.exists() {
            tracing::warn!("{}: 闪背材质包不存在", sleeve.sleeve_id);
            continue;
        }
        let material_name = format!("Sleeve/Materials/{}_M", sleeve.resource_name);
        let source_hash = sha256_with_dependencies(
            &bundle,
            &material_name,
            &manifest_by_name,
            &manifest_by_id,
            &decrypted,
            &dependency_prefixes,
        )?;
        expected_hashes.insert(sleeve.sleeve_id, source_hash.clone());
        if only_id.is_some_and(|id| id != sleeve.sleeve_id) {
            continue;
        }
        total += 1;
        let config_path = config_dir.join(format!("{}.json", sleeve.sleeve_id));
        if force || !sleeve_config_hash_matches(&config_path, &source_hash) {
            pending.push((sleeve.clone(), bundle, source_hash));
        }
    }
    if total == 0 {
        bail!("指定 premium 卡背的材质包不存在");
    }

    let mut stats = FoilStats::default();
    stats.skipped = total.saturating_sub(pending.len());
    if !pending.is_empty() {
        let assets: Vec<(i64, String)> = pending
            .iter()
            .map(|(sleeve, _, _)| {
                (
                    sleeve.sleeve_id,
                    format!("Sleeve/Materials/{}_M", sleeve.resource_name),
                )
            })
            .collect();
        let (source_maps, dependency_bundles) = build_texture_source_maps_for_assets(
            data_dir,
            &decrypted,
            asset_studio_path,
            variant,
            &assets,
            &dependency_prefixes,
        )?;
        let material_bundles: Vec<PathBuf> =
            pending.iter().map(|(_, path, _)| path.clone()).collect();
        let before_textures = count_png_files(&texture_dir);
        batch_export_textures(
            &dependency_bundles,
            &texture_dir,
            asset_studio_path,
            "导出闪背特效纹理",
        )?;
        batch_export_textures(
            &material_bundles,
            &texture_dir,
            asset_studio_path,
            "导出闪背本地纹理",
        )?;
        validate_exported_textures(&texture_dir)?;
        stats.textures = count_png_files(&texture_dir).saturating_sub(before_textures);
        process_sleeve_material_batches(
            &pending,
            &source_maps,
            &config_dir,
            asset_studio_path,
            &mut stats,
        )?;
    }

    let mut items = collect_sleeve_index_items(&config_dir, &expected_hashes, only_id);
    items.sort_by_key(|item| item.sleeve_id);
    let index = SleeveFoilIndex {
        config_version: CONFIG_VERSION,
        variant: variant.to_string(),
        items,
    };
    fs::write(
        output_root.join("index.json"),
        serde_json::to_string_pretty(&index)? + "\n",
    )?;
    Ok(stats)
}

fn collect_sleeve_index_items(
    config_dir: &Path,
    expected_hashes: &HashMap<i64, String>,
    only_id: Option<i64>,
) -> Vec<SleeveFoilIndexItem> {
    let Ok(entries) = fs::read_dir(config_dir) else {
        return Vec::new();
    };
    entries
        .filter_map(Result::ok)
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "json"))
        .filter_map(|entry| {
            serde_json::from_str::<SleeveFoilMaterial>(&fs::read_to_string(entry.path()).ok()?).ok()
        })
        .filter(|material| {
            material.config_version == CONFIG_VERSION
                && expected_hashes.contains_key(&material.sleeve_id)
                && (only_id.is_some_and(|id| id != material.sleeve_id)
                    || expected_hashes.get(&material.sleeve_id) == Some(&material.source_hash))
        })
        .map(|material| SleeveFoilIndexItem {
            id: material.id.clone(),
            sleeve_id: material.sleeve_id,
            parent_sleeve_id: material.parent_sleeve_id,
            config: format!("materials/{}.json", material.id),
            preview: material
                .textures
                .get("_MainTex")
                .and_then(|texture| texture.texture.clone()),
        })
        .collect()
}

fn collect_index_items(config_dir: &Path) -> Vec<FoilIndexItem> {
    let Ok(entries) = fs::read_dir(config_dir) else {
        return Vec::new();
    };
    entries
        .filter_map(Result::ok)
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "json"))
        .filter_map(|entry| read_material(&entry.path()).ok())
        .filter(|material| {
            material.config_version == CONFIG_VERSION
                && is_regular_foil_material(material.material_id)
        })
        .map(|material| index_item(&material))
        .collect()
}

fn collect_material_bundles(dir: &Path) -> anyhow::Result<Vec<PathBuf>> {
    let mut result = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.ends_with("_M.ab")
            && !name.starts_with('7')
            && material_id_from_path(&entry.path()).is_some_and(is_regular_foil_material)
        {
            result.push(entry.path());
        }
    }
    Ok(result)
}

fn material_id_from_path(path: &Path) -> Option<i64> {
    path.file_stem()?.to_str()?.strip_suffix("_M")?.parse().ok()
}

fn is_regular_foil_material(material_id: i64) -> bool {
    let id = material_id.to_string();
    material_id > 0 && id.len() == 9 && !id.starts_with('7') && (material_id / 10) % 10 == 1
}

fn id_matches(material_id: i64, requested: i64) -> bool {
    let card_style_id = material_id - 10;
    requested == material_id || requested == card_style_id || requested == card_style_id / 10
}

fn build_texture_source_maps(
    data_dir: &Path,
    decrypted: &Path,
    asset_studio_path: &Path,
    variant: &str,
    material_ids: &[i64],
) -> anyhow::Result<(HashMap<i64, HashMap<i64, PathBuf>>, Vec<PathBuf>)> {
    let assets: Vec<(i64, String)> = material_ids
        .iter()
        .map(|id| (*id, format!("Card/Materials/{id}_M")))
        .collect();
    build_texture_source_maps_for_assets(
        data_dir,
        decrypted,
        asset_studio_path,
        variant,
        &assets,
        &[
            "Card/Common/Foil/Textures/",
            "Assets/_Wizard2Resources/Card/Textures/",
        ],
    )
}

fn build_texture_source_maps_for_assets(
    data_dir: &Path,
    decrypted: &Path,
    asset_studio_path: &Path,
    variant: &str,
    assets: &[(i64, String)],
    dependency_prefixes: &[&str],
) -> anyhow::Result<(HashMap<i64, HashMap<i64, PathBuf>>, Vec<PathBuf>)> {
    let manifest_path = data_dir
        .join("manifests/json")
        .join(format!("assetbundle.{variant}.manifest.json"));
    let manifest: ManifestDocument = serde_json::from_str(
        &fs::read_to_string(&manifest_path)
            .with_context(|| format!("无法读取 {}", manifest_path.display()))?,
    )?;
    let by_id: HashMap<i64, &ManifestAsset> = manifest
        .assets
        .iter()
        .map(|asset| (asset.asset_id, asset))
        .collect();
    let by_name: HashMap<&str, &ManifestAsset> = manifest
        .assets
        .iter()
        .map(|asset| (asset.name.as_str(), asset))
        .collect();
    let mut candidates = HashSet::new();
    let mut material_candidates: HashMap<i64, Vec<PathBuf>> = HashMap::new();
    for (material_id, material_name) in assets {
        let Some(material) = by_name.get(material_name.as_str()) else {
            continue;
        };
        for dependency_id in &material.all_dependencies {
            let Some(dependency) = by_id.get(dependency_id) else {
                continue;
            };
            if dependency_prefixes
                .iter()
                .any(|prefix| dependency.name.starts_with(prefix))
            {
                let path = decrypted.join(format!("{}.ab", dependency.name));
                candidates.insert(path.clone());
                material_candidates
                    .entry(*material_id)
                    .or_default()
                    .push(path);
            }
        }
    }
    let candidates: Vec<PathBuf> = candidates
        .into_iter()
        .filter(|path| path.exists())
        .collect();
    let candidate_path_ids = batch_texture_path_ids(&candidates, asset_studio_path)?;
    let mut result = HashMap::new();
    for (material_id, paths) in material_candidates {
        let map = result.entry(material_id).or_insert_with(HashMap::new);
        for path in paths {
            if let Some(path_ids) = candidate_path_ids.get(&path) {
                for path_id in path_ids {
                    map.insert(*path_id, path.clone());
                }
            }
        }
    }
    Ok((result, candidates))
}

#[allow(clippy::too_many_arguments)]
fn export_one_from_files(
    files: &[PathBuf],
    id: &str,
    card_style_id: i64,
    material_id: i64,
    presentation: Option<CardPresentation>,
    source_hash: String,
    config_path: &Path,
    texture_sources: &HashMap<i64, PathBuf>,
) -> anyhow::Result<FoilMaterial> {
    let (parsed, textures) = resolve_material(files, texture_sources)?;
    let material = FoilMaterial {
        config_version: CONFIG_VERSION,
        id: id.to_string(),
        card_style_id,
        material_id,
        source_hash,
        keywords: parsed.keywords,
        textures,
        floats: parsed.floats,
        colors: parsed.colors,
        presentation,
    };
    fs::write(config_path, serde_json::to_string_pretty(&material)? + "\n")?;
    Ok(material)
}

fn resolve_material(
    files: &[PathBuf],
    texture_sources: &HashMap<i64, PathBuf>,
) -> anyhow::Result<(ParsedMaterial, BTreeMap<String, TextureEnv>)> {
    let material_dump = files
        .iter()
        .find(|path| is_dump_type(path, "Material Base"))
        .context("AssetStudio dump 中没有 Material")?;
    let parsed = parse_material_dump(&fs::read_to_string(material_dump)?)?;
    let local_textures = parse_local_textures(&files);

    let mut textures = BTreeMap::new();
    for (property, raw) in &parsed.textures {
        let texture = if raw.path_id == 0 {
            None
        } else if raw.file_id == 0 {
            let name = local_textures.get(&raw.path_id).cloned();
            if let Some(name) = name {
                Some(format!("textures/{name}.png"))
            } else {
                None
            }
        } else {
            let dependency = texture_sources.get(&raw.path_id);
            if let Some(dependency) = dependency {
                let name = dependency
                    .file_stem()
                    .and_then(|value| value.to_str())
                    .context("无效纹理 bundle 文件名")?;
                Some(format!("textures/{name}.png"))
            } else {
                None
            }
        };
        textures.insert(
            property.clone(),
            TextureEnv {
                texture,
                file_id: raw.file_id,
                path_id: raw.path_id,
                scale: raw.scale,
                offset: raw.offset,
            },
        );
    }

    Ok((parsed, textures))
}

fn presentation_card_id(card_style_id: i64) -> i64 {
    card_style_id / 10 + card_style_id % 2
}

fn load_presentations(data_dir: &Path) -> anyhow::Result<HashMap<i64, CardPresentation>> {
    let cards_path = data_dir.join("exports/analysis/cards_full.json");
    let cards: Vec<CardMetadata> = serde_json::from_str(
        &fs::read_to_string(&cards_path)
            .with_context(|| format!("无法读取 {}", cards_path.display()))?,
    )?;
    let stats_path = data_dir.join("exports/master-data/Chs/BaseCardMaster.json");
    let stat_rows: Vec<Vec<serde_json::Value>> = serde_json::from_str(
        &fs::read_to_string(&stats_path)
            .with_context(|| format!("无法读取 {}", stats_path.display()))?,
    )?;
    let stats: HashMap<i64, (Option<i64>, Option<i64>)> = stat_rows
        .into_iter()
        .filter_map(|row| {
            Some((
                row.first()?.as_i64()?,
                (
                    row.get(5).and_then(|value| value.as_i64()),
                    row.get(6).and_then(|value| value.as_i64()),
                ),
            ))
        })
        .collect();

    let mut result = HashMap::new();
    for card in cards {
        let id = card.card_id.to_string();
        if id.len() != 8 || id.starts_with('7') {
            continue;
        }
        let kind = match id.as_bytes().get(5).map(|value| value - b'0') {
            Some(1) => "follower",
            Some(2) => "amulet",
            Some(3) => "spell",
            _ => continue,
        };
        let rarity = match card.rarity {
            Some(1) => "bronze",
            Some(2) => "silver",
            Some(3) => "gold",
            Some(4) => "legend",
            _ => continue,
        };
        let class = id.as_bytes().get(3).map(|value| value - b'0').unwrap_or(0);
        let base_card_id = if card.card_id % 10 == 1 {
            card.card_id - 1
        } else {
            card.card_id
        };
        let (attack, defense) = stats
            .get(&card.card_id)
            .or_else(|| stats.get(&base_card_id))
            .copied()
            .unwrap_or_default();
        result.insert(
            card.card_id,
            CardPresentation {
                card_id: card.card_id,
                name: card.name_chs,
                kind: kind.to_string(),
                rarity: rarity.to_string(),
                class,
                cost: card.cost,
                attack,
                defense,
                is_evolution: card.card_id % 10 == 1,
                frame: format!("frame2d_{kind}_{rarity}.png"),
                class_icon: format!("card2d_class_icon_{class}.png"),
            },
        );
    }
    Ok(result)
}

fn progress(label: &str, len: usize) -> ProgressBar {
    let bar = ProgressBar::new(len as u64);
    bar.set_style(
        ProgressStyle::with_template("{msg:14} [{bar:36.cyan/blue}] {pos}/{len} {elapsed}")
            .unwrap()
            .progress_chars("=> "),
    );
    bar.set_message(label.to_string());
    bar
}

fn stage_batch(paths: &[PathBuf]) -> anyhow::Result<(TempDir, HashMap<String, PathBuf>)> {
    let temp = TempDir::new()?;
    let mut staged = HashMap::new();
    for path in paths {
        let name = path
            .file_name()
            .and_then(|value| value.to_str())
            .context("无效 AssetBundle 文件名")?
            .to_string();
        let target = temp.path().join(&name);
        #[cfg(unix)]
        std::os::unix::fs::symlink(path, &target)?;
        #[cfg(windows)]
        if std::os::windows::fs::symlink_file(path, &target).is_err() {
            fs::copy(path, &target)?;
        }
        staged.insert(name, path.clone());
    }
    Ok((temp, staged))
}

fn run_asset_studio_dump_batch(
    input_dir: &Path,
    output: &Path,
    asset_studio_path: &Path,
) -> anyhow::Result<()> {
    let status = Command::new(asset_studio_path)
        .arg(input_dir)
        .args([
            "-m",
            "dump",
            "--load-all",
            "-g",
            "fileName",
            "-f",
            "assetName_pathID",
            "-o",
            &output.to_string_lossy(),
            "--unity-version",
            UNITY_VERSION,
            "--log-level",
            "error",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .with_context(|| format!("无法启动 AssetStudio: {}", asset_studio_path.display()))?;
    if !status.success() {
        bail!("AssetStudio dump 失败: {:?}", status.code());
    }
    Ok(())
}

fn run_asset_studio_texture_batch(
    input_dir: &Path,
    output: &Path,
    asset_studio_path: &Path,
) -> anyhow::Result<()> {
    let status = Command::new(asset_studio_path)
        .arg(input_dir)
        .args([
            "-t",
            "tex2d",
            "-g",
            "none",
            "-f",
            "assetName",
            "-r",
            "-o",
            &output.to_string_lossy(),
            "--unity-version",
            UNITY_VERSION,
            "--max-export-tasks",
            "1",
            "--log-level",
            "error",
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?;
    if !status.success() {
        bail!("AssetStudio 批量纹理导出失败: {:?}", status.code());
    }
    Ok(())
}

fn batch_texture_path_ids(
    candidates: &[PathBuf],
    asset_studio_path: &Path,
) -> anyhow::Result<HashMap<PathBuf, Vec<i64>>> {
    let bar = progress("建立纹理索引", candidates.len());
    let mut result = HashMap::new();
    for chunk in candidates.chunks(BATCH_SIZE) {
        let (stage, staged) = stage_batch(chunk)?;
        let output = TempDir::new()?;
        run_asset_studio_dump_batch(stage.path(), output.path(), asset_studio_path)?;
        for file in collect_files(output.path()) {
            let Some(path_id) = path_id_from_dump_name(&file) else {
                continue;
            };
            let Ok(relative) = file.strip_prefix(output.path()) else {
                continue;
            };
            let Some(group) = relative.components().next() else {
                continue;
            };
            let group = group.as_os_str().to_string_lossy();
            let Some(filename) = group.strip_suffix("_export") else {
                continue;
            };
            if let Some(source) = staged.get(filename) {
                result
                    .entry(source.clone())
                    .or_insert_with(Vec::new)
                    .push(path_id);
            }
        }
        bar.inc(chunk.len() as u64);
    }
    bar.finish_and_clear();
    Ok(result)
}

fn batch_export_textures(
    bundles: &[PathBuf],
    texture_dir: &Path,
    asset_studio_path: &Path,
    label: &str,
) -> anyhow::Result<()> {
    let bar = progress(label, bundles.len());
    for chunk in bundles.chunks(BATCH_SIZE) {
        let (stage, _) = stage_batch(chunk)?;
        run_asset_studio_texture_batch(stage.path(), texture_dir, asset_studio_path)?;
        bar.inc(chunk.len() as u64);
    }
    bar.finish_and_clear();
    Ok(())
}

fn process_material_batches(
    pending: &[(i64, PathBuf, String)],
    source_maps: &HashMap<i64, HashMap<i64, PathBuf>>,
    presentations: &HashMap<i64, CardPresentation>,
    config_dir: &Path,
    asset_studio_path: &Path,
    stats: &mut FoilStats,
) -> anyhow::Result<()> {
    let bar = progress("转换材质", pending.len());
    for chunk in pending.chunks(BATCH_SIZE) {
        let paths: Vec<PathBuf> = chunk.iter().map(|(_, path, _)| path.clone()).collect();
        let (stage, _) = stage_batch(&paths)?;
        let output = TempDir::new()?;
        run_asset_studio_dump_batch(stage.path(), output.path(), asset_studio_path)?;
        for (material_id, bundle, source_hash) in chunk {
            let card_style_id = material_id - 10;
            let group_name = format!(
                "{}_export",
                bundle
                    .file_name()
                    .and_then(|value| value.to_str())
                    .unwrap_or_default()
            );
            let files = collect_files(&output.path().join(group_name));
            let result = source_maps
                .get(material_id)
                .context("材质纹理依赖索引缺失")
                .and_then(|sources| {
                    export_one_from_files(
                        &files,
                        &card_style_id.to_string(),
                        card_style_id,
                        *material_id,
                        presentations
                            .get(&presentation_card_id(card_style_id))
                            .cloned(),
                        source_hash.clone(),
                        &config_dir.join(format!("{card_style_id}.json")),
                        sources,
                    )
                });
            match result {
                Ok(_) => stats.processed += 1,
                Err(error) => {
                    tracing::error!("{material_id}: {error:#}");
                    stats.failed += 1;
                }
            }
            bar.inc(1);
        }
    }
    bar.finish_and_clear();
    Ok(())
}

fn process_sleeve_material_batches(
    pending: &[(SleeveMetadata, PathBuf, String)],
    source_maps: &HashMap<i64, HashMap<i64, PathBuf>>,
    config_dir: &Path,
    asset_studio_path: &Path,
    stats: &mut FoilStats,
) -> anyhow::Result<()> {
    let bar = progress("转换闪背材质", pending.len());
    for chunk in pending.chunks(BATCH_SIZE) {
        let paths: Vec<PathBuf> = chunk.iter().map(|(_, path, _)| path.clone()).collect();
        let (stage, _) = stage_batch(&paths)?;
        let output = TempDir::new()?;
        run_asset_studio_dump_batch(stage.path(), output.path(), asset_studio_path)?;
        for (sleeve, bundle, source_hash) in chunk {
            let group_name = format!(
                "{}_export",
                bundle
                    .file_name()
                    .and_then(|value| value.to_str())
                    .unwrap_or_default()
            );
            let files = collect_files(&output.path().join(group_name));
            let result = source_maps
                .get(&sleeve.sleeve_id)
                .context("闪背纹理依赖索引缺失")
                .and_then(|sources| resolve_material(&files, sources))
                .and_then(|(parsed, textures)| {
                    let material = SleeveFoilMaterial {
                        config_version: CONFIG_VERSION,
                        id: sleeve.sleeve_id.to_string(),
                        sleeve_id: sleeve.sleeve_id,
                        parent_sleeve_id: sleeve.parent_sleeve_id,
                        source_hash: source_hash.clone(),
                        keywords: parsed.keywords,
                        textures,
                        floats: parsed.floats,
                        colors: parsed.colors,
                    };
                    fs::write(
                        config_dir.join(format!("{}.json", sleeve.sleeve_id)),
                        serde_json::to_string_pretty(&material)? + "\n",
                    )?;
                    Ok(material)
                });
            match result {
                Ok(_) => stats.processed += 1,
                Err(error) => {
                    tracing::error!("{}: {error:#}", sleeve.sleeve_id);
                    stats.failed += 1;
                }
            }
            bar.inc(1);
        }
    }
    bar.finish_and_clear();
    Ok(())
}

fn count_png_files(dir: &Path) -> usize {
    fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "png"))
                .count()
        })
        .unwrap_or(0)
}

fn validate_exported_textures(dir: &Path) -> anyhow::Result<()> {
    let empty: Vec<String> = fs::read_dir(dir)?
        .filter_map(Result::ok)
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "png"))
        .filter_map(|entry| {
            entry
                .metadata()
                .ok()
                .filter(|metadata| metadata.len() == 0)
                .map(|_| entry.file_name().to_string_lossy().into_owned())
        })
        .collect();
    if !empty.is_empty() {
        bail!("闪卡纹理导出不完整（零字节）: {}", empty.join(", "));
    }
    Ok(())
}

fn collect_files(root: &Path) -> Vec<PathBuf> {
    WalkDir::new(root)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file())
        .map(|entry| entry.into_path())
        .collect()
}

fn is_dump_type(path: &Path, expected: &str) -> bool {
    fs::read_to_string(path)
        .ok()
        .and_then(|text| text.lines().next().map(|line| line.trim() == expected))
        .unwrap_or(false)
}

fn parse_local_textures(files: &[PathBuf]) -> HashMap<i64, String> {
    let mut result = HashMap::new();
    for path in files {
        if !is_dump_type(path, "Texture2D Base") {
            continue;
        }
        let Some(path_id) = path_id_from_dump_name(path) else {
            continue;
        };
        let Ok(text) = fs::read_to_string(path) else {
            continue;
        };
        if let Some(name) = text
            .lines()
            .find_map(|line| quoted_value(line.trim(), "string m_Name = "))
        {
            result.insert(path_id, name);
        }
    }
    result
}

fn path_id_from_dump_name(path: &Path) -> Option<i64> {
    let name = path.file_name()?.to_string_lossy();
    let value = name.rsplit_once('@')?.1.trim_end_matches(".txt").trim();
    value.parse().ok()
}

fn parse_material_dump(text: &str) -> anyhow::Result<ParsedMaterial> {
    #[derive(Clone, Copy, PartialEq)]
    enum Section {
        None,
        Keywords,
        Textures,
        Floats,
        Colors,
    }
    #[derive(Clone, Copy)]
    enum VecPart {
        None,
        Scale,
        Offset,
    }

    let mut result = ParsedMaterial::default();
    let mut section = Section::None;
    let mut property: Option<String> = None;
    let mut texture = RawTextureEnv::default();
    texture.scale = [1.0, 1.0];
    let mut vec_part = VecPart::None;
    let mut color = [0.0; 4];

    let flush_texture = |result: &mut ParsedMaterial,
                         property: &mut Option<String>,
                         texture: &mut RawTextureEnv| {
        if let Some(name) = property.take() {
            result.textures.insert(name, std::mem::take(texture));
            texture.scale = [1.0, 1.0];
        }
    };

    for raw_line in text.lines() {
        let line = raw_line.trim();
        match line {
            "vector m_ValidKeywords" => section = Section::Keywords,
            "vector m_InvalidKeywords" => section = Section::None,
            "map m_TexEnvs" => section = Section::Textures,
            "map m_Ints" => {
                flush_texture(&mut result, &mut property, &mut texture);
                section = Section::None;
            }
            "map m_Floats" => section = Section::Floats,
            "map m_Colors" => section = Section::Colors,
            _ => {}
        }

        if section == Section::Keywords {
            if let Some(value) = quoted_value(line, "string data = ") {
                result.keywords.push(value);
            }
            continue;
        }

        if let Some(name) = quoted_value(line, "string first = ") {
            match section {
                Section::Textures => {
                    flush_texture(&mut result, &mut property, &mut texture);
                    property = Some(name);
                    vec_part = VecPart::None;
                }
                Section::Floats | Section::Colors => property = Some(name),
                _ => {}
            }
            continue;
        }

        match section {
            Section::Textures if property.is_some() => {
                if let Some(value) = number_value::<i32>(line, "int m_FileID = ") {
                    texture.file_id = value;
                } else if let Some(value) = number_value::<i64>(line, "SInt64 m_PathID = ") {
                    texture.path_id = value;
                } else if line == "Vector2f m_Scale" {
                    vec_part = VecPart::Scale;
                } else if line == "Vector2f m_Offset" {
                    vec_part = VecPart::Offset;
                } else if let Some(value) = float_value(line, "float x = ") {
                    match vec_part {
                        VecPart::Scale => texture.scale[0] = value,
                        VecPart::Offset => texture.offset[0] = value,
                        VecPart::None => {}
                    }
                } else if let Some(value) = float_value(line, "float y = ") {
                    match vec_part {
                        VecPart::Scale => texture.scale[1] = value,
                        VecPart::Offset => texture.offset[1] = value,
                        VecPart::None => {}
                    }
                }
            }
            Section::Floats => {
                if let (Some(name), Some(value)) =
                    (property.take(), float_value(line, "float second = "))
                {
                    result.floats.insert(name, value);
                }
            }
            Section::Colors => {
                let component = [
                    ("float r = ", 0),
                    ("float g = ", 1),
                    ("float b = ", 2),
                    ("float a = ", 3),
                ]
                .into_iter()
                .find_map(|(prefix, index)| float_value(line, prefix).map(|value| (index, value)));
                if let Some((index, value)) = component {
                    color[index] = value;
                    if index == 3
                        && let Some(name) = property.take()
                    {
                        result.colors.insert(name, color);
                        color = [0.0; 4];
                    }
                }
            }
            _ => {}
        }
    }
    flush_texture(&mut result, &mut property, &mut texture);
    if result.textures.is_empty() {
        bail!("Material dump 未解析到纹理属性");
    }
    Ok(result)
}

fn quoted_value(line: &str, prefix: &str) -> Option<String> {
    let value = line.strip_prefix(prefix)?.trim();
    Some(value.strip_prefix('"')?.strip_suffix('"')?.to_string())
}

fn number_value<T: std::str::FromStr>(line: &str, prefix: &str) -> Option<T> {
    line.strip_prefix(prefix)?.trim().parse().ok()
}

fn float_value(line: &str, prefix: &str) -> Option<f32> {
    let value = number_value::<f32>(line, prefix)?;
    Some(if value.is_finite() { value } else { 0.0 })
}

fn sha256_file(path: &Path) -> anyhow::Result<String> {
    let mut hasher = Sha256::new();
    hasher.update(fs::read(path)?);
    Ok(format!("{:x}", hasher.finalize()))
}

fn sha256_with_dependencies(
    bundle: &Path,
    material_name: &str,
    manifest_by_name: &HashMap<&str, &ManifestAsset>,
    manifest_by_id: &HashMap<i64, &ManifestAsset>,
    decrypted: &Path,
    dependency_prefixes: &[&str],
) -> anyhow::Result<String> {
    let mut hasher = Sha256::new();
    hasher.update(fs::read(bundle)?);
    if let Some(material) = manifest_by_name.get(material_name) {
        for dependency_id in &material.all_dependencies {
            let Some(dependency) = manifest_by_id.get(dependency_id) else {
                continue;
            };
            if !dependency_prefixes
                .iter()
                .any(|prefix| dependency.name.starts_with(prefix))
            {
                continue;
            }
            hasher.update(dependency.name.as_bytes());
            let path = decrypted.join(format!("{}.ab", dependency.name));
            if path.exists() {
                hasher.update(fs::read(path)?);
            }
        }
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn config_hash_matches(path: &Path, hash: &str) -> bool {
    read_material(path)
        .map(|material| material.config_version == CONFIG_VERSION && material.source_hash == hash)
        .unwrap_or(false)
}

fn sleeve_config_hash_matches(path: &Path, hash: &str) -> bool {
    fs::read_to_string(path)
        .ok()
        .and_then(|value| serde_json::from_str::<SleeveFoilMaterial>(&value).ok())
        .is_some_and(|material| {
            material.config_version == CONFIG_VERSION && material.source_hash == hash
        })
}

fn read_material(path: &Path) -> anyhow::Result<FoilMaterial> {
    Ok(serde_json::from_str(&fs::read_to_string(path)?)?)
}

fn index_item(material: &FoilMaterial) -> FoilIndexItem {
    let preview = material
        .textures
        .get("_MainTex")
        .and_then(|texture| texture.texture.clone());
    FoilIndexItem {
        id: material.id.clone(),
        card_style_id: material.card_style_id,
        material_id: material.material_id,
        config: format!("materials/{}.json", material.id),
        preview,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimal_material() {
        let dump = r#"
vector m_ValidKeywords
  string data = "USE_MASK"
vector m_InvalidKeywords
map m_TexEnvs
  string first = "_MainTex"
  int m_FileID = 3
  SInt64 m_PathID = 42
  Vector2f m_Scale
    float x = 2
    float y = 3
  Vector2f m_Offset
    float x = 0.25
    float y = -0.5
map m_Ints
map m_Floats
  string first = "_Front1_MoveType"
  float second = 4
map m_Colors
  string first = "_Front1Color"
  float r = 1
  float g = 0.5
  float b = 0.25
  float a = 1
"#;
        let parsed = parse_material_dump(dump).unwrap();
        assert_eq!(parsed.keywords, ["USE_MASK"]);
        assert_eq!(parsed.textures["_MainTex"].scale, [2.0, 3.0]);
        assert_eq!(parsed.floats["_Front1_MoveType"], 4.0);
        assert_eq!(parsed.colors["_Front1Color"], [1.0, 0.5, 0.25, 1.0]);
    }

    #[test]
    fn identifies_regular_foil_material_ids() {
        assert!(is_regular_foil_material(101141110));
        assert!(is_regular_foil_material(101141111));
        assert!(!is_regular_foil_material(101141100));
        assert!(!is_regular_foil_material(701141110));
        assert!(id_matches(101141110, 10114110));
        assert!(id_matches(101141110, 101141100));
        assert!(id_matches(101141110, 101141110));
        assert_eq!(presentation_card_id(101141100), 10114110);
        assert_eq!(presentation_card_id(101141101), 10114111);
        assert_eq!(presentation_card_id(102441102), 10244110);
        assert_eq!(presentation_card_id(102441103), 10244111);
    }

    #[test]
    fn rejects_empty_exported_textures() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("valid.png"), b"png").unwrap();
        assert!(validate_exported_textures(dir.path()).is_ok());

        fs::write(dir.path().join("empty.png"), []).unwrap();
        assert!(validate_exported_textures(dir.path()).is_err());
    }

    #[test]
    fn sleeve_index_only_includes_current_source_hashes() {
        let dir = TempDir::new().unwrap();
        let material = SleeveFoilMaterial {
            config_version: CONFIG_VERSION,
            id: "123".to_string(),
            sleeve_id: 123,
            parent_sleeve_id: None,
            source_hash: "current".to_string(),
            keywords: Vec::new(),
            textures: BTreeMap::new(),
            floats: BTreeMap::new(),
            colors: BTreeMap::new(),
        };
        fs::write(
            dir.path().join("123.json"),
            serde_json::to_string(&material).unwrap(),
        )
        .unwrap();

        let expected = HashMap::from([(123, "current".to_string())]);
        assert_eq!(
            collect_sleeve_index_items(dir.path(), &expected, None).len(),
            1
        );
        let stale = HashMap::from([(123, "new".to_string())]);
        assert!(collect_sleeve_index_items(dir.path(), &stale, None).is_empty());
        assert_eq!(
            collect_sleeve_index_items(dir.path(), &stale, Some(999)).len(),
            1
        );
    }

    #[test]
    fn sleeve_source_hash_includes_dependency_content() {
        let dir = TempDir::new().unwrap();
        let bundle = dir.path().join("sleeve.ab");
        fs::write(&bundle, b"material").unwrap();
        let dependency_name = "Card/Common/Foil/Textures/effect";
        let dependency = dir.path().join(format!("{dependency_name}.ab"));
        fs::create_dir_all(dependency.parent().unwrap()).unwrap();
        fs::write(&dependency, b"first").unwrap();

        let assets = [
            ManifestAsset {
                name: "Sleeve/Materials/sleeve_123_M".to_string(),
                asset_id: 1,
                all_dependencies: vec![2],
            },
            ManifestAsset {
                name: dependency_name.to_string(),
                asset_id: 2,
                all_dependencies: Vec::new(),
            },
        ];
        let by_name = assets
            .iter()
            .map(|asset| (asset.name.as_str(), asset))
            .collect();
        let by_id = assets.iter().map(|asset| (asset.asset_id, asset)).collect();
        let first = sha256_with_dependencies(
            &bundle,
            "Sleeve/Materials/sleeve_123_M",
            &by_name,
            &by_id,
            dir.path(),
            &["Card/Common/Foil/Textures/"],
        )
        .unwrap();
        fs::write(&dependency, b"second").unwrap();
        let second = sha256_with_dependencies(
            &bundle,
            "Sleeve/Materials/sleeve_123_M",
            &by_name,
            &by_id,
            dir.path(),
            &["Card/Common/Foil/Textures/"],
        )
        .unwrap();
        assert_ne!(first, second);
    }
}
