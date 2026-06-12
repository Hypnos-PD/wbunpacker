//! MasterMemory 数据表导出模块
//!
//! # 概述
//!
//! 解析游戏的 Master/mastermemory.bytes 文件，
//! 将其中的 173 个 MasterMemory 表全部导出为 JSON。
//!
//! # 文件格式
//!
//! 与 manifest 相同的 MasterMemory 二进制格式：
//! 1. msgpack 编码的 TOC，末尾附加 16 字节 MD5
//! 2. TOC 顶层是 map，key 是表名，value 是 [offset, length]
//! 3. offset 相对于 TOC 结束位置（切片时须用原始数据含 MD5，
//!    因为最后一张表可能延伸进 MD5 区域）
//! 4. 表数据：直接 msgpack 数组（小表）或 Ext(99) LZ4 压缩（大表）

use anyhow::{anyhow, Context};
use rmpv::Value;
use std::collections::BTreeMap;
use std::path::Path;

// ---------------------------------------------------------------------------
// 数据结构
// ---------------------------------------------------------------------------

/// 单表导出结果
#[derive(Debug)]
pub struct ExportResult {
    pub name: String,
    pub rows: usize,
    pub path: String,
}

// ---------------------------------------------------------------------------
// TOC 解析
// ---------------------------------------------------------------------------

/// 解析 MasterMemory 文件的 TOC。
///
/// 返回 (toc_end, tables)：
/// - toc_end: TOC msgpack 体结束的字节位置
/// - tables: 表名到 (offset, length) 的映射（offset 相对于 toc_end）
pub fn parse_toc(raw: &[u8]) -> anyhow::Result<(usize, BTreeMap<String, (usize, usize)>)> {
    if raw.len() < 16 {
        return Err(anyhow!("数据太短: {} 字节", raw.len()));
    }

    let body = &raw[..raw.len() - 16];
    let mut cursor = &body[..];
    let root = rmpv::decode::value::read_value(&mut cursor)
        .with_context(|| "TOC msgpack 解码失败")?;
    let toc_end = body.len() - cursor.len();

    let map = root
        .as_map()
        .ok_or_else(|| anyhow!("TOC 根结构不是 map"))?;

    let mut tables = BTreeMap::new();
    for (k, v) in map {
        let name = k
            .as_str()
            .map(|s| s.to_string())
            .unwrap_or_else(|| format!("{:?}", k));
        let pair = v
            .as_array()
            .ok_or_else(|| anyhow!("TOC entry '{}' 不是数组", name))?;
        if pair.len() < 2 {
            return Err(anyhow!("TOC entry '{}' 字段不足", name));
        }
        let off = pair[0].as_i64().unwrap_or(0) as usize;
        let len = match &pair[1] {
            Value::Integer(i) => i.as_i64().unwrap_or(0) as usize,
            _ => 0,
        };
        tables.insert(name, (off, len));
    }

    Ok((toc_end, tables))
}

// ---------------------------------------------------------------------------
// 表提取
// ---------------------------------------------------------------------------

/// 从原始数据中提取并解压一张表。
///
/// 使用含 MD5 的 raw 切片，因为最后一张表可能延伸进 MD5 区域。
pub fn extract_table(
    raw: &[u8],
    toc_end: usize,
    off: usize,
    len: usize,
    name: &str,
) -> anyhow::Result<Vec<Value>> {
    let actual = toc_end + off;
    let table_data = raw
        .get(actual..actual + len)
        .ok_or_else(|| anyhow!("表 '{}' 偏移越界: {} + {} > {}", name, actual, len, raw.len()))?;

    let value = rmpv::decode::value::read_value(&mut &table_data[..])
        .with_context(|| format!("表 '{}' msgpack 解码失败", name))?;

    match value {
        Value::Ext(99, ref ext_data) => {
            let mut cursor = &ext_data[..];
            rmpv::decode::value::read_value(&mut cursor)
                .with_context(|| format!("表 '{}' 元数据解码失败", name))?;
            let skip = ext_data.len() - cursor.len();
            let compressed = &ext_data[skip..];

            let max_size = compressed.len() * 100;
            let decompressed = lz4_flex::block::decompress(compressed, max_size)
                .with_context(|| format!("表 '{}' LZ4 解压失败", name))?;

            let rows = rmpv::decode::value::read_value(&mut &decompressed[..])
                .with_context(|| format!("表 '{}' 解压后 msgpack 解码失败", name))?;

            rows.as_array()
                .cloned()
                .ok_or_else(|| anyhow!("表 '{}' 解压后不是 msgpack 数组", name))
        }
        Value::Array(rows) => Ok(rows),
        other => Err(anyhow!("表 '{}' 格式不支持: {:?}", name, other)),
    }
}

// ---------------------------------------------------------------------------
// JSON 转换
// ---------------------------------------------------------------------------

fn to_json_value(v: &Value) -> serde_json::Value {
    match v {
        Value::Nil => serde_json::Value::Null,
        Value::Boolean(b) => serde_json::Value::Bool(*b),
        Value::Integer(i) => {
            if let Some(n) = i.as_i64() {
                serde_json::Value::Number(n.into())
            } else if let Some(n) = i.as_u64() {
                serde_json::Value::Number(n.into())
            } else {
                serde_json::Value::Null
            }
        }
        Value::F32(f) => serde_json::json!(*f),
        Value::F64(f) => serde_json::json!(*f),
        Value::String(s) => match s.as_str() {
            Some(utf8) => serde_json::Value::String(utf8.to_string()),
            None => serde_json::Value::String(
                String::from_utf8_lossy(s.as_bytes()).into_owned(),
            ),
        },
        Value::Binary(b) => serde_json::Value::Array(
            b.iter().map(|&x| serde_json::Value::Number(x.into())).collect(),
        ),
        Value::Array(arr) => {
            serde_json::Value::Array(arr.iter().map(to_json_value).collect())
        }
        Value::Map(map) => {
            let obj: serde_json::Map<String, serde_json::Value> = map
                .iter()
                .map(|(k, v)| (format!("{:?}", to_json_value(k)), to_json_value(v)))
                .collect();
            serde_json::Value::Object(obj)
        }
        Value::Ext(_, data) => serde_json::Value::Array(
            data.iter().map(|&x| serde_json::Value::Number(x.into())).collect(),
        ),
    }
}

// ---------------------------------------------------------------------------
// 批量导出
// ---------------------------------------------------------------------------

/// 导出所有表到指定目录。
///
/// - raw: 完整的 mastermemory.bytes 原始数据（含 MD5）
/// - output_dir: 输出目录，每个表一个 JSON 文件
pub fn export_all(raw: &[u8], output_dir: &Path) -> anyhow::Result<Vec<ExportResult>> {
    let (toc_end, tables) = parse_toc(raw)?;
    std::fs::create_dir_all(output_dir)
        .with_context(|| format!("无法创建输出目录: {}", output_dir.display()))?;

    let total = tables.len();
    tracing::info!("导出 {} 个表到 {}", total, output_dir.display());

    let mut results = Vec::with_capacity(total);

    for (name, (off, len)) in &tables {
        let rows = extract_table(raw, toc_end, *off, *len, name)?;
        let json_rows: Vec<serde_json::Value> = rows.iter().map(|r| to_json_value(r)).collect();

        let json = serde_json::to_string_pretty(&json_rows)
            .with_context(|| format!("表 '{}' JSON 序列化失败", name))?;

        let path = output_dir.join(format!("{}.json", name));
        std::fs::write(&path, json)
            .with_context(|| format!("无法写入: {}", path.display()))?;

        tracing::debug!("  {}: {} 行", name, rows.len());
        results.push(ExportResult {
            name: name.clone(),
            rows: rows.len(),
            path: path.display().to_string(),
        });
    }

    Ok(results)
}


// ---------------------------------------------------------------------------
// cards_full.json 生成
// ---------------------------------------------------------------------------

/// cards_full.json 的单卡条目
#[derive(Debug, Clone, serde::Serialize)]

struct CardFullEntry {
    card_id: i64,
    base_card_id: i64,
    card_style_id: i64,
    class: i64,
    cost: Option<serde_json::Value>,
    rarity: Option<serde_json::Value>,
    type_flags: Option<serde_json::Value>,
    is_evolution: bool,
    evolves_to: i64,
    skills: Vec<SkillEntry>,
    resource_id: i64,
    name_chs: String,
    name_eng: String,
    name_jpn: String,
    name_kor: String,
    name_cht: String,
    text_keys: TextKeys,
}

#[derive(Debug, Clone, serde::Serialize)]
struct SkillEntry {
    skill_id: i64,
    #[serde(rename = "type")]
    skill_type: i64,
    subtype: i64,
}

#[derive(Debug, Clone, serde::Serialize)]
struct TextKeys {
    name: String,
    skill_desc: String,
    flavor_1: String,
    flavor_2: String,
    cv: String,
}

/// 从已导出的 master-data JSON 文件生成 cards_full.json。
///
/// 需要先运行 `wbu master -v all` 导出所有 5 语言的主数据表。
///
/// # 参数
/// - `master_data_dir`: exports/master-data/ 目录（下面有 Chs/ Eng/ Jpn/ Kor/ Cht/ 子目录）
/// - `output_path`: cards_full.json 输出路径
pub fn generate_cards_full(
    master_data_dir: &Path,
    output_path: &Path,
) -> anyhow::Result<usize> {
    use std::collections::HashMap;

    // 读取核心表（全部从 Chs 读，结构各变体一致）
    let chs_dir = master_data_dir.join("Chs");
    let card_master: Vec<Vec<serde_json::Value>> = read_json_table(&chs_dir, "CardMaster.json")?;
    let base_card: Vec<Vec<serde_json::Value>> = read_json_table(&chs_dir, "BaseCardMaster.json")?;
    let card_text: Vec<Vec<serde_json::Value>> = read_json_table(&chs_dir, "CardText.json")?;
    let skill_master: Vec<Vec<serde_json::Value>> = read_json_table(&chs_dir, "SkillMaster.json")?;

    // 建立索引
    let bcm_by_id: HashMap<i64, &Vec<serde_json::Value>> = base_card.iter()
        .filter_map(|r| r[0].as_i64().map(|id| (id, r)))
        .collect();
    let ct_by_cs: HashMap<i64, &Vec<serde_json::Value>> = card_text.iter()
        .filter_map(|r| r[0].as_i64().map(|id| (id, r)))
        .collect();
    let skills_by_cid: HashMap<i64, Vec<&Vec<serde_json::Value>>> = {
        let mut m: HashMap<i64, Vec<&Vec<serde_json::Value>>> = HashMap::new();
        for r in &skill_master {
            if let Some(cid) = r.get(4).and_then(|v| v.as_i64()) {
                m.entry(cid).or_default().push(r);
            }
        }
        m
    };

    // 读取 5 语言 MasterTextLabel
    let langs = ["Chs", "Eng", "Jpn", "Kor", "Cht"];
    let mut mtl_all: HashMap<String, HashMap<String, String>> = HashMap::new();
    for lang in &langs {
        let mtl: Vec<Vec<serde_json::Value>> = read_json_table(
            &master_data_dir.join(lang), "MasterTextLabel.json"
        )?;
        let mut map = HashMap::new();
        for r in &mtl {
            let key = r[0].as_str().unwrap_or("").to_string();
            let val = r[1].as_str().unwrap_or("").to_string();
            if !key.is_empty() { map.insert(key, val); }
        }
        mtl_all.insert(lang.to_string(), map);
    }

    // 辅助：查找文本
    let get_text = |lang: &str, key: &str| -> String {
        mtl_all.get(lang)
            .and_then(|m| m.get(key))
            .cloned()
            .unwrap_or_default()
    };

    // 生成 cards_full
    let mut entries: Vec<CardFullEntry> = Vec::new();
    for cm in &card_master {
        let card_id = cm[0].as_i64().unwrap_or(0);
        if card_id == 0 { continue; }

        let base_card_id = cm[1].as_i64().unwrap_or(card_id);
        let card_style_id = cm[2].as_i64().unwrap_or(0);
        let class = cm[3].as_i64().unwrap_or(0);
        let foil_type = cm[5].as_i64().unwrap_or(0);
        let is_evolution = foil_type != 0;
        let evolves_to = cm[7].as_i64().unwrap_or(0);
        let resource_id = cm[9].as_i64().unwrap_or(0);

        // BaseCardMaster 数据（只有基础形态有）
        let bcm = bcm_by_id.get(&card_id);
        let cost = bcm.and_then(|r| serde_json::to_value(&r[4]).ok());
        let rarity = bcm.and_then(|r| serde_json::to_value(&r[8]).ok());
        let type_flags = bcm.and_then(|r| serde_json::to_value(&r[1]).ok());

        // 技能
        let skills: Vec<SkillEntry> = skills_by_cid.get(&card_id)
            .map(|v| v.iter().map(|r| SkillEntry {
                skill_id: r[0].as_i64().unwrap_or(0),
                skill_type: r[1].as_i64().unwrap_or(0),
                subtype: r[2].as_i64().unwrap_or(0),
            }).collect())
            .unwrap_or_default();

        // 文本键：通过 card_style_id 查 CardText
        let cs_id_for_text = cm[4].as_i64().unwrap_or(card_style_id);
        let ct = ct_by_cs.get(&cs_id_for_text);
        let cn_key = ct.and_then(|r| r[1].as_str()).unwrap_or("").to_string();
        let sd_key = ct.and_then(|r| r[2].as_str()).unwrap_or("").to_string();
        let ft1_key = ct.and_then(|r| r[3].as_str()).unwrap_or("").to_string();
        let ft2_key = ct.and_then(|r| r[4].as_str()).unwrap_or("").to_string();
        let cv_key = ct.and_then(|r| r[5].as_str()).unwrap_or("").to_string();

        // 多语言卡名
        let name_chs = get_text("Chs", &cn_key);
        let name_eng = get_text("Eng", &cn_key);
        let name_jpn = get_text("Jpn", &cn_key);
        let name_kor = get_text("Kor", &cn_key);
        let name_cht = get_text("Cht", &cn_key);

        entries.push(CardFullEntry {
            card_id, base_card_id, card_style_id, class,
            cost, rarity, type_flags, is_evolution, evolves_to,
            skills, resource_id,
            name_chs, name_eng, name_jpn, name_kor, name_cht,
            text_keys: TextKeys {
                name: cn_key,
                skill_desc: sd_key,
                flavor_1: ft1_key,
                flavor_2: ft2_key,
                cv: cv_key,
            },
        });
    }

    let count = entries.len();
    if let Some(parent) = output_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(&entries)?;
    std::fs::write(output_path, json)?;
    Ok(count)
}

/// 读取一个已导出的 JSON 表文件
fn read_json_table(dir: &Path, filename: &str) -> anyhow::Result<Vec<Vec<serde_json::Value>>> {
    let path = dir.join(filename);
    let content = std::fs::read_to_string(&path)
        .with_context(|| format!("无法读取: {}", path.display()))?;
    let rows: Vec<Vec<serde_json::Value>> = serde_json::from_str(&content)
        .with_context(|| format!("JSON 解析失败: {}", path.display()))?;
    Ok(rows)
}

// ---------------------------------------------------------------------------
// pack_names.json 生成
// ---------------------------------------------------------------------------

/// 从多语言 MasterTextLabel 中提取卡包名称。
pub fn generate_pack_names(
    master_data_dir: &Path,
) -> anyhow::Result<std::collections::BTreeMap<String, serde_json::Value>> {
    let langs = ["Chs", "Eng", "Jpn", "Kor", "Cht"];
    let mut pack_names: std::collections::BTreeMap<String, serde_json::Value> =
        std::collections::BTreeMap::new();

    for lang in &langs {
        let mtl: Vec<Vec<serde_json::Value>> =
            read_json_table(&master_data_dir.join(lang), "MasterTextLabel.json")?;
        let lang_key = match *lang {
            "Chs" => "chs",
            "Eng" => "eng",
            "Jpn" => "jpn",
            "Kor" => "kor",
            "Cht" => "cht",
            _ => continue,
        };

        for r in &mtl {
            let key = r[0].as_str().unwrap_or("");
            let val = r[1].as_str().unwrap_or("");

            if let Some(pack_id) = key.strip_prefix("CPN_") {
                if let Ok(id_num) = pack_id.parse::<u32>() {
                    if id_num >= 10000 && id_num <= 10007 {
                        let entry = pack_names
                            .entry(pack_id.to_string())
                            .or_insert_with(|| serde_json::json!({}));
                        if let Some(obj) = entry.as_object_mut() {
                            let clean = val.split('[').next().unwrap_or(val).trim().to_string();
                            obj.insert(lang_key.to_string(), serde_json::Value::String(clean));
                        }
                    }
                }
            }
        }
    }

    Ok(pack_names)
}

// ---------------------------------------------------------------------------
// 测试
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use rmpv::encode;

    #[test]
    fn test_parse_toc() {
        let mut body = Vec::new();
        encode::write_value(
            &mut body,
            &Value::Map(vec![(
                Value::String("TestTable".into()),
                Value::Array(vec![
                    Value::Integer(0.into()),
                    Value::Integer((10_i64).into()),
                ]),
            )]),
        )
        .unwrap();
        body.extend_from_slice(&[0u8; 16]);

        let (toc_end, tables) = parse_toc(&body).unwrap();
        assert!(toc_end > 0);
        assert_eq!(tables.len(), 1);
        assert_eq!(tables["TestTable"], (0, 10));
    }
}
