use std::{
    cmp::Reverse,
    collections::{BTreeMap, HashMap},
    fs::{self, File},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result, bail};
use regex::bytes::Regex;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::{
    GameBuild, ManifestSource, Operator, ProtocolConfig, ProtocolFamily, Provenance, RuntimeConfig,
    RuntimeProfile,
};

const SKIP_NAME_HINTS: &[&str] = &["nvidia", "cuda", "dri", "/dev/dri", "gpu"];
const METADATA_SANITY: u32 = 0xFAB1_1BAF;
const DEFAULT_METADATA_VERSION: u32 = 31;
const MANIFEST_TEMPLATE: &str = "https://shadowverse-wb.akamaized.net/dl/manifests/Windows/{version}/assetbundle.{variant}.manifest";
const COMMON_PREFIX: [u8; 32] = [
    0x5E, 0xBB, 0x1B, 0x7E, 0x66, 0xF3, 0x77, 0x53, 0x85, 0x5E, 0xAE, 0xE1, 0xD9, 0xF9, 0xEF, 0xFE,
    0xE7, 0x40, 0x59, 0x1A, 0x30, 0x6A, 0x1B, 0xBC, 0x03, 0x4A, 0x95, 0x69, 0xA7, 0x78, 0xA5, 0x62,
];

#[derive(Debug, Clone)]
pub struct NativeDetectorOptions {
    pub game_root: PathBuf,
    pub data_root: PathBuf,
    pub process_name: String,
    pub pid: Option<u32>,
    pub memory_dump: Option<PathBuf>,
    pub variant: String,
    pub min_region_bytes: u64,
    pub max_region_bytes: u64,
    pub dotnet: PathBuf,
    pub il2cpp_dumper_dir: PathBuf,
    pub refresh_dump: bool,
}

#[derive(Debug, Clone)]
pub struct NativeDetection {
    pub draft: RuntimeProfile,
    pub profile: Option<RuntimeProfile>,
    pub raw: Value,
    pub dump_dir: PathBuf,
    pub work_dir: PathBuf,
    pub variant: String,
    pub manifest_template: String,
}

pub fn complete_native_detection(
    detection: &NativeDetection,
    resource_version: &str,
) -> Result<RuntimeProfile> {
    if resource_version.is_empty()
        || !resource_version.bytes().all(|byte| byte.is_ascii_digit())
        || resource_version.bytes().all(|byte| byte == b'0')
    {
        bail!("resource_version is invalid")
    }
    let manifest_url = build_manifest_url(
        &detection.manifest_template,
        resource_version,
        &detection.variant,
    )?;
    let manifest_path = detection
        .work_dir
        .join(format!("assetbundle.{}.manifest", detection.variant));
    let manifest_info = download_manifest(&manifest_url, &manifest_path)?;
    let mut profile = detection.draft.clone();
    profile.runtime.resource_version = resource_version.into();
    profile.manifest_source = Some(ManifestSource {
        variant: detection.variant.clone(),
        url: manifest_url,
        sha256: manifest_info.sha256,
        size: manifest_info.size,
    });
    profile.require_valid()?;
    Ok(profile)
}

fn build_manifest_url(template: &str, resource_version: &str, variant: &str) -> Result<String> {
    if !template.contains("{version}") {
        bail!("manifest template is missing {{version}}")
    }
    let mut url = template.replace("{version}", resource_version);
    if variant.eq_ignore_ascii_case("Jpn") {
        url = url.replace(".{variant}.manifest", ".manifest");
    } else if url.contains("{variant}") {
        url = url.replace("{variant}", variant);
    } else {
        bail!("manifest template is missing {variant}")
    }
    if url.contains('{') || url.contains('}') {
        bail!("manifest template contains unresolved placeholders")
    }
    Ok(url)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GameProcess {
    pub pid: u32,
    pub rss_kib: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryRegion {
    pub start: u64,
    pub end: u64,
    pub permissions: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetadataMatch {
    pub offset: usize,
    pub header_size: u32,
    pub ranges: Vec<(u32, u32)>,
    pub total_size: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ManifestObservation {
    resource_version: String,
    variant: String,
    url: String,
    occurrences: usize,
}

pub fn detect_with_native(options: &NativeDetectorOptions) -> Result<NativeDetection> {
    detect_native_platform(options)
}

#[cfg(not(target_os = "linux"))]
fn detect_native_platform(_options: &NativeDetectorOptions) -> Result<NativeDetection> {
    bail!("native memory detection is currently supported on Linux only")
}

#[cfg(target_os = "linux")]
fn detect_native_platform(options: &NativeDetectorOptions) -> Result<NativeDetection> {
    let gameassembly = options.game_root.join("GameAssembly.dll");
    if !gameassembly.is_file() {
        bail!("GameAssembly.dll not found: {}", gameassembly.display());
    }
    let gameassembly_sha256 = sha256_file(&gameassembly)?;
    let process = if options.memory_dump.is_some() {
        None
    } else {
        Some(match options.pid {
            Some(pid) => GameProcess {
                pid,
                rss_kib: process_memory_size(pid).unwrap_or(0),
            },
            None => find_game_process(&options.process_name)?,
        })
    };
    let dump_dir = if let Some(memory_dump) = &options.memory_dump {
        memory_dump.clone()
    } else {
        let process = process
            .as_ref()
            .expect("process is present without memory dump");
        let dump_root = options.data_root.join("runtime-cache").join("native");
        let dump_dir = dump_root.join(format!("{}-{}", process.pid, &gameassembly_sha256[..12]));
        if options.refresh_dump && dump_dir.exists() {
            fs::remove_dir_all(&dump_dir)
                .with_context(|| format!("remove old dump: {}", dump_dir.display()))?;
        }
        if !(dump_dir.join("regions.txt").is_file() && has_region_files(&dump_dir)) {
            fs::create_dir_all(&dump_dir)?;
            dump_process_memory(
                process.pid,
                &dump_dir,
                options.min_region_bytes,
                options.max_region_bytes,
            )?;
        }
        dump_dir
    };

    let (app_version, app_candidates) = discover_app_version(&dump_dir)?;
    let observations = discover_manifest_records(&dump_dir).unwrap_or_default();
    let manifest = select_manifest_optional(&observations, &options.variant)?;
    let work_key = manifest
        .as_ref()
        .map(|item| item.resource_version.as_str())
        .unwrap_or_else(|| &gameassembly_sha256[..12]);
    let work = options
        .data_root
        .join("runtime-cache")
        .join("native-work")
        .join(work_key);
    fs::create_dir_all(&work)?;
    let metadata_path = work.join("global-metadata.dat");
    let dumpout = work.join("dumpout");
    let stringliteral = dumpout.join("stringliteral.json");
    let reuse_work = !options.refresh_dump && metadata_path.is_file() && stringliteral.is_file();
    let metadata_sha256 = if reuse_work {
        sha256_file(&metadata_path)?
    } else {
        extract_metadata(&dump_dir, &metadata_path, DEFAULT_METADATA_VERSION)?.sha256
    };
    let (common_header, common_offset) = find_common_header(&metadata_path)?;

    if !reuse_work {
        run_il2cpp_dumper(
            &options.dotnet,
            &options.il2cpp_dumper_dir,
            &gameassembly,
            &metadata_path,
            &dumpout,
        )?;
    }
    let routing_candidates = routing_candidates(&stringliteral)?;
    let routing_header = choose_routing_header(&routing_candidates, &dump_dir)?;
    let generated_at = time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .context("format detection timestamp")?;

    let draft = RuntimeProfile {
        schema_version: 1,
        artifact_type: "runtime_profile".into(),
        operator: Operator::Cygames,
        game_build: GameBuild {
            gameassembly_sha256: Some(gameassembly_sha256.clone()),
            exe_version: None,
            unity_version: None,
            metadata_sha256: Some(metadata_sha256.clone()),
        },
        runtime: RuntimeConfig {
            base_url: "https://web11.shadowverse-wb.jp".into(),
            resource_version: manifest
                .as_ref()
                .map(|item| item.resource_version.clone())
                .unwrap_or_default(),
            app_version: app_version.clone(),
        },
        protocol: ProtocolConfig {
            family: ProtocolFamily::CygamesV1,
            codec_version: "1".into(),
            routing_header: Some(routing_header.clone()),
            common_header: Some(common_header.clone()),
        },
        client_headers: BTreeMap::new(),
        manifest_source: None,
        provenance: Provenance {
            producer: "svwb-toolkit-native".into(),
            producer_version: env!("CARGO_PKG_VERSION").into(),
            generated_at,
        },
    };
    let manifest_info = if let Some(manifest) = &manifest {
        let manifest_path = work.join(format!("assetbundle.{}.manifest", manifest.variant));
        Some(download_manifest(&manifest.url, &manifest_path)?)
    } else {
        None
    };
    let profile = manifest.as_ref().map(|manifest| {
        let mut profile = draft.clone();
        profile.manifest_source = Some(ManifestSource {
            variant: manifest.variant.clone(),
            url: manifest.url.clone(),
            sha256: manifest_info
                .as_ref()
                .expect("manifest info is present")
                .sha256
                .clone(),
            size: manifest_info
                .as_ref()
                .expect("manifest info is present")
                .size,
        });
        profile
    });
    if let Some(profile) = &profile {
        profile.require_valid()?;
    }
    let raw = json!({
        "schema_version": 1,
        "profile_complete": profile.is_some(),
        "resource_version": draft.runtime.resource_version,
        "app_version": draft.runtime.app_version,
        "routing_header": routing_header,
        "common_header": common_header,
        "manifest_variant": manifest.as_ref().map(|item| item.variant.clone()).unwrap_or(options.variant.clone()),
        "manifest_url": manifest.as_ref().map(|item| item.url.clone()),
        "manifest": manifest_info.as_ref().map(|item| item.raw.clone()),
        "app_version_candidates": app_candidates,
        "routing_header_candidates": routing_candidates,
        "metadata_sha256": metadata_sha256,
        "common_header_offset": format!("0x{common_offset:X}"),
        "gameassembly_sha256": gameassembly_sha256,
        "process": process.map(|process| json!({"pid": process.pid, "rss_kib": process.rss_kib})),
        "dump_dir": dump_dir,
    });
    Ok(NativeDetection {
        draft,
        profile,
        raw,
        dump_dir,
        work_dir: work,
        variant: options.variant.clone(),
        manifest_template: MANIFEST_TEMPLATE.into(),
    })
}

pub fn find_game_process(name: &str) -> Result<GameProcess> {
    find_game_process_platform(name)
}

#[cfg(not(target_os = "linux"))]
fn find_game_process_platform(_name: &str) -> Result<GameProcess> {
    bail!("process discovery is currently supported on Linux only")
}

#[cfg(target_os = "linux")]
fn find_game_process_platform(name: &str) -> Result<GameProcess> {
    let mut candidates = Vec::new();
    for entry in fs::read_dir("/proc")? {
        let entry = entry?;
        let Some(pid) = entry
            .file_name()
            .to_str()
            .and_then(|v| v.parse::<u32>().ok())
        else {
            continue;
        };
        let Ok(raw) = fs::read(format!("/proc/{pid}/cmdline")) else {
            continue;
        };
        let mut parts: Vec<&[u8]> = raw.split(|byte| *byte == 0).collect();
        while parts.last().is_some_and(|part| part.is_empty()) {
            parts.pop();
        }
        let Some(tail) = parts.last() else {
            continue;
        };
        let command = String::from_utf8_lossy(tail).replace('\\', "/");
        let file_name = Path::new(&command)
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("");
        if !file_name.eq_ignore_ascii_case(name) {
            continue;
        }
        candidates.push(GameProcess {
            pid,
            rss_kib: process_memory_size(pid).unwrap_or(0),
        });
    }
    candidates.sort_by_key(|process| Reverse(process.rss_kib));
    candidates.into_iter().next().with_context(|| {
        format!("process {name} is not running; launch the game to the title screen")
    })
}

pub fn process_memory_size(pid: u32) -> Result<u64> {
    let status = fs::read_to_string(format!("/proc/{pid}/status"))?;
    for line in status.lines() {
        if let Some(value) = line.strip_prefix("VmRSS:") {
            return value
                .split_whitespace()
                .next()
                .context("VmRSS has no value")?
                .parse()
                .context("parse VmRSS");
        }
    }
    Ok(0)
}

pub fn parse_maps(text: &str) -> Vec<MemoryRegion> {
    text.lines()
        .filter_map(|line| {
            let mut fields = line.split_whitespace();
            let bounds = fields.next()?;
            let permissions = fields.next()?.to_string();
            let (start, end) = bounds.split_once('-')?;
            let start = u64::from_str_radix(start, 16).ok()?;
            let end = u64::from_str_radix(end, 16).ok()?;
            let fields: Vec<&str> = fields.collect();
            let name = if fields.len() > 3 {
                fields[3..].join(" ")
            } else {
                String::new()
            };
            Some(MemoryRegion {
                start,
                end,
                permissions,
                name,
            })
        })
        .collect()
}

#[cfg(target_os = "linux")]
fn dump_process_memory(pid: u32, output: &Path, minimum: u64, maximum: u64) -> Result<()> {
    let maps = fs::read_to_string(format!("/proc/{pid}/maps"))?;
    let mut memory = File::open(format!("/proc/{pid}/mem"))?;
    let mut table = File::create(output.join("regions.txt"))?;
    for (index, region) in parse_maps(&maps).into_iter().enumerate() {
        let size = region.end.saturating_sub(region.start);
        if !region.permissions.starts_with('r')
            || size < minimum
            || size > maximum
            || SKIP_NAME_HINTS
                .iter()
                .any(|hint| region.name.to_ascii_lowercase().contains(hint))
        {
            continue;
        }
        let mut data = vec![0_u8; size as usize];
        memory.seek(SeekFrom::Start(region.start))?;
        if memory.read_exact(&mut data).is_err() {
            continue;
        }
        let file_name = format!("{index:06}_{:X}.bin", region.start);
        fs::write(output.join(&file_name), data)?;
        writeln!(
            table,
            "{file_name} 0x{:X} 0x{:X} {} {}",
            region.start, size, region.permissions, region.name
        )?;
    }
    Ok(())
}

pub fn scan_metadata(data: &[u8], needles: &[&[u8]]) -> Option<MetadataMatch> {
    for offset in (0..data.len().saturating_sub(0x1000)).step_by(8) {
        if offset + 0x1000 > data.len() {
            break;
        }
        let header_size = u32::from_le_bytes(data[offset + 8..offset + 12].try_into().ok()?);
        if !(0x20..=0x2000).contains(&header_size) || header_size % 8 != 0 {
            continue;
        }
        let count = (header_size - 8) / 8;
        if !(8..=60).contains(&count) {
            continue;
        }
        let mut ranges = Vec::new();
        let mut previous_end = 0_u32;
        let mut first = true;
        let mut valid = true;
        for index in 0..count as usize {
            let position = offset + 8 + index * 8;
            let start = u32::from_le_bytes(data[position..position + 4].try_into().ok()?);
            let size = u32::from_le_bytes(data[position + 4..position + 8].try_into().ok()?);
            if start > 0x4000_0000 || size > 0x4000_0000 {
                valid = false;
                break;
            }
            if size != 0 {
                if first {
                    if start != header_size {
                        valid = false;
                        break;
                    }
                    first = false;
                } else if start != previous_end {
                    valid = false;
                    break;
                }
                previous_end = start.saturating_add(size);
            }
            ranges.push((start, size));
        }
        if !valid || first || !(0x10_0000..=0x4000_0000).contains(&previous_end) {
            continue;
        }
        let contains = ranges.iter().any(|(start, size)| {
            let start = *start as usize;
            let end = start.saturating_add(*size as usize);
            end <= data.len()
                && needles
                    .iter()
                    .all(|needle| contains_bytes(&data[start..end], needle))
        });
        if contains {
            return Some(MetadataMatch {
                offset,
                header_size,
                ranges,
                total_size: previous_end,
            });
        }
    }
    None
}

pub fn extract_metadata(dump: &Path, output: &Path, version: u32) -> Result<MetadataInfo> {
    for path in sorted_region_files(dump)? {
        let data = fs::read(&path)?;
        if !contains_bytes(&data, b"Wizard2.ServerShared")
            || !contains_bytes(&data, b"MessagePackObjects")
        {
            continue;
        }
        let Some(found) = scan_metadata(&data, &[b"Wizard2.ServerShared", b"MessagePackObjects"])
        else {
            continue;
        };
        let end = found.offset + found.total_size as usize;
        if end > data.len() {
            bail!("metadata crosses memory region boundary");
        }
        let mut metadata = data[found.offset..end].to_vec();
        metadata[..4].copy_from_slice(&METADATA_SANITY.to_le_bytes());
        metadata[4..8].copy_from_slice(&version.to_le_bytes());
        fs::create_dir_all(output.parent().unwrap_or(Path::new(".")))?;
        fs::write(output, &metadata)?;
        return Ok(MetadataInfo {
            sha256: sha256_bytes(&metadata),
            size: metadata.len(),
            source_file: path,
            offset: found.offset,
            regions: found.ranges.len(),
        });
    }
    bail!("IL2CPP metadata was not found in the memory dump")
}

#[derive(Debug, Clone)]
pub struct MetadataInfo {
    pub sha256: String,
    pub size: usize,
    pub source_file: PathBuf,
    pub offset: usize,
    pub regions: usize,
}

pub fn find_common_header(metadata: &Path) -> Result<(String, usize)> {
    let data = fs::read(metadata)?;
    let Some(offset) = find_subslice(&data, &COMMON_PREFIX) else {
        bail!("CommonHeader prefix was not found")
    };
    if find_subslice(&data[offset + 1..], &COMMON_PREFIX).is_some() {
        bail!("CommonHeader prefix occurs more than once")
    }
    if offset + 52 > data.len() {
        bail!("CommonHeader extends past metadata")
    }
    Ok((hex_upper(&data[offset..offset + 52]), offset))
}

pub fn discover_manifests(dump: &Path) -> Result<Vec<(String, String, String, usize)>> {
    let items = discover_manifest_records(dump)?;
    if items.is_empty() {
        bail!("running client has no manifest URL")
    }
    Ok(items
        .into_iter()
        .map(|item| {
            (
                item.resource_version,
                item.variant,
                item.url,
                item.occurrences,
            )
        })
        .collect())
}

fn discover_manifest_records(dump: &Path) -> Result<Vec<ManifestObservation>> {
    let pattern = Regex::new(
        r"https://shadowverse-wb\.akamaized\.net/dl/manifests/Windows/(\d+)/assetbundle(?:\.([A-Za-z]+))?\.manifest",
    )?;
    let mut counts: HashMap<(String, String, String), usize> = HashMap::new();
    for path in sorted_region_files(dump)? {
        let data = fs::read(path)?;
        for capture in pattern.captures_iter(&data) {
            let version = String::from_utf8_lossy(&capture[1]).to_string();
            let variant = capture
                .get(2)
                .map(|value| String::from_utf8_lossy(value.as_bytes()).to_string())
                .unwrap_or_else(|| "Jpn".into());
            let url = String::from_utf8_lossy(&capture[0]).to_string();
            *counts.entry((version, variant, url)).or_default() += 1;
        }
    }
    let mut items: Vec<_> = counts
        .into_iter()
        .map(
            |((resource_version, variant, url), occurrences)| ManifestObservation {
                resource_version,
                variant,
                url,
                occurrences,
            },
        )
        .collect();
    items.sort_by_key(|item| Reverse(item.occurrences));
    Ok(items)
}

fn select_manifest_optional(
    observations: &[ManifestObservation],
    variant: &str,
) -> Result<Option<ManifestObservation>> {
    if observations.is_empty() {
        return Ok(None);
    }
    select_manifest(observations, variant).map(Some)
}

fn select_manifest(
    observations: &[ManifestObservation],
    variant: &str,
) -> Result<ManifestObservation> {
    let matching: Vec<_> = observations
        .iter()
        .filter(|item| item.variant.eq_ignore_ascii_case(variant))
        .cloned()
        .collect();
    let pool = if matching.is_empty() {
        observations.to_vec()
    } else {
        matching
    };
    let versions: std::collections::BTreeSet<_> = pool
        .iter()
        .map(|item| item.resource_version.as_str())
        .collect();
    if versions.len() != 1 {
        bail!("multiple resource versions found")
    }
    Ok(pool[0].clone())
}

fn discover_app_version(dump: &Path) -> Result<(String, Vec<Value>)> {
    let key = b"app-ver";
    let pattern = Regex::new(r"([0-9]+\.[0-9]+(?:\.[0-9]+){0,2})")?;
    let mut counts: HashMap<String, usize> = HashMap::new();
    for path in sorted_region_files(dump)? {
        let data = fs::read(path)?;
        let mut start = 0;
        while let Some(offset) = find_subslice(&data[start..], key) {
            let position = start + offset + key.len();
            let end = (position + 0x100).min(data.len());
            let window = &data[position..end];
            for capture in pattern.captures_iter(window) {
                let Some(whole) = capture.get(0) else {
                    continue;
                };
                if whole.start() > 0 && window[whole.start() - 1].is_ascii_digit() {
                    continue;
                }
                if whole.end() < window.len() && window[whole.end()].is_ascii_digit() {
                    continue;
                }
                let version = String::from_utf8_lossy(&capture[1]).to_string();
                *counts.entry(version).or_default() += 1;
            }
            start = position;
        }
    }
    let mut ranked: Vec<_> = counts.into_iter().collect();
    ranked.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
    let Some((version, _)) = ranked.first() else {
        bail!("App-Ver was not found next to the app-ver request header")
    };
    Ok((
        version.clone(),
        ranked
            .into_iter()
            .map(|(value, occurrences)| json!({"value": value, "occurrences": occurrences}))
            .collect(),
    ))
}

fn routing_candidates(path: &Path) -> Result<Vec<String>> {
    let values: Vec<Value> = serde_json::from_slice(&fs::read(path)?)?;
    let pattern = Regex::new(r"^[A-Za-z0-9]{40,64}$")?;
    let mut candidates = std::collections::BTreeSet::new();
    for value in values {
        let Some(text) = value.get("value").and_then(Value::as_str) else {
            continue;
        };
        if pattern.is_match(text.as_bytes())
            && text.bytes().any(|byte| byte.is_ascii_lowercase())
            && text.bytes().any(|byte| byte.is_ascii_uppercase())
            && text.bytes().any(|byte| byte.is_ascii_digit())
            && shannon_entropy(text) > 4.4
            && !text.contains("ABCDEFGHIJKLMNOPQRSTUVWXYZ")
            && !text.contains("abcdefghijklmnopqrstuvwxyz")
            && !text.contains("0123456789")
        {
            candidates.insert(text.to_string());
        }
    }
    Ok(candidates.into_iter().collect())
}

fn choose_routing_header(candidates: &[String], dump: &Path) -> Result<String> {
    if candidates.len() == 1 {
        return Ok(candidates[0].clone());
    }
    let mut present = Vec::new();
    let mut remaining: std::collections::HashSet<&str> =
        candidates.iter().map(String::as_str).collect();
    for path in sorted_region_files(dump)? {
        let data = fs::read(path)?;
        for candidate in candidates {
            if remaining.contains(candidate.as_str())
                && find_subslice(&data, candidate.as_bytes()).is_some()
            {
                present.push(candidate.clone());
                remaining.remove(candidate.as_str());
            }
        }
        if remaining.is_empty() {
            break;
        }
    }
    if present.len() == 1 {
        return Ok(present.remove(0));
    }
    bail!("Routing-Header is missing or ambiguous")
}

fn run_il2cpp_dumper(
    dotnet: &Path,
    tool_dir: &Path,
    gameassembly: &Path,
    metadata: &Path,
    output: &Path,
) -> Result<()> {
    let tool = tool_dir.join("Il2CppDumper.dll");
    if !tool.is_file() {
        bail!("Il2CppDumper.dll not found: {}", tool.display());
    }
    fs::create_dir_all(output)?;
    let status = Command::new(dotnet)
        .env("DOTNET_ROLL_FORWARD", "LatestMajor")
        .arg(&tool)
        .arg(gameassembly)
        .arg(metadata)
        .arg(output)
        .status()
        .context("run Il2CppDumper")?;
    if !output.join("stringliteral.json").is_file() {
        bail!(
            "Il2CppDumper exited with {} and did not produce stringliteral.json",
            status
        );
    }
    Ok(())
}

fn download_manifest(url: &str, output: &Path) -> Result<ManifestInfo> {
    let response = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .build()?
        .get(url)
        .header(reqwest::header::USER_AGENT, "svwb-toolkit/1")
        .send()?
        .error_for_status()?;
    let headers = response.headers().clone();
    let bytes = response.bytes()?;
    if bytes.len() > 128 << 20 {
        bail!("manifest exceeds 128 MiB")
    }
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(output, &bytes)?;
    Ok(ManifestInfo {
        sha256: sha256_bytes(&bytes),
        size: bytes.len() as u64,
        raw: json!({
            "http_status": 200,
            "content_type": headers.get(reqwest::header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).unwrap_or(""),
            "last_modified": headers.get(reqwest::header::LAST_MODIFIED).and_then(|v| v.to_str().ok()).unwrap_or(""),
            "etag": headers.get(reqwest::header::ETAG).and_then(|v| v.to_str().ok()).unwrap_or("").trim_matches('"'),
            "size": bytes.len(),
            "sha256": sha256_bytes(&bytes),
            "path": output,
        }),
    })
}

#[derive(Debug, Clone)]
struct ManifestInfo {
    sha256: String,
    size: u64,
    raw: Value,
}

fn sorted_region_files(dump: &Path) -> Result<Vec<PathBuf>> {
    let mut files: Vec<_> = fs::read_dir(dump)?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| {
                    name.ends_with(".bin")
                        && name
                            .bytes()
                            .next()
                            .is_some_and(|byte| byte.is_ascii_digit())
                })
        })
        .collect();
    files.sort();
    if files.is_empty() {
        bail!("dump directory has no region files")
    }
    Ok(files)
}

fn has_region_files(dump: &Path) -> bool {
    sorted_region_files(dump).is_ok()
}

fn sha256_file(path: &Path) -> Result<String> {
    let mut file = File::open(path)?;
    let mut hash = Sha256::new();
    let mut buffer = [0_u8; 1024 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    Ok(hex_bytes(&hash.finalize()))
}

fn sha256_bytes(data: &[u8]) -> String {
    hex_bytes(&Sha256::digest(data))
}

fn find_subslice(data: &[u8], needle: &[u8]) -> Option<usize> {
    memchr::memmem::find(data, needle)
}

fn contains_bytes(data: &[u8], needle: &[u8]) -> bool {
    find_subslice(data, needle).is_some()
}

fn shannon_entropy(value: &str) -> f64 {
    let mut counts = [0_usize; 256];
    for byte in value.bytes() {
        counts[byte as usize] += 1;
    }
    let size = value.len() as f64;
    counts
        .into_iter()
        .filter(|count| *count > 0)
        .map(|count| {
            let probability = count as f64 / size;
            -probability * probability.log2()
        })
        .sum()
}

fn hex_bytes(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

fn hex_upper(bytes: &[u8]) -> String {
    hex_bytes(bytes).to_ascii_uppercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_proc_maps() {
        let maps =
            "1000-2000 r-xp 00000000 00:00 0 /tmp/a\n3000-4000 rw-p 00000000 00:00 0 [heap]\n";
        let parsed = parse_maps(maps);
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].start, 0x1000);
        assert_eq!(parsed[0].permissions, "r-xp");
        assert_eq!(parsed[1].name, "[heap]");
    }

    #[test]
    fn extracts_synthetic_metadata() {
        let needles = [b"Wizard2.ServerShared".as_slice(), b"MessagePackObjects"];
        let header_size = 0x48_u32;
        let data_size = 0x10_0000_u32;
        let mut data = vec![0_u8; header_size as usize + data_size as usize];
        let first = b"Wizard2.ServerShared";
        let second = b"MessagePackObjects";
        let first_start = header_size as usize;
        data[first_start..first_start + first.len()].copy_from_slice(first);
        let second_start = first_start + first.len() + 5;
        data[second_start..second_start + second.len()].copy_from_slice(second);
        let count = (header_size - 8) / 8;
        for index in 0..count as usize {
            let (start, size) = if index == 0 {
                (header_size, data_size)
            } else {
                (header_size + data_size, 0)
            };
            let position = 8 + index * 8;
            data[position..position + 4].copy_from_slice(&start.to_le_bytes());
            data[position + 4..position + 8].copy_from_slice(&size.to_le_bytes());
        }
        let found = scan_metadata(&data, &needles).expect("metadata");
        assert_eq!(found.offset, 0);
        assert_eq!(found.total_size, header_size + data_size);
    }

    #[test]
    fn finds_unique_common_header() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("metadata.dat");
        let mut data = vec![0_u8; 128];
        data[8..40].copy_from_slice(&COMMON_PREFIX);
        data[40..60].fill(0xAB);
        fs::write(&path, data).unwrap();
        let (header, offset) = find_common_header(&path).unwrap();
        assert_eq!(offset, 8);
        assert_eq!(header.len(), 104);
    }

    #[test]
    fn finds_complete_app_version_after_header_name() {
        let directory = tempfile::tempdir().unwrap();
        let mut data = vec![0_u8; 512];
        let key = b"app-ver";
        data[16..16 + key.len()].copy_from_slice(key);
        data[80..86].copy_from_slice(b"1.10.0");
        fs::write(directory.path().join("000000_0.bin"), data).unwrap();
        let (version, _) = discover_app_version(directory.path()).unwrap();
        assert_eq!(version, "1.10.0");
    }

    #[test]
    fn builds_manifest_urls_from_known_template() {
        assert_eq!(
            build_manifest_url(MANIFEST_TEMPLATE, "318861727954", "Cht").unwrap(),
            "https://shadowverse-wb.akamaized.net/dl/manifests/Windows/318861727954/assetbundle.Cht.manifest"
        );
        assert_eq!(
            build_manifest_url(MANIFEST_TEMPLATE, "318861727954", "Jpn").unwrap(),
            "https://shadowverse-wb.akamaized.net/dl/manifests/Windows/318861727954/assetbundle.manifest"
        );
    }
}
