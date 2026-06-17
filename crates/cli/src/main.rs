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
        #[command(subcommand)]
        sub: Option<MasterCmd>,
    },

    /// 提取音频: 解密 Wwise 映射 → 解析 AKPK → WEM 提取 → WAV 输出
    Audio {
        /// 同时转码 MP3
        #[arg(long)]
        mp3: bool,
        #[command(subcommand)]
        sub: Option<AudioCmd>,
    },

    /// 纹理处理
    Texture {
        #[command(subcommand)]
        sub: TextureCmd,
    },

    /// 提取 HomeIllustration Spine 动画 → Web/Godot 可用格式（skel/atlas/png + config.json）
    HomeIllust {
        /// 同时复制语音文件（需先运行 wbu audio card）
        #[arg(long)]
        voices: bool,
        /// 额外导出 Unity 布局诊断数据，用于校准 Web 展示窗口
        #[arg(long)]
        layout_debug: bool,
        /// AssetStudio CLI 路径（覆盖配置文件）
        #[arg(long)]
        asset_studio: Option<String>,
    },

    /// 渲染完整卡牌图（MVP: 卡图 + Card2D 边框 + 名称/cost/攻血）
    Render {
        #[command(subcommand)]
        sub: RenderCmd,
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
    Download {
        name: String,
        #[arg(short, long, default_value = "Chs")]
        variant: String,
    },
    Decrypt {
        #[arg(short = 'f', long)]
        file: String,
        #[arg(short = 'n', long)]
        name: String,
        #[arg(short, long)]
        manifest: String,
    },
    Batch {
        #[arg(short, long, default_value = "Chs")]
        variant: String,
        #[arg(short = 'c', long, default_value = "8")]
        concurrency: usize,
    },
}

#[derive(Subcommand)]
enum MasterCmd {
    /// 生成 cards_full.json：合并 CardMaster + BaseCardMaster + SkillMaster + 5语言卡名
    Cards,
    /// 生成 pack_names.json：从多语言 MasterTextLabel 提取卡包名称
    Packs,
    /// 生成 emblems_full.json：合并 EmblemMaster + 多语言分类文本 + 关联卡名
    Emblems,
    /// 生成 stamps_full.json：合并 Stamp + StampCategory + 多语言名称
    Stamps,
}

#[derive(Subcommand)]
enum AudioCmd {
    /// 提取卡牌语音: 按语言/卡牌/槽位分发 → MP3 + voice_index.json
    Card,
}

#[derive(Subcommand)]
enum TextureCmd {
    /// 导出卡图纹理: AssetStudio 导出 → 按前缀分类 → 缩放至 848×1024
    Card {
        /// AssetStudio CLI 路径（覆盖配置文件）
        #[arg(long)]
        asset_studio: Option<String>,
        /// 跳过缩放步骤（默认会缩放至 848×1024）
        #[arg(long)]
        no_resize: bool,
    },
    /// 提取 PackIcons 图标: 从 IconItem AssetBundle 提取所有 pack-icons 图标（hash增量跳过）
    PackIcons {
        /// AssetStudio CLI 路径（覆盖配置文件）
        #[arg(long)]
        asset_studio: Option<String>,
    },
    /// 提取 Card2D 卡牌边框: UI/Card2D/frame2d_*.ab -> PNG
    CardFrames {
        /// AssetStudio CLI 路径（覆盖配置文件）
        #[arg(long)]
        asset_studio: Option<String>,
    },
    /// 提取 Home Illustration 静态展示图: UI/Home/utx_pict_Illustration_*.ab -> PNG
    HomeIllustPicts {
        /// AssetStudio CLI 路径（覆盖配置文件）
        #[arg(long)]
        asset_studio: Option<String>,
    },
    /// 提取徽章纹理: UI/Emblem/utx_tex_em_*.ab -> PNG
    Emblems {
        /// AssetStudio CLI 路径（覆盖配置文件）
        #[arg(long)]
        asset_studio: Option<String>,
    },
    /// 提取贴图纹理: UI/Stamp/stamp_*.ab -> PNG
    Stamps {
        /// AssetStudio CLI 路径（覆盖配置文件）
        #[arg(long)]
        asset_studio: Option<String>,
        /// 语言变体: Chs/Eng/Jpn/Kor/Cht，或 all
        #[arg(short, long, default_value = "Chs")]
        variant: String,
    },
}

#[derive(Subcommand)]
enum RenderCmd {
    /// 渲染单张/全部/自定义卡牌
    Card {
        /// card_id（不是 card_style_id）；可搭配 --name/--type/--cost 等覆盖参数
        #[arg(long)]
        id: Option<i64>,
        /// 渲染 cards_full.json 中所有卡牌
        #[arg(long)]
        all: bool,
        /// 自定义卡图路径；搭配 --id 时覆盖默认卡图，单独使用则为纯自定义渲染
        #[arg(long)]
        res: Option<String>,
        /// 覆盖卡名（可搭配 --id 或 --res）
        #[arg(long)]
        name: Option<String>,
        /// 覆盖卡牌种类：follower / spell / amulet（可搭配 --id 或 --res）
        #[arg(long, value_name = "KIND")]
        type_: Option<String>,
        /// 覆盖 cost（可搭配 --id 或 --res）
        #[arg(long)]
        cost: Option<i64>,
        /// 覆盖攻击力（仅 follower，可搭配 --id 或 --res）
        #[arg(long)]
        attack: Option<i64>,
        /// 覆盖体力（仅 follower，可搭配 --id 或 --res）
        #[arg(long, value_name = "LIFE")]
        life: Option<i64>,
        /// 覆盖职业图标编号 0-7（可搭配 --id 或 --res）
        #[arg(long)]
        class: Option<i64>,
        /// 覆盖稀有度：bronze / silver / gold / legend（可搭配 --id 或 --res）
        #[arg(long, value_name = "RARITY")]
        rarity: Option<String>,
        /// 输出语言目录名
        #[arg(short, long, default_value = "Chs")]
        variant: String,
        /// 卡名字体路径（默认使用 assets/fonts/dfweibeiw7-gb.ttc）
        #[arg(long)]
        font: Option<String>,
        /// 数字字体路径（默认使用 assets/fonts/Junicode-Bold.ttf）
        #[arg(long)]
        number_font: Option<String>,
    },
    /// 批量渲染 cards_full.json 中当前支持的卡牌
    Cards {
        /// 输出语言目录名
        #[arg(short, long, default_value = "Chs")]
        variant: String,
        /// 卡名字体路径（默认使用 assets/fonts/dfweibeiw7-gb.ttc）
        #[arg(long)]
        font: Option<String>,
        /// 数字字体路径（默认使用 assets/fonts/Junicode-Bold.ttf）
        #[arg(long)]
        number_font: Option<String>,
    },
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
                .unwrap_or_else(|_| "wbu=info,audio=debug".into()),
        )
        .init();

    let cli = Cli::parse();

    match cli.command {
        Command::Manifest {
            version,
            variant,
            format,
        } => {
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
            AssetCmd::Batch {
                variant,
                concurrency,
            } => {
                let cfg = config::load()?;

                for v in expand_variants(&variant) {
                    let manifest_path = format!(
                        "{}/manifests/json/assetbundle.{}.manifest.json",
                        cfg.data_dir, v
                    );
                    let json = std::fs::read_to_string(&manifest_path)
                        .with_context(|| format!("请先运行: wbu manifest -v {v} --format json"))?;
                    let m: manifest::Manifest = serde_json::from_str(&json)?;

                    let blobs_dir = std::path::Path::new(&cfg.data_dir).join("blobs");
                    let variant_dir = std::path::Path::new(&cfg.data_dir)
                        .join("variants")
                        .join(&v);

                    let stats = asset::batch_download(
                        &m,
                        &cfg.asset_bundle_address,
                        &cfg.asset_bundle_base_keys,
                        concurrency,
                        &blobs_dir,
                        &variant_dir,
                    )
                    .await?;

                    println!(
                        "[{v}] 完成: {} | 跳过: {} | 失败: {} | 硬链接: {} | 下载: {:.1} MB",
                        stats.done,
                        stats.skipped,
                        stats.failed,
                        stats.hardlinks,
                        stats.downloaded_bytes as f64 / 1024.0 / 1024.0
                    );
                }
            }
            AssetCmd::Download { name, variant } => {
                let cfg = config::load()?;
                let blobs_raw = std::path::Path::new(&cfg.data_dir)
                    .join("blobs")
                    .join("raw");

                for v in expand_variants(&variant) {
                    let manifest_path = format!(
                        "{}/manifests/json/assetbundle.{}.manifest.json",
                        cfg.data_dir, v
                    );
                    let json = std::fs::read_to_string(&manifest_path)
                        .with_context(|| format!("请先运行: wbu manifest -v {v} --format json"))?;
                    let m: manifest::Manifest = serde_json::from_str(&json)?;
                    let variant_links = std::path::Path::new(&cfg.data_dir)
                        .join("variants")
                        .join(&v);

                    if let Some(asset) = m.assets.iter().find(|a| a.name == name) {
                        let blob_path = asset::blob_path(&blobs_raw, "", &asset.hash);
                        let result = asset::download_asset(
                            &asset.hash,
                            &cfg.asset_bundle_address,
                            &blob_path,
                        )
                        .await?;
                        println!("[{v}] 下载: {} ({} bytes)", result.path, result.size);
                        let link_path = variant_links.join("raw").join(&asset.name);
                        if asset::hardlink_or_skip(&blob_path, &link_path)? {
                            println!("[{v}] 硬链接: {}", link_path.display());
                        }
                    } else if let Some(raw) = m.raw_assets.iter().find(|r| r.name == name) {
                        let blob_path = asset::blob_path(&blobs_raw, "", &raw.hash);
                        let result =
                            asset::download_asset(&raw.hash, &cfg.asset_bundle_address, &blob_path)
                                .await?;
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
            AssetCmd::Decrypt {
                file,
                name,
                manifest,
            } => {
                let cfg = config::load()?;
                let json = std::fs::read_to_string(&manifest)?;
                let m: manifest::Manifest = serde_json::from_str(&json)?;
                let asset = m
                    .assets
                    .iter()
                    .find(|a| a.name == name)
                    .ok_or_else(|| anyhow::anyhow!("manifest 中未找到资源: {name}"))?;
                let blobs_decrypted = std::path::Path::new(&cfg.data_dir)
                    .join("blobs")
                    .join("decrypted");
                let output = asset::blob_path(&blobs_decrypted, "", &asset.hash);
                asset::decrypt_file(
                    std::path::Path::new(&file),
                    &output,
                    &cfg.asset_bundle_base_keys,
                    asset.key,
                )?;
                println!("解密完成: {}", output.display());
            }
        },

        Command::Master {
            variant,
            force,
            sub,
        } => {
            let cfg = config::load()?;
            let data_dir = std::path::Path::new(&cfg.data_dir);

            match sub {
                Some(MasterCmd::Packs) => {
                    let master_data_dir = data_dir.join("exports").join("master-data");
                    let output_path = data_dir
                        .join("exports")
                        .join("analysis")
                        .join("pack_names.json");
                    println!("生成 pack_names.json...");
                    let packs = master_data::generate_pack_names(&master_data_dir)?;
                    let json = serde_json::to_string_pretty(&packs)?;
                    std::fs::create_dir_all(output_path.parent().unwrap())?;
                    std::fs::write(&output_path, json)?;
                    println!("完成: {} 个卡包 => {}", packs.len(), output_path.display());
                }
                Some(MasterCmd::Emblems) => {
                    let master_data_dir = data_dir.join("exports").join("master-data");
                    let output_path = data_dir
                        .join("exports")
                        .join("analysis")
                        .join("emblems_full.json");
                    println!("生成 emblems_full.json...");
                    let count = master_data::generate_emblems_full(&master_data_dir, &output_path)?;
                    println!("完成: {} 个徽章 => {}", count, output_path.display());
                }
                Some(MasterCmd::Stamps) => {
                    let master_data_dir = data_dir.join("exports").join("master-data");
                    let output_path = data_dir
                        .join("exports")
                        .join("analysis")
                        .join("stamps_full.json");
                    println!("生成 stamps_full.json...");
                    let count = master_data::generate_stamps_full(&master_data_dir, &output_path)?;
                    println!("完成: {} 个贴图 => {}", count, output_path.display());
                }
                Some(MasterCmd::Cards) => {
                    let master_data_dir = data_dir.join("exports").join("master-data");
                    let output_path = data_dir
                        .join("exports")
                        .join("analysis")
                        .join("cards_full.json");
                    println!("生成 cards_full.json...");
                    let count = master_data::generate_cards_full(&master_data_dir, &output_path)?;
                    println!("完成: {} 张卡 => {}", count, output_path.display());
                }
                None => {
                    for v in expand_variants(&variant) {
                        let manifest_path = format!(
                            "{}/manifests/json/assetbundle.{}.manifest.json",
                            cfg.data_dir, v
                        );
                        let json = std::fs::read_to_string(&manifest_path).with_context(|| {
                            format!("请先运行: wbu manifest -v {v} --format json")
                        })?;
                        let m: manifest::Manifest = serde_json::from_str(&json)?;

                        let master_entry = m
                            .raw_assets
                            .iter()
                            .find(|r| r.name == MASTER_BYTES_NAME)
                            .ok_or_else(|| {
                                anyhow::anyhow!("manifest 中未找到 {MASTER_BYTES_NAME}")
                            })?;

                        let blobs_raw = std::path::Path::new(&cfg.data_dir)
                            .join("blobs")
                            .join("raw");
                        let blob_path = asset::blob_path(&blobs_raw, "", &master_entry.hash);

                        if force && blob_path.exists() {
                            std::fs::remove_file(&blob_path)?;
                            println!("[{v}] 已删除缓存，重新下载...");
                        }

                        if !blob_path.exists() {
                            println!("[{v}] 下载 {MASTER_BYTES_NAME} ...");
                            asset::download_asset(
                                &master_entry.hash,
                                &cfg.asset_bundle_address,
                                &blob_path,
                            )
                            .await?;
                        }

                        let raw = std::fs::read(&blob_path)
                            .with_context(|| format!("无法读取: {}", blob_path.display()))?;

                        let output_dir = std::path::Path::new(&cfg.data_dir)
                            .join("exports")
                            .join("master-data")
                            .join(&v);

                        println!("[{v}] 导出到 {} ...", output_dir.display());
                        let results = master_data::export_all(&raw, &output_dir)?;

                        let total_rows: usize = results.iter().map(|r| r.rows).sum();
                        println!("[{v}] 完成: {} 个表, {} 行", results.len(), total_rows);
                    }
                }
            }
        }

        Command::Audio { mp3, sub } => {
            let cfg = config::load()?;
            let data_dir = std::path::Path::new(&cfg.data_dir);
            let vgmstream_path = std::path::Path::new(&cfg.vgmstream_path);

            match sub {
                Some(AudioCmd::Card) => {
                    let variant = "Chs";
                    let card_resource_path = data_dir
                        .join("exports")
                        .join("master-data")
                        .join(variant)
                        .join("CardResourceMaster.json");

                    let first_variant = "Chs";
                    let mapping_path = data_dir
                        .join("variants")
                        .join(first_variant)
                        .join("raw-assets")
                        .join("sound/WwiseIdMapping.bytes");
                    let mapping_data = std::fs::read(&mapping_path)
                        .with_context(|| format!("请先运行 wbu asset batch -v {first_variant}"))?;

                    let pck_root = data_dir
                        .join("variants")
                        .join(first_variant)
                        .join("raw-assets")
                        .join("sound");
                    let output_dir = data_dir.join("exports").join("card-voices");

                    println!("CardResourceMaster: {}", card_resource_path.display());
                    println!("pck 根目录: {}", pck_root.display());
                    println!("输出目录: {}", output_dir.display());

                    let audio_wav_dir = data_dir.join("exports").join("audio");
                    let stats = audio::card_voices::extract_card_voices(
                        &pck_root,
                        &output_dir,
                        &card_resource_path,
                        &audio_wav_dir,
                        &mapping_data,
                        vgmstream_path,
                        &cfg.ffmpeg_path,
                    )?;

                    println!(
                        "\n卡牌语音提取完成: {} 张卡, {} 个 MP3 (跳过: {})",
                        stats.cards_processed, stats.files_output, stats.files_skipped
                    );
                }
                None => {
                    let first_variant = "Chs";
                    let mapping_path = data_dir
                        .join("variants")
                        .join(first_variant)
                        .join("raw-assets")
                        .join("sound/WwiseIdMapping.bytes");
                    let mapping_data = std::fs::read(&mapping_path)
                        .with_context(|| format!("请先运行 wbu asset batch -v {first_variant}"))?;

                    let pck_dir = data_dir
                        .join("variants")
                        .join(first_variant)
                        .join("raw-assets")
                        .join("sound");
                    let output_dir = data_dir.join("exports").join("audio");

                    println!("扫描 {}", pck_dir.display());
                    let stats =
                        audio::extract_all(&pck_dir, &output_dir, &mapping_data, vgmstream_path)?;

                    println!(
                        "pck: {} | WAV: {} | 跳过: {} | 失败: {}",
                        stats.pck_files, stats.wav_output, stats.skipped, stats.failed
                    );

                    if mp3 {
                        let mp3_dir = data_dir.join("exports").join("audio-mp3");
                        println!("转码 MP3 → {}", mp3_dir.display());
                        let n = audio::convert_dir_to_mp3(&output_dir, &mp3_dir, &cfg.ffmpeg_path)?;
                        println!("MP3 完成: {n} 个文件");
                    }
                }
            }
        }

        Command::Texture { sub } => match sub {
            TextureCmd::Card {
                asset_studio,
                no_resize,
            } => {
                let cfg = config::load()?;
                let as_path = match &asset_studio {
                    Some(p) => std::path::PathBuf::from(p),
                    None => std::path::PathBuf::from(&cfg.asset_studio_path),
                };
                let data_dir = std::path::Path::new(&cfg.data_dir);
                texture::process_card_textures(data_dir, &as_path, no_resize)?;
            }
            TextureCmd::PackIcons { asset_studio } => {
                let cfg = config::load()?;
                let as_path = match &asset_studio {
                    Some(p) => std::path::PathBuf::from(p),
                    None => std::path::PathBuf::from(&cfg.asset_studio_path),
                };
                let data_dir = std::path::Path::new(&cfg.data_dir);
                texture::process_pack_icons(data_dir, &as_path)?;
            }
            TextureCmd::CardFrames { asset_studio } => {
                let cfg = config::load()?;
                let as_path = match &asset_studio {
                    Some(p) => std::path::PathBuf::from(p),
                    None => std::path::PathBuf::from(&cfg.asset_studio_path),
                };
                let data_dir = std::path::Path::new(&cfg.data_dir);
                texture::process_card_frames(data_dir, &as_path)?;
            }
            TextureCmd::HomeIllustPicts { asset_studio } => {
                let cfg = config::load()?;
                let as_path = match &asset_studio {
                    Some(p) => std::path::PathBuf::from(p),
                    None => std::path::PathBuf::from(&cfg.asset_studio_path),
                };
                let data_dir = std::path::Path::new(&cfg.data_dir);
                texture::process_home_illust_picts(data_dir, &as_path)?;
            }
            TextureCmd::Emblems { asset_studio } => {
                let cfg = config::load()?;
                let as_path = match &asset_studio {
                    Some(p) => std::path::PathBuf::from(p),
                    None => std::path::PathBuf::from(&cfg.asset_studio_path),
                };
                let data_dir = std::path::Path::new(&cfg.data_dir);
                texture::process_emblems(data_dir, &as_path)?;
            }
            TextureCmd::Stamps {
                asset_studio,
                variant,
            } => {
                let cfg = config::load()?;
                let as_path = match &asset_studio {
                    Some(p) => std::path::PathBuf::from(p),
                    None => std::path::PathBuf::from(&cfg.asset_studio_path),
                };
                let data_dir = std::path::Path::new(&cfg.data_dir);
                for v in expand_variants(&variant) {
                    texture::process_stamps(data_dir, &as_path, &v)?;
                }
            }
        },

        Command::HomeIllust {
            asset_studio,
            voices,
            layout_debug,
        } => {
            let cfg = config::load()?;
            let as_path = match &asset_studio {
                Some(p) => std::path::PathBuf::from(p),
                None => std::path::PathBuf::from(&cfg.asset_studio_path),
            };
            let data_dir = std::path::Path::new(&cfg.data_dir);
            let vgmstream_path = std::path::Path::new(&cfg.vgmstream_path);
            let stats = texture::home_illust::process_home_illustrations(
                data_dir,
                &as_path,
                vgmstream_path,
                voices,
                layout_debug,
            )?;
            println!(
                "HomeIllustration 提取完成: {} | 跳过: {} | 失败: {}",
                stats.processed, stats.skipped, stats.failed
            );
        }

        Command::Render { sub } => match sub {
            RenderCmd::Card {
                id,
                all,
                res,
                name,
                type_,
                cost,
                attack,
                life,
                class,
                rarity,
                variant,
                font,
                number_font,
            } => {
                let cfg = config::load()?;
                let data_dir = std::path::Path::new(&cfg.data_dir);
                let font_path = font.as_deref().map(std::path::Path::new);
                let number_font_path = number_font.as_deref().map(std::path::Path::new);

                let has_overrides = res.is_some()
                    || name.is_some()
                    || type_.is_some()
                    || rarity.is_some()
                    || cost.is_some()
                    || attack.is_some()
                    || life.is_some()
                    || class.is_some();

                if all {
                    anyhow::ensure!(
                        !has_overrides,
                        "--all 不支持与 --res / --name / --type 等覆盖参数同时使用"
                    );
                    let stats = texture::render_all_card_images(
                        data_dir,
                        &variant,
                        font_path,
                        number_font_path,
                    )?;
                    println!("批量渲染完成: {} | 跳过: {}", stats.rendered, stats.skipped);
                } else if let Some(card_id) = id {
                    if has_overrides {
                        // --id 带覆盖参数
                        let out = texture::render_card_with_overrides(
                            data_dir,
                            card_id,
                            res.as_deref(),
                            name.as_deref(),
                            type_.as_deref(),
                            cost,
                            attack,
                            life,
                            class,
                            rarity.as_deref(),
                            &variant,
                            font_path,
                            number_font_path,
                        )?;
                        println!("渲染完成: {}", out.display());
                    } else {
                        let out = texture::render_card_image(
                            data_dir,
                            card_id,
                            &variant,
                            font_path,
                            number_font_path,
                        )?;
                        println!("渲染完成: {}", out.display());
                    }
                } else if let Some(image_path) = res {
                    // --res 无 --id：纯自定义卡牌
                    let kind = type_.as_deref().unwrap_or("spell");
                    let card_name = name.as_deref().unwrap_or("Unknown");
                    let out = texture::render_custom_card(
                        data_dir,
                        std::path::Path::new(&image_path),
                        card_name,
                        kind,
                        cost,
                        attack,
                        life,
                        class,
                        rarity.as_deref(),
                        &variant,
                        font_path,
                        number_font_path,
                    )?;
                    println!("渲染完成: {}", out.display());
                } else {
                    anyhow::bail!("请指定 --id、--all 或 --res");
                }
            }
            RenderCmd::Cards {
                variant,
                font,
                number_font,
            } => {
                let cfg = config::load()?;
                let data_dir = std::path::Path::new(&cfg.data_dir);
                let font_path = font.as_deref().map(std::path::Path::new);
                let number_font_path = number_font.as_deref().map(std::path::Path::new);
                let stats = texture::render_all_card_images(
                    data_dir,
                    &variant,
                    font_path,
                    number_font_path,
                )?;
                println!("批量渲染完成: {} | 跳过: {}", stats.rendered, stats.skipped);
            }
        },
        Command::Metadb { .. } => todo!("metadb"),
        Command::Localize { .. } => todo!("localize"),
    }

    Ok(())
}
