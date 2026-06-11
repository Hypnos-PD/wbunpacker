mod config;

use clap::{Parser, Subcommand};

// ============================================================================
// CLI 顶层结构
// ============================================================================

/// Shadowverse: Worlds Beyond 资源解包工具。
#[derive(Parser)]
#[command(name = "wbu", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// 查询服务器当前资源版本号 (骨架)
    Version {
        #[arg(short, long, default_value = "json")]
        format: String,
    },

    /// 下载并解析资源清单
    Manifest {
        /// 资源版本号，如 "100000000000"；不指定则使用配置文件中的 DefaultVersion
        #[arg(short = 'V', long)]
        version: Option<String>,
        /// 语言变体: Chs/Eng/Jpn/Kor/Cht
        #[arg(short, long, default_value = "Chs")]
        variant: String,
        /// 输出格式: raw(二进制) 或 json
        #[arg(short, long, default_value = "raw")]
        format: String,
    },

    /// 下载和解密 AssetBundle (骨架)
    Asset {
        #[command(subcommand)]
        sub: AssetCmd,
    },

    /// 导出主数据表 (骨架)
    Master {
        #[arg(short, long)]
        manifest: Option<String>,
        #[arg(short, long)]
        cache: Option<String>,
    },

    /// 提取卡牌语音 (骨架)
    Audio {
        #[command(subcommand)]
        sub: AudioCmd,
    },

    /// 处理卡图纹理 (骨架)
    Texture {
        #[arg(long)]
        asset_studio: Option<String>,
    },

    /// 解密客户端 meta.db (骨架)
    Metadb {
        path: String,
        #[arg(short, long)]
        output: Option<String>,
    },

    /// 导出本地化文本 (骨架)
    Localize {
        #[arg(short, long)]
        file: String,
    },
}

#[derive(Subcommand)]
enum AssetCmd {
    Download { name: String, #[arg(short, long)] manifest: String },
    Decrypt { #[arg(short, long)] file: String, #[arg(short, long)] manifest: String },
    Batch { #[arg(short, long)] manifest: String, #[arg(short, long, default_value = "8")] concurrency: usize },
}

#[derive(Subcommand)]
enum AudioCmd {
    WwiseMap { #[arg(short, long)] mapping: String, #[arg(short, long)] pck_root: String, #[arg(long)] wwiser_path: String, #[arg(short, long, default_value = "data/exports/audio/wem_mapping.json")] output: String },
    Extract { #[arg(short, long)] pck_root: Option<String>, #[arg(short, long)] force: bool },
}

// ============================================================================
// 主函数
// ============================================================================

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "wbu=info".into()),
        )
        .init();

    let cli = Cli::parse();

    match cli.command {
        Command::Manifest { version, variant, format } => {
            let cfg = config::load()?;
            let version = version.unwrap_or(cfg.default_version);
            let raw = manifest::download(&version, &variant, &cfg.manifest_address).await?;

            match format.as_str() {
                "json" => {
                    let m = manifest::parse(&raw)?;
                    let json = manifest::to_json(&m)?;
                    let out = format!("data/manifests/json/assetbundle.{}.manifest.json", variant);
                    std::fs::create_dir_all("data/manifests/json")?;
                    std::fs::write(&out, json)?;
                    println!("{}", out);
                }
                _ => {
                    let out = format!("data/manifests/raw/assetbundle.{}.manifest", variant);
                    std::fs::create_dir_all("data/manifests/raw")?;
                    std::fs::write(&out, &raw)?;
                    println!("{}", out);
                }
            }
        }

        Command::Version { .. } => todo!("version"),
        Command::Asset { .. } => todo!("asset"),
        Command::Master { .. } => todo!("master"),
        Command::Audio { .. } => todo!("audio"),
        Command::Texture { .. } => todo!("texture"),
        Command::Metadb { .. } => todo!("metadb"),
        Command::Localize { .. } => todo!("localize"),
    }

    Ok(())
}