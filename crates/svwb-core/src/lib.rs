use std::{collections::BTreeMap, fs, path::Path, str::FromStr};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};
use url::Url;

pub mod credential;
pub mod doctor;
pub mod native_detect;
pub mod probe;

pub const RUNTIME_PROFILE_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Operator {
    Cygames,
    Netease,
}

impl Operator {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Cygames => "cygames",
            Self::Netease => "netease",
        }
    }
}

impl FromStr for Operator {
    type Err = &'static str;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value.trim().to_ascii_lowercase().as_str() {
            "cygames" | "cy" => Ok(Self::Cygames),
            "netease" | "ne" => Ok(Self::Netease),
            _ => Err("operator must be one of: cygames, cy, netease, ne"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeProfile {
    pub schema_version: u32,
    pub artifact_type: String,
    pub operator: Operator,
    pub game_build: GameBuild,
    pub runtime: RuntimeConfig,
    pub protocol: ProtocolConfig,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub client_headers: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub manifest_source: Option<ManifestSource>,
    pub provenance: Provenance,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GameBuild {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gameassembly_sha256: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exe_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unity_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata_sha256: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeConfig {
    pub base_url: String,
    pub resource_version: String,
    pub app_version: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProtocolConfig {
    pub family: ProtocolFamily,
    pub codec_version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub routing_header: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub common_header: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProtocolFamily {
    CygamesV1,
    NeteaseV1,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestSource {
    pub variant: String,
    pub url: String,
    pub sha256: String,
    pub size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Provenance {
    pub producer: String,
    pub producer_version: String,
    pub generated_at: String,
}

impl RuntimeProfile {
    pub fn load(path: &Path) -> Result<Self> {
        let raw = fs::read_to_string(path)
            .with_context(|| format!("read runtime profile: {}", path.display()))?;
        serde_json::from_str(&raw)
            .with_context(|| format!("parse runtime profile: {}", path.display()))
    }

    pub fn validate(&self) -> Vec<String> {
        let mut issues = Vec::new();

        if self.schema_version != RUNTIME_PROFILE_SCHEMA_VERSION {
            issues.push(format!(
                "schema_version must be {RUNTIME_PROFILE_SCHEMA_VERSION}"
            ));
        }
        if self.artifact_type != "runtime_profile" {
            issues.push("artifact_type must be runtime_profile".into());
        }

        if self.game_build.gameassembly_sha256.is_none() && self.game_build.exe_version.is_none() {
            issues.push("game_build requires gameassembly_sha256 or exe_version".into());
        }
        validate_optional_sha256(
            "game_build.gameassembly_sha256",
            self.game_build.gameassembly_sha256.as_deref(),
            &mut issues,
        );
        validate_optional_sha256(
            "game_build.metadata_sha256",
            self.game_build.metadata_sha256.as_deref(),
            &mut issues,
        );

        validate_https_url(
            "runtime.base_url",
            &self.runtime.base_url,
            false,
            &mut issues,
        );
        validate_resource_version(&self.runtime.resource_version, &mut issues);
        validate_app_version(&self.runtime.app_version, &mut issues);

        if self.protocol.codec_version.trim().is_empty() {
            issues.push("protocol.codec_version is empty".into());
        }
        match (self.operator, self.protocol.family) {
            (Operator::Cygames, ProtocolFamily::CygamesV1) => {
                let routing = self.protocol.routing_header.as_deref().unwrap_or("");
                if !valid_routing_header(routing) {
                    issues.push(
                        "protocol.routing_header must be 16-64 ASCII letters, digits, '-' or '_'"
                            .into(),
                    );
                }
                let common = self.protocol.common_header.as_deref().unwrap_or("");
                if !valid_common_header(common) {
                    issues.push("protocol.common_header must be 104 hexadecimal characters".into());
                }
            }
            (Operator::Netease, ProtocolFamily::NeteaseV1) => {
                if self.protocol.routing_header.is_some() {
                    issues.push("netease profile must not contain routing_header".into());
                }
                if self.protocol.common_header.is_some() {
                    issues.push("netease profile must not contain common_header".into());
                }
            }
            _ => issues.push("operator and protocol.family do not match".into()),
        }

        for (name, value) in &self.client_headers {
            if name.trim().is_empty() {
                issues.push("client_headers contains an empty name".into());
            }
            if value.trim().is_empty() {
                issues.push(format!("client_headers.{name} is empty"));
            }
        }

        if let Some(source) = &self.manifest_source {
            if source.variant.trim().is_empty() {
                issues.push("manifest_source.variant is empty".into());
            }
            validate_https_url("manifest_source.url", &source.url, false, &mut issues);
            if !valid_sha256(&source.sha256) {
                issues.push("manifest_source.sha256 must be 64 lowercase hex characters".into());
            }
            if source.size == 0 {
                issues.push("manifest_source.size must be greater than zero".into());
            }
        }

        if self.provenance.producer.trim().is_empty() {
            issues.push("provenance.producer is empty".into());
        }
        if self.provenance.producer_version.trim().is_empty() {
            issues.push("provenance.producer_version is empty".into());
        }
        validate_rfc3339(
            "provenance.generated_at",
            &self.provenance.generated_at,
            &mut issues,
        );

        issues
    }

    pub fn require_valid(&self) -> Result<()> {
        let issues = self.validate();
        if issues.is_empty() {
            return Ok(());
        }
        bail!("{}", issues.join("\n"))
    }

    pub fn profile_id(&self) -> String {
        let headers: Vec<[&str; 2]> = self
            .client_headers
            .iter()
            .map(|(name, value)| [name.as_str(), value.as_str()])
            .collect();
        let identity = serde_json::json!([
            "svwb-runtime-profile-v1",
            self.schema_version,
            self.operator,
            self.game_build.gameassembly_sha256,
            self.game_build.exe_version,
            self.game_build.unity_version,
            self.game_build.metadata_sha256,
            self.runtime.base_url,
            self.runtime.resource_version,
            self.runtime.app_version,
            self.protocol.family,
            self.protocol.codec_version,
            self.protocol.routing_header,
            self.protocol.common_header,
            headers,
        ]);
        let bytes = serde_json::to_vec(&identity).expect("profile identity is serializable");
        let digest = Sha256::digest(bytes);
        format!("sha256:{}", hex_bytes(&digest))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeProbe {
    pub schema_version: u32,
    pub artifact_type: String,
    pub subject_type: RuntimeProbeSubjectType,
    pub subject_id: String,
    pub operator: Operator,
    pub tested_at: String,
    pub request_scope: String,
    pub status: RuntimeProbeStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resource_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app_version: Option<String>,
    pub resource_version_unknown: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeProbeSubjectType {
    RuntimeProfile,
    RuntimeProfileDraft,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeProbeStatus {
    Success,
    Failed,
}

impl RuntimeProbe {
    pub fn load(path: &Path) -> Result<Self> {
        let raw = fs::read_to_string(path)
            .with_context(|| format!("read runtime probe: {}", path.display()))?;
        serde_json::from_str(&raw)
            .with_context(|| format!("parse runtime probe: {}", path.display()))
    }

    pub fn validate(&self) -> Vec<String> {
        let mut issues = Vec::new();
        if self.schema_version != 1 {
            issues.push("schema_version must be 1".into());
        }
        if self.artifact_type != "runtime_probe" {
            issues.push("artifact_type must be runtime_probe".into());
        }
        if !self.subject_id.starts_with("sha256:")
            || !valid_sha256(self.subject_id.trim_start_matches("sha256:"))
        {
            issues.push("subject_id must be sha256:<64 lowercase hex characters>".into());
        }
        validate_rfc3339("tested_at", &self.tested_at, &mut issues);
        if self.request_scope.trim().is_empty() {
            issues.push("request_scope is empty".into());
        }
        match self.status {
            RuntimeProbeStatus::Success => {
                if let Some(value) = &self.resource_version {
                    validate_resource_version(value, &mut issues);
                } else if !self.resource_version_unknown {
                    issues.push("successful probe requires resource_version".into());
                }
                if let Some(value) = &self.app_version {
                    validate_app_version(value, &mut issues);
                } else {
                    issues.push("successful probe requires app_version".into());
                }
                if self.error.is_some() {
                    issues.push("successful probe must not contain error".into());
                }
            }
            RuntimeProbeStatus::Failed => {
                if self.error.as_deref().is_none_or(str::is_empty) {
                    issues.push("failed probe requires error".into());
                }
            }
        }
        issues
    }
}

fn validate_optional_sha256(name: &str, value: Option<&str>, issues: &mut Vec<String>) {
    if let Some(value) = value
        && !valid_sha256(value)
    {
        issues.push(format!("{name} must be 64 lowercase hex characters"));
    }
}

fn validate_https_url(name: &str, value: &str, allow_query: bool, issues: &mut Vec<String>) {
    let parsed = Url::parse(value);
    match parsed {
        Ok(url)
            if url.scheme() == "https"
                && url.host_str().is_some()
                && url.username().is_empty()
                && url.password().is_none()
                && (allow_query || url.query().is_none())
                && url.fragment().is_none() => {}
        _ => issues.push(format!(
            "{name} must be an HTTPS URL without credentials, query or fragment"
        )),
    }
}

fn validate_resource_version(value: &str, issues: &mut Vec<String>) {
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        issues.push("runtime.resource_version must contain digits only".into());
    } else if value.bytes().all(|byte| byte == b'0') {
        issues.push("runtime.resource_version must not be all zeroes".into());
    }
}

fn validate_app_version(value: &str, issues: &mut Vec<String>) {
    let parts: Vec<&str> = value.split('.').collect();
    let valid = !value.is_empty()
        && parts.len() <= 4
        && parts
            .iter()
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()));
    if !valid {
        issues.push("runtime.app_version must contain one to four numeric components".into());
    }
}

fn valid_routing_header(value: &str) -> bool {
    (16..=64).contains(&value.len())
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}

fn valid_common_header(value: &str) -> bool {
    value.len() == 104 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn validate_rfc3339(name: &str, value: &str, issues: &mut Vec<String>) {
    if OffsetDateTime::parse(value, &Rfc3339).is_err() {
        issues.push(format!("{name} must be an RFC3339 timestamp"));
    }
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

    fn profile(operator: Operator, family: ProtocolFamily) -> RuntimeProfile {
        RuntimeProfile {
            schema_version: 1,
            artifact_type: "runtime_profile".into(),
            operator,
            game_build: GameBuild {
                gameassembly_sha256: Some("a".repeat(64)),
                metadata_sha256: Some("b".repeat(64)),
                ..GameBuild::default()
            },
            runtime: RuntimeConfig {
                base_url: "https://example.test".into(),
                resource_version: "123456789".into(),
                app_version: "1.2.3".into(),
            },
            protocol: ProtocolConfig {
                family,
                codec_version: "1".into(),
                routing_header: (operator == Operator::Cygames)
                    .then(|| "Abcdefghijklmnop1234".into()),
                common_header: (operator == Operator::Cygames).then(|| "AB".repeat(52)),
            },
            client_headers: BTreeMap::new(),
            manifest_source: None,
            provenance: Provenance {
                producer: "svwb".into(),
                producer_version: "0.1.0".into(),
                generated_at: "2026-10-30T08:00:00Z".into(),
            },
        }
    }

    #[test]
    fn validates_cygames_profile() {
        let profile = profile(Operator::Cygames, ProtocolFamily::CygamesV1);
        assert!(profile.validate().is_empty(), "{:?}", profile.validate());
    }

    #[test]
    fn validates_netease_profile() {
        let profile = profile(Operator::Netease, ProtocolFamily::NeteaseV1);
        assert!(profile.validate().is_empty(), "{:?}", profile.validate());
    }

    #[test]
    fn rejects_mismatched_operator_and_protocol() {
        let profile = profile(Operator::Netease, ProtocolFamily::CygamesV1);
        assert!(
            profile
                .validate()
                .iter()
                .any(|issue| issue.contains("do not match"))
        );
    }

    #[test]
    fn accepts_operator_aliases() {
        assert_eq!("cy".parse::<Operator>(), Ok(Operator::Cygames));
        assert_eq!("cygames".parse::<Operator>(), Ok(Operator::Cygames));
        assert_eq!("ne".parse::<Operator>(), Ok(Operator::Netease));
        assert_eq!("netease".parse::<Operator>(), Ok(Operator::Netease));
    }

    #[test]
    fn profile_id_is_stable_and_ignores_provenance_time() {
        let mut first = profile(Operator::Cygames, ProtocolFamily::CygamesV1);
        let original = first.profile_id();
        first.provenance.generated_at = "2027-01-01T00:00:00Z".into();
        assert_eq!(first.profile_id(), original);
        first.runtime.resource_version = "987654321".into();
        assert_ne!(first.profile_id(), original);
    }

    #[test]
    fn validates_runtime_probe() {
        let probe = RuntimeProbe {
            schema_version: 1,
            artifact_type: "runtime_probe".into(),
            subject_type: RuntimeProbeSubjectType::RuntimeProfile,
            subject_id: format!("sha256:{}", "a".repeat(64)),
            operator: Operator::Cygames,
            tested_at: "2026-10-30T08:00:00Z".into(),
            request_scope: "title_version".into(),
            status: RuntimeProbeStatus::Success,
            resource_version: Some("318861727954".into()),
            app_version: Some("1.9.10".into()),
            resource_version_unknown: false,
            error: None,
        };
        assert!(probe.validate().is_empty(), "{:?}", probe.validate());
    }
}
