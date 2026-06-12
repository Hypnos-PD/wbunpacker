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
