//! LeaderSkin detail voice extraction.
//!
//! Exports Wwise events named `Play_dx_dtl_{id}_{animation}_{option}` into
//! `exports/leader-skin-voices/{lang}/{id}/{animation}_{option}.mp3`.

use anyhow::Context;
use indicatif::{ProgressBar, ProgressStyle};
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::{extract_wem, parse_akpk, wav_to_mp3, wem_to_wav};

const LANG_DIRS: &[(&str, &str)] = &[("eng", "English(US)"), ("jpn", "Japanese(JP)")];

#[derive(Debug, Default)]
pub struct LeaderSkinVoiceStats {
    pub pck_files: usize,
    pub files_output: usize,
    pub files_skipped: usize,
    pub failed: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DetailEvent {
    id: String,
    animation: String,
    option: String,
}

#[derive(Debug, Serialize)]
struct LeaderSkinVoiceIndex {
    leaders: BTreeMap<String, BTreeMap<String, BTreeMap<String, Vec<String>>>>,
}

pub fn extract_leader_skin_voices(
    pck_root: &Path,
    output_dir: &Path,
    audio_wav_dir: &Path,
    mapping_data: &[u8],
    vgmstream_path: &Path,
    ffmpeg_path: &str,
) -> anyhow::Result<LeaderSkinVoiceStats> {
    let event_table = crate::wwise::decrypt_wwise_event_table(mapping_data)
        .context("无法解密 WwiseIdMapping.bytes")?;
    let mut stats = LeaderSkinVoiceStats::default();
    let mut index = LeaderSkinVoiceIndex {
        leaders: BTreeMap::new(),
    };

    for &(lang, lang_dir) in LANG_DIRS {
        let pck_dir = pck_root.join("Windows").join("d").join(lang_dir);
        let pck_files = detail_pck_files(&pck_dir)?;
        stats.pck_files += pck_files.len();
        let pb = ProgressBar::new(pck_files.len() as u64);
        pb.set_style(
            ProgressStyle::with_template("{spinner} [{bar:30}] {pos}/{len} {msg}")?
                .progress_chars("=> "),
        );

        let mut lang_index: BTreeMap<String, BTreeMap<String, Vec<String>>> = BTreeMap::new();
        for pck_path in pck_files {
            let message = pck_path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("dx_dtl")
                .to_string();
            pb.set_message(message);
            match process_detail_pck(
                &pck_path,
                lang,
                output_dir,
                audio_wav_dir,
                vgmstream_path,
                ffmpeg_path,
                &event_table,
                &mut lang_index,
            ) {
                Ok(result) => {
                    stats.files_output += result.output;
                    stats.files_skipped += result.skipped;
                }
                Err(err) => {
                    tracing::warn!("{}: {err}", pck_path.display());
                    stats.failed += 1;
                }
            }
            pb.inc(1);
        }
        pb.finish_and_clear();
        normalize_lang_index(&mut lang_index);
        index.leaders.insert(lang.to_string(), lang_index);
    }

    std::fs::create_dir_all(output_dir)?;
    let index_path = output_dir.join("voice_index.json");
    std::fs::write(&index_path, serde_json::to_string_pretty(&index)?)?;
    Ok(stats)
}

#[derive(Debug, Default)]
struct ProcessResult {
    output: usize,
    skipped: usize,
}

#[allow(clippy::too_many_arguments)]
fn process_detail_pck(
    pck_path: &Path,
    lang: &str,
    output_dir: &Path,
    audio_wav_dir: &Path,
    vgmstream_path: &Path,
    ffmpeg_path: &str,
    event_table: &BTreeMap<u32, String>,
    lang_index: &mut BTreeMap<String, BTreeMap<String, Vec<String>>>,
) -> anyhow::Result<ProcessResult> {
    use crate::wwise::{collect_hirc_mappings, extract_banks_from_pck};

    let pck_data = std::fs::read(pck_path)?;
    let wem_offsets = parse_akpk(&pck_data);
    if wem_offsets.is_empty() {
        return Ok(ProcessResult::default());
    }

    let mut wem_to_sound = BTreeMap::new();
    let mut sound_to_action = BTreeMap::new();
    let mut action_to_event = BTreeMap::new();
    for bank in extract_banks_from_pck(&pck_data) {
        collect_hirc_mappings(
            &bank,
            &mut wem_to_sound,
            &mut sound_to_action,
            &mut action_to_event,
        );
    }

    let mut wem_to_name: BTreeMap<u32, String> = BTreeMap::new();
    for (wem_id, sound_id) in &wem_to_sound {
        if let Some(action_id) = sound_to_action.get(sound_id)
            && let Some(event_id) = action_to_event.get(action_id)
            && let Some(name) = event_table.get(event_id)
        {
            wem_to_name.insert(*wem_id, name.clone());
        }
    }

    let mut result = ProcessResult::default();
    for (wem_id, event_name) in wem_to_name {
        let Some(event) = parse_detail_event(&event_name) else {
            continue;
        };
        let Some(offset) = wem_offsets.get(&wem_id) else {
            continue;
        };
        let rel_path = format!(
            "{}/{}/{}_{}.mp3",
            lang, event.id, event.animation, event.option
        );
        let mp3_path = output_dir.join(&rel_path);
        if mp3_path.exists() {
            insert_voice(lang_index, &event, rel_path);
            result.skipped += 1;
            continue;
        }
        if let Some(parent) = mp3_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        if output_voice_file(
            &pck_data,
            *offset,
            &event_name,
            audio_wav_dir,
            lang,
            &mp3_path,
            vgmstream_path,
            ffmpeg_path,
        )? {
            insert_voice(lang_index, &event, rel_path);
            result.output += 1;
        }
    }
    Ok(result)
}

#[allow(clippy::too_many_arguments)]
fn output_voice_file(
    pck_data: &[u8],
    offset: u32,
    event_name: &str,
    audio_wav_dir: &Path,
    lang: &str,
    mp3_path: &Path,
    vgmstream_path: &Path,
    ffmpeg_path: &str,
) -> anyhow::Result<bool> {
    let wav_source = audio_wav_dir.join(lang).join(format!("{event_name}.wav"));
    if wav_source.exists() {
        wav_to_mp3(&wav_source, mp3_path, ffmpeg_path)?;
        return Ok(true);
    }

    let Some(wem_data) = extract_wem(pck_data, offset) else {
        return Ok(false);
    };
    let tmp_wav = mp3_path.with_extension("tmp.wav");
    wem_to_wav(wem_data, &tmp_wav, vgmstream_path)?;
    let convert_result = wav_to_mp3(&tmp_wav, mp3_path, ffmpeg_path);
    let remove_result = std::fs::remove_file(&tmp_wav);
    if let Err(err) = convert_result {
        return Err(err);
    }
    if let Err(err) = remove_result {
        tracing::warn!("临时 WAV 删除失败 {}: {err}", tmp_wav.display());
    }
    Ok(true)
}

fn detail_pck_files(pck_dir: &Path) -> anyhow::Result<Vec<PathBuf>> {
    if !pck_dir.exists() {
        return Ok(Vec::new());
    }
    let mut files = Vec::new();
    for entry in std::fs::read_dir(pck_dir)? {
        let path = entry?.path();
        let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
            continue;
        };
        if name.starts_with("dx_dtl_") && name.ends_with(".pck") {
            files.push(path);
        }
    }
    files.sort();
    Ok(files)
}

fn parse_detail_event(event_name: &str) -> Option<DetailEvent> {
    let rest = event_name
        .strip_prefix("Play_dx_dtl_")
        .or_else(|| event_name.strip_prefix("play_dx_dtl_"))?;
    let (id, detail) = rest.split_once('_')?;
    let (animation, option) = detail.rsplit_once('_')?;
    if id.is_empty() || animation.is_empty() || option.is_empty() {
        return None;
    }
    Some(DetailEvent {
        id: id.to_string(),
        animation: animation.to_string(),
        option: option.to_string(),
    })
}

fn insert_voice(
    lang_index: &mut BTreeMap<String, BTreeMap<String, Vec<String>>>,
    event: &DetailEvent,
    rel_path: String,
) {
    lang_index
        .entry(event.id.clone())
        .or_default()
        .entry(event.animation.clone())
        .or_default()
        .push(rel_path);
}

fn normalize_lang_index(lang_index: &mut BTreeMap<String, BTreeMap<String, Vec<String>>>) {
    for animations in lang_index.values_mut() {
        for paths in animations.values_mut() {
            paths.sort();
            paths.dedup();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_detail_event_when_animation_contains_underscores() {
        let event = parse_detail_event("Play_dx_dtl_1001_03_hello_extra_2")
            .expect("detail event should parse");

        assert_eq!(event.id, "1001");
        assert_eq!(event.animation, "03_hello_extra");
        assert_eq!(event.option, "2");
    }

    #[test]
    fn rejects_non_detail_event() {
        assert_eq!(parse_detail_event("Play_dx_home_1001_1"), None);
    }
}
