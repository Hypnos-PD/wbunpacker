mod config;

use anyhow::Context;
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
        #[arg(short, long)]
        variant: String,
        /// 输出格式: raw(二进制) 或 json
        #[arg(short, long, default_value = "raw")]
        format: String,
    },

    /// 下载和解密 AssetBundle
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
    /// 下载单个 AssetBundle 或 RawAsset（按 manifest 中的路径名）
    Download {
        /// 资源路径名 (manifest assets[].name)
        name: String,
        /// 语言变体
        #[arg(short, long, default_value = "Chs")]
        variant: String,
    },
    /// 解密已下载的加密 AssetBundle 文件
    Decrypt {
        /// 加密文件路径
        #[arg(short = 'f', long)]
        file: String,
        /// manifest 中的资源名 (assets[].name)，用于查找解密 key
        #[arg(short = 'n', long)]
        name: String,
        /// manifest JSON 文件路径
        #[arg(short, long)]
        manifest: String,
    },
    /// 批量下载并解密所有 AssetBundle（支持断点续传）
    Batch {
        /// 语言变体
        #[arg(short, long, default_value = "Chs")]
        variant: String,
        /// 并发下载数
        #[arg(short = 'c', long, default_value = "8")]
        concurrency: usize,
    },
}

#[derive(Subcommand)]
enum AudioCmd {
    WwiseMap {
        #[arg(short, long)]
        mapping: String,
        #[arg(short, long)]
        pck_root: String,
        #[arg(long)]
        wwiser_path: String,
        #[arg(short, long, default_value = "data/exports/audio/wem_mapping.json")]
        output: String,
    },
    Extract {
        #[arg(short, long)]
        pck_root: Option<String>,
        #[arg(short, long)]
        force: bool,
    },
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

            let manifests_dir = format!("{}/manifests", cfg.data_dir);

            match format.as_str() {
                "json" => {
                    let m = manifest::parse(&raw)?;
                    let json = manifest::to_json(&m)?;
                    let out = format!("{}/json/assetbundle.{}.manifest.json", manifests_dir, variant);
                    std::fs::create_dir_all(format!("{}/json", manifests_dir))?;
                    std::fs::write(&out, json)?;
                    println!("{}", out);
                }
                _ => {
                    let out = format!("{}/raw/assetbundle.{}.manifest", manifests_dir, variant);
                    std::fs::create_dir_all(format!("{}/raw", manifests_dir))?;
                    std::fs::write(&out, &raw)?;
                    println!("{}", out);
                }
            }
        }

        Command::Version { .. } => todo!("version"),
        Command::Asset { sub } => match sub {
            AssetCmd::Batch { variant, concurrency } => {
                let cfg = config::load()?;
                let manifest_path = format!("{}/manifests/json/assetbundle.{}.manifest.json", cfg.data_dir, variant);
                let json = std::fs::read_to_string(&manifest_path)
                    .with_context(|| format!("请先运行: wbu manifest -v {variant} --format json"))?;
                let m: manifest::Manifest = serde_json::from_str(&json)?;

                let downloads_dir = format!("{}/downloads", cfg.data_dir);
                let stats = asset::batch_download(
                    &m,
                    &cfg.asset_bundle_address,
                    &cfg.asset_bundle_base_keys,
                    concurrency,
                    std::path::Path::new(&format!("{}/raw", downloads_dir)),
                    std::path::Path::new(&format!("{}/decrypted", downloads_dir)),
                    std::path::Path::new(&format!("{}/raw-assets", downloads_dir)),
                ).await?;

                println!("完成: {} | 跳过: {} | 失败: {} | 下载: {:.1} MB",
                    stats.done, stats.skipped, stats.failed,
                    stats.downloaded_bytes as f64 / 1024.0 / 1024.0);
            }
            AssetCmd::Download { name, variant } => {
                let cfg = config::load()?;
                let manifest_path = format!("{}/manifests/json/assetbundle.{}.manifest.json", cfg.data_dir, variant);
                let json = std::fs::read_to_string(&manifest_path)
                    .with_context(|| format!("请先运行: wbu manifest -v {variant} --format json"))?;
                let m: manifest::Manifest = serde_json::from_str(&json)?;

                // 查找 asset 或 raw_asset
                if let Some(asset) = m.assets.iter().find(|a| a.name == name) {
                    let dest = std::path::Path::new(&format!("{}/downloads/raw", cfg.data_dir)).join(&asset.name);
                    let result = asset::download_asset(&asset.hash, &cfg.asset_bundle_address, &dest).await?;
                    println!("下载完成: {} ({} bytes)", result.path, result.size);
                } else if let Some(raw) = m.raw_assets.iter().find(|r| r.name == name) {
                    let dest = std::path::Path::new(&format!("{}/downloads/raw-assets", cfg.data_dir)).join(&raw.name);
                    let result = asset::download_asset(&raw.hash, &cfg.asset_bundle_address, &dest).await?;
                    println!("下载完成: {} ({} bytes)", result.path, result.size);
                } else {
                    anyhow::bail!("未找到资源: {name}");
                }
            }
            AssetCmd::Decrypt { file, name, manifest } => {
                let cfg = config::load()?;
                let json = std::fs::read_to_string(&manifest)?;
                let m: manifest::Manifest = serde_json::from_str(&json)?;

                let asset = m.assets.iter()
                    .find(|a| a.name == name)
                    .ok_or_else(|| anyhow::anyhow!("manifest 中未找到资源: {name}"))?;

                let output_dir = format!("{}/downloads/decrypted", cfg.data_dir);
                let output = std::path::Path::new(&output_dir).join(&asset.name).with_extension("ab");

                if let Some(parent) = output.parent() {
                    std::fs::create_dir_all(parent)?;
                }

                let file_path = std::path::Path::new(&file);
                asset::decrypt_file(file_path, &output, &cfg.asset_bundle_base_keys, asset.key)?;
                println!("解密完成: {}", output.display());
            }
        },
        Command::Master { .. } => todo!("master"),
        Command::Audio { .. } => todo!("audio"),
        Command::Texture { .. } => todo!("texture"),
        Command::Metadb { .. } => todo!("metadb"),
        Command::Localize { .. } => todo!("localize"),
    }

    Ok(())
}