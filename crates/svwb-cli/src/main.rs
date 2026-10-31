use std::{
    path::{Path, PathBuf},
    str::FromStr,
};

use anyhow::{Result, bail};
use catalog_release::{
    CatalogDiff, CatalogRelease, CatalogSource, pull as pull_catalog_release,
    stage as stage_catalog_release,
};
use clap::{Parser, Subcommand};
use svwb_core::{
    Operator, RuntimeProbe, RuntimeProbeStatus, RuntimeProfile,
    credential::{inspect_cygames_systemd, inspect_netease_auth},
    doctor::{CheckStatus, run as run_doctor},
    native_detect::{NativeDetectorOptions, complete_native_detection, detect_with_native},
    netease_detect::{NeteaseDetectorOptions, detect as detect_netease},
    probe::{
        activate_runtime_profile, get_active_runtime_profile, list_runtime_profiles,
        probe_runtime_draft_via_wbarts, probe_runtime_profile_via_wbarts, publish_runtime_profile,
        runtime_profile_from_dumper,
    },
};

#[derive(Parser)]
#[command(name = "svwb", version, about = "SVWB data and protocol toolkit")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
#[allow(clippy::large_enum_variant)]
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
    /// Build and compare WBU catalog releases
    Catalog {
        #[command(subcommand)]
        command: CatalogCommand,
    },
}

#[derive(Subcommand)]
#[allow(clippy::large_enum_variant)]
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
    /// Detect a local Cygames or Netease client
    Detect {
        #[arg(long, default_value = "cygames")]
        operator: String,
        #[arg(long)]
        game_root: Option<PathBuf>,
        #[arg(long)]
        data_root: Option<PathBuf>,
        #[arg(long)]
        variant: Option<String>,
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

#[derive(Subcommand)]
enum CatalogCommand {
    /// Build a content-addressed catalog release manifest
    Build {
        #[arg(long, default_value = ".")]
        root: PathBuf,
        #[arg(long)]
        output: Option<PathBuf>,
        #[arg(long)]
        source_file: Option<PathBuf>,
        #[arg(long)]
        dry_run: bool,
    },
    /// Verify a catalog release against its source directory
    Verify {
        #[arg(long, default_value = ".")]
        root: PathBuf,
        #[arg(long)]
        manifest: PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// Compare two catalog release manifests
    Diff {
        #[arg(long)]
        old: PathBuf,
        #[arg(long)]
        new: PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// Materialize a manifest into content-addressed objects
    Stage {
        #[arg(long, default_value = ".")]
        root: PathBuf,
        #[arg(long)]
        manifest: PathBuf,
        #[arg(long)]
        output: PathBuf,
        #[arg(long)]
        previous: Option<PathBuf>,
    },
    /// Generate the mutable latest.json pointer for a release
    Pointer {
        #[arg(long)]
        manifest: PathBuf,
        #[arg(long)]
        output: PathBuf,
    },
    /// Download a release into its logical data paths
    Pull {
        #[arg(long)]
        manifest: PathBuf,
        #[arg(long, default_value = "https://wbarts-catalog.cn-nb1.rains3.com")]
        base_url: String,
        #[arg(long, default_value = ".")]
        root: PathBuf,
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
                game_root,
                data_root,
                variant.as_deref(),
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
        Command::Catalog { command } => match command {
            CatalogCommand::Build {
                root,
                output,
                source_file,
                dry_run,
            } => build_catalog(&root, output.as_deref(), source_file.as_deref(), dry_run),
            CatalogCommand::Verify {
                root,
                manifest,
                json,
            } => verify_catalog(&root, &manifest, json),
            CatalogCommand::Diff { old, new, json } => diff_catalog(&old, &new, json),
            CatalogCommand::Stage {
                root,
                manifest,
                output,
                previous,
            } => stage_catalog(&root, &manifest, &output, previous.as_deref()),
            CatalogCommand::Pointer { manifest, output } => catalog_pointer(&manifest, &output),
            CatalogCommand::Pull {
                manifest,
                base_url,
                root,
            } => pull_catalog(&manifest, &base_url, &root),
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
    game_root: Option<PathBuf>,
    data_root: Option<PathBuf>,
    variant: Option<&str>,
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
    if operator == Operator::Netease {
        let result = detect_netease(&NeteaseDetectorOptions {
            game_root,
            variant: variant.unwrap_or("Chs").into(),
            timeout: std::time::Duration::from_secs(60),
        })?;
        let body = serde_json::to_string_pretty(&result.profile)? + "\n";
        write_new_file(output, body.as_bytes())?;
        if let Some(raw_output) = raw_output {
            let body = serde_json::to_string_pretty(&result.raw)? + "\n";
            write_new_file(raw_output, body.as_bytes())?;
        }
        println!(
            "profile={} profile_id={}",
            output.display(),
            result.profile.profile_id()
        );
        return Ok(());
    }
    let variant = variant.unwrap_or("Cht");
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| anyhow::anyhow!("HOME is not set"))?;
    let game_root =
        game_root.unwrap_or_else(|| home.join(".local/share/Steam/steamapps/common/ShadowverseWB"));
    let data_root = data_root.unwrap_or_else(|| home.join("Data/svwb_dumper"));
    let il2cpp_dumper_dir =
        il2cpp_dumper_dir.unwrap_or_else(|| home.join(".local/share/svwb_dumper/Il2CppDumper"));
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
        let probe = probe_runtime_draft_via_wbarts(&result.draft, api_base, token_file)?;
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
    let body = serde_json::to_string_pretty(&profile)? + "\n";
    write_new_file(output, body.as_bytes())?;
    if let Some(raw_output) = raw_output {
        let body = serde_json::to_string_pretty(&raw)? + "\n";
        write_new_file(raw_output, body.as_bytes())?;
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
    let probe = probe_runtime_profile_via_wbarts(&profile, api_base, token_file)?;
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

fn build_catalog(
    root: &Path,
    output: Option<&Path>,
    source_file: Option<&Path>,
    dry_run: bool,
) -> Result<()> {
    let sources: Vec<CatalogSource> = match source_file {
        Some(path) => serde_json::from_str(
            &std::fs::read_to_string(path)
                .map_err(|error| anyhow::anyhow!("read {}: {error}", path.display()))?,
        )
        .map_err(|error| anyhow::anyhow!("parse {}: {error}", path.display()))?,
        None => Vec::new(),
    };
    let release = CatalogRelease::build_now(root, sources)?;
    if dry_run {
        println!("{}", serde_json::to_string_pretty(&release.summary())?);
        return Ok(());
    }
    let output =
        output.ok_or_else(|| anyhow::anyhow!("--output is required unless --dry-run is set"))?;
    let data_root = root
        .canonicalize()
        .map_err(|error| anyhow::anyhow!("resolve {}: {error}", root.display()))?
        .join("data");
    let output_absolute = std::path::absolute(output)
        .map_err(|error| anyhow::anyhow!("resolve {}: {error}", output.display()))?;
    if output_absolute.starts_with(data_root) {
        bail!("catalog manifest output must be outside the data directory");
    }
    let body = serde_json::to_string_pretty(&release)? + "\n";
    write_new_file(output, body.as_bytes())?;
    println!(
        "catalog={} release_id={} files={} bytes={}",
        output.display(),
        release.release_id,
        release.files.len(),
        release.files.iter().map(|file| file.size).sum::<u64>()
    );
    Ok(())
}

fn verify_catalog(root: &Path, manifest: &Path, json: bool) -> Result<()> {
    let release = CatalogRelease::load(manifest)?;
    release.verify_root(root)?;
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "ok": true,
                "release_id": release.release_id,
                "file_count": release.files.len(),
            }))?
        );
    } else {
        println!(
            "valid catalog release: release_id={} files={}",
            release.release_id,
            release.files.len()
        );
    }
    Ok(())
}

fn diff_catalog(old: &Path, new: &Path, json: bool) -> Result<()> {
    let old = CatalogRelease::load(old)?;
    let new = CatalogRelease::load(new)?;
    let diff = CatalogDiff::between(&old, &new);
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "old_release_id": old.release_id,
                "new_release_id": new.release_id,
                "unchanged": old.release_id == new.release_id,
                "added": diff.added,
                "removed": diff.removed,
                "changed": diff.changed,
            }))?
        );
    } else {
        println!(
            "old={} new={} added={} removed={} changed={}",
            old.release_id,
            new.release_id,
            diff.added.len(),
            diff.removed.len(),
            diff.changed.len()
        );
        for path in &diff.added {
            println!("A {path}");
        }
        for path in &diff.removed {
            println!("D {path}");
        }
        for path in &diff.changed {
            println!("M {path}");
        }
    }
    Ok(())
}

fn stage_catalog(
    root: &Path,
    manifest: &Path,
    output: &Path,
    previous: Option<&Path>,
) -> Result<()> {
    let release = CatalogRelease::load(manifest)?;
    let previous_release = previous.map(CatalogRelease::load).transpose()?;
    let summary = stage_catalog_release(root, &release, previous_release.as_ref(), output)?;
    println!(
        "staged={} release_id={} copied={} reused={} skipped={} bytes={}",
        output.display(),
        release.release_id,
        summary.copied,
        summary.reused,
        summary.skipped,
        summary.copied_bytes
    );
    Ok(())
}

fn catalog_pointer(manifest: &Path, output: &Path) -> Result<()> {
    let release = CatalogRelease::load(manifest)?;
    let pointer = release.pointer();
    pointer.validate()?;
    let body = serde_json::to_string_pretty(&pointer)? + "\n";
    write_new_file(output, body.as_bytes())?;
    println!(
        "pointer={} release_id={} manifest={}",
        output.display(),
        pointer.release_id,
        pointer.manifest
    );
    Ok(())
}

fn pull_catalog(manifest: &Path, base_url: &str, root: &Path) -> Result<()> {
    let release = CatalogRelease::load(manifest)?;
    let summary = pull_catalog_release(
        root,
        &release,
        base_url,
        std::time::Duration::from_secs(120),
    )?;
    println!(
        "release_id={} downloaded={} reused={} bytes={}",
        release.release_id, summary.downloaded, summary.reused, summary.downloaded_bytes
    );
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
