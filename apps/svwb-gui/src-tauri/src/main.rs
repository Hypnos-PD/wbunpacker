#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::{path::PathBuf, str::FromStr};

use serde::Serialize;
use svwb_core::{
    Operator, RuntimeProbe, RuntimeProfile,
    credential::{CredentialSummary, inspect_cygames_systemd, inspect_netease_auth},
    doctor::{DoctorReport, run as run_doctor},
    native_detect::{NativeDetectorOptions, complete_native_detection, detect_with_native},
    probe::{
        activate_runtime_profile, probe_runtime_draft_via_wbarts, probe_runtime_profile_via_wbarts,
        publish_runtime_profile,
    },
};

#[derive(Serialize)]
struct ValidationReport {
    ok: bool,
    file: String,
    operator: String,
    issues: Vec<String>,
}

#[tauri::command]
fn doctor_report(config: Option<String>) -> DoctorReport {
    run_doctor(config.map(PathBuf::from))
}

#[tauri::command]
fn validate_profile(path: String) -> Result<ValidationReport, String> {
    let profile = RuntimeProfile::load(&PathBuf::from(&path)).map_err(|error| error.to_string())?;
    let issues = profile.validate();
    Ok(ValidationReport {
        ok: issues.is_empty(),
        file: path,
        operator: profile.operator.as_str().into(),
        issues,
    })
}

#[tauri::command]
fn credential_inspect(operator: String, path: String) -> Result<CredentialSummary, String> {
    let operator = Operator::from_str(&operator).map_err(str::to_string)?;
    let path = PathBuf::from(path);
    match operator {
        Operator::Cygames => inspect_cygames_systemd(&path),
        Operator::Netease => inspect_netease_auth(&path),
    }
    .map_err(|error| error.to_string())
}

#[tauri::command]
async fn probe_profile(
    path: String,
    api_base: String,
    token_file: String,
) -> Result<RuntimeProbe, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let profile =
            RuntimeProfile::load(&PathBuf::from(path)).map_err(|error| error.to_string())?;
        probe_runtime_profile_via_wbarts(&profile, &api_base, &PathBuf::from(token_file))
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn publish_profile(
    path: String,
    api_base: String,
    token_file: String,
) -> Result<serde_json::Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let profile =
            RuntimeProfile::load(&PathBuf::from(path)).map_err(|error| error.to_string())?;
        publish_runtime_profile(&profile, &api_base, &PathBuf::from(token_file))
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn activate_profile(
    profile_id: String,
    api_base: String,
    token_file: String,
) -> Result<serde_json::Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
        activate_runtime_profile(&profile_id, &api_base, &PathBuf::from(token_file))
            .map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
async fn detect_profile(
    game_root: Option<String>,
    data_root: Option<String>,
    variant: String,
    pid: Option<u32>,
    dotnet: String,
    il2cpp_dumper_dir: Option<String>,
    api_base: String,
    token_file: Option<String>,
    output: String,
) -> Result<serde_json::Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or_else(|| "HOME is not set".to_string())?;
        let detection = detect_with_native(&NativeDetectorOptions {
            game_root: game_root
                .map(PathBuf::from)
                .unwrap_or_else(|| home.join(".local/share/Steam/steamapps/common/ShadowverseWB")),
            data_root: data_root
                .map(PathBuf::from)
                .unwrap_or_else(|| home.join("Data/svwb_dumper")),
            process_name: "ShadowverseWB.exe".into(),
            pid,
            memory_dump: None,
            variant,
            min_region_bytes: 4 * 1024,
            max_region_bytes: 256 * 1024 * 1024,
            dotnet: PathBuf::from(dotnet),
            il2cpp_dumper_dir: il2cpp_dumper_dir
                .map(PathBuf::from)
                .unwrap_or_else(|| home.join(".local/share/svwb_dumper/Il2CppDumper")),
            refresh_dump: false,
        })
        .map_err(|error| error.to_string())?;
        let profile = if let Some(profile) = detection.profile {
            profile
        } else {
            let token_file = token_file
                .ok_or_else(|| "manifest URL and JWT file are unavailable".to_string())?;
            let token_path = PathBuf::from(token_file);
            let probe = probe_runtime_draft_via_wbarts(&detection.draft, &api_base, &token_path)
                .map_err(|error| error.to_string())?;
            let resource_version = probe
                .resource_version
                .as_deref()
                .ok_or_else(|| "probe returned no resource_version".to_string())?;
            complete_native_detection(&detection, resource_version)
                .map_err(|error| error.to_string())?
        };
        let path = PathBuf::from(&output);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        let body =
            serde_json::to_string_pretty(&profile).map_err(|error| error.to_string())? + "\n";
        let temporary = path.with_extension(format!("json.tmp-{}", std::process::id()));
        std::fs::write(&temporary, body).map_err(|error| error.to_string())?;
        std::fs::rename(&temporary, &path).map_err(|error| error.to_string())?;
        Ok(serde_json::json!({
            "profile_id": profile.profile_id(),
            "profile_path": path,
        }))
    })
    .await
    .map_err(|error| error.to_string())?
}

fn main() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            doctor_report,
            validate_profile,
            credential_inspect,
            probe_profile,
            publish_profile,
            activate_profile,
            detect_profile
        ])
        .run(tauri::generate_context!())
        .expect("svwb-gui runtime error");
}
