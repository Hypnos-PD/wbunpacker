#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::{
    io::{BufRead, BufReader},
    path::PathBuf,
    process::{Command, Stdio},
    str::FromStr,
};

use serde::{Deserialize, Serialize};
use svwb_core::{
    Operator, RuntimeProbe, RuntimeProfile,
    credential::{CredentialSummary, inspect_cygames_systemd, inspect_netease_auth},
    doctor::{DoctorReport, run as run_doctor},
    native_detect::{NativeDetectorOptions, complete_native_detection, detect_with_native},
    netease_detect::{NeteaseDetectorOptions, detect as detect_netease},
    probe::{
        activate_runtime_profile, probe_runtime_draft_via_wbarts, probe_runtime_profile_via_wbarts,
        publish_runtime_profile,
    },
};
use tauri::Emitter;

#[derive(Serialize)]
struct ValidationReport {
    ok: bool,
    file: String,
    operator: String,
    issues: Vec<String>,
}

#[derive(Clone, Deserialize, Serialize)]
struct PipelineStepInfo {
    id: String,
    group: String,
    description: String,
    command: Vec<String>,
    cwd: String,
    depends_on: Vec<String>,
}

#[derive(Serialize)]
struct PipelineCommandResult {
    success: bool,
    code: Option<i32>,
}

#[derive(Clone, Serialize)]
struct PipelineOutputEvent {
    stream: String,
    line: String,
}

fn svwb_executable(configured: Option<String>) -> String {
    configured
        .filter(|value| !value.trim().is_empty())
        .or_else(|| std::env::var("SVWB_EXE").ok())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "svwb".into())
}

fn command_failure(exe: &str, output: std::process::Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if !stderr.is_empty() {
        stderr
    } else if !stdout.is_empty() {
        stdout
    } else {
        format!("{exe} exited with {}", output.status)
    }
}

#[tauri::command]
async fn pipeline_list(
    file: String,
    executable: Option<String>,
) -> Result<Vec<PipelineStepInfo>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let exe = svwb_executable(executable);
        let output = Command::new(&exe)
            .args(["pipeline", "list", "--file", &file, "--json"])
            .output()
            .map_err(|error| format!("run {exe}: {error}"))?;
        if !output.status.success() {
            return Err(command_failure(&exe, output));
        }
        serde_json::from_slice(&output.stdout).map_err(|error| error.to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn pipeline_check(file: String, executable: Option<String>) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let exe = svwb_executable(executable);
        let output = Command::new(&exe)
            .args(["pipeline", "check", "--file", &file])
            .output()
            .map_err(|error| format!("run {exe}: {error}"))?;
        if !output.status.success() {
            return Err(command_failure(&exe, output));
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn pipeline_run(
    app: tauri::AppHandle,
    file: String,
    executable: Option<String>,
    only: Vec<String>,
    dry_run: bool,
    continue_on_error: bool,
) -> Result<PipelineCommandResult, String> {
    tauri::async_runtime::spawn_blocking(move || {
        if only.is_empty() {
            return Err("select at least one pipeline step".into());
        }
        let exe = svwb_executable(executable);
        let mut command = Command::new(&exe);
        command.args(["pipeline", "run", "--file", &file]);
        if dry_run {
            command.arg("--dry-run");
        }
        if continue_on_error {
            command.arg("--continue-on-error");
        }
        for id in &only {
            command.args(["--only", id]);
        }
        let mut child = command
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| format!("run {exe}: {error}"))?;
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let stdout_app = app.clone();
        let stdout_thread = std::thread::spawn(move || {
            if let Some(stdout) = stdout {
                for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                    let _ = stdout_app.emit(
                        "pipeline-output",
                        PipelineOutputEvent {
                            stream: "stdout".into(),
                            line,
                        },
                    );
                }
            }
        });
        let stderr_app = app;
        let stderr_thread = std::thread::spawn(move || {
            if let Some(stderr) = stderr {
                for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                    let _ = stderr_app.emit(
                        "pipeline-output",
                        PipelineOutputEvent {
                            stream: "stderr".into(),
                            line,
                        },
                    );
                }
            }
        });
        let status = child.wait().map_err(|error| error.to_string())?;
        let _ = stdout_thread.join();
        let _ = stderr_thread.join();
        Ok(PipelineCommandResult {
            success: status.success(),
            code: status.code(),
        })
    })
    .await
    .map_err(|error| error.to_string())?
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
    operator: String,
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
        let operator = Operator::from_str(&operator).map_err(str::to_string)?;
        let profile = match operator {
            Operator::Netease => {
                detect_netease(&NeteaseDetectorOptions {
                    game_root: game_root.map(PathBuf::from),
                    variant,
                    timeout: std::time::Duration::from_secs(60),
                })
                .map_err(|error| error.to_string())?
                .profile
            }
            Operator::Cygames => {
                let home = std::env::var_os("HOME")
                    .map(PathBuf::from)
                    .ok_or_else(|| "HOME is not set".to_string())?;
                let detection = detect_with_native(&NativeDetectorOptions {
                    game_root: game_root.map(PathBuf::from).unwrap_or_else(|| {
                        home.join(".local/share/Steam/steamapps/common/ShadowverseWB")
                    }),
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
                if let Some(profile) = detection.profile {
                    profile
                } else {
                    let token_file = token_file
                        .ok_or_else(|| "manifest URL and JWT file are unavailable".to_string())?;
                    let token_path = PathBuf::from(token_file);
                    let probe =
                        probe_runtime_draft_via_wbarts(&detection.draft, &api_base, &token_path)
                            .map_err(|error| error.to_string())?;
                    let resource_version = probe
                        .resource_version
                        .as_deref()
                        .ok_or_else(|| "probe returned no resource_version".to_string())?;
                    complete_native_detection(&detection, resource_version)
                        .map_err(|error| error.to_string())?
                }
            }
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
            detect_profile,
            pipeline_list,
            pipeline_check,
            pipeline_run
        ])
        .run(tauri::generate_context!())
        .expect("svwb-gui runtime error");
}
