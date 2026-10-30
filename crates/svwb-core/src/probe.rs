use std::{collections::BTreeMap, fs, path::Path, time::Duration};

use anyhow::{Context, Result, bail};
use reqwest::blocking::Client;
use serde_json::{Value, json};
use sha2::Digest;
use url::Url;

use crate::{
    GameBuild, ManifestSource, Operator, ProtocolConfig, ProtocolFamily, Provenance, RuntimeConfig,
    RuntimeProbe, RuntimeProbeSubjectType, RuntimeProfile,
};

pub fn runtime_profile_from_dumper(value: &Value) -> Result<RuntimeProfile> {
    let string = |name: &str| -> Result<String> {
        value
            .get(name)
            .and_then(Value::as_str)
            .map(str::to_string)
            .with_context(|| format!("dumper field {name} is missing"))
    };
    let manifest = value
        .get("manifest")
        .and_then(Value::as_object)
        .context("dumper manifest metadata is missing")?;
    let manifest_size = manifest
        .get("size")
        .and_then(Value::as_u64)
        .context("dumper manifest size is missing")?;
    let manifest_sha256 = manifest
        .get("sha256")
        .and_then(Value::as_str)
        .context("dumper manifest SHA-256 is missing")?
        .to_string();
    let profile = RuntimeProfile {
        schema_version: 1,
        artifact_type: "runtime_profile".into(),
        operator: Operator::Cygames,
        game_build: GameBuild {
            gameassembly_sha256: Some(string("gameassembly_sha256")?),
            exe_version: None,
            unity_version: None,
            metadata_sha256: Some(string("metadata_sha256")?),
        },
        runtime: RuntimeConfig {
            base_url: "https://web11.shadowverse-wb.jp".into(),
            resource_version: string("resource_version")?,
            app_version: string("app_version")?,
        },
        protocol: ProtocolConfig {
            family: ProtocolFamily::CygamesV1,
            codec_version: "1".into(),
            routing_header: Some(string("routing_header")?),
            common_header: Some(string("common_header")?),
        },
        client_headers: BTreeMap::new(),
        manifest_source: Some(ManifestSource {
            variant: string("manifest_variant")?,
            url: string("manifest_url")?,
            sha256: manifest_sha256,
            size: manifest_size,
        }),
        provenance: Provenance {
            producer: "svwb-toolkit".into(),
            producer_version: env!("CARGO_PKG_VERSION").into(),
            generated_at: string("generated_at")?,
        },
    };
    profile.require_valid()?;
    Ok(profile)
}

pub fn probe_runtime_profile_via_wbarts(
    profile: &RuntimeProfile,
    api_base: &str,
    token_file: &Path,
) -> Result<RuntimeProbe> {
    profile.require_valid()?;
    let value = send_admin(
        api_base,
        token_file,
        reqwest::Method::POST,
        "/api/admin/svwb-profile-probe",
        Some(json!({"profile": profile})),
    )?;
    let probe: RuntimeProbe =
        serde_json::from_value(value).context("parse runtime probe response")?;
    let issues = probe.validate();
    if !issues.is_empty() {
        bail!("runtime probe response is invalid: {}", issues.join("; "));
    }
    let expected = profile.profile_id();
    if probe.subject_type != RuntimeProbeSubjectType::RuntimeProfile || probe.subject_id != expected
    {
        bail!(
            "runtime probe subject mismatch: response={} local={}",
            probe.subject_id,
            expected
        );
    }
    Ok(probe)
}

pub fn probe_runtime_draft_via_wbarts(
    profile: &RuntimeProfile,
    api_base: &str,
    token_file: &Path,
) -> Result<RuntimeProbe> {
    let value = send_admin(
        api_base,
        token_file,
        reqwest::Method::POST,
        "/api/admin/svwb-profile-probe",
        Some(json!({
            "target": {
                "schema_version": profile.schema_version,
                "operator": profile.operator,
                "game_build": profile.game_build,
                "runtime": {
                    "base_url": profile.runtime.base_url,
                    "app_version": profile.runtime.app_version,
                },
                "protocol": profile.protocol,
                "client_headers": profile.client_headers,
            }
        })),
    )?;
    let probe: RuntimeProbe =
        serde_json::from_value(value).context("parse runtime probe response")?;
    let issues = probe.validate();
    if !issues.is_empty() {
        bail!("runtime probe response is invalid: {}", issues.join("; "));
    }
    let expected = runtime_draft_id(profile);
    if probe.subject_type != RuntimeProbeSubjectType::RuntimeProfileDraft
        || probe.subject_id != expected
    {
        bail!(
            "runtime draft subject mismatch: response={} local={}",
            probe.subject_id,
            expected
        );
    }
    Ok(probe)
}

pub fn runtime_draft_id(profile: &RuntimeProfile) -> String {
    let headers: Vec<[&str; 2]> = profile
        .client_headers
        .iter()
        .map(|(name, value)| [name.as_str(), value.as_str()])
        .collect();
    let identity = json!([
        "svwb-runtime-profile-draft-v1",
        profile.schema_version,
        profile.operator,
        profile.game_build.gameassembly_sha256,
        profile.game_build.exe_version,
        profile.game_build.unity_version,
        profile.game_build.metadata_sha256,
        profile.runtime.base_url,
        profile.runtime.app_version,
        profile.protocol.family,
        profile.protocol.codec_version,
        profile.protocol.routing_header,
        profile.protocol.common_header,
        headers,
    ]);
    let bytes = serde_json::to_vec(&identity).expect("runtime draft identity is serializable");
    let digest = sha2::Sha256::digest(bytes);
    format!("sha256:{}", hex_bytes(&digest))
}

pub fn publish_runtime_profile(
    profile: &RuntimeProfile,
    api_base: &str,
    token_file: &Path,
) -> Result<Value> {
    profile.require_valid()?;
    send_admin(
        api_base,
        token_file,
        reqwest::Method::POST,
        "/api/admin/svwb-profiles",
        Some(json!({"profile": profile})),
    )
}

pub fn activate_runtime_profile(
    profile_id: &str,
    api_base: &str,
    token_file: &Path,
) -> Result<Value> {
    send_admin(
        api_base,
        token_file,
        reqwest::Method::POST,
        "/api/admin/svwb-profiles/activate",
        Some(json!({"profile_id": profile_id})),
    )
}

pub fn list_runtime_profiles(
    operator: Operator,
    api_base: &str,
    token_file: &Path,
) -> Result<Value> {
    send_admin(
        api_base,
        token_file,
        reqwest::Method::GET,
        &format!("/api/admin/svwb-profiles?operator={}", operator.as_str()),
        None,
    )
}

pub fn get_active_runtime_profile(
    operator: Operator,
    api_base: &str,
    token_file: &Path,
) -> Result<Value> {
    send_admin(
        api_base,
        token_file,
        reqwest::Method::GET,
        &format!(
            "/api/admin/svwb-profiles/active?operator={}",
            operator.as_str()
        ),
        None,
    )
}

fn send_admin(
    api_base: &str,
    token_file: &Path,
    method: reqwest::Method,
    path: &str,
    body: Option<Value>,
) -> Result<Value> {
    validate_api_base(api_base)?;
    let token = read_token_file(token_file)?;
    let client = Client::builder()
        .timeout(Duration::from_secs(45))
        .build()
        .context("create WBArts HTTP client")?;
    let mut request = client
        .request(
            method,
            format!("{}{}", api_base.trim_end_matches('/'), path),
        )
        .bearer_auth(token);
    if let Some(body) = body {
        request = request.json(&body);
    }
    let response = request.send().context("send WBArts admin request")?;
    let status = response.status();
    let text = response.text().context("read WBArts admin response")?;
    if text.len() > 64 << 10 {
        bail!("WBArts admin response exceeds 64 KiB");
    }
    if !status.is_success() {
        bail!(
            "WBArts admin request returned HTTP {}: {}",
            status.as_u16(),
            text.trim()
        );
    }
    if text.trim().is_empty() {
        return Ok(json!({}));
    }
    serde_json::from_str(&text).context("parse WBArts admin response")
}

fn validate_api_base(value: &str) -> Result<()> {
    let url = Url::parse(value).context("parse WBArts API base")?;
    let local = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "::1"));
    if url.host_str().is_none()
        || url.username() != ""
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || (url.scheme() != "https" && !(url.scheme() == "http" && local))
    {
        bail!("WBArts API base must use HTTPS, except for HTTP on loopback");
    }
    Ok(())
}

fn read_token_file(path: &Path) -> Result<String> {
    let metadata =
        fs::metadata(path).with_context(|| format!("read token metadata: {}", path.display()))?;
    if !private_permissions(&metadata) {
        bail!("token file must not be readable by group or others");
    }
    let token = fs::read_to_string(path)
        .with_context(|| format!("read token file: {}", path.display()))?
        .trim()
        .to_string();
    if token.is_empty() {
        bail!("token file is empty");
    }
    Ok(token)
}

#[cfg(unix)]
fn private_permissions(metadata: &fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    metadata.permissions().mode() & 0o077 == 0
}

#[cfg(not(unix))]
fn private_permissions(_metadata: &fs::Metadata) -> bool {
    true
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_dumper_runtime_snapshot() {
        let value = json!({
            "generated_at": "2026-10-30T08:00:00Z",
            "resource_version": "318861727954",
            "app_version": "1.9.10",
            "routing_header": "Abcdefghijklmnop1234",
            "common_header": "AB".repeat(52),
            "manifest_variant": "Cht",
            "manifest_url": "https://shadowverse-wb.akamaized.net/dl/manifests/Windows/318861727954/assetbundle.Cht.manifest",
            "manifest": {
                "sha256": "c".repeat(64),
                "size": 1234
            },
            "metadata_sha256": "b".repeat(64),
            "gameassembly_sha256": "a".repeat(64)
        });
        let profile = runtime_profile_from_dumper(&value).expect("convert profile");
        assert_eq!(profile.operator, Operator::Cygames);
        assert_eq!(profile.runtime.resource_version, "318861727954");
        assert_eq!(profile.protocol.family, ProtocolFamily::CygamesV1);
        assert!(!profile.profile_id().is_empty());
    }

    #[test]
    fn runtime_draft_id_matches_server_contract() {
        let profile = RuntimeProfile {
            schema_version: 1,
            artifact_type: "runtime_profile".into(),
            operator: Operator::Cygames,
            game_build: GameBuild {
                gameassembly_sha256: Some("a".repeat(64)),
                exe_version: Some("1.9.10.19088".into()),
                unity_version: Some("2022.3.62f2".into()),
                metadata_sha256: Some("b".repeat(64)),
            },
            runtime: RuntimeConfig {
                base_url: "https://web11.shadowverse-wb.jp".into(),
                resource_version: String::new(),
                app_version: "1.9.10".into(),
            },
            protocol: ProtocolConfig {
                family: ProtocolFamily::CygamesV1,
                codec_version: "1".into(),
                routing_header: Some("Abcdefghijklmnop1234".into()),
                common_header: Some("AB".repeat(52)),
            },
            client_headers: BTreeMap::from([
                ("device".into(), "3".into()),
                ("device_name".into(), "Windows PC".into()),
                ("platform".into(), "4".into()),
            ]),
            manifest_source: None,
            provenance: Provenance {
                producer: "svwb".into(),
                producer_version: "0.1.1".into(),
                generated_at: "2026-10-30T08:00:00Z".into(),
            },
        };
        assert_eq!(
            runtime_draft_id(&profile),
            "sha256:7d9d781a43e1d83ba10502fc4bca08700ae206f02ca9527302570303a5b3b897"
        );
    }
}
