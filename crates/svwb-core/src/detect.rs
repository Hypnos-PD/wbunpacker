use std::{
    path::{Path, PathBuf},
    process::Command,
};

use anyhow::{Context, Result, bail};

use crate::{RuntimeProfile, probe::runtime_profile_from_dumper};

#[derive(Debug, Clone)]
pub struct PythonDumperOptions {
    pub python: PathBuf,
    pub script: PathBuf,
    pub game_root: Option<PathBuf>,
    pub data_root: Option<PathBuf>,
    pub variant: String,
    pub pid: Option<u32>,
}

pub fn detect_with_python_dumper(options: &PythonDumperOptions) -> Result<RuntimeProfile> {
    if !options.script.is_file() {
        bail!("dumper script not found: {}", options.script.display());
    }
    let temporary = tempfile::tempdir().context("create dumper output directory")?;
    let snapshot_path = temporary.path().join("runtime.json");
    let mut command = Command::new(&options.python);
    command
        .arg(&options.script)
        .arg("--dry-run")
        .arg("--json-out")
        .arg(&snapshot_path)
        .arg("--variant")
        .arg(&options.variant);
    if let Some(game_root) = &options.game_root {
        command.arg("--game-root").arg(game_root);
    }
    if let Some(data_root) = &options.data_root {
        command.arg("--data-root").arg(data_root);
    }
    if let Some(pid) = options.pid {
        command.arg("--pid").arg(pid.to_string());
    }
    let output = command.output().context("run Python dumper")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let detail = stderr.trim();
        bail!(
            "Python dumper exited with {}{}",
            output.status,
            if detail.is_empty() {
                String::new()
            } else {
                format!(": {detail}")
            }
        );
    }
    let raw = std::fs::read_to_string(&snapshot_path)
        .with_context(|| format!("read dumper snapshot: {}", snapshot_path.display()))?;
    let value: serde_json::Value =
        serde_json::from_str(&raw).context("parse dumper runtime snapshot")?;
    runtime_profile_from_dumper(&value)
}

pub fn default_python_dumper() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("SVWB_DUMPER") {
        return Some(PathBuf::from(path));
    }
    let sibling = Path::new("../svwb_dumper/scripts/sync_runtime.py");
    sibling.is_file().then(|| sibling.to_path_buf())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn detects_profile_through_adapter_command() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().unwrap();
        let script = directory.path().join("fake-dumper.sh");
        std::fs::write(
            &script,
            r#"#!/bin/sh
while [ "$#" -gt 0 ]; do
  if [ "$1" = "--json-out" ]; then
    shift
    out="$1"
  fi
  shift
done
cat > "$out" <<'JSON'
{
  "generated_at": "2026-10-30T08:00:00Z",
  "resource_version": "318861727954",
  "app_version": "1.9.10",
  "routing_header": "Abcdefghijklmnop1234",
  "common_header": "ABABABABABABABABABABABABABABABABABABABABABABABABABABABABABABABABABABABABABABABABABABABABABABABABABABABAB",
  "manifest_variant": "Cht",
  "manifest_url": "https://shadowverse-wb.akamaized.net/dl/manifests/Windows/318861727954/assetbundle.Cht.manifest",
  "manifest": {"sha256": "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc", "size": 1234},
  "metadata_sha256": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
  "gameassembly_sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
}
JSON
"#,
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();

        let profile = detect_with_python_dumper(&PythonDumperOptions {
            python: PathBuf::from("/bin/sh"),
            script,
            game_root: None,
            data_root: None,
            variant: "Cht".into(),
            pid: None,
        })
        .unwrap();
        assert_eq!(profile.runtime.resource_version, "318861727954");
        assert_eq!(profile.protocol.codec_version, "1");
    }
}
