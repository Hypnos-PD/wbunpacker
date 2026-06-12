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
        /// 强制重新下载 mastermemory.bytes（即使已缓存）
        #[arg(short = 'F', long)]
        force: bool,
    },

    /// 提取音频: 解密 Wwise 映射 → 解析 AKPK → WEM 提取 → WAV 输出
    Audio {
        /// 同时转码 MP3
        #[arg(long)]
        mp3: bool,
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

        Command::Master { variant, force } => {
            let cfg = config::load()?;

            for v in expand_variants(&variant) {
                let manifest_path = format!("{}/manifests/json/assetbundle.{}.manifest.json", cfg.data_dir, v);
                let json = std::fs::read_to_string(&manifest_path)
                    .with_context(|| format!("请先运行: wbu manifest -v {v} --format json"))?;
                let m: manifest::Manifest = serde_json::from_str(&json)?;

                let master_entry = m.raw_assets.iter()
                    .find(|r| r.name == MASTER_BYTES_NAME)
                    .ok_or_else(|| anyhow::anyhow!("manifest 中未找到 {}", MASTER_BYTES_NAME))?;

                let blobs_raw = std::path::Path::new(&cfg.data_dir).join("blobs").join("raw");
                let blob_path = asset::blob_path(&blobs_raw, "", &master_entry.hash);

                // --force: 删除旧缓存，强制重下
                if force && blob_path.exists() {
                    std::fs::remove_file(&blob_path)?;
                    println!("[{v}] 已删除缓存，重新下载...");
                }

                if !blob_path.exists() {
                    println!("[{v}] 下载 {} ...", MASTER_BYTES_NAME);
                    asset::download_asset(&master_entry.hash, &cfg.asset_bundle_address, &blob_path).await?;
                }

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

        Command::Audio { mp3 } => {
            let cfg = config::load()?;
            let vgmstream_path = std::path::Path::new(&cfg.vgmstream_path);

            // 用第一个变体找 WwiseIdMapping（所有变体指向同一文件）
            let first_variant = "Chs";
            let mapping_path = std::path::Path::new(&cfg.data_dir)
                .join("variants").join(first_variant).join("raw-assets")
                .join("sound/WwiseIdMapping.bytes");
            let mapping_data = std::fs::read(&mapping_path)
                .with_context(|| format!("请先运行 wbu asset batch -v {first_variant}"))?;

            let pck_dir = std::path::Path::new(&cfg.data_dir)
                .join("variants").join(first_variant).join("raw-assets").join("sound");
            let output_dir = std::path::Path::new(&cfg.data_dir)
                .join("exports").join("audio");

            println!("扫描 {}", pck_dir.display());
            let stats = audio::extract_all(&pck_dir, &output_dir, &mapping_data, vgmstream_path)?;

            println!("pck: {} | WAV: {} | 跳过: {} | 失败: {}",
                stats.pck_files, stats.wav_output, stats.skipped, stats.failed);

            if mp3 {
                let mp3_dir = std::path::Path::new(&cfg.data_dir)
                    .join("exports").join("audio-mp3");
                println!("转码 MP3 → {}", mp3_dir.display());
                let n = audio::convert_dir_to_mp3(&output_dir, &mp3_dir, &cfg.ffmpeg_path)?;
                println!("MP3 完成: {n} 个文件");
            }
        }
        Command::Texture { .. } => todo!("texture"),
        Command::Metadb { .. } => todo!("metadb"),
        Command::Localize { .. } => todo!("localize"),
    }

    Ok(())
}
