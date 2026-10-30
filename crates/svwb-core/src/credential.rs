use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use url::Url;

use crate::Operator;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialKind {
    CygamesSystemd,
    NeteaseAuthFile,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CredentialSummary {
    pub operator: Operator,
    pub kind: CredentialKind,
    pub fingerprint: String,
    pub valid: bool,
    pub issues: Vec<String>,
    pub files: Vec<CredentialFile>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CredentialFile {
    pub name: String,
    pub exists: bool,
    pub readable: bool,
    pub private: bool,
}

pub fn inspect_cygames_systemd(directory: &Path) -> Result<CredentialSummary> {
    let names = ["svwb-client-id", "svwb-uuid", "svwb-auth-key"];
    let mut issues = Vec::new();
    let mut files = Vec::new();
    let mut hash = Sha256::new();
    hash.update(b"svwb-cygames-credential-v1\0");

    for name in names {
        let path = directory.join(name);
        let metadata = fs::metadata(&path);
        let exists = metadata.is_ok();
        let readable = fs::read_to_string(&path).is_ok();
        let private = metadata
            .as_ref()
            .map(|metadata| private_permissions(&path, metadata))
            .unwrap_or(false);
        files.push(CredentialFile {
            name: name.into(),
            exists,
            readable,
            private,
        });
        if !readable {
            issues.push(format!("{name} is missing or unreadable"));
            continue;
        }
        let value = fs::read_to_string(&path)?.trim().to_string();
        hash.update(name.as_bytes());
        hash.update([0]);
        hash.update(value.as_bytes());
        hash.update([0]);
        match name {
            "svwb-client-id"
                if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) =>
            {
                issues.push("svwb-client-id must contain digits only".into());
            }
            "svwb-uuid" if !valid_uuid(&value) => {
                issues.push("svwb-uuid must be a UUID".into());
            }
            "svwb-auth-key"
                if STANDARD
                    .decode(&value)
                    .map(|bytes| bytes.len() != 50)
                    .unwrap_or(true) =>
            {
                issues.push("svwb-auth-key must be Base64 encoding 50 bytes".into());
            }
            _ => {}
        }
        if !private {
            issues.push(format!("{name} should not be readable by group or others"));
        }
    }

    Ok(summary(
        Operator::Cygames,
        CredentialKind::CygamesSystemd,
        hash,
        issues,
        files,
    ))
}

pub fn inspect_netease_auth(path: &Path) -> Result<CredentialSummary> {
    let metadata = fs::metadata(path)
        .with_context(|| format!("read credential metadata: {}", path.display()))?;
    let raw = fs::read(path).with_context(|| format!("read credential: {}", path.display()))?;
    let value: Value = serde_json::from_slice(&raw)
        .with_context(|| format!("parse credential: {}", path.display()))?;
    let mut issues = Vec::new();
    validate_netease_auth(&value, &mut issues);
    let private = private_permissions(path, &metadata);
    if !private {
        issues.push("credential file should not be readable by group or others".into());
    }

    let mut hash = Sha256::new();
    hash.update(b"svwb-netease-credential-v1\0");
    hash.update(&raw);
    Ok(summary(
        Operator::Netease,
        CredentialKind::NeteaseAuthFile,
        hash,
        issues,
        vec![CredentialFile {
            name: "svwb-cn-auth".into(),
            exists: true,
            readable: true,
            private,
        }],
    ))
}

pub fn credential_fingerprint(bytes: &[u8]) -> String {
    let mut hash = Sha256::new();
    hash.update(bytes);
    format!("sha256:{}", hex_bytes(&hash.finalize()))
}

fn summary(
    operator: Operator,
    kind: CredentialKind,
    hash: Sha256,
    issues: Vec<String>,
    files: Vec<CredentialFile>,
) -> CredentialSummary {
    CredentialSummary {
        operator,
        kind,
        fingerprint: format!("sha256:{}", hex_bytes(&hash.finalize())),
        valid: issues.is_empty(),
        issues,
        files,
    }
}

fn validate_netease_auth(value: &Value, issues: &mut Vec<String>) {
    let Some(object) = value.as_object() else {
        issues.push("credential root must be an object".into());
        return;
    };
    let fields = [
        "schema_version",
        "base_url",
        "app_version",
        "resource_version",
        "user_agent",
        "unity_version",
        "platform",
        "device",
        "http_udid",
        "short_udid",
        "app_channel",
        "login_channel",
        "ne_platform",
        "runtime_platform",
        "anti_cheat_value",
        "secret_key",
        "local_uuid",
        "sdk_request",
    ];
    for field in fields {
        if object.get(field).is_none_or(Value::is_null) {
            issues.push(format!("missing field {field}"));
        }
    }
    if object.get("schema_version").and_then(Value::as_u64) != Some(1) {
        issues.push("schema_version must be 1".into());
    }
    if let Some(base_url) = object.get("base_url").and_then(Value::as_str) {
        let valid = Url::parse(base_url).is_ok_and(|url| {
            url.scheme() == "https"
                && url.host_str().is_some()
                && url.username().is_empty()
                && url.password().is_none()
                && url.query().is_none()
                && url.fragment().is_none()
        });
        if !valid {
            issues.push(
                "base_url must be an HTTPS URL without credentials, query or fragment".into(),
            );
        }
    }
    for field in ["http_udid", "local_uuid"] {
        if let Some(value) = object.get(field).and_then(Value::as_str)
            && !valid_uuid(value)
        {
            issues.push(format!("{field} must be a UUID"));
        }
    }
    if let Some(sdk) = object.get("sdk_request").and_then(Value::as_object) {
        for field in ["sessionid", "sauth_json", "udid", "unisdk_deviceid"] {
            if sdk
                .get(field)
                .and_then(Value::as_str)
                .is_none_or(str::is_empty)
            {
                issues.push(format!("sdk_request.{field} is missing or empty"));
            }
        }
    } else if object.get("sdk_request").is_some() {
        issues.push("sdk_request must be an object".into());
    }
}

fn valid_uuid(value: &str) -> bool {
    let compact = value.replace('-', "");
    compact.len() == 32 && compact.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[cfg(unix)]
fn private_permissions(_path: &Path, metadata: &fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;
    metadata.permissions().mode() & 0o077 == 0
}

#[cfg(not(unix))]
fn private_permissions(_path: &Path, _metadata: &fs::Metadata) -> bool {
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

pub fn require_path(path: &Path) -> Result<PathBuf> {
    if path.as_os_str().is_empty() {
        bail!("credential path is empty");
    }
    Ok(path.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_uuid_formats() {
        assert!(valid_uuid("00000000-0000-0000-0000-000000000000"));
        assert!(valid_uuid("00000000000000000000000000000000"));
        assert!(!valid_uuid("not-a-uuid"));
    }
}
