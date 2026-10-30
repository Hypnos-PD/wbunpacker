use std::{
    path::{Path, PathBuf},
    str::FromStr,
};

use anyhow::{Result, bail};
use clap::{Parser, Subcommand, ValueEnum};
use svwb_core::{
    Operator, RuntimeProbe, RuntimeProbeStatus, RuntimeProfile,
    credential::{inspect_cygames_systemd, inspect_netease_auth},
    detect::{PythonDumperOptions, default_python_dumper, detect_with_python_dumper},
    doctor::{CheckStatus, run as run_doctor},
    native_detect::{NativeDetectorOptions, complete_native_detection, detect_with_native},
    probe::{
        activate_runtime_profile, get_active_runtime_profile, list_runtime_profiles,
        probe_cygames_draft_via_wbarts, probe_cygames_via_wbarts, publish_runtime_profile,
        runtime_profile_from_dumper,
    },
};

#[derive(Parser)]
#[command(name = "svwb", version, about = "SVWB data and protocol toolkit")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum DetectEngine {
    Python,
    Native,
}

#[derive(Subcommand)]
enum Command {
    /// Check local toolkit configuration and external commands
    Doctor {
        #[arg(long)]
        config: Option<PathBuf>,
        #[arg(long)]
        json: bool,
    },
    /// Inspect and validate runtime profiles
    Profile {
        #[command(subcommand)]
        command: ProfileCommand,
    },
    /// Inspect game credential material without printing secret values
    Credential {
        #[command(subcommand)]
        command: CredentialCommand,
    },
    /// Validate probe artifacts
    Probe {
        #[command(subcommand)]
        command: ProbeCommand,
    },
}

#[derive(Subcommand)]
enum ProfileCommand {
    /// Validate a runtime profile
    Validate {
        file: PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// Publish a candidate profile without activating it
    Publish {
        file: PathBuf,
        #[arg(long, default_value = "https://sva.hypd.asia")]
        api_base: String,
        #[arg(long)]
        token_file: PathBuf,
    },
    /// List stored profiles for an operator
    List {
        #[arg(long, default_value = "cygames")]
        operator: String,
        #[arg(long, default_value = "https://sva.hypd.asia")]
        api_base: String,
        #[arg(long)]
        token_file: PathBuf,
    },
    /// Show the active profile for an operator
    Active {
        #[arg(long, default_value = "cygames")]
        operator: String,
        #[arg(long, default_value = "https://sva.hypd.asia")]
        api_base: String,
        #[arg(long)]
        token_file: PathBuf,
    },
    /// Activate a stored profile
    Activate {
        profile_id: String,
        #[arg(long, default_value = "https://sva.hypd.asia")]
        api_base: String,
        #[arg(long)]
        token_file: PathBuf,
    },
    /// Print a normalized runtime profile
    Inspect { file: PathBuf },
    /// Print the stable profile ID
    Id { file: PathBuf },
    /// Execute a write-free probe through WBArts
    Probe {
        file: PathBuf,
        #[arg(long, default_value = "https://sva.hypd.asia")]
        api_base: String,
        #[arg(long)]
        token_file: PathBuf,
        #[arg(long)]
        output: Option<PathBuf>,
        #[arg(long)]
        json: bool,
    },
    /// Convert svwb_dumper runtime.json to a RuntimeProfile
    ImportDumper {
        file: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
    /// Detect the running Cygames client through the current dumper adapter
    Detect {
        #[arg(long, default_value = "cygames")]
        operator: String,
        #[arg(long)]
        dumper: Option<PathBuf>,
        #[arg(long, value_enum, default_value = "native")]
        engine: DetectEngine,
        #[arg(long, default_value = "python3")]
        python: PathBuf,
        #[arg(long)]
        game_root: Option<PathBuf>,
        #[arg(long)]
        data_root: Option<PathBuf>,
        #[arg(long, default_value = "Cht")]
        variant: String,
        #[arg(long, default_value = "ShadowverseWB.exe")]
        process: String,
        #[arg(long)]
        pid: Option<u32>,
        #[arg(long)]
        memory_dump: Option<PathBuf>,
        #[arg(long, default_value_t = 4)]
        min_region_kib: u64,
        #[arg(long, default_value_t = 256)]
        max_region_mib: u64,
        #[arg(long, default_value = "dotnet")]
        dotnet: PathBuf,
        #[arg(long)]
        il2cpp_dumper_dir: Option<PathBuf>,
        #[arg(long)]
        raw_output: Option<PathBuf>,
        #[arg(long)]
        refresh_dump: bool,
        #[arg(long, default_value = "https://sva.hypd.asia")]
        api_base: String,
        #[arg(long)]
        token_file: Option<PathBuf>,
        #[arg(long)]
        output: PathBuf,
    },
}

#[derive(Subcommand)]
enum CredentialCommand {
    /// Inspect an existing credential source
    Inspect {
        /// Operator: cygames/cy or netease/ne
        #[arg(long)]
        operator: String,
        /// Cygames credential directory or Netease auth file
        path: PathBuf,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum ProbeCommand {
    /// Validate a RuntimeProbe JSON file
    Validate {
        file: PathBuf,
        #[arg(long)]
        json: bool,
    },
}

fn main() {
    if let Err(error) = run() {
        eprintln!("error: {error:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    match Cli::parse().command {
        Command::Doctor { config, json } => doctor(config, json),
        Command::Profile { command } => match command {
            ProfileCommand::Validate { file, json } => validate_profile(&file, json),
            ProfileCommand::Inspect { file } => inspect_profile(&file),
            ProfileCommand::Id { file } => profile_id(&file),
            ProfileCommand::Probe {
                file,
                api_base,
                token_file,
                output,
                json,
            } => probe_profile(&file, &api_base, &token_file, output.as_deref(), json),
            ProfileCommand::ImportDumper { file, output } => import_dumper(&file, &output),
            ProfileCommand::Detect {
                operator,
                dumper,
                engine,
                python,
                game_root,
                data_root,
                variant,
                process,
                pid,
                memory_dump,
                min_region_kib,
                max_region_mib,
                dotnet,
                il2cpp_dumper_dir,
                raw_output,
                refresh_dump,
                api_base,
                token_file,
                output,
            } => detect_profile(
                &operator,
                dumper,
                engine,
                python,
                game_root,
                data_root,
                &variant,
                &process,
                pid,
                memory_dump,
                min_region_kib,
                max_region_mib,
                dotnet,
                il2cpp_dumper_dir,
                raw_output.as_deref(),
                refresh_dump,
                &api_base,
                token_file.as_deref(),
                &output,
            ),
            ProfileCommand::Publish {
                file,
                api_base,
                token_file,
            } => publish_profile(&file, &api_base, &token_file),
            ProfileCommand::List {
                operator,
                api_base,
                token_file,
            } => list_profiles(&operator, &api_base, &token_file),
            ProfileCommand::Active {
                operator,
                api_base,
                token_file,
            } => active_profile(&operator, &api_base, &token_file),
            ProfileCommand::Activate {
                profile_id,
                api_base,
                token_file,
            } => activate_profile(&profile_id, &api_base, &token_file),
        },
        Command::Credential { command } => match command {
            CredentialCommand::Inspect {
                operator,
                path,
                json,
            } => inspect_credential(&operator, &path, json),
        },
        Command::Probe { command } => match command {
            ProbeCommand::Validate { file, json } => validate_probe(&file, json),
        },
    }
}

fn doctor(config: Option<PathBuf>, json: bool) -> Result<()> {
    let report = run_doctor(config);

    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!("config: {}", report.config);
        for check in &report.checks {
            println!(
                "{:5} {:<28} {}",
                status_label(check.status),
                check.name,
                check.message
            );
        }
    }

    if !report.ok {
        bail!("doctor found configuration errors");
    }
    Ok(())
}

fn validate_profile(file: &Path, json: bool) -> Result<()> {
    let profile = RuntimeProfile::load(file)?;
    let issues = profile.validate();
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "ok": issues.is_empty(),
                "file": file,
                "operator": profile.operator.as_str(),
                "issues": issues,
            }))?
        );
    } else if issues.is_empty() {
        println!(
            "valid runtime profile: operator={} profile={}",
            profile.operator.as_str(),
            file.display()
        );
    } else {
        for issue in &issues {
            eprintln!("- {issue}");
        }
    }
    if !issues.is_empty() {
        bail!("runtime profile has {} validation issue(s)", issues.len());
    }
    Ok(())
}

fn inspect_profile(file: &Path) -> Result<()> {
    let profile = RuntimeProfile::load(file)?;
    profile.require_valid()?;
    println!("{}", serde_json::to_string_pretty(&profile)?);
    Ok(())
}

fn profile_id(file: &Path) -> Result<()> {
    let profile = RuntimeProfile::load(file)?;
    profile.require_valid()?;
    println!("{}", profile.profile_id());
    Ok(())
}

fn import_dumper(file: &Path, output: &Path) -> Result<()> {
    let raw: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(file)
            .map_err(|error| anyhow::anyhow!("read dumper output: {error}"))?,
    )?;
    let profile = runtime_profile_from_dumper(&raw)?;
    let body = serde_json::to_string_pretty(&profile)? + "\n";
    write_new_file(output, body.as_bytes())?;
    println!(
        "profile={} profile_id={}",
        output.display(),
        profile.profile_id()
    );
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn detect_profile(
    operator: &str,
    dumper: Option<PathBuf>,
    engine: DetectEngine,
    python: PathBuf,
    game_root: Option<PathBuf>,
    data_root: Option<PathBuf>,
    variant: &str,
    process: &str,
    pid: Option<u32>,
    memory_dump: Option<PathBuf>,
    min_region_kib: u64,
    max_region_mib: u64,
    dotnet: PathBuf,
    il2cpp_dumper_dir: Option<PathBuf>,
    raw_output: Option<&Path>,
    refresh_dump: bool,
    api_base: &str,
    token_file: Option<&Path>,
    output: &Path,
) -> Result<()> {
    let operator = Operator::from_str(operator).map_err(anyhow::Error::msg)?;
    if operator != Operator::Cygames {
        bail!("profile detect currently supports operator cygames only");
    }
    let (profile, raw) = match engine {
        DetectEngine::Python => {
            if memory_dump.is_some() {
                bail!("--memory-dump applies to the native engine only");
            }
            let script = dumper.or_else(default_python_dumper).ok_or_else(|| {
                anyhow::anyhow!("dumper script not found; pass --dumper or set SVWB_DUMPER")
            })?;
            let profile = detect_with_python_dumper(&PythonDumperOptions {
                python,
                script,
                game_root,
                data_root,
                variant: variant.into(),
                pid,
            })?;
            (profile, None)
        }
        DetectEngine::Native => {
            if dumper.is_some() {
                bail!("--dumper applies to the Python engine only");
            }
            let home = std::env::var_os("HOME")
                .map(PathBuf::from)
                .ok_or_else(|| anyhow::anyhow!("HOME is not set"))?;
            let game_root = game_root
                .unwrap_or_else(|| home.join(".local/share/Steam/steamapps/common/ShadowverseWB"));
            let data_root = data_root.unwrap_or_else(|| home.join("Data/svwb_dumper"));
            let il2cpp_dumper_dir = il2cpp_dumper_dir
                .unwrap_or_else(|| home.join(".local/share/svwb_dumper/Il2CppDumper"));
            let result = detect_with_native(&NativeDetectorOptions {
                game_root,
                data_root,
                process_name: process.into(),
                pid,
                memory_dump,
                variant: variant.into(),
                min_region_bytes: min_region_kib.saturating_mul(1024),
                max_region_bytes: max_region_mib.saturating_mul(1024 * 1024),
                dotnet,
                il2cpp_dumper_dir,
                refresh_dump,
            })?;
            let mut raw = result.raw.clone();
            let profile = if let Some(profile) = &result.profile {
                profile.clone()
            } else {
                if let Some(raw_output) = raw_output {
                    let body = serde_json::to_string_pretty(&raw)? + "\n";
                    write_new_file(raw_output, body.as_bytes())?;
                }
                let token_file = token_file.ok_or_else(|| {
                    anyhow::anyhow!(
                        "manifest URL is unavailable; pass --token-file to probe resource_version"
                    )
                })?;
                let probe = probe_cygames_draft_via_wbarts(&result.draft, api_base, token_file)?;
                if probe.status == RuntimeProbeStatus::Failed {
                    bail!(
                        "runtime probe failed: {}",
                        probe.error.as_deref().unwrap_or("unknown error")
                    );
                }
                let resource_version = probe
                    .resource_version
                    .as_deref()
                    .ok_or_else(|| anyhow::anyhow!("runtime probe returned no resource_version"))?;
                let profile = complete_native_detection(&result, resource_version)?;
                raw["profile_complete"] = serde_json::json!(true);
                raw["resource_version"] = serde_json::json!(profile.runtime.resource_version);
                if let Some(manifest) = &profile.manifest_source {
                    raw["manifest_variant"] = serde_json::json!(manifest.variant);
                    raw["manifest_url"] = serde_json::json!(manifest.url);
                    raw["manifest"] = serde_json::json!({
                        "sha256": manifest.sha256,
                        "size": manifest.size,
                    });
                }
                profile
            };
            (profile, Some(raw))
        }
    };
    let body = serde_json::to_string_pretty(&profile)? + "\n";
    write_new_file(output, body.as_bytes())?;
    if let Some(raw_output) = raw_output {
        if let Some(raw) = raw {
            let body = serde_json::to_string_pretty(&raw)? + "\n";
            write_new_file(raw_output, body.as_bytes())?;
        } else {
            eprintln!("raw output is available only with --engine native");
        }
    }
    println!(
        "profile={} profile_id={}",
        output.display(),
        profile.profile_id()
    );
    Ok(())
}

fn probe_profile(
    file: &Path,
    api_base: &str,
    token_file: &Path,
    output: Option<&Path>,
    json: bool,
) -> Result<()> {
    let profile = RuntimeProfile::load(file)?;
    let probe = probe_cygames_via_wbarts(&profile, api_base, token_file)?;
    let body = serde_json::to_string_pretty(&probe)? + "\n";
    if let Some(output) = output {
        write_new_file(output, body.as_bytes())?;
    }
    if json || output.is_none() {
        print!("{body}");
    } else {
        println!(
            "probe={:?} operator={} resource_version={} app_version={}",
            probe.status,
            probe.operator.as_str(),
            probe.resource_version.as_deref().unwrap_or(""),
            probe.app_version.as_deref().unwrap_or("")
        );
    }
    if probe.status == RuntimeProbeStatus::Failed {
        bail!(
            "runtime probe failed: {}",
            probe.error.as_deref().unwrap_or("unknown error")
        );
    }
    Ok(())
}

fn publish_profile(file: &Path, api_base: &str, token_file: &Path) -> Result<()> {
    let profile = RuntimeProfile::load(file)?;
    let value = publish_runtime_profile(&profile, api_base, token_file)?;
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}

fn list_profiles(operator: &str, api_base: &str, token_file: &Path) -> Result<()> {
    let operator = Operator::from_str(operator).map_err(anyhow::Error::msg)?;
    let value = list_runtime_profiles(operator, api_base, token_file)?;
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}

fn active_profile(operator: &str, api_base: &str, token_file: &Path) -> Result<()> {
    let operator = Operator::from_str(operator).map_err(anyhow::Error::msg)?;
    let value = get_active_runtime_profile(operator, api_base, token_file)?;
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}

fn activate_profile(profile_id: &str, api_base: &str, token_file: &Path) -> Result<()> {
    let value = activate_runtime_profile(profile_id, api_base, token_file)?;
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}

fn inspect_credential(operator: &str, path: &Path, json: bool) -> Result<()> {
    let operator = Operator::from_str(operator).map_err(anyhow::Error::msg)?;
    let summary = match operator {
        Operator::Cygames => inspect_cygames_systemd(path)?,
        Operator::Netease => inspect_netease_auth(path)?,
    };
    if json {
        println!("{}", serde_json::to_string_pretty(&summary)?);
    } else {
        println!(
            "operator={} kind={:?} valid={} fingerprint={}",
            summary.operator.as_str(),
            summary.kind,
            summary.valid,
            summary.fingerprint
        );
        for file in &summary.files {
            println!(
                "{:<16} exists={} readable={} private={}",
                file.name, file.exists, file.readable, file.private
            );
        }
        for issue in &summary.issues {
            println!("issue: {issue}");
        }
    }
    if !summary.valid {
        bail!("credential validation failed");
    }
    Ok(())
}

fn validate_probe(file: &Path, json: bool) -> Result<()> {
    let probe = RuntimeProbe::load(file)?;
    let issues = probe.validate();
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "ok": issues.is_empty(),
                "file": file,
                "subject_type": probe.subject_type,
                "subject_id": probe.subject_id,
                "issues": issues,
            }))?
        );
    } else if issues.is_empty() {
        println!("valid runtime probe: {}", file.display());
    } else {
        for issue in &issues {
            eprintln!("- {issue}");
        }
    }
    if !issues.is_empty() {
        bail!("runtime probe has {} validation issue(s)", issues.len());
    }
    Ok(())
}

fn status_label(status: CheckStatus) -> &'static str {
    match status {
        CheckStatus::Ok => "OK",
        CheckStatus::Warn => "WARN",
        CheckStatus::Error => "ERROR",
    }
}

fn write_new_file(path: &Path, content: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension(format!(
        "{}.tmp-{}",
        path.extension()
            .and_then(|value| value.to_str())
            .unwrap_or(""),
        std::process::id()
    ));
    std::fs::write(&temporary, content)?;
    std::fs::rename(&temporary, path)?;
    Ok(())
}
