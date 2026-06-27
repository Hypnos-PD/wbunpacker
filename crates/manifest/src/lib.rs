//! Manifest 下载与解析模块
//!
//! # 概述
//!
//! Manifest（资源清单）是整个解包管线的入口数据。游戏客户端在启动时从服务器下载
//! 此文件，其中包含所有 AssetBundle 和 RawAsset 的列表、hash、解密 key 等。
//!
//! # 二进制格式（MasterMemory）
//!
//! Manifest 是 MasterMemory 格式的二进制文件：
//! 1. msgpack 编码的 TOC（目录），末尾附加 16 字节 MD5
//! 2. TOC 顶层是一个 map，key 是表名（小写），value 是 `[offset: u64, length: u64]`
//! 3. 每张表的数据位于 msgpack_body 的 `[offset..offset+length]` 区间内
//! 4. 表数据本身是 msgpack 数组，每行是一个数组，字段按索引位置对应
//!
//! ```text
//! {
//!   "asset":      [0, 2007040],
//!   "raw_asset":  [2007040, 421961],
//!   "assetname":  [2429001, 2271],
//!   "config":     [2431272, 2431290]
//! }
//! ```
//!
//! # 使用流程
//!
//! 1. `download()` — 从游戏服务器下载原始 manifest 二进制
//! 2. `parse()`     — 剥离 MD5 尾缀 → 解析 TOC → 按偏移切片解析各表
//! 3. `to_json()`   — 将解析结果序列化为 JSON（调试用）

use anyhow::{Context, anyhow};
use lz4_flex::block::decompress as lz4_decompress;
use rmpv::Value;
use serde::{Deserialize, Serialize};
use tracing::{debug, info};

pub mod diff;
pub use diff::*;

// ============================================================================
// 数据结构
// ============================================================================

/// 单个 AssetBundle 条目 —— 加密的 Unity 资源包。
///
/// 字段顺序对应 Wizard2.Domain.ManifestAsset 的 MessagePack Key 索引。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestAsset {
    /// 资源路径，如 "Assets/_Wizard2Resources/Card/Textures/100123100"
    /// Key 0 — 主键
    pub name: String,
    /// 内容 hash（AssetBundle CRC），用于校验完整性
    /// Key 1
    pub hash: String,
    /// AssetBundle ID（MasterMemory 内部分配的整数 ID）
    /// Key 2
    pub asset_id: i64,
    /// 此 AB 依赖的其他 AssetId 列表
    /// Key 3
    pub all_dependencies: Vec<i64>,
    /// XOR 解密 key（与 BaseKeys 组合生成 keystream）
    /// Key 4
    pub key: i64,
    /// 文件大小（字节）
    /// Key 5
    pub size: i64,
    /// 资源分类，如 "All"、"Card"、"Sound"、"UI" 等
    /// Key 6
    pub category: String,
    /// 下载优先级分组（数值越小越优先）
    /// Key 7
    pub group: i64,
    /// CRC64 校验和
    /// Key 8
    pub checksum: u64,
}

/// 单个 RawAsset 条目 —— 无需解密的原始文件。
///
/// 字段顺序对应 Wizard2.Domain.ManifestRawAsset。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RawAsset {
    /// 原始资源路径
    /// Key 0 — 主键
    pub name: String,
    /// 内容 hash
    /// Key 1
    pub hash: String,
    /// 文件大小（字节）
    /// Key 2
    pub size: i64,
    /// 资源分类，如 "pck"、"bytes"、"usm"、"acb"、"bnk" 等
    /// Key 3
    pub category: String,
    /// 下载优先级分组
    /// Key 4
    pub group: i64,
}

/// CDN 配置条目（从 config 表解析）。
///
/// 字段: Key 0 = key: str, Key 1 = value: str
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestConfigEntry {
    pub key: String,
    pub value: String,
}

/// 加载名映射 —— AssetBundle 路径 → CriWare 加载时使用的名字。
///
/// 字段: Key 0 = asset_name: str, Key 1 = name: str
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoadNameEntry {
    /// AssetBundle 完整路径
    pub asset_name: String,
    /// CriWare 加载名（不带扩展名和路径前缀）
    pub name: String,
}

/// 从 manifest 解析出的完整资源清单。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    /// 所有加密 AssetBundle 的列表
    pub assets: Vec<ManifestAsset>,
    /// 所有原始资源（音频包、数据表、视频等）的列表
    pub raw_assets: Vec<RawAsset>,
    /// CDN 配置（下载地址前缀等）
    pub config: Vec<ManifestConfigEntry>,
    /// AssetBundle 的加载名映射
    pub load_names: Vec<LoadNameEntry>,
}

// ============================================================================
// 公共 API
// ============================================================================

/// 从游戏服务器下载指定语言和版本的 manifest。
pub async fn download(version: &str, variant: &str, base_url: &str) -> anyhow::Result<Vec<u8>> {
    // Jpn 是默认语言，URL 中不带语言后缀
    let url = if variant.eq_ignore_ascii_case("Jpn") {
        base_url
            .replace("{version}", version)
            .replace(".{variant}.manifest", ".manifest")
    } else {
        base_url
            .replace("{version}", version)
            .replace("{variant}", variant)
    };
    info!("下载 manifest: {url}");

    let response = reqwest::get(&url)
        .await
        .with_context(|| format!("manifest 请求失败: {url}"))?;
    debug!("HTTP 状态: {}", response.status());
    if !response.status().is_success() {
        return Err(anyhow!(
            "manifest 下载失败: HTTP {} — {}",
            response.status().as_u16(),
            url
        ));
    }

    let bytes = response
        .bytes()
        .await
        .with_context(|| "读取 manifest 响应体失败")?;

    debug!("manifest 下载完成: {} 字节", bytes.len());
    Ok(bytes.to_vec())
}

/// 解析原始 msgpack 二进制为 Manifest 结构体。
///
/// # MasterMemory TOC 流程
///
/// 1. 剥离末尾 16 字节 MD5 → `msgpack_body`
/// 2. 解析顶层 map → 得到 `{"asset": [off, len], "raw_asset": [off, len], ...}`
/// 3. 按 off/len 从 `msgpack_body` 切出各表的 bytes
/// 4. 每张表的 bytes 是 msgpack 数组，逐行按字段索引解析
pub fn parse(raw: &[u8]) -> anyhow::Result<Manifest> {
    if raw.len() < 16 {
        return Err(anyhow!(
            "manifest 数据太短: {} 字节 (最少 16 字节 MD5)",
            raw.len()
        ));
    }

    // 剥离末尾 MD5
    let msgpack_body = &raw[..raw.len() - 16];

    debug!(
        "manifest 总 {} 字节, msgpack 体 {} 字节",
        raw.len(),
        msgpack_body.len()
    );

    // 解析顶层 msgpack map (TOC)，记录 TOC 结束位置
    let mut cursor = &msgpack_body[..];
    let root = rmpv::decode::value::read_value(&mut cursor).with_context(|| "msgpack 解码失败")?;
    let toc_end = msgpack_body.len() - cursor.len();
    debug!("TOC 结束位置: {}", toc_end);

    let tables = root
        .as_map()
        .ok_or_else(|| anyhow!("manifest 根结构不是 map"))?;

    let mut assets = Vec::new();
    let mut raw_assets = Vec::new();
    let mut config = Vec::new();
    let mut load_names = Vec::new();

    for (key_val, toc_val) in tables {
        let table_name = key_val
            .as_str()
            .map(|s| s.to_string())
            .or_else(|| key_val.to_string().into());

        match table_name.as_deref() {
            Some("asset") => {
                let (off, len) = parse_toc_entry(toc_val)?;
                let rows = decode_and_decompress_table(msgpack_body, toc_end, off, len, "asset")?;
                assets = parse_asset_table_from_rows(&rows)?;
                info!("asset 表: {} 条", assets.len());
            }
            Some("raw_asset") => {
                let (off, len) = parse_toc_entry(toc_val)?;
                let rows =
                    decode_and_decompress_table(msgpack_body, toc_end, off, len, "raw_asset")?;
                raw_assets = parse_raw_asset_table_from_rows(&rows)?;
                info!("raw_asset 表: {} 条", raw_assets.len());
            }
            Some("assetname") => {
                let (off, len) = parse_toc_entry(toc_val)?;
                let rows =
                    decode_and_decompress_table(msgpack_body, toc_end, off, len, "assetname")?;
                load_names = parse_load_name_table_from_rows(&rows)?;
                info!("assetname 表: {} 条", load_names.len());
            }
            Some("config") => {
                let (off, len) = parse_toc_entry(toc_val)?;
                let rows = decode_and_decompress_table(msgpack_body, toc_end, off, len, "config")?;
                config = parse_config_table_from_rows(&rows)?;
                info!("config 表: {} 条", config.len());
            }
            unknown => {
                debug!("跳过未知表: {:?}", unknown);
            }
        }
    }

    Ok(Manifest {
        assets,
        raw_assets,
        config,
        load_names,
    })
}
pub fn to_json(manifest: &Manifest) -> anyhow::Result<String> {
    serde_json::to_string_pretty(manifest).context("Manifest JSON 序列化失败")
}

// ============================================================================
// 内部函数
// ============================================================================

/// 从 TOC value 中提取 (offset, length) 对。
fn parse_toc_entry(value: &Value) -> anyhow::Result<(usize, usize)> {
    let pair = value
        .as_array()
        .ok_or_else(|| anyhow!("TOC entry 不是数组"))?;
    if pair.len() < 2 {
        return Err(anyhow!("TOC entry 字段不足: 需要 [offset, length]"));
    }
    let off = pair[0].as_i64().unwrap_or(0) as usize;
    let len = match &pair[1] {
        Value::Integer(i) => i.as_i64().unwrap_or(0) as usize,
        _ => 0,
    };
    Ok((off, len))
}

/// 从 msgpack_body 中提取表数据并解压（如果需要）。
///
/// 表数据可能有两种形式：
/// - 小表（如 config）直接是 msgpack 数组
/// - 大表是 Ext(99, lz4_compressed_msgpack) 格式，需要 LZ4 HC 解压
fn decode_and_decompress_table(
    body: &[u8],
    toc_end: usize,
    toc_off: usize,
    toc_len: usize,
    name: &str,
) -> anyhow::Result<Vec<Value>> {
    let actual_off = toc_end + toc_off;
    let table_data = body.get(actual_off..actual_off + toc_len).ok_or_else(|| {
        anyhow!(
            "{name} 表偏移越界: {actual_off} + {toc_len} > {}",
            body.len()
        )
    })?;

    let value = rmpv::decode::value::read_value(&mut &table_data[..])
        .with_context(|| format!("{name} 表 msgpack 解码失败"))?;

    match value {
        // LZ4 压缩的表: ext type=99
        Value::Ext(99, ref ext_data) => {
            // 格式: [msgpack int32 元数据] + [LZ4 压缩的 msgpack 数组]
            let mut cursor = &ext_data[..];
            rmpv::decode::value::read_value(&mut cursor)
                .with_context(|| format!("{name} 元数据 msgpack 解码失败"))?;
            let skip = ext_data.len() - cursor.len();
            let compressed = &ext_data[skip..];

            let max_size = compressed.len() * 100; // 足够大的上限
            let decompressed = lz4_decompress(compressed, max_size)
                .with_context(|| format!("{name} LZ4 解压失败"))?;
            let rows = rmpv::decode::value::read_value(&mut &decompressed[..])
                .with_context(|| format!("{name} 解压后 msgpack 解码失败"))?;
            rows.as_array()
                .cloned()
                .ok_or_else(|| anyhow!("{name} 解压后不是 msgpack 数组"))
        }
        // 未压缩的小表
        Value::Array(rows) => Ok(rows),
        other => Err(anyhow!("{name} 表格式不支持: {:?}", other)),
    }
}
// ============================================================================
// 各表解析
// ============================================================================

/// 解析 asset 表（加密 AssetBundle 条目）。
///
/// Wizard2.Domain.ManifestAsset 字段顺序:
///   0: name          — 资源路径 (string, 主键)
///   1: hash          — AssetBundle CRC (string)
///   2: asset_id      — 内部 ID (int)
///   3: all_deps      — 依赖列表 ([int])
///   4: key           — XOR 解密 key (i64)
///   5: size          — 文件大小 (int)
///   6: category      — 分类 (string)
///   7: group         — 分组 (int)
///   8: checksum      — CRC64 (i64)
fn parse_asset_table_from_rows(rows: &[Value]) -> anyhow::Result<Vec<ManifestAsset>> {
    rows.iter()
        .filter(|row| row.is_array())
        .map(|row| {
            let fields = row.as_array().ok_or_else(|| anyhow!("asset 行不是数组"))?;

            Ok(ManifestAsset {
                name: get_str_at(fields, 0).unwrap_or_default(),
                hash: get_str_at(fields, 1).unwrap_or_default(),
                asset_id: get_i64_at(fields, 2).unwrap_or(0),
                all_dependencies: get_int_array_at(fields, 3),
                key: get_i64_at(fields, 4).unwrap_or(0),
                size: get_i64_at(fields, 5).unwrap_or(0),
                category: get_str_at(fields, 6).unwrap_or_default(),
                group: get_i64_at(fields, 7).unwrap_or(0),
                checksum: get_u64_at(fields, 8).unwrap_or(0u64),
            })
        })
        .collect()
}

/// 解析 raw_asset 表（原始资源条目）。
///
/// Wizard2.Domain.ManifestRawAsset 字段顺序:
///   0: name      — 路径 (string, 主键)
///   1: hash      — CRC (string)
///   2: size      — 文件大小 (int)
///   3: category  — 分类: pck/bytes/usm/... (string)
///   4: group     — 分组 (int)
fn parse_raw_asset_table_from_rows(rows: &[Value]) -> anyhow::Result<Vec<RawAsset>> {
    rows.iter()
        .filter(|row| row.is_array())
        .map(|row| {
            let fields = row
                .as_array()
                .ok_or_else(|| anyhow!("raw_asset 行不是数组"))?;

            Ok(RawAsset {
                name: get_str_at(fields, 0).unwrap_or_default(),
                hash: get_str_at(fields, 1).unwrap_or_default(),
                size: get_i64_at(fields, 2).unwrap_or(0),
                category: get_str_at(fields, 3).unwrap_or_default(),
                group: get_i64_at(fields, 4).unwrap_or(0),
            })
        })
        .collect()
}

/// 解析 config 表（CDN 配置等键值对）。
///
/// 字段: 0: key (string), 1: value (string)
fn parse_config_table_from_rows(rows: &[Value]) -> anyhow::Result<Vec<ManifestConfigEntry>> {
    rows.iter()
        .filter(|row| row.is_array())
        .map(|row| {
            let fields = row.as_array().ok_or_else(|| anyhow!("config 行不是数组"))?;

            Ok(ManifestConfigEntry {
                key: get_str_at(fields, 0).unwrap_or_default(),
                value: get_str_at(fields, 1).unwrap_or_default(),
            })
        })
        .collect()
}

/// 解析 assetname 表（加载名映射）。
///
/// Wizard2.Domain.AssetBundleLoadName 字段顺序:
///   0: asset_name  — AB 完整路径 (string, 主键)
///   1: name        — CriWare 加载名 (string)
fn parse_load_name_table_from_rows(rows: &[Value]) -> anyhow::Result<Vec<LoadNameEntry>> {
    rows.iter()
        .filter(|row| row.is_array())
        .map(|row| {
            let fields = row
                .as_array()
                .ok_or_else(|| anyhow!("assetname 行不是数组"))?;

            Ok(LoadNameEntry {
                asset_name: get_str_at(fields, 0).unwrap_or_default(),
                name: get_str_at(fields, 1).unwrap_or_default(),
            })
        })
        .collect()
}

// ============================================================================
// 辅助函数
// ============================================================================

fn get_str_at(array: &[Value], idx: usize) -> Option<String> {
    array
        .get(idx)
        .and_then(|v| v.as_str().map(|s| s.to_string()))
}

fn get_i64_at(array: &[Value], idx: usize) -> Option<i64> {
    array.get(idx).and_then(|v| match v {
        Value::Integer(i) => i.as_i64(),
        _ => None,
    })
}

fn get_u64_at(array: &[Value], idx: usize) -> Option<u64> {
    array.get(idx).and_then(|v| match v {
        Value::Integer(i) => i.as_u64(),
        _ => None,
    })
}

fn get_int_array_at(array: &[Value], idx: usize) -> Vec<i64> {
    array
        .get(idx)
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| match v {
                    Value::Integer(i) => i.as_i64(),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use rmpv::encode;

    fn make_test_data(table_rows: Vec<Value>, table_name: &str) -> Vec<u8> {
        // 编码表数据为 msgpack 数组
        let mut body = Vec::new();
        encode::write_value(&mut body, &Value::Array(table_rows)).unwrap();
        let body_len = body.len();

        // 构造 TOC，偏移从 TOC 结束位置算（0即跟在 TOC 后面）
        let toc = Value::Map(vec![(
            Value::String(table_name.into()),
            Value::Array(vec![
                Value::Integer(0.into()),
                Value::Integer((body_len as i64).into()),
            ]),
        )]);

        let mut buf = Vec::new();
        encode::write_value(&mut buf, &toc).unwrap();
        buf.extend_from_slice(&body);
        buf.extend_from_slice(&[0u8; 16]);
        buf
    }

    #[test]
    fn test_parse_too_short() {
        assert!(parse(&[1, 2, 3]).is_err());
    }

    #[test]
    fn test_parse_unknown_table_skipped() {
        let row = vec![Value::Array(vec![
            Value::String("a".into()),
            Value::String("b".into()),
        ])];
        let buf = make_test_data(row, "unknown_table");
        let m = parse(&buf).expect("未知表应跳过");
        assert!(m.assets.is_empty());
        assert!(m.raw_assets.is_empty());
    }

    #[test]
    fn test_parse_missing_fields() {
        let row = vec![Value::Array(vec![
            Value::String("only_name".into()),
            Value::String("only_hash".into()),
        ])];
        let buf = make_test_data(row, "asset");
        let m = parse(&buf).expect("缺字段应容错");
        assert_eq!(m.assets[0].name, "only_name");
        assert_eq!(m.assets[0].key, 0);
        assert_eq!(m.assets[0].size, 0);
        assert_eq!(m.assets[0].category, "");
    }
}
