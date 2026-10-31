use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::{Read, Write},
    path::Path,
};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

pub const CATALOG_RELEASE_SCHEMA_VERSION: u32 = 1;
const RELEASE_ID_DOMAIN: &str = "wbu-catalog-release-v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogRelease {
    pub schema_version: u32,
    pub release_id: String,
    pub generated_at: String,
    pub producer: CatalogProducer,
    #[serde(default)]
    pub sources: Vec<CatalogSource>,
    pub files: Vec<CatalogFile>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogProducer {
    pub name: String,
    pub version: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogSource {
    pub kind: CatalogSourceKind,
    pub name: String,
    pub reference: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CatalogSourceKind {
    GameResources,
    RuntimeProfile,
    OfficialApi,
    Manual,
    Generated,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogFile {
    pub path: String,
    pub object: String,
    pub size: u64,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CatalogSummary {
    pub release_id: String,
    pub file_count: usize,
    pub total_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CatalogPointer {
    pub schema_version: u32,
    pub release_id: String,
    pub generated_at: String,
    pub manifest: String,
    pub object_base: String,
    pub file_count: usize,
    pub total_bytes: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct CatalogDiff {
    pub added: Vec<String>,
    pub removed: Vec<String>,
    pub changed: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct CatalogStageSummary {
    pub copied: usize,
    pub reused: usize,
    pub skipped: usize,
    pub copied_bytes: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct CatalogPullSummary {
    pub downloaded: usize,
    pub reused: usize,
    pub downloaded_bytes: u64,
}

impl CatalogRelease {
    pub fn build_now(root: &Path, sources: Vec<CatalogSource>) -> Result<Self> {
        Self::build(root, sources, OffsetDateTime::now_utc())
    }

    pub fn build(
        root: &Path,
        sources: Vec<CatalogSource>,
        generated_at: OffsetDateTime,
    ) -> Result<Self> {
        let root = root
            .canonicalize()
            .with_context(|| format!("resolve catalog root: {}", root.display()))?;
        let data_root = root.join("data");
        if !data_root.is_dir() {
            bail!("catalog data directory not found: {}", data_root.display());
        }
        let mut files = Vec::new();
        collect_files(&root, &data_root, &mut files)?;
        if files.is_empty() {
            bail!("catalog data directory has no release files");
        }
        files.sort_by(|left, right| left.path.cmp(&right.path));
        let release_id = release_id(&files)?;
        let generated_at = generated_at
            .format(&Rfc3339)
            .context("format catalog release time")?;
        let release = Self {
            schema_version: CATALOG_RELEASE_SCHEMA_VERSION,
            release_id,
            generated_at,
            producer: CatalogProducer {
                name: "wbunpacker".into(),
                version: env!("CARGO_PKG_VERSION").into(),
            },
            sources,
            files,
        };
        release.validate()?;
        Ok(release)
    }

    pub fn load(path: &Path) -> Result<Self> {
        let raw = fs::read_to_string(path)
            .with_context(|| format!("read catalog release: {}", path.display()))?;
        let release: Self = serde_json::from_str(&raw)
            .with_context(|| format!("parse catalog release: {}", path.display()))?;
        release.validate()?;
        Ok(release)
    }

    pub fn validate(&self) -> Result<()> {
        if self.schema_version != CATALOG_RELEASE_SCHEMA_VERSION {
            bail!("catalog release schema_version must be {CATALOG_RELEASE_SCHEMA_VERSION}");
        }
        if !valid_release_id(&self.release_id) {
            bail!("catalog release_id is invalid");
        }
        OffsetDateTime::parse(&self.generated_at, &Rfc3339)
            .context("catalog generated_at must be RFC3339")?;
        if self.producer.name.trim().is_empty() || self.producer.version.trim().is_empty() {
            bail!("catalog producer name and version are required");
        }
        let mut previous = None;
        for source in &self.sources {
            if source.name.trim().is_empty() || source.reference.trim().is_empty() {
                bail!("catalog source name and reference are required");
            }
        }
        for file in &self.files {
            validate_catalog_path(&file.path)?;
            if !valid_sha256(&file.sha256) {
                bail!("catalog file {} has an invalid SHA-256", file.path);
            }
            if file.object != object_key(&file.path, &file.sha256) {
                bail!("catalog file {} has an invalid object key", file.path);
            }
            if previous.is_some_and(|value: &str| value >= file.path.as_str()) {
                bail!("catalog files must be sorted by path without duplicates");
            }
            previous = Some(file.path.as_str());
        }
        let expected = release_id(&self.files)?;
        if self.release_id != expected {
            bail!(
                "catalog release_id mismatch: manifest={} calculated={expected}",
                self.release_id
            );
        }
        Ok(())
    }

    pub fn summary(&self) -> CatalogSummary {
        CatalogSummary {
            release_id: self.release_id.clone(),
            file_count: self.files.len(),
            total_bytes: self.files.iter().map(|file| file.size).sum(),
        }
    }

    pub fn pointer(&self) -> CatalogPointer {
        let hash = self.release_id.trim_start_matches("sha256:");
        CatalogPointer {
            schema_version: 1,
            release_id: self.release_id.clone(),
            generated_at: self.generated_at.clone(),
            manifest: format!("releases/{hash}.json"),
            object_base: "objects/".into(),
            file_count: self.files.len(),
            total_bytes: self.files.iter().map(|file| file.size).sum(),
        }
    }

    pub fn verify_root(&self, root: &Path) -> Result<()> {
        let rebuilt = Self::build(
            root,
            self.sources.clone(),
            OffsetDateTime::parse(&self.generated_at, &Rfc3339)?,
        )?;
        if rebuilt.files != self.files {
            bail!("catalog files do not match the current data directory");
        }
        if rebuilt.release_id != self.release_id {
            bail!("catalog release_id does not match the current data directory");
        }
        Ok(())
    }
}

impl CatalogPointer {
    pub fn validate(&self) -> Result<()> {
        if self.schema_version != 1 {
            bail!("catalog pointer schema_version must be 1");
        }
        if !valid_release_id(&self.release_id) {
            bail!("catalog pointer release_id is invalid");
        }
        OffsetDateTime::parse(&self.generated_at, &Rfc3339)
            .context("catalog pointer generated_at must be RFC3339")?;
        let hash = self.release_id.trim_start_matches("sha256:");
        if self.manifest != format!("releases/{hash}.json")
            || self.object_base != "objects/"
            || self.file_count == 0
        {
            bail!("catalog pointer fields are invalid");
        }
        Ok(())
    }
}

impl CatalogDiff {
    pub fn between(old: &CatalogRelease, new: &CatalogRelease) -> Self {
        let old_files = file_map(&old.files);
        let new_files = file_map(&new.files);
        let mut diff = Self::default();
        for (path, file) in &new_files {
            match old_files.get(path) {
                None => diff.added.push((*path).to_string()),
                Some(previous) if previous.size != file.size || previous.sha256 != file.sha256 => {
                    diff.changed.push((*path).to_string());
                }
                Some(_) => {}
            }
        }
        for path in old_files.keys() {
            if !new_files.contains_key(path) {
                diff.removed.push((*path).to_string());
            }
        }
        diff
    }

    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty() && self.changed.is_empty()
    }
}

pub fn stage(
    root: &Path,
    release: &CatalogRelease,
    previous: Option<&CatalogRelease>,
    output: &Path,
) -> Result<CatalogStageSummary> {
    release.validate()?;
    let root = root
        .canonicalize()
        .with_context(|| format!("resolve catalog root: {}", root.display()))?;
    let objects = output.join("objects");
    fs::create_dir_all(&objects)
        .with_context(|| format!("create catalog object directory: {}", objects.display()))?;
    let previous_files = previous.map(|release| file_map(&release.files));
    let mut summary = CatalogStageSummary::default();
    for file in &release.files {
        if previous_files
            .as_ref()
            .and_then(|files| files.get(file.path.as_str()))
            .is_some_and(|old| old.size == file.size && old.sha256 == file.sha256)
        {
            summary.skipped += 1;
            continue;
        }
        let object = objects.join(&file.object);
        if object.is_file() {
            summary.reused += 1;
            continue;
        }
        if let Some(parent) = object.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("create object directory: {}", parent.display()))?;
        }
        let source = root.join(&file.path);
        copy_object(&source, &object, file)?;
        summary.copied += 1;
        summary.copied_bytes += file.size;
    }
    let manifest = serde_json::to_vec_pretty(release)?;
    write_atomic(&output.join("manifest.json"), &manifest)?;
    Ok(summary)
}

pub fn pull(
    root: &Path,
    release: &CatalogRelease,
    base_url: &str,
    timeout: std::time::Duration,
) -> Result<CatalogPullSummary> {
    release.validate()?;
    let client = reqwest::blocking::Client::builder()
        .timeout(timeout)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .context("create catalog download client")?;
    let object_base = format!("{}/objects/", base_url.trim_end_matches('/'));
    let mut summary = CatalogPullSummary::default();
    for file in &release.files {
        let target = root.join(&file.path);
        if target.is_file() {
            let (size, sha256) = hash_file(&file.path, &target)?;
            if size == file.size && sha256 == file.sha256 {
                summary.reused += 1;
                continue;
            }
        }
        let url = format!("{object_base}{}", file.object);
        let response = client
            .get(&url)
            .send()
            .with_context(|| format!("GET {url}"))?
            .error_for_status()
            .with_context(|| format!("GET {url} status"))?;
        let mut body = Vec::with_capacity(file.size.min(8 * 1024 * 1024) as usize);
        response
            .take(file.size + 1)
            .read_to_end(&mut body)
            .with_context(|| format!("read {url}"))?;
        if body.len() as u64 != file.size {
            bail!(
                "catalog object size mismatch: {} expected={} actual={}",
                file.object,
                file.size,
                body.len()
            );
        }
        let digest = format!("{:x}", Sha256::digest(&body));
        if digest != file.sha256 {
            bail!(
                "catalog object SHA-256 mismatch: {} expected={} actual={digest}",
                file.object,
                file.sha256
            );
        }
        write_atomic(&target, &body)?;
        summary.downloaded += 1;
        summary.downloaded_bytes += file.size;
    }
    Ok(summary)
}

fn copy_object(source: &Path, object: &Path, file: &CatalogFile) -> Result<()> {
    let mut input =
        File::open(source).with_context(|| format!("open catalog file: {}", source.display()))?;
    let temporary = object.with_extension(format!("tmp-{}", std::process::id()));
    let mut output = File::create(&temporary)
        .with_context(|| format!("create staged object: {}", temporary.display()))?;
    let mut hasher = Sha256::new();
    let mut size = 0u64;
    let mut buffer = [0u8; 1024 * 1024];
    loop {
        let read = input
            .read(&mut buffer)
            .with_context(|| format!("read catalog file: {}", source.display()))?;
        if read == 0 {
            break;
        }
        size += read as u64;
        hasher.update(&buffer[..read]);
        output
            .write_all(&buffer[..read])
            .with_context(|| format!("write staged object: {}", temporary.display()))?;
    }
    output
        .flush()
        .with_context(|| format!("flush staged object: {}", temporary.display()))?;
    drop(output);
    let digest = format!("{:x}", hasher.finalize());
    if size != file.size || digest != file.sha256 {
        let _ = fs::remove_file(&temporary);
        bail!(
            "catalog file changed while staging: path={} manifest={}x{} actual={}x{}",
            file.path,
            file.size,
            file.sha256,
            size,
            digest
        );
    }
    fs::rename(&temporary, object)
        .with_context(|| format!("publish staged object: {}", object.display()))?;
    Ok(())
}

fn write_atomic(path: &Path, body: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create manifest directory: {}", parent.display()))?;
    }
    let temporary = path.with_extension(format!("tmp-{}", std::process::id()));
    fs::write(&temporary, body)
        .with_context(|| format!("write staged manifest: {}", temporary.display()))?;
    fs::rename(&temporary, path)
        .with_context(|| format!("publish staged manifest: {}", path.display()))?;
    Ok(())
}

fn collect_files(root: &Path, directory: &Path, output: &mut Vec<CatalogFile>) -> Result<()> {
    let mut entries = fs::read_dir(directory)
        .with_context(|| format!("read catalog directory: {}", directory.display()))?
        .collect::<std::io::Result<Vec<_>>>()
        .with_context(|| format!("list catalog directory: {}", directory.display()))?;
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        let path = entry.path();
        let file_type = entry
            .file_type()
            .with_context(|| format!("read file type: {}", path.display()))?;
        if file_type.is_symlink() {
            bail!(
                "catalog release does not accept symlinks: {}",
                path.display()
            );
        }
        if file_type.is_dir() {
            if path == root.join("data/wbcapture") {
                continue;
            }
            collect_files(root, &path, output)?;
            continue;
        }
        if !file_type.is_file() || ignored_catalog_file(&path) {
            continue;
        }
        let relative = path
            .strip_prefix(root)
            .with_context(|| format!("resolve catalog path: {}", path.display()))?;
        let path = relative
            .components()
            .map(|component| {
                component
                    .as_os_str()
                    .to_str()
                    .map(str::to_string)
                    .with_context(|| {
                        format!("catalog path is not valid UTF-8: {}", relative.display())
                    })
            })
            .collect::<Result<Vec<_>>>()?
            .join("/");
        validate_catalog_path(&path)?;
        let (size, sha256) = hash_file(&path, &entry.path())?;
        output.push(CatalogFile {
            object: object_key(&path, &sha256),
            path,
            size,
            sha256,
        });
    }
    Ok(())
}

fn ignored_catalog_file(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| {
            matches!(
                name.to_ascii_lowercase().as_str(),
                ".ds_store" | "thumbs.db"
            )
        })
}

fn hash_file(relative: &str, path: &Path) -> Result<(u64, String)> {
    let mut file =
        File::open(path).with_context(|| format!("open catalog file: {}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut size = 0u64;
    let mut buffer = [0u8; 1024 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .with_context(|| format!("read catalog file: {}", path.display()))?;
        if read == 0 {
            break;
        }
        size += read as u64;
        hasher.update(&buffer[..read]);
    }
    if relative.is_empty() {
        bail!("catalog path is empty");
    }
    Ok((size, format!("{:x}", hasher.finalize())))
}

fn release_id(files: &[CatalogFile]) -> Result<String> {
    let mut body = Vec::new();
    body.extend_from_slice(RELEASE_ID_DOMAIN.as_bytes());
    body.push(b'\n');
    for file in files {
        validate_catalog_path(&file.path)?;
        body.extend_from_slice(file.path.as_bytes());
        body.push(b'\t');
        body.extend_from_slice(file.size.to_string().as_bytes());
        body.push(b'\t');
        body.extend_from_slice(file.sha256.as_bytes());
        body.push(b'\n');
    }
    Ok(format!("sha256:{:x}", Sha256::digest(body)))
}

fn file_map(files: &[CatalogFile]) -> BTreeMap<&str, &CatalogFile> {
    files
        .iter()
        .map(|file| (file.path.as_str(), file))
        .collect()
}

fn object_key(path: &str, sha256: &str) -> String {
    let extension = Path::new(path)
        .extension()
        .and_then(|value| value.to_str())
        .filter(|value| {
            !value.is_empty()
                && value.len() <= 16
                && value.bytes().all(|byte| byte.is_ascii_alphanumeric())
        })
        .map(|value| format!(".{}", value.to_ascii_lowercase()))
        .unwrap_or_default();
    format!("{}/{sha256}{extension}", &sha256[..2])
}

fn validate_catalog_path(path: &str) -> Result<()> {
    if !path.starts_with("data/")
        || path.starts_with("data/wbcapture/")
        || path.contains('\\')
        || path.contains('\0')
        || path.contains('\t')
        || path.contains('\r')
        || path.contains('\n')
        || path
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        bail!("invalid catalog path: {path}");
    }
    Ok(())
}

fn valid_release_id(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(valid_sha256)
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(root: &Path, path: &str, body: &[u8]) {
        let path = root.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, body).unwrap();
    }

    #[test]
    fn excludes_wbcapture_and_system_files() {
        let root = tempfile::tempdir().unwrap();
        write(root.path(), "data/cards.json", b"[]");
        write(root.path(), "data/ui/icon.png", b"icon");
        write(root.path(), "data/wbcapture/SV_WB_Cards.csv", b"capture");
        write(root.path(), "data/.DS_Store", b"junk");
        let release =
            CatalogRelease::build(root.path(), vec![], OffsetDateTime::UNIX_EPOCH).unwrap();
        let paths = release
            .files
            .iter()
            .map(|file| file.path.as_str())
            .collect::<Vec<_>>();
        assert_eq!(paths, ["data/cards.json", "data/ui/icon.png"]);
    }

    #[test]
    fn release_id_is_deterministic_and_time_independent() {
        let root = tempfile::tempdir().unwrap();
        write(root.path(), "data/b.json", b"b");
        write(root.path(), "data/a.json", b"a");
        let first = CatalogRelease::build(root.path(), vec![], OffsetDateTime::UNIX_EPOCH).unwrap();
        let second = CatalogRelease::build(
            root.path(),
            vec![],
            OffsetDateTime::UNIX_EPOCH + time::Duration::hours(1),
        )
        .unwrap();
        assert_eq!(first.release_id, second.release_id);
        assert_eq!(first.files[0].path, "data/a.json");
    }

    #[test]
    fn verify_detects_changed_files() {
        let root = tempfile::tempdir().unwrap();
        write(root.path(), "data/cards.json", b"[]");
        let release =
            CatalogRelease::build(root.path(), vec![], OffsetDateTime::UNIX_EPOCH).unwrap();
        write(root.path(), "data/cards.json", b"[1]");
        assert!(release.verify_root(root.path()).is_err());
    }

    #[test]
    fn diff_reports_added_removed_and_changed() {
        let old_root = tempfile::tempdir().unwrap();
        write(old_root.path(), "data/change.json", b"old");
        write(old_root.path(), "data/remove.json", b"remove");
        let old =
            CatalogRelease::build(old_root.path(), vec![], OffsetDateTime::UNIX_EPOCH).unwrap();

        let new_root = tempfile::tempdir().unwrap();
        write(new_root.path(), "data/change.json", b"new");
        write(new_root.path(), "data/add.json", b"add");
        let new =
            CatalogRelease::build(new_root.path(), vec![], OffsetDateTime::UNIX_EPOCH).unwrap();

        let diff = CatalogDiff::between(&old, &new);
        assert_eq!(diff.added, ["data/add.json"]);
        assert_eq!(diff.removed, ["data/remove.json"]);
        assert_eq!(diff.changed, ["data/change.json"]);
    }

    #[test]
    fn example_manifest_is_valid() {
        let release: CatalogRelease = serde_json::from_str(include_str!(
            "../../../contracts/examples/catalog-release.json"
        ))
        .unwrap();
        release.validate().unwrap();
        let pointer: CatalogPointer = serde_json::from_str(include_str!(
            "../../../contracts/examples/catalog-pointer.json"
        ))
        .unwrap();
        pointer.validate().unwrap();
        assert_eq!(release.pointer(), pointer);
    }

    #[test]
    fn stage_writes_content_addressed_objects_and_manifest() {
        let root = tempfile::tempdir().unwrap();
        write(root.path(), "data/a.json", b"a");
        let release =
            CatalogRelease::build(root.path(), vec![], OffsetDateTime::UNIX_EPOCH).unwrap();
        let output = tempfile::tempdir().unwrap();
        let summary = stage(root.path(), &release, None, output.path()).unwrap();
        assert_eq!(summary.copied, 1);
        assert!(output.path().join("manifest.json").is_file());
        assert!(
            output
                .path()
                .join("objects")
                .join(&release.files[0].object)
                .is_file()
        );
    }

    #[test]
    fn stage_delta_skips_unchanged_files() {
        let old_root = tempfile::tempdir().unwrap();
        write(old_root.path(), "data/same.json", b"same");
        write(old_root.path(), "data/change.json", b"old");
        let old =
            CatalogRelease::build(old_root.path(), vec![], OffsetDateTime::UNIX_EPOCH).unwrap();

        let new_root = tempfile::tempdir().unwrap();
        write(new_root.path(), "data/same.json", b"same");
        write(new_root.path(), "data/change.json", b"new");
        let new =
            CatalogRelease::build(new_root.path(), vec![], OffsetDateTime::UNIX_EPOCH).unwrap();

        let output = tempfile::tempdir().unwrap();
        let summary = stage(new_root.path(), &new, Some(&old), output.path()).unwrap();
        assert_eq!(summary.copied, 1);
        assert_eq!(summary.skipped, 1);
    }
}
