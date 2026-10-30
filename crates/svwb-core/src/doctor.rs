use std::{
    env,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::native_detect::find_game_process;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DoctorReport {
    pub ok: bool,
    pub config: String,
    pub checks: Vec<DoctorCheck>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DoctorCheck {
    pub name: String,
    pub status: CheckStatus,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CheckStatus {
    Ok,
    Warn,
    Error,
}

pub fn run(config: Option<PathBuf>) -> DoctorReport {
    let config = config.unwrap_or_else(default_config_path);
    let mut checks = Vec::new();

    let parsed = match std::fs::read_to_string(&config) {
        Ok(raw) => match toml::from_str::<toml::Value>(&raw) {
            Ok(value) => {
                checks.push(DoctorCheck {
                    name: "config".into(),
                    status: CheckStatus::Ok,
                    message: config.display().to_string(),
                });
                Some(value)
            }
            Err(error) => {
                checks.push(DoctorCheck {
                    name: "config".into(),
                    status: CheckStatus::Error,
                    message: format!("{}: {error}", config.display()),
                });
                None
            }
        },
        Err(error) => {
            checks.push(DoctorCheck {
                name: "config".into(),
                status: CheckStatus::Error,
                message: format!("{}: {error}", config.display()),
            });
            None
        }
    };

    for command in ["dotnet", "ffmpeg", "vgmstream-cli", "AssetStudioModCLI"] {
        checks.push(check_command(command, command));
    }

    if let Some(home) = env::var_os("HOME").map(PathBuf::from) {
        let dumper = home.join(".local/share/svwb_dumper/Il2CppDumper/Il2CppDumper.dll");
        checks.push(file_check("il2cpp_dumper", &dumper));
    }

    match find_game_process("ShadowverseWB.exe") {
        Ok(process) => checks.push(DoctorCheck {
            name: "process.ShadowverseWB.exe".into(),
            status: CheckStatus::Ok,
            message: format!("pid={} rss={} KiB", process.pid, process.rss_kib),
        }),
        Err(error) => checks.push(DoctorCheck {
            name: "process.ShadowverseWB.exe".into(),
            status: CheckStatus::Warn,
            message: error.to_string(),
        }),
    }

    if let Some(config) = parsed {
        for key in [
            "data_dir",
            "manifest_address",
            "default_version",
            "asset_bundle_address",
            "asset_bundle_base_keys",
            "asset_studio_path",
        ] {
            let ok = config
                .get(key)
                .and_then(toml::Value::as_str)
                .is_some_and(|value| !value.trim().is_empty());
            checks.push(DoctorCheck {
                name: format!("config.{key}"),
                status: if ok {
                    CheckStatus::Ok
                } else {
                    CheckStatus::Error
                },
                message: if ok {
                    "set".into()
                } else {
                    "missing or empty".into()
                },
            });
        }

        for key in ["asset_studio_path", "vgmstream_path", "ffmpeg_path"] {
            if let Some(value) = config.get(key).and_then(toml::Value::as_str)
                && !value.trim().is_empty()
            {
                checks.push(check_command(key, value));
            }
        }
    }

    let ok = checks
        .iter()
        .all(|check| check.status != CheckStatus::Error);
    DoctorReport {
        ok,
        config: config.display().to_string(),
        checks,
    }
}

fn default_config_path() -> PathBuf {
    env::var_os("WBU_CONFIG")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("config/Config.local.toml"))
}

fn check_command(name: &str, command: &str) -> DoctorCheck {
    let path = Path::new(command);
    let found = if path.components().count() > 1 {
        path.is_file()
    } else {
        env::var_os("PATH")
            .map(|value| {
                env::split_paths(&value).any(|directory| directory.join(command).is_file())
            })
            .unwrap_or(false)
    };
    DoctorCheck {
        name: format!("command.{name}"),
        status: if found {
            CheckStatus::Ok
        } else {
            CheckStatus::Warn
        },
        message: if found {
            command.into()
        } else {
            "not found".into()
        },
    }
}

fn file_check(name: &str, path: &Path) -> DoctorCheck {
    DoctorCheck {
        name: format!("file.{name}"),
        status: if path.is_file() {
            CheckStatus::Ok
        } else {
            CheckStatus::Warn
        },
        message: path.display().to_string(),
    }
}
