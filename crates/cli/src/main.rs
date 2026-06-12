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
        #[arg(short = 'V', long)]
        version: Option<String>,
        #[arg(short, long)]
        variant: String,
        #[arg(short, long, default_value = "raw")]
        format: String,
    },

    /// 下载和解密 AssetBundle
    Asset {
        #[command(subcommand)]
        sub: AssetCmd,
    },

    /// 导出主数据表 (MasterMemory)
    Master {
        /// 语言变体: Chs/Eng/Jpn/Kor/Cht，或 all
        #[arg(short, long, default_value = "Chs")]
        variant: String,
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
    Download { name: String, #[arg(short, long, default_value = "Chs")] variant: String },
    Decrypt { #[arg(short = 'f', long)] file: String, #[arg(short = 'n', long)] name: String, #[arg(short, long)] manifest: String },
    Batch { #[arg(short, long, default_value = "Chs")] variant: String, #[arg(short = 'c', long, default_value = "8")] concurrency: usize },
}

#[derive(Subcommand)]
enum AudioCmd {
    WwiseMap { #[arg(short, long)] mapping: String, #[arg(short, long)] pck_root: String, #[arg(long)] wwiser_path: String, #[arg(short, long, default_value = "data/exports/audio/wem_mapping.json")] output: String },
    Extract { #[arg(short, long)] pck_root: Option<String>, #[arg(short, long)] force: bool },
}

// ============================================================================
// 常量
// ============================================================================

const ALL_VARIANTS: &[&str] = &["Chs", "Eng", "Jpn", "Kor", "Cht"];
const MASTER_BYTES_NAME: &str = "Master/mastermemory.bytes";

// ============================================================================
// 工具函数
// ============================================================================

fn expand_variants(variant: &str) -> Vec<String> {
    if variant.eq_ignore_ascii_case("all") {
        ALL_VARIANTS.iter().map(|v| v.to_string()).collect()
    } else {
        vec![variant.to_string()]
    }
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

            for v in expand_variants(&variant) {
                let raw = manifest::download(&version, &v, &cfg.manifest_address).await?;
                let manifests_dir = format!("{}/manifests", cfg.data_dir);

                match format.as_str() {
                    "json" => {
                        let m = manifest::parse(&raw)?;
                        let json = manifest::to_json(&m)?;
                        let out = format!("{}/json/assetbundle.{}.manifest.json", manifests_dir, v);
                        std::fs::create_dir_all(format!("{}/json", manifests_dir))?;
                        std::fs::write(&out, json)?;
                        println!("{}", out);
                    }
                    _ => {
                        let out = format!("{}/raw/assetbundle.{}.manifest", manifests_dir, v);
                        std::fs::create_dir_all(format!("{}/raw", manifests_dir))?;
                        std::fs::write(&out, &raw)?;
                        println!("{}", out);
                    }
                }
            }
        }

        Command::Version { .. } => todo!("version"),

        Command::Asset { sub } => match sub {
            AssetCmd::Batch { variant, concurrency } => {
                let cfg = config::load()?;

                for v in expand_variants(&variant) {
                    let manifest_path = format!("{}/manifests/json/assetbundle.{}.manifest.json", cfg.data_dir, v);
                    let json = std::fs::read_to_string(&manifest_path)
                        .with_context(|| format!("请先运行: wbu manifest -v {v} --format json"))?;
                    let m: manifest::Manifest = serde_json::from_str(&json)?;

                    let blobs_dir = std::path::Path::new(&cfg.data_dir).join("blobs");
                    let variant_dir = std::path::Path::new(&cfg.data_dir).join("variants").join(&v);

                    let stats = asset::batch_download(
                        &m, &cfg.asset_bundle_address, &cfg.asset_bundle_base_keys,
                        concurrency, &blobs_dir, &variant_dir,
                    ).await?;

                    println!("[{v}] 完成: {} | 跳过: {} | 失败: {} | 硬链接: {} | 下载: {:.1} MB",
                        stats.done, stats.skipped, stats.failed, stats.hardlinks,
                        stats.downloaded_bytes as f64 / 1024.0 / 1024.0);
                }
            }
            AssetCmd::Download { name, variant } => {
                let cfg = config::load()?;
                let blobs_raw = std::path::Path::new(&cfg.data_dir).join("blobs").join("raw");

                for v in expand_variants(&variant) {
                    let manifest_path = format!("{}/manifests/json/assetbundle.{}.manifest.json", cfg.data_dir, v);
                    let json = std::fs::read_to_string(&manifest_path)
                        .with_context(|| format!("请先运行: wbu manifest -v {v} --format json"))?;
                    let m: manifest::Manifest = serde_json::from_str(&json)?;
                    let variant_links = std::path::Path::new(&cfg.data_dir).join("variants").join(&v);

                    if let Some(asset) = m.assets.iter().find(|a| a.name == name) {
                        let blob_path = asset::blob_path(&blobs_raw, "", &asset.hash);
                        let result = asset::download_asset(&asset.hash, &cfg.asset_bundle_address, &blob_path).await?;
                        println!("[{v}] 下载: {} ({} bytes)", result.path, result.size);
                        let link_path = variant_links.join("raw").join(&asset.name);
                        if asset::hardlink_or_skip(&blob_path, &link_path)? {
                            println!("[{v}] 硬链接: {}", link_path.display());
                        }
                    } else if let Some(raw) = m.raw_assets.iter().find(|r| r.name == name) {
                        let blob_path = asset::blob_path(&blobs_raw, "", &raw.hash);
                        let result = asset::download_asset(&raw.hash, &cfg.asset_bundle_address, &blob_path).await?;
                        println!("[{v}] 下载: {} ({} bytes)", result.path, result.size);
                        let link_path = variant_links.join("raw-assets").join(&raw.name);
                        if asset::hardlink_or_skip(&blob_path, &link_path)? {
                            println!("[{v}] 硬链接: {}", link_path.display());
                        }
                    } else {
                        println!("[{v}] 未找到资源: {name}");
                    }
                }
            }
            AssetCmd::Decrypt { file, name, manifest } => {
                let cfg = config::load()?;
                let json = std::fs::read_to_string(&manifest)?;
                let m: manifest::Manifest = serde_json::from_str(&json)?;
                let asset = m.assets.iter().find(|a| a.name == name)
                    .ok_or_else(|| anyhow::anyhow!("manifest 中未找到资源: {name}"))?;
                let blobs_decrypted = std::path::Path::new(&cfg.data_dir).join("blobs").join("decrypted");
                let output = asset::blob_path(&blobs_decrypted, "", &asset.hash);
                asset::decrypt_file(std::path::Path::new(&file), &output, &cfg.asset_bundle_base_keys, asset.key)?;
                println!("解密完成: {}", output.display());
            }
        },

        Command::Master { variant } => {
            let cfg = config::load()?;

            for v in expand_variants(&variant) {
                // 1. 加载 manifest，找到 mastermemory.bytes 的 hash
                let manifest_path = format!("{}/manifests/json/assetbundle.{}.manifest.json", cfg.data_dir, v);
                let json = std::fs::read_to_string(&manifest_path)
                    .with_context(|| format!("请先运行: wbu manifest -v {v} --format json"))?;
                let m: manifest::Manifest = serde_json::from_str(&json)?;

                let master_entry = m.raw_assets.iter()
                    .find(|r| r.name == MASTER_BYTES_NAME)
                    .ok_or_else(|| anyhow::anyhow!("manifest 中未找到 {MASTER_BYTES_NAME}"))?;

                // 2. 确保文件已下载（blob 存储）
                let blobs_raw = std::path::Path::new(&cfg.data_dir).join("blobs").join("raw");
                let blob_path = asset::blob_path(&blobs_raw, "", &master_entry.hash);

                if !blob_path.exists() {
                    println!("[{v}] 下载 {MASTER_BYTES_NAME}...");
                    asset::download_asset(&master_entry.hash, &cfg.asset_bundle_address, &blob_path).await?;
                }

                // 3. 读取并导出
                let raw = std::fs::read(&blob_path)
                    .with_context(|| format!("无法读取: {}", blob_path.display()))?;

                let output_dir = std::path::Path::new(&cfg.data_dir)
                    .join("exports").join("master-data").join(&v);

                println!("[{v}] 导出到 {} ...", output_dir.display());
                let results = master_data::export_all(&raw, &output_dir)?;

                let total_rows: usize = results.iter().map(|r| r.rows).sum();
                println!("[{v}] 完成: {} 个表, {} 行", results.len(), total_rows);
            }
        }

        Command::Audio { .. } => todo!("audio"),
        Command::Texture { .. } => todo!("texture"),
        Command::Metadb { .. } => todo!("metadb"),
        Command::Localize { .. } => todo!("localize"),
    }

    Ok(())
}