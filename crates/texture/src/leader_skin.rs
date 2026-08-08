//! LeaderSkin Spine asset extraction.

use anyhow::{Context, bail};
use indicatif::{ProgressBar, ProgressStyle};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const UNITY_VERSION: &str = "2022.3.62f2";
const LEADER_SKIN_DIR: &str = "Prefabs/LeaderSkin";
const CONFIG_VERSION: u32 = 20;

#[derive(Debug, Default)]
pub struct LeaderSkinStats {
    pub processed: usize,
    pub skipped: usize,
    pub failed: usize,
    pub metadata_updated: usize,
}

#[derive(Debug, serde::Serialize)]
struct LeaderSkinConfig {
    config_version: u32,
    id: String,
    kind: String,
    num_id: i64,
    source_hash: String,
    name: String,
    names: BTreeMap<String, String>,
    animations: Vec<String>,
    idle_animation: String,
    skins: Vec<String>,
    skin: String,
    premultiplied_alpha: bool,
    skel: String,
    atlas: String,
    png: String,
}

struct ExportedFiles {
    skel: PathBuf,
    atlas: PathBuf,
    png: PathBuf,
}

pub fn process_leader_skins(
    data_dir: &Path,
    asset_studio_path: &Path,
    variant: &str,
) -> anyhow::Result<LeaderSkinStats> {
    let source_dir = data_dir
        .join("variants")
        .join(variant)
        .join("decrypted")
        .join(LEADER_SKIN_DIR);
    if !source_dir.exists() {
        bail!(
            "LeaderSkin 目录不存在: {}（请先运行 wbu asset batch -v {}）",
            source_dir.display(),
            variant
        );
    }

    let output_root = data_dir.join("exports").join("leader-skin");
    fs::create_dir_all(&output_root)?;

    let mut bundles = Vec::new();
    for entry in fs::read_dir(&source_dir)? {
        let entry = entry?;
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
            continue;
        };
        if path.is_file() && name.ends_with(".ab") && is_leader_skin_bundle(name) {
            bundles.push(path);
        }
    }
    bundles.sort();

    let pb = ProgressBar::new(bundles.len() as u64);
    pb.set_style(
        ProgressStyle::with_template(
            "{spinner:.green} [{elapsed_precise}] [{bar:30.cyan/blue}] {pos}/{len} {msg}",
        )
        .unwrap()
        .progress_chars("=> "),
    );

    let mut stats = LeaderSkinStats::default();
    for bundle in bundles {
        let stem = bundle
            .file_stem()
            .and_then(|value| value.to_str())
            .context("invalid LeaderSkin bundle name")?
            .to_string();
        pb.set_message(stem.clone());

        let source_hash = sha256_file(&bundle)?;
        let output_dir = output_root.join(&stem);
        let config_path = output_dir.join("config.json");
        let resources_fresh = config_hash_matches(&config_path, source_hash.as_str());
        let metadata_current = config_has_version(&config_path);

        if !resources_fresh {
            // Full extraction: AssetStudio + metadata
            match extract_one(
                data_dir,
                &bundle,
                &output_dir,
                asset_studio_path,
                &source_hash,
            ) {
                Ok(()) => stats.processed += 1,
                Err(error) => {
                    stats.failed += 1;
                    eprintln!("LeaderSkin 提取失败 {}: {error:#}", bundle.display());
                }
            }
        } else if !metadata_current {
            // Fast: update metadata from existing resource files only
            match update_metadata(data_dir, &output_dir, &stem, &source_hash) {
                Ok(()) => stats.metadata_updated += 1,
                Err(error) => {
                    stats.failed += 1;
                    eprintln!("LeaderSkin 元数据更新失败 {}: {error:#}", bundle.display());
                }
            }
        } else {
            stats.skipped += 1;
        }
        pb.inc(1);
    }
    pb.finish_with_message("LeaderSkin done");

    Ok(stats)
}

fn is_leader_skin_bundle(name: &str) -> bool {
    name.strip_suffix(".ab")
        .and_then(|stem| stem.split_once('_'))
        .is_some_and(|(kind, id)| {
            matches!(kind, "class" | "vs") && id.chars().all(|c| c.is_ascii_digit())
        })
}

fn extract_one(
    data_dir: &Path,
    bundle: &Path,
    output_dir: &Path,
    asset_studio_path: &Path,
    source_hash: &str,
) -> anyhow::Result<()> {
    let stem = bundle
        .file_stem()
        .and_then(|value| value.to_str())
        .context("invalid LeaderSkin bundle name")?;
    let temp_dir = output_dir.with_file_name(format!("_tmp_{stem}"));
    if output_dir.exists() {
        fs::remove_dir_all(output_dir)?;
    }
    if temp_dir.exists() {
        fs::remove_dir_all(&temp_dir)?;
    }
    fs::create_dir_all(&temp_dir)?;

    run_asset_studio(bundle, &temp_dir, asset_studio_path)?;
    let exported = find_exported_files(&temp_dir, stem)?;
    let (kind, id) = stem
        .split_once('_')
        .context("invalid LeaderSkin bundle stem")?;
    let skel_meta = extract_skel_metadata(&exported.skel, kind)?;
    let num_id: i64 = id.parse()?;
    let (name, names) = load_leader_skin_name(data_dir, num_id).unwrap_or_else(|| {
        let mut map = BTreeMap::new();
        map.insert("jpn".to_string(), stem.to_string());
        (stem.to_string(), map)
    });

    fs::create_dir_all(output_dir)?;
    fs::copy(&exported.skel, output_dir.join(format!("{stem}.skel")))?;
    copy_atlas_with_page_name(
        &exported.atlas,
        &output_dir.join(format!("{stem}.atlas")),
        Some(&exported.png),
        &format!("{stem}.png"),
    )?;
    fs::copy(&exported.png, output_dir.join(format!("{stem}.png")))?;

    let config = LeaderSkinConfig {
        config_version: CONFIG_VERSION,
        id: stem.to_string(),
        kind: kind.to_string(),
        num_id,
        source_hash: source_hash.to_string(),
        name,
        names,
        animations: skel_meta.animations,
        idle_animation: skel_meta.idle_animation,
        skins: skel_meta.skins,
        skin: skel_meta.skin,
        premultiplied_alpha: atlas_has_pma(&exported.atlas)?,
        skel: format!("{stem}.skel"),
        atlas: format!("{stem}.atlas"),
        png: format!("{stem}.png"),
    };
    fs::write(
        output_dir.join("config.json"),
        serde_json::to_string_pretty(&config)? + "\n",
    )?;

    let _ = fs::remove_dir_all(&temp_dir);
    Ok(())
}

#[derive(Debug)]
struct SkelMetadata {
    animations: Vec<String>,
    idle_animation: String,
    skins: Vec<String>,
    skin: String,
}

fn extract_skel_metadata(_path: &Path, kind: &str) -> anyhow::Result<SkelMetadata> {
    if kind == "vs" {
        return Ok(SkelMetadata {
            animations: vec!["idle_1P".into(), "idle_2P".into()],
            idle_animation: "idle_1P".into(),
            skins: vec!["default".into()],
            skin: String::new(),
        });
    }
    // Standard class animations
    let animations = vec![
        "00_idle".into(),
        "01_damage_1".into(),
        "01_damage_2".into(),
        "01_damage_3".into(),
        "01_damage_4".into(),
        "01_damage_5".into(),
        "02_lose_1".into(),
        "02_lose_2".into(),
        "03_hello".into(),
        "04_thanks".into(),
        "05_good".into(),
        "06_sorry".into(),
        "07_shock".into(),
        "08_think".into(),
        "09_excite".into(),
        "10_extra_a1".into(),
        "10_extra_a2".into(),
        "10_extra_a3".into(),
        "10_extra_b1".into(),
        "10_extra_b2".into(),
        "10_extra_b3".into(),
        "11_result_win".into(),
        "11_result_win_loop".into(),
        "12_result_lose_loop".into(),
    ];
    Ok(SkelMetadata {
        animations,
        idle_animation: "00_idle".into(),
        skins: vec!["default".into(), "JP".into(), "US".into()],
        skin: "JP".into(),
    })
}

fn load_leader_skin_name(
    data_dir: &Path,
    num_id: i64,
) -> Option<(String, BTreeMap<String, String>)> {
    let master_root = data_dir.join("exports").join("master-data");
    let ls_path = master_root.join("Chs").join("LeaderSkinMaster.json");
    let text = fs::read_to_string(ls_path).ok()?;
    let rows: Vec<Vec<serde_json::Value>> = serde_json::from_str(&text).ok()?;

    let ent_key = rows
        .iter()
        .find(|row| row.first().and_then(|value| value.as_i64()) == Some(num_id))
        .and_then(|row| row.get(9))
        .and_then(|value| value.as_i64())
        .map(|label_id| format!("ENT_{label_id}"));

    let jpn_name = rows
        .iter()
        .find(|row| row.first().and_then(|value| value.as_i64()) == Some(num_id))
        .and_then(|row| row.get(1))
        .and_then(|value| value.as_str())
        .map(ToString::to_string);

    let name = jpn_name.clone()?;
    let mut names = BTreeMap::new();

    // Look up localized names from MasterTextLabel if ENT key is available
    if let Some(ref key) = ent_key {
        let lang_variants = [
            ("Chs", "chs"),
            ("Cht", "cht"),
            ("Eng", "eng"),
            ("Jpn", "jpn"),
            ("Kor", "kor"),
        ];
        for (variant, lang_code) in lang_variants {
            let mtl_path = master_root.join(variant).join("MasterTextLabel.json");
            if let Ok(mtl_text) = fs::read_to_string(&mtl_path)
                && let Ok(mtl_data) = serde_json::from_str::<Vec<Vec<serde_json::Value>>>(&mtl_text)
                && let Some(row) = mtl_data
                    .iter()
                    .find(|r| r.first().and_then(|v| v.as_str()) == Some(key.as_str()))
                && let Some(localized) = row.get(1).and_then(|v| v.as_str())
            {
                names.insert(lang_code.to_string(), localized.to_string());
            }
        }
    }

    // Ensure at least Japanese name is present
    if let Some(ref jpn) = jpn_name {
        names
            .entry("jpn".to_string())
            .or_insert_with(|| jpn.clone());
    }

    Some((name, names))
}

fn atlas_has_pma(path: &Path) -> anyhow::Result<bool> {
    let text =
        fs::read_to_string(path).with_context(|| format!("读取 atlas 失败: {}", path.display()))?;
    Ok(text.lines().any(|line| line.trim() == "pma:true"))
}

fn run_asset_studio(
    bundle: &Path,
    output_dir: &Path,
    asset_studio_path: &Path,
) -> anyhow::Result<()> {
    let status = Command::new(asset_studio_path)
        .arg(bundle)
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

fn find_exported_files(temp_dir: &Path, stem: &str) -> anyhow::Result<ExportedFiles> {
    let mut files = Vec::new();
    collect_files(temp_dir, &mut files);

    let skel = find_named_file(&files, stem, "skel")?;
    let atlas = find_named_file(&files, stem, "atlas")?;
    let png = find_named_file(&files, stem, "png")?;
    Ok(ExportedFiles { skel, atlas, png })
}

fn find_named_file(files: &[PathBuf], stem: &str, extension: &str) -> anyhow::Result<PathBuf> {
    let exact = format!("{stem}.{extension}");
    let prefixed = format!("spine_{stem}.{extension}");
    files
        .iter()
        .find(|path| {
            path.file_name()
                .and_then(|value| value.to_str())
                .is_some_and(|name| name == exact || name == prefixed)
        })
        .cloned()
        .with_context(|| format!("missing exported {extension}: {stem}"))
}

fn collect_files(dir: &Path, result: &mut Vec<PathBuf>) {
    if let Ok(entries) = fs::read_dir(dir) {
        for entry in entries.flatten() {
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

fn config_hash_matches(path: &Path, expected_hash: &str) -> bool {
    let Ok(text) = fs::read_to_string(path) else {
        return false;
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
        return false;
    };
    value.get("source_hash").and_then(|value| value.as_str()) == Some(expected_hash)
}

fn config_has_version(path: &Path) -> bool {
    let Ok(text) = fs::read_to_string(path) else {
        return false;
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
        return false;
    };
    value.get("config_version").and_then(|value| value.as_u64()) == Some(CONFIG_VERSION as u64)
}

/// Fast metadata-only update: read existing skel/atlas, rebuild config.json without AssetStudio.
fn update_metadata(
    data_dir: &Path,
    output_dir: &Path,
    stem: &str,
    source_hash: &str,
) -> anyhow::Result<()> {
    let skel_path = output_dir.join(format!("{stem}.skel"));
    let atlas_path = output_dir.join(format!("{stem}.atlas"));
    let (kind, id) = stem
        .split_once('_')
        .context("invalid LeaderSkin bundle stem")?;

    let skel_meta = extract_skel_metadata(&skel_path, kind)?;
    let num_id: i64 = id.parse()?;
    let (name, names) = load_leader_skin_name(data_dir, num_id).unwrap_or_else(|| {
        let mut map = BTreeMap::new();
        map.insert("jpn".to_string(), stem.to_string());
        (stem.to_string(), map)
    });

    let config = LeaderSkinConfig {
        config_version: CONFIG_VERSION,
        id: stem.to_string(),
        kind: kind.to_string(),
        num_id,
        source_hash: source_hash.to_string(),
        name,
        names,
        animations: skel_meta.animations,
        idle_animation: skel_meta.idle_animation,
        skins: skel_meta.skins,
        skin: skel_meta.skin,
        premultiplied_alpha: atlas_has_pma(&atlas_path)?,
        skel: format!("{stem}.skel"),
        atlas: format!("{stem}.atlas"),
        png: format!("{stem}.png"),
    };
    fs::write(
        output_dir.join("config.json"),
        serde_json::to_string_pretty(&config)? + "\n",
    )?;
    Ok(())
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
        .and_then(|path| path.file_name())
        .and_then(|name| name.to_str())
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
