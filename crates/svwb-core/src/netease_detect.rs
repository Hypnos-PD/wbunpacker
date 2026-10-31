use std::{
    collections::BTreeMap,
    env,
    fs::File,
    io::Read,
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Context, Result, bail};
use reqwest::blocking::Client;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::{
    GameBuild, ManifestSource, Operator, ProtocolConfig, ProtocolFamily, Provenance, RuntimeConfig,
    RuntimeProfile,
};

const NETEASE_BASE_URL: &str = "https://ma68sv2.game.163.com";
const NETEASE_SERVER_LIST_URL: &str = "https://ma68.update.netease.com/serverlist_wizard2";
const NETEASE_GAME_VERSION_URL: &str = "https://ma68.update.netease.com/pc_serverlist_game";
const NETEASE_MANIFEST_TEMPLATE: &str = "https://ma68.update.netease.com/plx/wizard2/obt/manifests/Windows/develop/assetbundle.{variant}.manifest";
const BODY_LIMIT: usize = 64 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct NeteaseDetectorOptions {
    pub game_root: Option<PathBuf>,
    pub variant: String,
    pub timeout: Duration,
}

#[derive(Debug, Clone)]
pub struct NeteaseDetection {
    pub profile: RuntimeProfile,
    pub raw: Value,
}

pub fn detect(options: &NeteaseDetectorOptions) -> Result<NeteaseDetection> {
    if options.variant.trim().is_empty() {
        bail!("variant is empty");
    }
    let game_root = resolve_game_root(options.game_root.as_deref())?;
    let gameassembly = find_gameassembly(&game_root)?;
    let gameassembly_sha256 = sha256_file(&gameassembly)?;
    let client = Client::builder()
        .timeout(options.timeout)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .context("create Netease HTTP client")?;
    let server_list = fetch(&client, NETEASE_SERVER_LIST_URL)?;
    let resource_version = parse_resource_version(&server_list)?;
    let game_version = fetch(&client, NETEASE_GAME_VERSION_URL)?;
    let app_version = parse_app_version(&game_version)?;
    let manifest_url = NETEASE_MANIFEST_TEMPLATE.replace("{variant}", &options.variant);
    let manifest = fetch(&client, &manifest_url)?;
    let manifest_sha256 = hex_digest(&manifest);
    let profile = profile_from_release(
        &options.variant,
        gameassembly_sha256,
        resource_version.clone(),
        app_version.clone(),
        manifest_url.clone(),
        manifest_sha256.clone(),
        manifest.len() as u64,
    )?;
    Ok(NeteaseDetection {
        raw: json!({
            "operator": "netease",
            "gameassembly": gameassembly,
            "app_version": app_version,
            "resource_version": resource_version,
            "manifest_variant": options.variant,
            "manifest_url": manifest_url,
            "manifest": {
                "sha256": manifest_sha256,
                "size": manifest.len(),
            },
            "profile_complete": true,
        }),
        profile,
    })
}

fn resolve_game_root(explicit: Option<&Path>) -> Result<PathBuf> {
    if let Some(path) = explicit {
        return Ok(path.to_path_buf());
    }
    if let Some(path) = env::var_os("SVWB_NETEASE_GAME_ROOT").map(PathBuf::from) {
        return Ok(path);
    }
    let mut candidates = Vec::new();
    if let Some(local_app_data) = env::var_os("LOCALAPPDATA") {
        candidates.push(PathBuf::from(local_app_data).join("Shadowverse"));
    }
    if let Some(prefix) = env::var_os("WINEPREFIX") {
        collect_wine_shadowverse_roots(Path::new(&prefix), &mut candidates);
    }
    if let Some(home) = env::var_os("HOME").map(PathBuf::from) {
        let games = home.join("Games");
        if let Ok(entries) = std::fs::read_dir(&games) {
            for entry in entries.flatten() {
                collect_wine_shadowverse_roots(&entry.path(), &mut candidates);
            }
        }
    }
    candidates = candidates
        .into_iter()
        .filter_map(|path| path.canonicalize().ok())
        .collect();
    candidates.sort();
    candidates.dedup();
    candidates.retain(|path| find_gameassembly(path).is_ok());
    match candidates.len() {
        0 => bail!(
            "Netease Shadowverse install not found; pass --game-root or set SVWB_NETEASE_GAME_ROOT"
        ),
        1 => Ok(candidates.remove(0)),
        _ => bail!(
            "multiple Netease Shadowverse installs found: {}",
            candidates
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

fn collect_wine_shadowverse_roots(prefix: &Path, candidates: &mut Vec<PathBuf>) {
    let users = prefix.join("drive_c/users");
    let Ok(entries) = std::fs::read_dir(users) else {
        return;
    };
    for entry in entries.flatten() {
        let candidate = entry.path().join("AppData/Local/Shadowverse");
        if candidate.is_dir() {
            candidates.push(candidate);
        }
    }
}

fn find_gameassembly(root: &Path) -> Result<PathBuf> {
    for name in ["GameAssembly.dll", "gameassembly.dll"] {
        let path = root.join(name);
        if path.is_file() {
            return Ok(path);
        }
    }
    bail!("GameAssembly.dll not found under {}", root.display())
}

fn profile_from_release(
    variant: &str,
    gameassembly_sha256: String,
    resource_version: String,
    app_version: String,
    manifest_url: String,
    manifest_sha256: String,
    manifest_size: u64,
) -> Result<RuntimeProfile> {
    let profile = RuntimeProfile {
        schema_version: 1,
        artifact_type: "runtime_profile".into(),
        operator: Operator::Netease,
        game_build: GameBuild {
            gameassembly_sha256: Some(gameassembly_sha256),
            exe_version: None,
            unity_version: None,
            metadata_sha256: None,
        },
        runtime: RuntimeConfig {
            base_url: NETEASE_BASE_URL.into(),
            resource_version,
            app_version,
        },
        protocol: ProtocolConfig {
            family: ProtocolFamily::NeteaseV1,
            codec_version: "1".into(),
            routing_header: None,
            common_header: None,
        },
        client_headers: BTreeMap::new(),
        manifest_source: Some(ManifestSource {
            variant: variant.into(),
            url: manifest_url,
            sha256: manifest_sha256,
            size: manifest_size,
        }),
        provenance: Provenance {
            producer: "svwb-toolkit".into(),
            producer_version: env!("CARGO_PKG_VERSION").into(),
            generated_at: time::OffsetDateTime::now_utc()
                .format(&time::format_description::well_known::Rfc3339)
                .context("format profile generation time")?,
        },
    };
    profile.require_valid()?;
    Ok(profile)
}

fn fetch(client: &Client, url: &str) -> Result<Vec<u8>> {
    let response = client
        .get(url)
        .header("User-Agent", "svwb-toolkit/1")
        .header("Cache-Control", "no-cache")
        .send()
        .with_context(|| format!("GET {url}"))?
        .error_for_status()
        .with_context(|| format!("GET {url} status"))?;
    let mut body = Vec::with_capacity(BODY_LIMIT.min(8 * 1024 * 1024));
    response
        .take(BODY_LIMIT as u64 + 1)
        .read_to_end(&mut body)
        .with_context(|| format!("read {url}"))?;
    if body.len() > BODY_LIMIT {
        bail!("GET {url} response exceeds {BODY_LIMIT} bytes");
    }
    Ok(body)
}

fn parse_resource_version(body: &[u8]) -> Result<String> {
    let body = sanitize_json_string_controls(body);
    let entries: Vec<Value> =
        serde_json::from_slice(&body).context("decode Netease server list")?;
    for entry in entries {
        if scalar(entry.get("server")) != "正式服"
            || scalar(entry.get("platform")) != "all"
            || scalar(entry.get("channel")) != "all"
            || scalar(entry.get("isDebug")) != "0"
        {
            continue;
        }
        let resource_version = scalar(entry.get("resVer"));
        if valid_resource_version(&resource_version) {
            return Ok(resource_version);
        }
        bail!("Netease server list has an invalid formal resource version");
    }
    bail!("Netease server list has no formal production entry")
}

fn parse_app_version(body: &[u8]) -> Result<String> {
    let value: Value = serde_json::from_slice(body).context("decode Netease game version")?;
    let app_version = value
        .get("version")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    if !valid_app_version(&app_version) {
        bail!("Netease game version response has an invalid version");
    }
    Ok(app_version)
}

fn scalar(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(value)) => value.clone(),
        Some(Value::Number(value)) => value.to_string(),
        _ => String::new(),
    }
}

fn valid_resource_version(value: &str) -> bool {
    !value.is_empty()
        && value.bytes().all(|byte| byte.is_ascii_digit())
        && value.bytes().any(|byte| byte != b'0')
}

fn valid_app_version(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 16
        && value.split('.').count() <= 4
        && value
            .split('.')
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()))
}

fn sanitize_json_string_controls(body: &[u8]) -> Vec<u8> {
    let mut result = Vec::with_capacity(body.len());
    let mut in_string = false;
    let mut escaped = false;
    for &value in body {
        if in_string {
            if escaped {
                result.push(value);
                escaped = false;
                continue;
            }
            match value {
                b'\\' => {
                    result.push(value);
                    escaped = true;
                }
                b'"' => {
                    result.push(value);
                    in_string = false;
                }
                value if value < 0x20 => result.push(b' '),
                _ => result.push(value),
            }
            continue;
        }
        result.push(value);
        if value == b'"' {
            in_string = true;
        }
    }
    result
}

fn sha256_file(path: &Path) -> Result<String> {
    let mut file =
        File::open(path).with_context(|| format!("open GameAssembly.dll: {}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 1024 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .with_context(|| format!("read GameAssembly.dll: {}", path.display()))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn hex_digest(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut output = String::with_capacity(64);
    for byte in digest {
        use std::fmt::Write as _;
        write!(&mut output, "{byte:02x}").expect("write digest");
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_formal_server_entries_with_control_characters() {
        let body = r#"[
            {"server":"测试服","platform":"all","channel":"all","isDebug":"0","resVer":"10000001"},
            {"server":"正式服","platform":"all","channel":"all","isDebug":"0","resVer":"10000200","notice":"line
line"}
        ]"#;
        assert_eq!(parse_resource_version(body.as_bytes()).unwrap(), "10000200");
    }

    #[test]
    fn builds_complete_netease_profile() {
        let profile = profile_from_release(
            "Chs",
            "a".repeat(64),
            "10000200".into(),
            "11.10.0".into(),
            NETEASE_MANIFEST_TEMPLATE.replace("{variant}", "Chs"),
            "b".repeat(64),
            1234,
        )
        .unwrap();
        assert_eq!(profile.operator, Operator::Netease);
        assert_eq!(profile.runtime.base_url, NETEASE_BASE_URL);
        assert!(profile.client_headers.is_empty());
        assert_eq!(profile.manifest_source.unwrap().variant, "Chs");
    }

    #[test]
    fn rejects_zero_resource_version() {
        let body = r#"[{"server":"正式服","platform":"all","channel":"all","isDebug":"0","resVer":"00000000"}]"#;
        assert!(parse_resource_version(body.as_bytes()).is_err());
    }
}
