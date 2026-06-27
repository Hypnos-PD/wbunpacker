//! Manifest 差异比较模块。
//!
//! 提供 `diff_manifests` 函数，对比两个 `Manifest` 值，
//! 按稳定 name key 分类所有四个 section（assets、raw_assets、config、load_names）的变更。

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::{LoadNameEntry, Manifest, ManifestAsset, ManifestConfigEntry, RawAsset};

// ============================================================================
// 公开类型
// ============================================================================

/// 两个 Manifest 之间的所有变更汇总。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ManifestChanges {
    // --- AssetBundle 变更 ---
    pub added_assets: Vec<ManifestAsset>,
    pub removed_assets: Vec<ManifestAsset>,
    pub content_changed_assets: Vec<(ManifestAsset, ManifestAsset)>,
    pub metadata_changed_assets: Vec<(ManifestAsset, ManifestAsset)>,

    // --- RawAsset 变更 ---
    pub added_raw_assets: Vec<RawAsset>,
    pub removed_raw_assets: Vec<RawAsset>,
    pub content_changed_raw_assets: Vec<(RawAsset, RawAsset)>,
    pub metadata_changed_raw_assets: Vec<(RawAsset, RawAsset)>,

    // --- Config 变更 ---
    pub config_changes: Vec<ConfigChange>,

    // --- LoadName 变更 ---
    pub load_name_changes: Vec<LoadNameChange>,
}

/// Config 表（按 `.key` 索引）的单条变更。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConfigChange {
    Added(ManifestConfigEntry),
    Removed(ManifestConfigEntry),
    ValueChanged {
        key: String,
        old_value: String,
        new_value: String,
    },
}

/// LoadName 表（按 `.asset_name` 索引）的单条变更。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum LoadNameChange {
    Added(LoadNameEntry),
    Removed(LoadNameEntry),
    NameChanged {
        asset_name: String,
        old_name: String,
        new_name: String,
    },
}

// ============================================================================
// 公开 API
// ============================================================================

/// 对比两个 `Manifest` 值，返回所有分类的变更。
///
/// # Key 策略
///
/// | Section    | Key         |
/// |------------|-------------|
/// | assets     | `.name`     |
/// | raw_assets | `.name`     |
/// | config     | `.key`      |
/// | load_names | `.asset_name` |
///
/// # 分类规则
///
/// - **added**: 存在于 `new` 但不存在于 `old`。
/// - **removed**: 存在于 `old` 但不存在于 `new`。
/// - **content_changed**: 两者存在，但 content 字段（hash、size、checksum）变化。
/// - **metadata_changed**: 两者存在，content 字段相同，但 metadata 字段变化。
/// - 两者完全相同的条目不出现在任何变更列表中。
pub fn diff_manifests(old: &Manifest, new: &Manifest) -> ManifestChanges {
    let asset_diff = diff_assets(&old.assets, &new.assets);
    let raw_diff = diff_raw_assets(&old.raw_assets, &new.raw_assets);

    ManifestChanges {
        added_assets: asset_diff.added,
        removed_assets: asset_diff.removed,
        content_changed_assets: asset_diff.content_changed,
        metadata_changed_assets: asset_diff.metadata_changed,
        added_raw_assets: raw_diff.added,
        removed_raw_assets: raw_diff.removed,
        content_changed_raw_assets: raw_diff.content_changed,
        metadata_changed_raw_assets: raw_diff.metadata_changed,
        config_changes: diff_config(&old.config, &new.config),
        load_name_changes: diff_load_names(&old.load_names, &new.load_names),
    }
}

// ============================================================================
// 内部类型
// ============================================================================

struct AssetDiff {
    added: Vec<ManifestAsset>,
    removed: Vec<ManifestAsset>,
    content_changed: Vec<(ManifestAsset, ManifestAsset)>,
    metadata_changed: Vec<(ManifestAsset, ManifestAsset)>,
}

struct RawAssetDiff {
    added: Vec<RawAsset>,
    removed: Vec<RawAsset>,
    content_changed: Vec<(RawAsset, RawAsset)>,
    metadata_changed: Vec<(RawAsset, RawAsset)>,
}

// ============================================================================
// AssetBundle diff
// ============================================================================

fn diff_assets(old: &[ManifestAsset], new: &[ManifestAsset]) -> AssetDiff {
    let old_map: HashMap<&str, &ManifestAsset> = old.iter().map(|a| (a.name.as_str(), a)).collect();
    let new_map: HashMap<&str, &ManifestAsset> = new.iter().map(|a| (a.name.as_str(), a)).collect();

    let mut added = Vec::new();
    let mut removed = Vec::new();
    let mut content_changed = Vec::new();
    let mut metadata_changed = Vec::new();

    for a in new {
        if !old_map.contains_key(a.name.as_str()) {
            added.push(a.clone());
        }
    }

    for a in old {
        if !new_map.contains_key(a.name.as_str()) {
            removed.push(a.clone());
        }
    }

    for a in new {
        if let Some(old_a) = old_map.get(a.name.as_str()) {
            if content_fields_differ_asset(old_a, a) {
                content_changed.push(((*old_a).clone(), a.clone()));
            } else if metadata_fields_differ_asset(old_a, a) {
                metadata_changed.push(((*old_a).clone(), a.clone()));
            }
        }
    }

    AssetDiff {
        added,
        removed,
        content_changed,
        metadata_changed,
    }
}

fn content_fields_differ_asset(old: &ManifestAsset, new: &ManifestAsset) -> bool {
    old.hash != new.hash || old.size != new.size || old.checksum != new.checksum
}

fn metadata_fields_differ_asset(old: &ManifestAsset, new: &ManifestAsset) -> bool {
    old.asset_id != new.asset_id
        || old.all_dependencies != new.all_dependencies
        || old.category != new.category
        || old.group != new.group
        || old.key != new.key
}

// ============================================================================
// RawAsset diff
// ============================================================================

fn diff_raw_assets(old: &[RawAsset], new: &[RawAsset]) -> RawAssetDiff {
    let old_map: HashMap<&str, &RawAsset> = old.iter().map(|a| (a.name.as_str(), a)).collect();
    let new_map: HashMap<&str, &RawAsset> = new.iter().map(|a| (a.name.as_str(), a)).collect();

    let mut added = Vec::new();
    let mut removed = Vec::new();
    let mut content_changed = Vec::new();
    let mut metadata_changed = Vec::new();

    for a in new {
        if !old_map.contains_key(a.name.as_str()) {
            added.push(a.clone());
        }
    }

    for a in old {
        if !new_map.contains_key(a.name.as_str()) {
            removed.push(a.clone());
        }
    }

    for a in new {
        if let Some(old_a) = old_map.get(a.name.as_str()) {
            if raw_content_fields_differ(old_a, a) {
                content_changed.push(((*old_a).clone(), a.clone()));
            } else if raw_metadata_fields_differ(old_a, a) {
                metadata_changed.push(((*old_a).clone(), a.clone()));
            }
        }
    }

    RawAssetDiff {
        added,
        removed,
        content_changed,
        metadata_changed,
    }
}

fn raw_content_fields_differ(old: &RawAsset, new: &RawAsset) -> bool {
    old.hash != new.hash || old.size != new.size
}

fn raw_metadata_fields_differ(old: &RawAsset, new: &RawAsset) -> bool {
    old.category != new.category || old.group != new.group
}

// ============================================================================
// Config diff
// ============================================================================

fn diff_config(old: &[ManifestConfigEntry], new: &[ManifestConfigEntry]) -> Vec<ConfigChange> {
    let old_map: HashMap<&str, &str> = old
        .iter()
        .map(|c| (c.key.as_str(), c.value.as_str()))
        .collect();

    let mut changes = Vec::new();

    for entry in new {
        match old_map.get(entry.key.as_str()) {
            None => changes.push(ConfigChange::Added(entry.clone())),
            Some(old_value) if *old_value != entry.value => {
                changes.push(ConfigChange::ValueChanged {
                    key: entry.key.clone(),
                    old_value: old_value.to_string(),
                    new_value: entry.value.clone(),
                });
            }
            _ => {}
        }
    }

    let new_keys: HashMap<&str, ()> = new.iter().map(|c| (c.key.as_str(), ())).collect();
    for entry in old {
        if !new_keys.contains_key(entry.key.as_str()) {
            changes.push(ConfigChange::Removed(entry.clone()));
        }
    }

    changes
}

// ============================================================================
// LoadName diff
// ============================================================================

fn diff_load_names(old: &[LoadNameEntry], new: &[LoadNameEntry]) -> Vec<LoadNameChange> {
    let old_map: HashMap<&str, &str> = old
        .iter()
        .map(|l| (l.asset_name.as_str(), l.name.as_str()))
        .collect();

    let mut changes = Vec::new();

    for entry in new {
        match old_map.get(entry.asset_name.as_str()) {
            None => changes.push(LoadNameChange::Added(entry.clone())),
            Some(old_name) if *old_name != entry.name => {
                changes.push(LoadNameChange::NameChanged {
                    asset_name: entry.asset_name.clone(),
                    old_name: old_name.to_string(),
                    new_name: entry.name.clone(),
                });
            }
            _ => {}
        }
    }

    let new_keys: HashMap<&str, ()> = new.iter().map(|l| (l.asset_name.as_str(), ())).collect();
    for entry in old {
        if !new_keys.contains_key(entry.asset_name.as_str()) {
            changes.push(LoadNameChange::Removed(entry.clone()));
        }
    }

    changes
}

// ============================================================================
// 测试
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn make_asset(name: &str, hash: &str, size: i64, checksum: u64) -> ManifestAsset {
        ManifestAsset {
            name: name.to_string(),
            hash: hash.to_string(),
            asset_id: 1,
            all_dependencies: vec![],
            key: 0,
            size,
            category: "All".to_string(),
            group: 0,
            checksum,
        }
    }

    fn make_raw(name: &str, hash: &str, size: i64) -> RawAsset {
        RawAsset {
            name: name.to_string(),
            hash: hash.to_string(),
            size,
            category: "pck".to_string(),
            group: 0,
        }
    }

    fn make_config(key: &str, value: &str) -> ManifestConfigEntry {
        ManifestConfigEntry {
            key: key.to_string(),
            value: value.to_string(),
        }
    }

    fn make_load_name(asset_name: &str, name: &str) -> LoadNameEntry {
        LoadNameEntry {
            asset_name: asset_name.to_string(),
            name: name.to_string(),
        }
    }

    fn empty_manifest() -> Manifest {
        Manifest {
            assets: vec![],
            raw_assets: vec![],
            config: vec![],
            load_names: vec![],
        }
    }

    #[test]
    fn test_asset_added() {
        let old = Manifest {
            assets: vec![],
            ..empty_manifest()
        };
        let new = Manifest {
            assets: vec![make_asset("a", "h1", 100, 1)],
            ..empty_manifest()
        };
        let changes = diff_manifests(&old, &new);
        assert_eq!(changes.added_assets.len(), 1);
        assert_eq!(changes.added_assets[0].name, "a");
        assert!(changes.removed_assets.is_empty());
        assert!(changes.content_changed_assets.is_empty());
        assert!(changes.metadata_changed_assets.is_empty());
    }

    #[test]
    fn test_asset_removed() {
        let old = Manifest {
            assets: vec![make_asset("a", "h1", 100, 1)],
            ..empty_manifest()
        };
        let new = Manifest {
            assets: vec![],
            ..empty_manifest()
        };
        let changes = diff_manifests(&old, &new);
        assert_eq!(changes.removed_assets.len(), 1);
        assert_eq!(changes.removed_assets[0].name, "a");
        assert!(changes.added_assets.is_empty());
    }

    #[test]
    fn test_asset_content_changed_hash() {
        let old = Manifest {
            assets: vec![make_asset("a", "hash_old", 100, 1)],
            ..empty_manifest()
        };
        let new = Manifest {
            assets: vec![make_asset("a", "hash_new", 100, 1)],
            ..empty_manifest()
        };
        let changes = diff_manifests(&old, &new);
        assert_eq!(changes.content_changed_assets.len(), 1);
        assert!(changes.metadata_changed_assets.is_empty());
        let (ref old_a, ref new_a) = changes.content_changed_assets[0];
        assert_eq!(old_a.hash, "hash_old");
        assert_eq!(new_a.hash, "hash_new");
    }

    #[test]
    fn test_asset_content_changed_size() {
        let old = Manifest {
            assets: vec![make_asset("a", "h", 100, 1)],
            ..empty_manifest()
        };
        let new = Manifest {
            assets: vec![make_asset("a", "h", 200, 1)],
            ..empty_manifest()
        };
        let changes = diff_manifests(&old, &new);
        assert_eq!(changes.content_changed_assets.len(), 1);
        let (ref old_a, ref new_a) = changes.content_changed_assets[0];
        assert_eq!(old_a.size, 100);
        assert_eq!(new_a.size, 200);
    }

    #[test]
    fn test_asset_content_changed_checksum() {
        let old = Manifest {
            assets: vec![make_asset("a", "h", 100, 1)],
            ..empty_manifest()
        };
        let new = Manifest {
            assets: vec![make_asset("a", "h", 100, 999)],
            ..empty_manifest()
        };
        let changes = diff_manifests(&old, &new);
        assert_eq!(changes.content_changed_assets.len(), 1);
    }

    #[test]
    fn test_asset_metadata_changed_asset_id() {
        let old_a = ManifestAsset {
            asset_id: 10,
            ..make_asset("a", "h", 100, 1)
        };
        let new_a = ManifestAsset {
            asset_id: 20,
            ..make_asset("a", "h", 100, 1)
        };
        let old = Manifest {
            assets: vec![old_a],
            ..empty_manifest()
        };
        let new = Manifest {
            assets: vec![new_a],
            ..empty_manifest()
        };
        let changes = diff_manifests(&old, &new);
        assert_eq!(changes.metadata_changed_assets.len(), 1);
        assert!(changes.content_changed_assets.is_empty());
    }

    #[test]
    fn test_asset_metadata_changed_category() {
        let old_a = ManifestAsset {
            category: "Card".to_string(),
            ..make_asset("a", "h", 100, 1)
        };
        let new_a = ManifestAsset {
            category: "Sound".to_string(),
            ..make_asset("a", "h", 100, 1)
        };
        let old = Manifest {
            assets: vec![old_a],
            ..empty_manifest()
        };
        let new = Manifest {
            assets: vec![new_a],
            ..empty_manifest()
        };
        let changes = diff_manifests(&old, &new);
        assert_eq!(changes.metadata_changed_assets.len(), 1);
    }

    #[test]
    fn test_asset_metadata_changed_group() {
        let old_a = ManifestAsset {
            group: 1,
            ..make_asset("a", "h", 100, 1)
        };
        let new_a = ManifestAsset {
            group: 2,
            ..make_asset("a", "h", 100, 1)
        };
        let old = Manifest {
            assets: vec![old_a],
            ..empty_manifest()
        };
        let new = Manifest {
            assets: vec![new_a],
            ..empty_manifest()
        };
        let changes = diff_manifests(&old, &new);
        assert_eq!(changes.metadata_changed_assets.len(), 1);
    }

    #[test]
    fn test_asset_metadata_changed_key() {
        let old_a = ManifestAsset {
            key: 5,
            ..make_asset("a", "h", 100, 1)
        };
        let new_a = ManifestAsset {
            key: 6,
            ..make_asset("a", "h", 100, 1)
        };
        let old = Manifest {
            assets: vec![old_a],
            ..empty_manifest()
        };
        let new = Manifest {
            assets: vec![new_a],
            ..empty_manifest()
        };
        let changes = diff_manifests(&old, &new);
        assert_eq!(changes.metadata_changed_assets.len(), 1);
    }

    #[test]
    fn test_asset_metadata_changed_dependencies() {
        let old_a = ManifestAsset {
            all_dependencies: vec![1],
            ..make_asset("a", "h", 100, 1)
        };
        let new_a = ManifestAsset {
            all_dependencies: vec![1, 2],
            ..make_asset("a", "h", 100, 1)
        };
        let old = Manifest {
            assets: vec![old_a],
            ..empty_manifest()
        };
        let new = Manifest {
            assets: vec![new_a],
            ..empty_manifest()
        };
        let changes = diff_manifests(&old, &new);
        assert_eq!(changes.metadata_changed_assets.len(), 1);
    }

    #[test]
    fn test_asset_unchanged() {
        let a = make_asset("a", "h", 100, 1);
        let old = Manifest {
            assets: vec![a.clone()],
            ..empty_manifest()
        };
        let new = Manifest {
            assets: vec![a],
            ..empty_manifest()
        };
        let changes = diff_manifests(&old, &new);
        assert!(changes.added_assets.is_empty());
        assert!(changes.removed_assets.is_empty());
        assert!(changes.content_changed_assets.is_empty());
        assert!(changes.metadata_changed_assets.is_empty());
    }

    #[test]
    fn test_asset_content_priority_over_metadata() {
        let old_a = ManifestAsset {
            hash: "old_hash".to_string(),
            asset_id: 10,
            ..make_asset("a", "old_hash", 100, 1)
        };
        let new_a = ManifestAsset {
            hash: "new_hash".to_string(),
            asset_id: 20,
            ..make_asset("a", "new_hash", 100, 1)
        };
        let old = Manifest {
            assets: vec![old_a],
            ..empty_manifest()
        };
        let new = Manifest {
            assets: vec![new_a],
            ..empty_manifest()
        };
        let changes = diff_manifests(&old, &new);
        assert_eq!(changes.content_changed_assets.len(), 1);
        assert!(changes.metadata_changed_assets.is_empty());
    }

    #[test]
    fn test_raw_added() {
        let old = empty_manifest();
        let new = Manifest {
            raw_assets: vec![make_raw("r1", "h1", 500)],
            ..empty_manifest()
        };
        let changes = diff_manifests(&old, &new);
        assert_eq!(changes.added_raw_assets.len(), 1);
        assert_eq!(changes.added_raw_assets[0].name, "r1");
    }

    #[test]
    fn test_raw_removed() {
        let old = Manifest {
            raw_assets: vec![make_raw("r1", "h1", 500)],
            ..empty_manifest()
        };
        let new = empty_manifest();
        let changes = diff_manifests(&old, &new);
        assert_eq!(changes.removed_raw_assets.len(), 1);
    }

    #[test]
    fn test_raw_content_changed_hash() {
        let old = Manifest {
            raw_assets: vec![make_raw("r1", "old_hash", 500)],
            ..empty_manifest()
        };
        let new = Manifest {
            raw_assets: vec![make_raw("r1", "new_hash", 500)],
            ..empty_manifest()
        };
        let changes = diff_manifests(&old, &new);
        assert_eq!(changes.content_changed_raw_assets.len(), 1);
    }

    #[test]
    fn test_raw_content_changed_size() {
        let old = Manifest {
            raw_assets: vec![make_raw("r1", "h", 100)],
            ..empty_manifest()
        };
        let new = Manifest {
            raw_assets: vec![make_raw("r1", "h", 200)],
            ..empty_manifest()
        };
        let changes = diff_manifests(&old, &new);
        assert_eq!(changes.content_changed_raw_assets.len(), 1);
    }

    #[test]
    fn test_raw_metadata_changed_category() {
        let old_a = RawAsset {
            category: "pck".to_string(),
            ..make_raw("r1", "h", 500)
        };
        let new_a = RawAsset {
            category: "usm".to_string(),
            ..make_raw("r1", "h", 500)
        };
        let old = Manifest {
            raw_assets: vec![old_a],
            ..empty_manifest()
        };
        let new = Manifest {
            raw_assets: vec![new_a],
            ..empty_manifest()
        };
        let changes = diff_manifests(&old, &new);
        assert_eq!(changes.metadata_changed_raw_assets.len(), 1);
    }

    #[test]
    fn test_raw_metadata_changed_group() {
        let old_a = RawAsset {
            group: 0,
            ..make_raw("r1", "h", 500)
        };
        let new_a = RawAsset {
            group: 1,
            ..make_raw("r1", "h", 500)
        };
        let old = Manifest {
            raw_assets: vec![old_a],
            ..empty_manifest()
        };
        let new = Manifest {
            raw_assets: vec![new_a],
            ..empty_manifest()
        };
        let changes = diff_manifests(&old, &new);
        assert_eq!(changes.metadata_changed_raw_assets.len(), 1);
    }

    #[test]
    fn test_raw_unchanged() {
        let r = make_raw("r1", "h", 500);
        let old = Manifest {
            raw_assets: vec![r.clone()],
            ..empty_manifest()
        };
        let new = Manifest {
            raw_assets: vec![r],
            ..empty_manifest()
        };
        let changes = diff_manifests(&old, &new);
        assert!(changes.added_raw_assets.is_empty());
        assert!(changes.removed_raw_assets.is_empty());
        assert!(changes.content_changed_raw_assets.is_empty());
        assert!(changes.metadata_changed_raw_assets.is_empty());
    }

    #[test]
    fn test_raw_content_priority_over_metadata() {
        let old_a = RawAsset {
            hash: "old_hash".to_string(),
            category: "pck".to_string(),
            ..make_raw("r1", "old_hash", 500)
        };
        let new_a = RawAsset {
            hash: "new_hash".to_string(),
            category: "usm".to_string(),
            ..make_raw("r1", "new_hash", 500)
        };
        let old = Manifest {
            raw_assets: vec![old_a],
            ..empty_manifest()
        };
        let new = Manifest {
            raw_assets: vec![new_a],
            ..empty_manifest()
        };
        let changes = diff_manifests(&old, &new);
        assert_eq!(changes.content_changed_raw_assets.len(), 1);
        assert!(changes.metadata_changed_raw_assets.is_empty());
    }

    #[test]
    fn test_config_added() {
        let old = empty_manifest();
        let new = Manifest {
            config: vec![make_config("cdn_url", "https://example.com")],
            ..empty_manifest()
        };
        let changes = diff_manifests(&old, &new);
        assert_eq!(changes.config_changes.len(), 1);
        assert!(matches!(changes.config_changes[0], ConfigChange::Added(_)));
    }

    #[test]
    fn test_config_removed() {
        let old = Manifest {
            config: vec![make_config("cdn_url", "https://example.com")],
            ..empty_manifest()
        };
        let new = empty_manifest();
        let changes = diff_manifests(&old, &new);
        assert_eq!(changes.config_changes.len(), 1);
        assert!(matches!(
            changes.config_changes[0],
            ConfigChange::Removed(_)
        ));
    }

    #[test]
    fn test_config_value_changed() {
        let old = Manifest {
            config: vec![make_config("cdn_url", "https://old.example.com")],
            ..empty_manifest()
        };
        let new = Manifest {
            config: vec![make_config("cdn_url", "https://new.example.com")],
            ..empty_manifest()
        };
        let changes = diff_manifests(&old, &new);
        assert_eq!(changes.config_changes.len(), 1);
        assert!(matches!(&changes.config_changes[0],
            ConfigChange::ValueChanged { key, old_value, new_value }
            if key == "cdn_url"
            && old_value == "https://old.example.com"
            && new_value == "https://new.example.com"
        ));
    }

    #[test]
    fn test_config_unchanged() {
        let cfg = make_config("cdn_url", "https://example.com");
        let old = Manifest {
            config: vec![cfg.clone()],
            ..empty_manifest()
        };
        let new = Manifest {
            config: vec![cfg],
            ..empty_manifest()
        };
        let changes = diff_manifests(&old, &new);
        assert!(changes.config_changes.is_empty());
    }

    #[test]
    fn test_load_name_added() {
        let old = empty_manifest();
        let new = Manifest {
            load_names: vec![make_load_name("Assets/Resources/card", "card")],
            ..empty_manifest()
        };
        let changes = diff_manifests(&old, &new);
        assert_eq!(changes.load_name_changes.len(), 1);
        assert!(matches!(
            changes.load_name_changes[0],
            LoadNameChange::Added(_)
        ));
    }

    #[test]
    fn test_load_name_removed() {
        let old = Manifest {
            load_names: vec![make_load_name("Assets/Resources/card", "card")],
            ..empty_manifest()
        };
        let new = empty_manifest();
        let changes = diff_manifests(&old, &new);
        assert_eq!(changes.load_name_changes.len(), 1);
        assert!(matches!(
            changes.load_name_changes[0],
            LoadNameChange::Removed(_)
        ));
    }

    #[test]
    fn test_load_name_name_changed() {
        let old = Manifest {
            load_names: vec![make_load_name("Assets/Resources/card", "card_old")],
            ..empty_manifest()
        };
        let new = Manifest {
            load_names: vec![make_load_name("Assets/Resources/card", "card_new")],
            ..empty_manifest()
        };
        let changes = diff_manifests(&old, &new);
        assert_eq!(changes.load_name_changes.len(), 1);
        assert!(matches!(&changes.load_name_changes[0],
            LoadNameChange::NameChanged { asset_name, old_name, new_name }
            if asset_name == "Assets/Resources/card"
            && old_name == "card_old"
            && new_name == "card_new"
        ));
    }

    #[test]
    fn test_load_name_unchanged() {
        let ln = make_load_name("Assets/Resources/card", "card");
        let old = Manifest {
            load_names: vec![ln.clone()],
            ..empty_manifest()
        };
        let new = Manifest {
            load_names: vec![ln],
            ..empty_manifest()
        };
        let changes = diff_manifests(&old, &new);
        assert!(changes.load_name_changes.is_empty());
    }

    #[test]
    fn test_empty_manifests() {
        let changes = diff_manifests(&empty_manifest(), &empty_manifest());
        assert!(changes.added_assets.is_empty());
        assert!(changes.removed_assets.is_empty());
        assert!(changes.content_changed_assets.is_empty());
        assert!(changes.metadata_changed_assets.is_empty());
        assert!(changes.added_raw_assets.is_empty());
        assert!(changes.removed_raw_assets.is_empty());
        assert!(changes.content_changed_raw_assets.is_empty());
        assert!(changes.metadata_changed_raw_assets.is_empty());
        assert!(changes.config_changes.is_empty());
        assert!(changes.load_name_changes.is_empty());
    }
}
