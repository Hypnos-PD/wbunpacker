mod config;
mod diff_output;
use anyhow::Context;
use clap::{Parser, Subcommand};
use std::process::Command as ProcessCommand;
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
    /// 下载并解析资源清单
    Manifest {
        #[arg(short = 'V', long)]
        version: Option<String>,
        #[arg(short, long, default_value = "Chs")]
        variant: String,
        #[arg(short, long, default_value = "raw")]
        format: String,
        #[command(subcommand)]
        sub: Option<ManifestCmd>,
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
        #[arg(long)]
        dll: Option<String>,
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
        #[arg(long)]
        diff: Option<String>,
        /// diff 模式下载后用 AssetStudioModCLI 导出全部内容
        #[arg(long)]
        extract: bool,
        /// AssetStudioModCLI 路径（覆盖配置文件）
        #[arg(long)]
        asset_studio: Option<String>,
    },
}
#[derive(Subcommand)]
enum ManifestCmd {
    /// 比较两个清单版本的差异
    Diff {
        /// Git revision (commit/tag) or path to an old manifest .json file
        #[arg(short = 'o', long)]
        old: String,
        /// Git revision (commit/tag) or path to a new manifest .json file
        #[arg(short = 'n', long)]
        new: String,
        /// Variant filter: variant name or "all"
        #[arg(short = 'v', long, default_value = "all")]
        variant: String,
        /// Override Git repo path; defaults to <data_dir>/manifests/json
        #[arg(short = 'r', long)]
        repo: Option<String>,
        /// Override output directory
        #[arg(short = 'O', long)]
        output: Option<String>,
        /// Show top N changed items in summary
        #[arg(short = 't', long, default_value = "20")]
        top: usize,
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
// manifest diff 实现
// ============================================================================

/// Context bundled for diff output writers to avoid parameter bloat.
struct DiffOutputCtx {
    output_dir: String,
    mode: &'static str,
    repo_path: String,
    old_rev: String,
    new_rev: String,
    old_label: String,
    new_label: String,
    variants: Vec<String>,
}

/// Run `manifest diff` with mode detection (file vs git) and output.
fn run_diff(
    old: &str,
    new: &str,
    variant: &str,
    repo: Option<&str>,
    output_override: Option<&str>,
    top: usize,
) -> anyhow::Result<()> {
    let old_is_file = std::path::Path::new(old).exists();
    let new_is_file = std::path::Path::new(new).exists();

    anyhow::ensure!(
        old_is_file == new_is_file,
        "mixed file and revision — both must be files or both must be revisions"
    );

    let cfg = config::load()?;

    let variants = if old_is_file {
        anyhow::ensure!(
            !variant.eq_ignore_ascii_case("all"),
            "file mode requires a single variant (e.g. --variant Chs)"
        );
        anyhow::ensure!(
            repo.is_none(),
            "`--repo` is only used in Git mode; do not set it when passing file paths"
        );
        vec![variant.to_string()]
    } else {
        expand_variants(variant)
    };

    let mut variant_diffs: Vec<(String, manifest::Manifest, manifest::Manifest)> = Vec::new();
    let (mode, repo_path, old_rev, new_rev, old_label, new_label, old_time, new_time);

    if old_is_file {
        mode = "file";
        repo_path = String::new();
        old_rev = String::new();
        new_rev = String::new();
        old_label = file_timestamp(old)?;
        new_label = file_timestamp(new)?;
        old_time = old_label.clone();
        new_time = new_label.clone();

        let old_manifest = read_manifest_file(old)?;
        let new_manifest = read_manifest_file(new)?;
        variant_diffs.push((variant.to_string(), old_manifest, new_manifest));
    } else {
        mode = "git";
        repo_path = match repo {
            Some(r) => r.to_string(),
            None => format!("{}/manifests/json", cfg.data_dir),
        };

        // Verify git repo
        let git_dir_check = ProcessCommand::new("git")
            .args(["-C", &repo_path, "rev-parse", "--git-dir"])
            .output()
            .context("failed to run git rev-parse")?;
        anyhow::ensure!(
            git_dir_check.status.success(),
            "not a valid Git repository: {repo_path}"
        );

        old_rev = old.to_string();
        new_rev = new.to_string();

        old_time = git_commit_time(&repo_path, old)?;
        new_time = git_commit_time(&repo_path, new)?;
        old_label = git_version_label(&repo_path, old)?;
        new_label = git_version_label(&repo_path, new)?;

        for v in &variants {
            let old_json = git_show_manifest(&repo_path, old, v)?;
            let new_json = git_show_manifest(&repo_path, new, v)?;
            let old_manifest: manifest::Manifest = serde_json::from_str(&old_json)
                .with_context(|| format!("failed to parse old manifest for variant {v}"))?;
            let new_manifest: manifest::Manifest = serde_json::from_str(&new_json)
                .with_context(|| format!("failed to parse new manifest for variant {v}"))?;
            variant_diffs.push((v.clone(), old_manifest, new_manifest));
        }
    };

    // Compute diffs for all variants
    let mut diffs: Vec<(String, manifest::ManifestChanges)> = Vec::new();
    for (v, old_m, new_m) in &variant_diffs {
        diffs.push((v.clone(), manifest::diff_manifests(old_m, new_m)));
    }

    // Determine output directory (guarantee trailing /)
    let mut output_dir = if let Some(o) = output_override {
        o.to_string()
    } else {
        diff_output::build_diff_output_dir(
            &cfg.data_dir,
            &old_label,
            &new_label,
            &old_time,
            &new_time,
        )
    };
    if !output_dir.ends_with('/') {
        output_dir.push('/');
    }
    std::fs::create_dir_all(&output_dir)
        .with_context(|| format!("failed to create output directory: {output_dir}"))?;

    let ctx = DiffOutputCtx {
        output_dir,
        mode,
        repo_path,
        old_rev,
        new_rev,
        old_label,
        new_label,
        variants,
    };

    // Write all 6 output files
    write_summary_json(&ctx, &variant_diffs, &diffs)?;
    write_changed_json(&ctx, &diffs)?;
    write_metadata_changed_json(&ctx, &diffs)?;
    write_added_manifest_json(&ctx, &variant_diffs, &diffs)?;
    write_removed_manifest_json(&ctx, &variant_diffs, &diffs)?;

    // Print summary
    print_diff_summary(&ctx, &variant_diffs, &diffs, top);

    Ok(())
}

// --- helpers ---

fn read_manifest_file(path: &str) -> anyhow::Result<manifest::Manifest> {
    let json = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read manifest file: {path}"))?;
    serde_json::from_str(&json).with_context(|| format!("failed to parse manifest: {path}"))
}

fn file_timestamp(path: &str) -> anyhow::Result<String> {
    let meta =
        std::fs::metadata(path).with_context(|| format!("failed to get metadata for: {path}"))?;
    let modified = meta
        .modified()
        .with_context(|| format!("failed to get modification time for: {path}"))?;
    let dur = modified
        .duration_since(std::time::UNIX_EPOCH)
        .context("invalid system time")?;
    let secs = dur.as_secs();
    // Convert to naive UTC datetime-like components
    let days = secs / 86400;
    let time_of_day = secs % 86400;
    let hours = time_of_day / 3600;
    let minutes = (time_of_day % 3600) / 60;
    let seconds = time_of_day % 60;

    // Compute year/month/day from days since Unix epoch (simplified, accurate enough)
    let (y, m, d) = days_since_epoch_to_ymd(days as i64);

    Ok(format!(
        "{y:04}{m:02}{d:02}-{hours:02}{minutes:02}{seconds:02}"
    ))
}

fn days_since_epoch_to_ymd(mut days: i64) -> (i64, u32, u32) {
    // Shift to start from 0000-03-01 (easier month length pattern)
    days += 719468; // days from 0000-03-01 to 1970-01-01
    let era = (if days >= 0 { days } else { days - 146096 }) / 146097;
    let doe = days - era * 146097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 {
        (mp + 3) as u32
    } else {
        (mp - 9) as u32
    };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

fn git_show_manifest(repo: &str, rev: &str, variant: &str) -> anyhow::Result<String> {
    let path = format!("assetbundle.{variant}.manifest.json");
    let output = ProcessCommand::new("git")
        .args(["-C", repo, "show", &format!("{rev}:{path}")])
        .output()
        .with_context(|| format!("failed to run git show {rev}:{path}"))?;
    anyhow::ensure!(
        output.status.success(),
        "git show failed for {rev}:{path}: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    String::from_utf8(output.stdout).context("git show output is not valid UTF-8")
}

fn git_commit_time(repo: &str, rev: &str) -> anyhow::Result<String> {
    let output = ProcessCommand::new("git")
        .args(["-C", repo, "log", "-1", "--format=%ai", rev])
        .output()
        .with_context(|| format!("failed to run git log for {rev}"))?;
    anyhow::ensure!(
        output.status.success(),
        "git log failed for {rev}: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    let raw = String::from_utf8(output.stdout).context("git log output is not valid UTF-8")?;
    let raw = raw.trim();
    // git --format=%ai outputs: "2026-06-27 09:20:00 +0000"
    parse_git_ai_timestamp(raw)
}

fn parse_git_ai_timestamp(raw: &str) -> anyhow::Result<String> {
    // Format: "YYYY-MM-DD HH:MM:SS +TZOFF"
    let parts: Vec<&str> = raw.split(&['-', ' ', ':']).collect();
    anyhow::ensure!(parts.len() >= 6, "unexpected git timestamp format: {raw}");
    let y = parts[0];
    let m = parts[1];
    let d = parts[2];
    let h = parts[3];
    let min = parts[4];
    let s = parts[5];
    Ok(format!("{y}{m}{d}-{h}{min}{s}"))
}

fn git_version_label(repo: &str, rev: &str) -> anyhow::Result<String> {
    // Try extracting "ver.XXXXX" from commit subject
    let subject_output = ProcessCommand::new("git")
        .args(["-C", repo, "log", "-1", "--format=%s", rev])
        .output()
        .with_context(|| format!("failed to run git log for {rev}"))?;
    if subject_output.status.success() {
        let subject = String::from_utf8_lossy(&subject_output.stdout);
        if let Some(label) = extract_ver_label(subject.trim()) {
            return Ok(label);
        }
    }
    // Fall back to short SHA
    let sha_output = ProcessCommand::new("git")
        .args(["-C", repo, "rev-parse", "--short", rev])
        .output()
        .with_context(|| format!("failed to run git rev-parse for {rev}"))?;
    anyhow::ensure!(
        sha_output.status.success(),
        "git rev-parse failed for {rev}"
    );
    Ok(String::from_utf8_lossy(&sha_output.stdout)
        .trim()
        .to_string())
}

fn extract_ver_label(subject: &str) -> Option<String> {
    // Look for "ver." followed by digits
    if let Some(pos) = subject.find("ver.") {
        let after = &subject[pos + 4..];
        let digits: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
        if !digits.is_empty() {
            return Some(digits);
        }
    }
    None
}

fn format_with_commas(n: usize) -> String {
    let s = n.to_string();
    let len = s.len();
    let mut result = String::with_capacity(len + (len.saturating_sub(1)) / 3);
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (len - i).is_multiple_of(3) {
            result.push(',');
        }
        result.push(c);
    }
    result
}

// --- JSON output writers ---

fn is_multi_variant(variants: &[String]) -> bool {
    variants.len() > 1
}

fn write_summary_json(
    ctx: &DiffOutputCtx,
    variant_diffs: &[(String, manifest::Manifest, manifest::Manifest)],
    diffs: &[(String, manifest::ManifestChanges)],
) -> anyhow::Result<()> {
    let mut map = serde_json::Map::new();
    map.insert(
        "mode".to_string(),
        serde_json::Value::String(ctx.mode.to_string()),
    );
    if ctx.mode == "git" {
        map.insert(
            "repo".to_string(),
            serde_json::Value::String(ctx.repo_path.clone()),
        );
        map.insert(
            "old_rev".to_string(),
            serde_json::Value::String(ctx.old_rev.clone()),
        );
        map.insert(
            "new_rev".to_string(),
            serde_json::Value::String(ctx.new_rev.clone()),
        );
    }
    map.insert(
        "old_label".to_string(),
        serde_json::Value::String(ctx.old_label.clone()),
    );
    map.insert(
        "new_label".to_string(),
        serde_json::Value::String(ctx.new_label.clone()),
    );
    map.insert(
        "variants".to_string(),
        serde_json::Value::Array(
            ctx.variants
                .iter()
                .map(|v| serde_json::Value::String(v.clone()))
                .collect(),
        ),
    );

    let mut summary_map = serde_json::Map::new();
    for (v, changes) in diffs {
        let (old_m, new_m) = variant_diffs
            .iter()
            .find(|(var, _, _)| var == v)
            .map(|(_, old, new)| (old, new))
            .with_context(|| format!("variant {v} not found in variant_diffs"))?;

        let old_cfg_count = old_m.config.len();
        let new_cfg_count = new_m.config.len();
        let old_ln_count = old_m.load_names.len();
        let new_ln_count = new_m.load_names.len();

        summary_map.insert(v.clone(), serde_json::json!({
            "assets": {
                "old_count": old_m.assets.len(),
                "new_count": new_m.assets.len(),
                "added": changes.added_assets.len(),
                "removed": changes.removed_assets.len(),
                "content_changed": changes.content_changed_assets.len(),
                "metadata_changed": changes.metadata_changed_assets.len(),
            },
            "raw_assets": {
                "old_count": old_m.raw_assets.len(),
                "new_count": new_m.raw_assets.len(),
                "added": changes.added_raw_assets.len(),
                "removed": changes.removed_raw_assets.len(),
                "content_changed": changes.content_changed_raw_assets.len(),
                "metadata_changed": changes.metadata_changed_raw_assets.len(),
            },
            "config": {
                "old_count": old_cfg_count,
                "new_count": new_cfg_count,
                "added": changes.config_changes.iter().filter(|c| matches!(c, manifest::ConfigChange::Added(_))).count(),
                "removed": changes.config_changes.iter().filter(|c| matches!(c, manifest::ConfigChange::Removed(_))).count(),
                "value_changed": changes.config_changes.iter().filter(|c| matches!(c, manifest::ConfigChange::ValueChanged { .. })).count(),
            },
            "load_names": {
                "old_count": old_ln_count,
                "new_count": new_ln_count,
                "added": changes.load_name_changes.iter().filter(|c| matches!(c, manifest::LoadNameChange::Added(_))).count(),
                "removed": changes.load_name_changes.iter().filter(|c| matches!(c, manifest::LoadNameChange::Removed(_))).count(),
                "name_changed": changes.load_name_changes.iter().filter(|c| matches!(c, manifest::LoadNameChange::NameChanged { .. })).count(),
            },
        }));
    }
    map.insert(
        "summary".to_string(),
        serde_json::Value::Object(summary_map),
    );

    let json = serde_json::to_string_pretty(&serde_json::Value::Object(map))?;
    let path = std::path::Path::new(&ctx.output_dir).join("summary.json");
    std::fs::write(&path, json)?;
    println!("Wrote summary.json");
    Ok(())
}

fn write_variant_group_json<F>(
    ctx: &DiffOutputCtx,
    diffs: &[(String, manifest::ManifestChanges)],
    filename: &str,
    single_key: &str,
    build: F,
) -> anyhow::Result<()>
where
    F: Fn(&manifest::ManifestChanges) -> serde_json::Value,
{
    let mut top_map = serde_json::Map::new();
    if is_multi_variant(&ctx.variants) {
        for (v, changes) in diffs {
            top_map.insert(v.clone(), build(changes));
        }
    } else {
        top_map.insert(
            "variant".to_string(),
            serde_json::Value::String(ctx.variants[0].clone()),
        );
        top_map.insert(single_key.to_string(), build(&diffs[0].1));
    }
    let json = serde_json::to_string_pretty(&serde_json::Value::Object(top_map))?;
    let path = std::path::Path::new(&ctx.output_dir).join(filename);
    std::fs::write(&path, json)?;
    println!("Wrote {}", filename);
    Ok(())
}

fn write_changed_json(
    ctx: &DiffOutputCtx,
    diffs: &[(String, manifest::ManifestChanges)],
) -> anyhow::Result<()> {
    write_variant_group_json(
        ctx,
        diffs,
        "changed.json",
        "content_changed",
        build_content_changed_section,
    )
}

fn build_content_changed_section(changes: &manifest::ManifestChanges) -> serde_json::Value {
    let assets: Vec<_> = changes
        .content_changed_assets
        .iter()
        .map(|(old, new)| {
            serde_json::json!({
                "old": old,
                "new": new,
            })
        })
        .collect();
    let raw_assets: Vec<_> = changes
        .content_changed_raw_assets
        .iter()
        .map(|(old, new)| {
            serde_json::json!({
                "old": old,
                "new": new,
            })
        })
        .collect();
    serde_json::json!({
        "assets": assets,
        "raw_assets": raw_assets,
    })
}

fn write_metadata_changed_json(
    ctx: &DiffOutputCtx,
    diffs: &[(String, manifest::ManifestChanges)],
) -> anyhow::Result<()> {
    write_variant_group_json(
        ctx,
        diffs,
        "metadata_changed.json",
        "metadata_changed",
        build_metadata_changed_section,
    )
}

fn build_metadata_changed_section(changes: &manifest::ManifestChanges) -> serde_json::Value {
    let assets: Vec<_> = changes
        .metadata_changed_assets
        .iter()
        .map(|(old, new)| {
            let changed_fields = asset_metadata_changed_fields(old, new);
            serde_json::json!({
                "old": old,
                "new": new,
                "changes": changed_fields,
            })
        })
        .collect();
    let raw_assets: Vec<_> = changes
        .metadata_changed_raw_assets
        .iter()
        .map(|(old, new)| {
            let changed_fields = raw_metadata_changed_fields(old, new);
            serde_json::json!({
                "old": old,
                "new": new,
                "changes": changed_fields,
            })
        })
        .collect();
    let config: Vec<_> = changes
        .config_changes
        .iter()
        .filter_map(|c| match c {
            manifest::ConfigChange::ValueChanged {
                key,
                old_value,
                new_value,
            } => Some(serde_json::json!({
                "key": key,
                "old_value": old_value,
                "new_value": new_value,
            })),
            _ => None,
        })
        .collect();
    let load_names: Vec<_> = changes
        .load_name_changes
        .iter()
        .filter_map(|c| match c {
            manifest::LoadNameChange::NameChanged {
                asset_name,
                old_name,
                new_name,
            } => Some(serde_json::json!({
                "asset_name": asset_name,
                "old_name": old_name,
                "new_name": new_name,
            })),
            _ => None,
        })
        .collect();
    serde_json::json!({
        "assets": assets,
        "raw_assets": raw_assets,
        "config": config,
        "load_names": load_names,
    })
}

fn asset_metadata_changed_fields(
    old: &manifest::ManifestAsset,
    new: &manifest::ManifestAsset,
) -> Vec<&'static str> {
    let mut fields = Vec::new();
    if old.asset_id != new.asset_id {
        fields.push("asset_id");
    }
    if old.all_dependencies != new.all_dependencies {
        fields.push("dependencies");
    }
    if old.category != new.category {
        fields.push("category");
    }
    if old.group != new.group {
        fields.push("group");
    }
    if old.key != new.key {
        fields.push("key");
    }
    fields
}

fn raw_metadata_changed_fields(
    old: &manifest::RawAsset,
    new: &manifest::RawAsset,
) -> Vec<&'static str> {
    let mut fields = Vec::new();
    if old.category != new.category {
        fields.push("category");
    }
    if old.group != new.group {
        fields.push("group");
    }
    fields
}

fn write_added_manifest_json(
    ctx: &DiffOutputCtx,
    variant_diffs: &[(String, manifest::Manifest, manifest::Manifest)],
    diffs: &[(String, manifest::ManifestChanges)],
) -> anyhow::Result<()> {
    let mut top_map = serde_json::Map::new();
    for (v, _, _new_manifest) in variant_diffs {
        let changes = diffs.iter().find(|(var, _)| var == v).map(|(_, c)| c);
        let added = build_added_manifest(changes);
        top_map.insert(v.clone(), serde_json::to_value(added)?);
    }
    let json = serde_json::to_string_pretty(&serde_json::Value::Object(top_map))?;
    let path = std::path::Path::new(&ctx.output_dir).join("added_manifest.json");
    std::fs::write(&path, json)?;
    println!("Wrote added_manifest.json");
    Ok(())
}

fn build_added_manifest(
    changes: Option<&manifest::ManifestChanges>,
) -> manifest::Manifest {
    let Some(c) = changes else {
        return manifest::Manifest {
            assets: vec![],
            raw_assets: vec![],
            config: vec![],
            load_names: vec![],
        };
    };
    let assets: Vec<manifest::ManifestAsset> = c.added_assets.clone();
    let raw_assets: Vec<manifest::RawAsset> = c.added_raw_assets.clone();
    manifest::Manifest {
        assets,
        raw_assets,
        config: vec![],
        load_names: vec![],
    }
}

fn build_removed_manifest(
    changes: Option<&manifest::ManifestChanges>,
) -> manifest::Manifest {
    let Some(c) = changes else {
        return manifest::Manifest {
            assets: vec![],
            raw_assets: vec![],
            config: vec![],
            load_names: vec![],
        };
    };
    manifest::Manifest {
        assets: c.removed_assets.clone(),
        raw_assets: c.removed_raw_assets.clone(),
        config: vec![],
        load_names: vec![],
    }
}

fn write_removed_manifest_json(
    ctx: &DiffOutputCtx,
    variant_diffs: &[(String, manifest::Manifest, manifest::Manifest)],
    diffs: &[(String, manifest::ManifestChanges)],
) -> anyhow::Result<()> {
    let mut top_map = serde_json::Map::new();
    for (v, _, _) in variant_diffs {
        let changes = diffs.iter().find(|(var, _)| var == v).map(|(_, c)| c);
        let removed = build_removed_manifest(changes);
        top_map.insert(v.clone(), serde_json::to_value(removed)?);
    }
    let json = serde_json::to_string_pretty(&serde_json::Value::Object(top_map))?;
    let path = std::path::Path::new(&ctx.output_dir).join("removed_manifest.json");
    std::fs::write(&path, json)?;
    println!("Wrote removed_manifest.json");
    Ok(())
}

fn print_diff_summary(
    ctx: &DiffOutputCtx,
    variant_diffs: &[(String, manifest::Manifest, manifest::Manifest)],
    diffs: &[(String, manifest::ManifestChanges)],
    top: usize,
) {
    println!();
    println!("=== Manifest Diff: {} → {} ===", ctx.old_label, ctx.new_label);
    if ctx.mode == "git" {
        println!("Repo: {}", ctx.repo_path);
    }
    println!("Mode: {}", ctx.mode);
    println!();

    for (v, changes) in diffs {
        let (old_m, new_m) = variant_diffs
            .iter()
            .find(|(var, _, _)| var == v)
            .map(|(_, old, new)| (old, new))
            .expect("variant {v} not found in variant_diffs");

        let asset_old_count = old_m.assets.len();
        let asset_new_count = new_m.assets.len();
        let raw_old_count = old_m.raw_assets.len();
        let raw_new_count = new_m.raw_assets.len();
        let cfg_old_count = old_m.config.len();
        let cfg_new_count = new_m.config.len();
        let ln_old_count = old_m.load_names.len();
        let ln_new_count = new_m.load_names.len();

        println!("  {v} ─────────────────────────────────────────");

        // assets
        let a_add = changes.added_assets.len();
        let a_rem = changes.removed_assets.len();
        let a_content = changes.content_changed_assets.len();
        let a_meta = changes.metadata_changed_assets.len();
        println!(
            "  assets:      {} → {}   +{} / -{}",
            format_with_commas(asset_old_count),
            format_with_commas(asset_new_count),
            a_add,
            a_rem,
        );
        println!("    content changed:    {}", format_with_commas(a_content));
        println!("    metadata only:      {}", format_with_commas(a_meta));

        // Print top N added asset names if any
        if a_add > 0 && top > 0 {
            println!("    added (top {}):", top.min(a_add));
            for a in changes.added_assets.iter().take(top) {
                println!("      + {}", a.name);
            }
            if a_add > top {
                println!("      ... and {} more", a_add - top);
            }
        }
        if a_rem > 0 && top > 0 {
            println!("    removed (top {}):", top.min(a_rem));
            for a in changes.removed_assets.iter().take(top) {
                println!("      - {}", a.name);
            }
            if a_rem > top {
                println!("      ... and {} more", a_rem - top);
            }
        }

        // raw_assets
        let r_add = changes.added_raw_assets.len();
        let r_rem = changes.removed_raw_assets.len();
        let r_content = changes.content_changed_raw_assets.len();
        let r_meta = changes.metadata_changed_raw_assets.len();
        println!(
            "  raw_assets:  {} → {}   +{} / -{}",
            format_with_commas(raw_old_count),
            format_with_commas(raw_new_count),
            r_add,
            r_rem,
        );
        println!("    content changed:    {}", format_with_commas(r_content));
        println!("    metadata only:      {}", format_with_commas(r_meta));

        // config
        let cfg_add = changes
            .config_changes
            .iter()
            .filter(|c| matches!(c, manifest::ConfigChange::Added(_)))
            .count();
        let cfg_rem = changes
            .config_changes
            .iter()
            .filter(|c| matches!(c, manifest::ConfigChange::Removed(_)))
            .count();
        let cfg_val = changes
            .config_changes
            .iter()
            .filter(|c| matches!(c, manifest::ConfigChange::ValueChanged { .. }))
            .count();
        println!(
            "  config:       {} → {}             +{} / -{}",
            cfg_old_count, cfg_new_count, cfg_add, cfg_rem,
        );
        if cfg_val > 0 {
            println!("    value changed:       {}", cfg_val);
        }

        // load_names
        let ln_add = changes
            .load_name_changes
            .iter()
            .filter(|c| matches!(c, manifest::LoadNameChange::Added(_)))
            .count();
        let ln_rem = changes
            .load_name_changes
            .iter()
            .filter(|c| matches!(c, manifest::LoadNameChange::Removed(_)))
            .count();
        let ln_name = changes
            .load_name_changes
            .iter()
            .filter(|c| matches!(c, manifest::LoadNameChange::NameChanged { .. }))
            .count();
        if ln_add == 0 && ln_rem == 0 && ln_name == 0 {
            println!("  load_names:   unchanged");
        } else {
            println!(
                "  load_names:   {} → {}             +{} / -{}",
                ln_old_count, ln_new_count, ln_add, ln_rem,
            );
            if ln_name > 0 {
                println!("    name changed:        {}", ln_name);
            }
        }
        println!();
    }

    println!("Output: {}", ctx.output_dir);
}

// ============================================================================
// Diff 下载/提取辅助函数
// ============================================================================

async fn download_diff_set(
    export: &std::collections::HashMap<String, manifest::Manifest>,
    variants: &[String],
    target_dir: &std::path::Path,
    address: &str,
    base_keys: &str,
    concurrency: usize,
    label: &str,
) -> anyhow::Result<()> {
    std::fs::create_dir_all(target_dir)?;
    for v in variants {
        let m = match export.get(v) {
            Some(m) => m,
            None => {
                println!("[{v}] {label} 清单中未找到此变体，跳过");
                continue;
            }
        };
        if m.assets.is_empty() && m.raw_assets.is_empty() {
            println!("[{v}] {label} 中无变更资源，跳过");
            continue;
        }
        println!(
            "[{v}] {label} 模式: {} 个资产 + {} 个 raw 资源",
            m.assets.len(),
            m.raw_assets.len()
        );
        let blobs_dir = target_dir.join("blobs");
        let variant_dir = target_dir.join("variants").join(v);
        let stats = asset::batch_download(
            m,
            address,
            base_keys,
            concurrency,
            &blobs_dir,
            &variant_dir,
        )
        .await?;
        println!(
            "[{v}] {label} 完成: {} | 跳过: {} | 失败: {} | 硬链接: {} | 下载: {:.1} MB",
            stats.done,
            stats.skipped,
            stats.failed,
            stats.hardlinks,
            stats.downloaded_bytes as f64 / 1024.0 / 1024.0
        );
    }
    Ok(())
}

fn run_asset_studio_extract(
    asset_studio_path: &std::path::Path,
    input_dir: &std::path::Path,
    output_dir: &std::path::Path,
    variant: &str,
    label: &str,
) -> anyhow::Result<()> {
    if !input_dir.exists() {
        println!("[{variant}] {label} 解密目录不存在，跳过 AssetStudio 提取");
        return Ok(());
    }
    std::fs::create_dir_all(output_dir)?;
    println!("[{variant}] AssetStudio 导出 {label} 全部内容...");
    let status = ProcessCommand::new(asset_studio_path)
        .arg(input_dir)
        .args([
            "-t",
            "all",
            "-o",
            &output_dir.to_string_lossy(),
            "-r",
            "--unity-version",
            texture::UNITY_VERSION,
            "--log-level",
            "warning",
        ])
        .status()
        .with_context(|| format!("无法启动 AssetStudio: {}", asset_studio_path.display()))?;
    if !status.success() {
        anyhow::bail!("[{variant}] {label} AssetStudio 退出码: {:?}", status.code());
    }
    // Flatten: move all files from subdirectories to output_dir root
    let count = flatten_dir(output_dir, output_dir)?;
    println!("[{variant}] {label} 提取完成: {count} 个文件 → {}", output_dir.display());
    Ok(())
}

/// 递归地把子目录里的文件全部移到 root 目录，然后删除空子目录
fn flatten_dir(dir: &std::path::Path, root: &std::path::Path) -> std::io::Result<usize> {
    let mut count = 0;
    let entries: Vec<std::path::PathBuf> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .collect();
    for entry in entries {
        if entry.is_dir() {
            count += flatten_dir(&entry, root)?;
            let _ = std::fs::remove_dir(&entry);
        } else {
            let dst = root.join(entry.file_name().unwrap_or_default());
            if entry != dst {
                let mut dst = dst;
                // Handle filename collisions: append _1, _2, etc.
                if dst.exists() {
                    let stem = dst.file_stem().unwrap_or_default().to_string_lossy().to_string();
                    let ext = dst.extension().unwrap_or_default().to_string_lossy();
                    for i in 1u32.. {
                        let candidate = if ext.is_empty() {
                            root.join(format!("{stem}_{i}"))
                        } else {
                            root.join(format!("{stem}_{i}.{ext}"))
                        };
                        if !candidate.exists() {
                            dst = candidate;
                            break;
                        }
                    }
                }
                std::fs::rename(&entry, &dst)?;
            }
            count += 1;
        }
    }
    Ok(count)
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
            sub,
        } => match sub {
            Some(ManifestCmd::Diff {
                old,
                new,
                variant,
                repo,
                output,
                top,
            }) => {
                run_diff(
                    &old,
                    &new,
                    &variant,
                    repo.as_deref(),
                    output.as_deref(),
                    top,
                )?;
            }
            None => {
                let cfg = config::load()?;
                let version = version.unwrap_or(cfg.default_version);
                for v in expand_variants(&variant) {
                    let raw = manifest::download(&version, &v, &cfg.manifest_address).await?;
                    let manifests_dir = format!("{}/manifests", cfg.data_dir);
                    match format.as_str() {
                        "json" => {
                            let m = manifest::parse(&raw)?;
                            let json = manifest::to_json(&m)?;
                            let out =
                                format!("{}/json/assetbundle.{}.manifest.json", manifests_dir, v);
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
        },
        Command::Asset { sub } => match sub {
            AssetCmd::Batch {
                variant,
                concurrency,
                diff,
                extract,
                asset_studio,
            } => {
                let cfg = config::load()?;
                if extract && diff.is_none() {
                    anyhow::bail!("--extract 只能与 --diff 一起使用");
                }
                if let Some(diff_path) = diff {
                    let json_path = {
                        let p = std::path::Path::new(&diff_path);
                        if p.extension() == Some(std::ffi::OsStr::new("json")) {
                            p.to_path_buf()
                        } else {
                            p.join("added_manifest.json")
                        }
                    };
                    let diff_dir = json_path
                        .parent()
                        .ok_or_else(|| anyhow::anyhow!("无法确定 diff 输出目录"))?
                        .to_path_buf();
                    let json_str = std::fs::read_to_string(&json_path).with_context(|| {
                        format!("无法读取 diff manifest: {}", json_path.display())
                    })?;
                    let export: std::collections::HashMap<String, manifest::Manifest> =
                        serde_json::from_str(&json_str).with_context(|| {
                            format!("无法解析 diff manifest: {}", json_path.display())
                        })?;
                    let variants = expand_variants(&variant);

                    // 下载新增资源
                    download_diff_set(
                        &export,
                        &variants,
                        &diff_dir.join("added"),
                        &cfg.asset_bundle_address,
                        &cfg.asset_bundle_base_keys,
                        concurrency,
                        "新增",
                    )
                    .await?;

                    if extract {
                        // 下载删除的资源
                        let removed_path = diff_dir.join("removed_manifest.json");
                        if removed_path.exists() {
                            let removed_json = std::fs::read_to_string(&removed_path)
                                .with_context(|| {
                                    format!("无法读取 removed manifest: {}", removed_path.display())
                                })?;
                            let removed: std::collections::HashMap<String, manifest::Manifest> =
                                serde_json::from_str(&removed_json).with_context(|| {
                                    format!(
                                        "无法解析 removed manifest: {}",
                                        removed_path.display()
                                    )
                                })?;
                            download_diff_set(
                                &removed,
                                &variants,
                                &diff_dir.join("removed"),
                                &cfg.asset_bundle_address,
                                &cfg.asset_bundle_base_keys,
                                concurrency,
                                "删除",
                            )
                            .await?;
                        } else {
                            println!("removed_manifest.json 不存在，跳过删除资源");
                        }

                        let asset_studio_path = if let Some(as_path) = asset_studio {
                            std::path::PathBuf::from(as_path)
                        } else {
                            std::path::PathBuf::from(&cfg.asset_studio_path)
                        };
                        let extracted_dir = diff_dir.join("extracted");
                        for v in &variants {
                            run_asset_studio_extract(
                                &asset_studio_path,
                                &diff_dir.join("added").join("variants").join(v).join("decrypted"),
                                &extracted_dir.join("added"),
                                v,
                                "新增",
                            )?;
                            run_asset_studio_extract(
                                &asset_studio_path,
                                &diff_dir
                                    .join("removed")
                                    .join("variants")
                                    .join(v)
                                    .join("decrypted"),
                                &extracted_dir.join("removed"),
                                v,
                                "删除",
                            )?;
                        }
                    }
                } else {
                    for v in expand_variants(&variant) {
                        let manifest_path = format!(
                            "{}/manifests/json/assetbundle.{}.manifest.json",
                            cfg.data_dir, v
                        );
                        let json = std::fs::read_to_string(&manifest_path).with_context(|| {
                            format!("请先运行: wbu manifest -v {v} --format json")
                        })?;
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
        Command::Metadb { path, output, dll } => {
            let cfg = config::load()?;
            let data_dir = std::path::Path::new(&cfg.data_dir);
            let default_dll = r"C:\Program Files (x86)\Steam\steamapps\common\ShadowverseWB\ShadowverseWB_Data\Plugins\x86_64\libnative.dll";
            let dll_path = dll.as_deref().unwrap_or(default_dll);
            let output_path = output.unwrap_or_else(|| {
                data_dir
                    .join("exports")
                    .join("meta")
                    .join("meta.db")
                    .display()
                    .to_string()
            });
            metadb::decrypt_metadb(
                std::path::Path::new(&path),
                std::path::Path::new(&output_path),
                std::path::Path::new(dll_path),
                &cfg.sqlite3mc_key,
                &cfg.sqlite3mc_base_key,
            )?;
            println!("Decrypted: {}", output_path);
        }
    }
    Ok(())
}
