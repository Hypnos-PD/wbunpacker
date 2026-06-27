//! 卡牌语音提取模块
//!
//! 从按卡拆分的 .pck 文件中提取语音 WEM，
//! 转码为 MP3，按语言和卡牌声音前缀组织。
//!
//! 对照 W2AU 的 extract_card_audio.py 实现，支持：
//! - 动态 slot 名（play_pair_10721110 而非坍缩的 play_pair）
//! - 多 pair/cross/token/skill 变体
//! - 增量：复用已有 WAV → 跳过 pck 解包
//! //!
//! # 管线
//!
//! ```text
//! CardResourceMaster.json
//!     │   列42 = dx_{前缀}, 列43-52 = 事件名列表
//!     ▼
//!   {前缀: {slot名: [event_name, ...]}}
//!
//! WwiseIdMapping.bytes  (AES-256-CBC)
//!     │   decrypt_wwise_event_table()
//!     ▼
//!   event_id → event_name 映射 (全局)
//!
//! sound/Windows/d/{lang}/dx_{前缀}.pck
//!     │   parse_akpk() → wem_id → offset
//!     │   extract_banks_from_pck() + collect_hirc_mappings()
//!     │   → wem_id → event_id → event_name
//!     │   匹配 slot → wem_id
//!     │   extract_wem() → RIFF
//!     │   wem_to_wav() → WAV
//!     │   wav_to_mp3() → MP3
//!     ▼
//!   exports/card-voices/{lang}/{prefix}/{slot}.mp3
//!           +
//!   voice_index.json
//! ```

use anyhow::Context;
use indicatif::{ProgressBar, ProgressStyle};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::{extract_wem, parse_akpk, wav_to_mp3, wem_to_wav};

// ============================================================================
// 常量
// ============================================================================

/// CardResourceMaster 中各列对应的语音 slot 基类
/// 索引从 0 开始，值经过 classify_* 函数后可能变成带 ID 的动态 slot 名
const VOICE_COLUMNS: &[(usize, &str)] = &[
    (43, "play"),   // Play_dx_{prefix}_1, Play_dx_{prefix}_1_enh, ...
    (45, "attack"), // Play_dx_{prefix}_2
    (46, "evo_attack"),
    (47, "evolve"),  // Play_dx_{prefix}_4, Play_dx_{prefix}_4_sp
    (48, "destroy"), // Play_dx_{prefix}_3
    (49, "evo_destroy"),
    (50, "skill"),
    (51, "evo_skill"),
    (52, "act"),
];

/// 语言 → pck 子目录映射
const LANG_DIRS: &[(&str, &str)] = &[("eng", "English(US)"), ("jpn", "Japanese(JP)")];

/// Play 后缀分类规则: (suffix, slot_name)
/// 这些是静态映射的后缀，不含 ID 项
const PLAY_SUFFIX_RULES: &[(&str, &str)] = &[
    ("1_enh8", "play_enhance_8"),
    ("1_enh7", "play_enhance_7"),
    ("1_enh4", "play_enhance_4"),
    ("1_enh", "play_enhance"),
    ("1_sky", "play_sky"),
    ("1_super_sky", "play_super_sky"),
    ("1_lottery", "play_lottery"),
    ("1", "play"),
];

/// slot 显示顺序（参照 W2AU 的 SLOT_ORDER）
const SLOT_ORDER: &[(&str, u32)] = &[
    ("play", 0),
    ("play_enhance", 1),
    ("play_enhance_4", 2),
    ("play_enhance_7", 3),
    ("play_enhance_8", 4),
    ("play_sky", 5),
    ("play_super_sky", 6),
    ("play_mode1", 7),
    ("play_mode2", 8),
    ("play_mode3", 9),
    ("play_mode4", 10),
    ("play_lottery", 11),
    ("play_skill", 12),
    ("play_cross", 13),
    ("play_pair", 14),
    ("play_token", 15),
    ("attack", 20),
    ("evo_attack", 21),
    ("evolve", 30),
    ("super_evolve", 31),
    ("destroy", 40),
    ("evo_destroy", 41),
    ("skill", 50),
    ("evo_skill", 51),
    ("act", 60),
    ("act_mode1", 61),
    ("act_mode2", 62),
];

// ============================================================================
// 槽位标签（多语言）
// ============================================================================

fn slot_labels() -> BTreeMap<&'static str, SlotLabel> {
    let mut m = BTreeMap::new();
    m.insert("play", lbl("打出", "Play", "登場", "등장", "登場"));
    m.insert(
        "play_enhance",
        lbl("爆能", "Enhance", "エンハンス", "인핸스", "爆能"),
    );
    m.insert(
        "play_enhance_4",
        lbl(
            "爆能(4)",
            "Enhance(4)",
            "エンハンス(4)",
            "인핸스(4)",
            "爆能(4)",
        ),
    );
    m.insert(
        "play_enhance_7",
        lbl(
            "爆能(7)",
            "Enhance(7)",
            "エンハンス(7)",
            "인핸스(7)",
            "爆能(7)",
        ),
    );
    m.insert(
        "play_enhance_8",
        lbl(
            "爆能(8)",
            "Enhance(8)",
            "エンハンス(8)",
            "인핸스(8)",
            "爆能(8)",
        ),
    );
    m.insert("play_sky", lbl("奥义", "Sky", "奥義", "오의", "奧義"));
    m.insert(
        "play_super_sky",
        lbl("解放奥义", "Super Sky", "解放奥義", "해방 오의", "解放奧義"),
    );
    m.insert(
        "play_mode1",
        lbl("模式1", "Mode 1", "モード1", "모드1", "模式1"),
    );
    m.insert(
        "play_mode2",
        lbl("模式2", "Mode 2", "モード2", "모드2", "模式2"),
    );
    m.insert(
        "play_mode3",
        lbl("模式3", "Mode 3", "モード3", "모드3", "模式3"),
    );
    m.insert(
        "play_mode4",
        lbl("模式4", "Mode 4", "モード4", "모드4", "模式4"),
    );
    m.insert(
        "play_lottery",
        lbl("抽卡", "Lottery", "カード排出", "카드뽑기", "抽卡"),
    );
    m.insert("play_skill", lbl("技能", "Skill", "スキル", "스킬", "技能"));
    m.insert("play_cross", lbl("关联", "Cross", "関連", "관련", "關聯"));
    m.insert("play_pair", lbl("联动", "Pair", "関連", "관련", "聯動"));
    m.insert(
        "play_token",
        lbl("Token", "Token", "トークン", "토큰", "Token"),
    );
    m.insert("attack", lbl("攻击", "Attack", "攻撃", "공격", "攻擊"));
    m.insert(
        "evo_attack",
        lbl(
            "进化攻击",
            "Evo Attack",
            "進化攻撃",
            "진화 공격",
            "進化攻擊",
        ),
    );
    m.insert("evolve", lbl("进化", "Evolve", "進化", "진화", "進化"));
    m.insert(
        "super_evolve",
        lbl("超进化", "Super Evolve", "超進化", "초진화", "超進化"),
    );
    m.insert("destroy", lbl("破坏", "Destroy", "破壊", "파괴", "破壞"));
    m.insert(
        "evo_destroy",
        lbl(
            "进化破坏",
            "Evo Destroy",
            "進化破壊",
            "진화 파괴",
            "進化破壞",
        ),
    );
    m.insert("skill", lbl("技能", "Skill", "スキル", "스킬", "技能"));
    m.insert(
        "evo_skill",
        lbl(
            "进化技能",
            "Evo Skill",
            "進化スキル",
            "진화 스킬",
            "進化技能",
        ),
    );
    m.insert("act", lbl("行动", "Act", "行動", "행동", "行動"));
    m.insert(
        "act_mode1",
        lbl(
            "行动模式1",
            "Act Mode 1",
            "行動モード1",
            "행동 모드1",
            "行動模式1",
        ),
    );
    m.insert(
        "act_mode2",
        lbl("行动模式2", "Act Mode 2", "行動モード2", "kor", "行動模式2"),
    );
    m
}

fn lbl(chs: &str, eng: &str, jpn: &str, kor: &str, cht: &str) -> SlotLabel {
    SlotLabel {
        chs: chs.into(),
        eng: eng.into(),
        jpn: jpn.into(),
        kor: kor.into(),
        cht: cht.into(),
    }
}

// ============================================================================
// 数据结构
// ============================================================================

/// 槽位多语言标签
#[derive(Debug, Clone, serde::Serialize)]
pub struct SlotLabel {
    pub chs: String,
    pub eng: String,
    pub jpn: String,
    pub kor: String,
    pub cht: String,
}

/// voice_index.json 的顶层结构
#[derive(Debug, Clone, serde::Serialize)]
pub struct VoiceIndex {
    pub labels: BTreeMap<String, SlotLabel>,
    /// cards[lang][prefix][slot] = "eng/10001110/play.mp3"
    pub cards: BTreeMap<String, BTreeMap<String, BTreeMap<String, String>>>,
}

/// 卡牌语音提取结果
#[derive(Debug, Default)]
pub struct CardVoiceStats {
    pub cards_processed: usize,
    pub files_output: usize,
    pub files_skipped: usize,
    pub failed: usize,
}

// ============================================================================
// 公共 API
// ============================================================================

/// 从 CardResourceMaster 构建 prefix → voice slots 映射。
///
/// slot 名为动态生成的完整名称，如 `play_pair_10721110` 而非坍缩的 `play_pair`。
pub fn build_voice_map(
    card_resource_path: &Path,
) -> anyhow::Result<BTreeMap<String, BTreeMap<String, Vec<String>>>> {
    let json =
        std::fs::read_to_string(card_resource_path).context("无法读取 CardResourceMaster.json")?;
    let data: Vec<Vec<serde_json::Value>> = serde_json::from_str(&json)?;

    let mut result: BTreeMap<String, BTreeMap<String, Vec<String>>> = BTreeMap::new();

    for rec in &data {
        let prefix_raw = rec.get(42).and_then(|v| v.as_str()).unwrap_or("");
        if !prefix_raw.starts_with("dx_") {
            continue;
        }
        let prefix = prefix_raw.strip_prefix("dx_").unwrap_or(prefix_raw);

        let mut slots: BTreeMap<String, Vec<String>> = BTreeMap::new();

        for &(col, base_slot) in VOICE_COLUMNS {
            let val = rec.get(col).and_then(|v| v.as_str()).unwrap_or("");
            if val.is_empty() {
                continue;
            }
            let events: Vec<String> = val
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| s.starts_with("Play_dx_"))
                .collect();
            if events.is_empty() {
                continue;
            }

            match base_slot {
                "play" => {
                    for evt in &events {
                        let suffix = evt
                            .strip_prefix(&format!("Play_dx_{}_", prefix))
                            .unwrap_or(evt);
                        let slot = classify_play_suffix(suffix);
                        slots.entry(slot).or_default().push(evt.clone());
                    }
                }
                "evolve" => {
                    for evt in &events {
                        let slot = if evt.contains("_sp") {
                            "super_evolve".to_string()
                        } else {
                            "evolve".to_string()
                        };
                        slots.entry(slot).or_default().push(evt.clone());
                    }
                }
                "skill" => {
                    for evt in &events {
                        let slot = if evt.contains("_evo") {
                            "evo_skill".to_string()
                        } else {
                            "skill".to_string()
                        };
                        slots.entry(slot).or_default().push(evt.clone());
                    }
                }
                "act" => {
                    for evt in &events {
                        let suffix = evt
                            .strip_prefix(&format!("Play_dx_{}_", prefix))
                            .unwrap_or(evt);
                        let slot = classify_act_suffix(suffix);
                        slots.entry(slot).or_default().push(evt.clone());
                    }
                }
                _ => {
                    slots
                        .entry(base_slot.to_string())
                        .or_default()
                        .extend(events.clone());
                }
            }
        }

        if !slots.is_empty() {
            // 按 SLOT_ORDER 排序
            let mut sorted: BTreeMap<String, Vec<String>> = BTreeMap::new();
            // BTreeMap 默认按 key 字母序，不够好。我们手动按 slot_sort_key 排
            let mut slot_keys: Vec<String> = slots.keys().cloned().collect();
            slot_keys.sort_by(|a, b| slot_sort_key(a).cmp(&slot_sort_key(b)));
            for k in slot_keys {
                if let Some(v) = slots.remove(&k) {
                    sorted.insert(k, v);
                }
            }
            result.insert(prefix.to_string(), sorted);
        }
    }

    Ok(result)
}

/// 提取所有卡牌语音。
pub fn extract_card_voices(
    pck_root: &Path,
    output_dir: &Path,
    card_resource_path: &Path,
    audio_wav_dir: &Path,
    mapping_data: &[u8],
    vgmstream_path: &Path,
    ffmpeg_path: &str,
) -> anyhow::Result<CardVoiceStats> {
    let voice_map = build_voice_map(card_resource_path)?;
    let event_table = crate::wwise::decrypt_wwise_event_table(mapping_data)
        .context("无法解密 WwiseIdMapping.bytes")?;
    tracing::info!("Wwise 事件表: {} 个条目", event_table.len());
    println!("CardResourceMaster: {} 个声音前缀", voice_map.len());

    let mut stats = CardVoiceStats::default();
    let mut voice_index = VoiceIndex {
        labels: BTreeMap::new(),
        cards: BTreeMap::new(),
    };

    let mut all_slots: HashSet<String> = HashSet::new();

    for &(lang, lang_dir) in LANG_DIRS {
        println!("\n=== {} ({}) ===", lang, lang_dir);

        let pck_dir = pck_root.join("Windows").join("d").join(lang_dir);
        let prefix_count = voice_map.len();
        let pb = ProgressBar::new(prefix_count as u64);
        pb.set_style(
            ProgressStyle::with_template("{spinner} [{bar:30}] {pos}/{len} {msg}")
                .unwrap()
                .progress_chars("=> "),
        );

        let mut lang_cards: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();

        for (prefix, slots) in &voice_map {
            pb.set_message(format!("{}", prefix));

            let pck_path = pck_dir.join(format!("dx_{}.pck", prefix));
            if !pck_path.exists() {
                pb.inc(1);
                continue;
            }

            match process_card_pck(
                &pck_path,
                slots,
                lang,
                prefix,
                output_dir,
                audio_wav_dir,
                vgmstream_path,
                ffmpeg_path,
                &event_table,
            ) {
                Ok((card_slots, skipped)) => {
                    stats.files_skipped += skipped;
                    if !card_slots.is_empty() {
                        lang_cards.insert(prefix.clone(), card_slots.clone());
                        stats.cards_processed += 1;
                        for slot in card_slots.keys() {
                            all_slots.insert(slot.clone());
                        }
                        stats.files_output += card_slots.len();
                    }
                }
                Err(e) => {
                    tracing::warn!("{}: {}", prefix, e);
                }
            }

            pb.inc(1);
        }

        pb.finish_and_clear();
        voice_index.cards.insert(lang.to_string(), lang_cards);
        println!("  {} 张卡有语音", stats.cards_processed);
    }

    // 构建 labels：基类 slot 用预定义标签，变体 slot 用动态标签
    let base_labels = slot_labels();
    for slot in &all_slots {
        if voice_index.labels.contains_key(slot) {
            continue;
        }
        voice_index
            .labels
            .insert(slot.clone(), make_slot_label(slot, &base_labels));
    }

    let index_path = output_dir.join("voice_index.json");
    let json = serde_json::to_string_pretty(&voice_index)?;
    std::fs::write(&index_path, json)?;
    println!("\nvoice_index.json → {}", index_path.display());
    println!(
        "总计: {} 张卡, {} 个 MP3 (跳过: {})",
        stats.cards_processed, stats.files_output, stats.files_skipped
    );

    Ok(stats)
}

// ============================================================================
// 内部函数
// ============================================================================

/// 处理单个 pck 文件：匹配 slot → event → wem → MP3。
///
/// 优先复用 `audio_wav_dir` 中已有的 WAV（避免重复解码），
/// 不存在时才回退到 pck 解包 + vgmstream。
fn process_card_pck(
    pck_path: &Path,
    slots: &BTreeMap<String, Vec<String>>,
    lang: &str,
    prefix: &str,
    output_root: &Path,
    audio_wav_dir: &Path,
    vgmstream_path: &Path,
    ffmpeg_path: &str,
    event_table: &BTreeMap<u32, String>,
) -> anyhow::Result<(BTreeMap<String, String>, usize)> {
    use crate::wwise::{collect_hirc_mappings, extract_banks_from_pck};

    let card_out = output_root.join(lang).join(prefix);
    std::fs::create_dir_all(&card_out)?;

    let pck_data = std::fs::read(pck_path)?;
    let wem_offsets = parse_akpk(&pck_data);
    if wem_offsets.is_empty() {
        return Ok((BTreeMap::new(), 0));
    }

    // 解析 HIRC：wem_id → event_name
    let mut wem_to_sound = BTreeMap::new();
    let mut sound_to_action = BTreeMap::new();
    let mut action_to_event = BTreeMap::new();

    let banks = extract_banks_from_pck(&pck_data);
    for bank in &banks {
        collect_hirc_mappings(
            bank,
            &mut wem_to_sound,
            &mut sound_to_action,
            &mut action_to_event,
        );
    }

    let mut wem_to_name: BTreeMap<u32, String> = BTreeMap::new();
    for (wem_id, sound_id) in &wem_to_sound {
        if let Some(action_id) = sound_to_action.get(sound_id) {
            if let Some(event_id) = action_to_event.get(action_id) {
                if let Some(name) = event_table.get(event_id) {
                    wem_to_name.insert(*wem_id, name.clone());
                }
            }
        }
    }

    // 反转：event_name → [wem_id]
    let mut name_to_wems: HashMap<String, Vec<u32>> = HashMap::new();
    for (wem_id, name) in &wem_to_name {
        name_to_wems.entry(name.clone()).or_default().push(*wem_id);
    }

    // 构建：wem_id → slot（可能有多个 slot 指向同一 wem）
    let mut wem_to_slot: HashMap<u32, String> = HashMap::new();
    for (slot, events) in slots {
        for evt in events {
            if let Some(wem_ids) = name_to_wems.get(evt) {
                for &wid in wem_ids {
                    wem_to_slot.entry(wid).or_insert_with(|| slot.clone());
                }
            }
        }
    }

    // 反转：slot → [wem_id]（一个 slot 可能匹配多个 WEM，取第一个匹配的）
    let mut slot_to_wems: HashMap<String, Vec<u32>> = HashMap::new();
    for (wem_id, slot) in &wem_to_slot {
        slot_to_wems.entry(slot.clone()).or_default().push(*wem_id);
    }

    // 提取并转码：每个 slot 对应一个 MP3 文件
    let mut result: BTreeMap<String, String> = BTreeMap::new();
    let mut skipped = 0usize;

    for (slot, wem_ids) in &slot_to_wems {
        let mp3_path = card_out.join(format!("{}.mp3", slot));

        // 跳过已存在（增量）
        if mp3_path.exists() {
            skipped += 1;
            result.insert(slot.clone(), format!("{}/{}/{}.mp3", lang, prefix, slot));
            continue;
        }

        // 取第一个匹配的 wem_id
        let wem_id = match wem_ids.first() {
            Some(&id) => id,
            None => continue,
        };

        // 获取事件名（用于查找已有 WAV）
        let event_name = wem_to_name.get(&wem_id);

        // 1) 尝试复用 wbu audio 已提取的 WAV
        let wav_source = event_name.and_then(|name| {
            let wav_path = audio_wav_dir.join(lang).join(format!("{}.wav", name));
            if wav_path.exists() {
                Some(wav_path)
            } else {
                None
            }
        });

        // 2) 回退：从 pck 提取 WEM → 临时 WAV
        let need_tmp = wav_source.is_none();
        let tmp_wav = if need_tmp {
            if let Some(&offset) = wem_offsets.get(&wem_id) {
                if let Some(wem_bytes) = extract_wem(&pck_data, offset) {
                    let tmp = card_out.join(format!("_tmp_{}.wav", wem_id));
                    match wem_to_wav(wem_bytes, &tmp, vgmstream_path) {
                        Ok(()) => Some(tmp),
                        Err(_) => None,
                    }
                } else {
                    None
                }
            } else {
                None
            }
        } else {
            None
        };

        // 3) WAV → MP3
        let source_wav = wav_source.as_ref().or(tmp_wav.as_ref());
        if let Some(wav_path) = source_wav {
            if wav_to_mp3(wav_path, &mp3_path, ffmpeg_path).is_ok() {
                if let Some(ref tmp) = tmp_wav {
                    let _ = std::fs::remove_file(tmp);
                }
                result.insert(slot.clone(), format!("{}/{}/{}.mp3", lang, prefix, slot));
            }
        } else if need_tmp {
            let _ = std::fs::remove_file(card_out.join(format!("_tmp_{}.wav", wem_id)));
        }
    }

    // 按 slot_sort_key 排序结果
    let mut sorted: BTreeMap<String, String> = BTreeMap::new();
    let mut keys: Vec<String> = result.keys().cloned().collect();
    keys.sort_by(|a, b| slot_sort_key(a).cmp(&slot_sort_key(b)));
    for k in keys {
        if let Some(v) = result.remove(&k) {
            sorted.insert(k, v);
        }
    }

    Ok((sorted, skipped))
}

// ============================================================================
// 后缀分类（参照 W2AU extract_card_audio.py）
// ============================================================================

/// 分类 Play 事件的后缀 → slot 名。
///
/// 动态后缀会在 slot 名中保留目标 ID：
/// - `9_10721110` → `play_pair_10721110`
/// - `7_10721110` → `play_cross_10721110`
/// - `8_xxx` → `play_token_xxx`
/// - `1_skill_xxx` → `play_skill_xxx`
/// - `1_mode2` → `play_mode2`
fn classify_play_suffix(suffix: &str) -> String {
    // 优先匹配带 ID 的动态后缀
    if suffix.starts_with("9_") {
        return format!("play_pair_{}", &suffix[2..]);
    }
    if suffix.starts_with("7_") {
        return format!("play_cross_{}", &suffix[2..]);
    }
    if suffix.starts_with("8_") || suffix.starts_with("11_") {
        return format!("play_token_{}", &suffix[2..]);
    }
    if suffix.starts_with("1_skill_") {
        return format!("play_skill_{}", &suffix[8..]);
    }
    if suffix.starts_with("1_mode") {
        return format!("play_mode{}", &suffix[6..]);
    }

    // 静态规则
    for &(key, slot) in PLAY_SUFFIX_RULES {
        if suffix == key || suffix.starts_with(key) {
            return slot.to_string();
        }
    }
    "play".to_string()
}

/// 分类 Act 事件的后缀 → slot 名。
fn classify_act_suffix(suffix: &str) -> String {
    if suffix.starts_with("10_mode") {
        return format!("act_mode{}", &suffix[7..]);
    }
    "act".to_string()
}

// ============================================================================
// Slot 排序与标签
// ============================================================================

/// slot 排序键：基类顺序 → 变体后缀数字 → 名称
fn slot_sort_key(slot: &str) -> (u32, u32, &str) {
    let base_order = SLOT_ORDER
        .iter()
        .find(|&&(b, _)| slot == b || slot.starts_with(&format!("{}_", b)))
        .map(|&(_, o)| o)
        .unwrap_or(99);

    // 提取变体后缀数字：play_pair_10721110 → 10721110
    let variant_num = slot
        .rsplit('_')
        .next()
        .and_then(|s| s.parse::<u32>().ok())
        .unwrap_or(0);

    (base_order, variant_num, slot)
}

/// 为动态 slot 生成标签。
///
/// 基类 slot (play, attack, ...) 使用预定义标签；
/// 变体 slot (play_pair_xxx, play_cross_xxx, ...) 生成 `基类名·ID` 格式。
fn make_slot_label(slot: &str, base_labels: &BTreeMap<&'static str, SlotLabel>) -> SlotLabel {
    // 查预定义标签
    if let Some(lbl) = base_labels.get(slot) {
        return lbl.clone();
    }

    // 尝试匹配变体 slot: play_pair_10721110 → base="play_pair", suffix="10721110"
    for (base, _) in SLOT_ORDER {
        if slot.starts_with(base)
            && slot.len() > base.len() + 1
            && slot.as_bytes()[base.len()] == b'_'
        {
            let suffix = &slot[base.len() + 1..];
            if let Some(base_lbl) = base_labels.get(base) {
                return SlotLabel {
                    chs: format!("{}·{}", base_lbl.chs, suffix),
                    eng: format!("{}·{}", base_lbl.eng, suffix),
                    jpn: format!("{}·{}", base_lbl.jpn, suffix),
                    kor: format!("{}·{}", base_lbl.kor, suffix),
                    cht: format!("{}·{}", base_lbl.cht, suffix),
                };
            }
            break;
        }
    }

    // 兜底
    SlotLabel {
        chs: slot.to_string(),
        eng: slot.to_string(),
        jpn: slot.to_string(),
        kor: slot.to_string(),
        cht: slot.to_string(),
    }
}
