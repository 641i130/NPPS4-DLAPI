// Copyright (c) 2023 Dark Energy Processor
//
// This software is provided 'as-is', without any express or implied
// warranty. In no event will the authors be held liable for any damages
// arising from the use of this software.
//
// Permission is granted to anyone to use this software for any purpose,
// including commercial applications, and to alter it and redistribute it
// freely, subject to the following restrictions:
//
// 1. The origin of this software must not be misrepresented; you must not
//    claim that you wrote the original software. If you use this software
//    in a product, an acknowledgment in the product documentation would be
//    appreciated but is not required.
// 2. Altered source versions must be plainly marked as such, and must not be
//    misrepresented as being the original software.
// 3. This notice may not be removed or altered from any source distribution.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Mutex,
    time::SystemTime,
};
use anyhow::{anyhow, Context};
use serde_json::Value;

use crate::models::{
    BatchDownloadInfoModel, ChecksumModel, DownloadInfoModel, DownloadUpdateModel,
    FileEntry, MD5_EMPTY, SHA256_EMPTY,
};

pub const PLATFORM_MAP: &[&str] = &["iOS", "Android"];

/// JSON cache keyed by file path, storing (mtime, parsed value).
pub struct JsonCache {
    map: HashMap<PathBuf, (SystemTime, Value)>,
}

impl JsonCache {
    pub fn new() -> Self {
        JsonCache { map: HashMap::new() }
    }

    pub fn read_json(&mut self, path: &Path) -> anyhow::Result<Value> {
        let meta = std::fs::metadata(path)
            .with_context(|| format!("Cannot stat: {}", path.display()))?;
        let mtime = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);

        if let Some((cached_mtime, value)) = self.map.get(path) {
            if *cached_mtime >= mtime {
                return Ok(value.clone());
            }
        }

        let raw = std::fs::read_to_string(path)
            .with_context(|| format!("Cannot read: {}", path.display()))?;
        let value: Value = serde_json::from_str(&raw)
            .with_context(|| format!("Invalid JSON in: {}", path.display()))?;

        self.map.insert(path.to_path_buf(), (mtime, value.clone()));
        Ok(value)
    }
}

// ── Version helpers ────────────────────────────────────────────────────────────

fn parse_version(s: &str) -> Option<(u32, u32)> {
    let mut parts = s.splitn(2, '.');
    let major: u32 = parts.next()?.parse().ok()?;
    let minor: u32 = parts.next()?.parse().ok()?;
    Some((major, minor))
}

fn version_string(v: (u32, u32)) -> String {
    format!("{}.{}", v.0, v.1)
}

/// Parse a JSON array of version strings into sorted `(major, minor)` tuples.
fn parse_versions_from_json(value: &Value) -> Vec<(u32, u32)> {
    let mut versions: Vec<(u32, u32)> = value
        .as_array()
        .unwrap_or(&vec![])
        .iter()
        .filter_map(|v| v.as_str().and_then(parse_version))
        .collect();
    versions.sort();
    versions
}

// ── FileState ─────────────────────────────────────────────────────────────────

pub struct FileState {
    pub archive_root: PathBuf,
    pub json_cache: Mutex<JsonCache>,
    /// Cached preferred platform index (0=iOS, 1=Android).
    /// -1 means not yet determined.
    update_preference: Mutex<Option<usize>>,
}

impl FileState {
    pub fn new(archive_root: PathBuf) -> Self {
        FileState {
            archive_root,
            json_cache: Mutex::new(JsonCache::new()),
            update_preference: Mutex::new(None),
        }
    }

    fn read_json(&self, path: &Path) -> anyhow::Result<Value> {
        self.json_cache.lock().unwrap().read_json(path)
    }

    /// Return the index (0=iOS, 1=Android) of the platform that has package data.
    pub fn get_update_preference(&self) -> anyhow::Result<usize> {
        {
            let pref = self.update_preference.lock().unwrap();
            if let Some(p) = *pref {
                return Ok(p);
            }
        }
        for (i, plat) in PLATFORM_MAP.iter().enumerate() {
            let info_path = self.archive_root
                .join(plat)
                .join("package")
                .join("info.json");
            if info_path.is_file() {
                *self.update_preference.lock().unwrap() = Some(i);
                return Ok(i);
            }
        }
        Err(anyhow!("No package data found in archive root"))
    }

    /// Return the latest game version as `(major, minor)`.
    pub fn get_latest_version(&self) -> anyhow::Result<(u32, u32)> {
        let pref = self.get_update_preference()?;
        let info_path = self.archive_root
            .join(PLATFORM_MAP[pref])
            .join("package")
            .join("info.json");
        let value = self.read_json(&info_path)?;
        let versions = parse_versions_from_json(&value);
        versions
            .into_iter()
            .max()
            .ok_or_else(|| anyhow!("No versions found in package/info.json"))
    }

    /// Return the release_info key map.
    pub fn get_release_info(&self) -> anyhow::Result<serde_json::Map<String, Value>> {
        let path = self.archive_root.join("release_info.json");
        let value = self.read_json(&path)?;
        value
            .as_object()
            .cloned()
            .ok_or_else(|| anyhow!("release_info.json is not an object"))
    }

    /// Get download entries for updates from `old_client_version` to latest.
    pub fn get_update_file(
        &self,
        old_client_version: &str,
        platform: usize,
    ) -> anyhow::Result<Vec<DownloadUpdateModel>> {
        let current_version = parse_version(old_client_version)
            .ok_or_else(|| anyhow!("Invalid version string: {old_client_version}"))?;

        let platform_str = PLATFORM_MAP[platform];
        let update_dir = self.archive_root.join(platform_str).join("update");

        // infov2.json in the update dir is a list of available update versions
        let versions_path = update_dir.join("infov2.json");
        let versions_value = self.read_json(&versions_path)?;
        let updates = parse_versions_from_json(&versions_value);

        if updates.is_empty() {
            return Ok(vec![]);
        }
        if current_version >= *updates.last().unwrap() {
            // Already up to date
            return Ok(vec![]);
        }

        let mut result = Vec::new();
        for ver in updates.iter().filter(|&&v| v > current_version) {
            let ver_str = version_string(*ver);
            let ver_dir = update_dir.join(&ver_str);
            let files_path = ver_dir.join("infov2.json");
            let files_value = self.read_json(&files_path)?;

            let entries: Vec<FileEntry> = serde_json::from_value(files_value)
                .with_context(|| format!("Bad infov2.json in {}", ver_dir.display()))?;

            for entry in entries {
                let full_path = ver_dir.join(&entry.name);
                let url = self.path_to_url(&full_path);
                result.push(DownloadUpdateModel {
                    url,
                    size: entry.size,
                    checksums: ChecksumModel { md5: entry.md5, sha256: entry.sha256 },
                    version: ver_str.clone(),
                });
            }
        }

        Ok(result)
    }

    /// Get all packages of the given type, excluding specific package IDs.
    /// Returns `None` if the package type directory does not exist.
    pub fn get_batch_list(
        &self,
        pkgtype: u8,
        platform: usize,
        exclude: &[i64],
    ) -> anyhow::Result<Option<Vec<BatchDownloadInfoModel>>> {
        let latest = self.get_latest_version()?;
        let ver_str = version_string(latest);
        let platform_str = PLATFORM_MAP[platform];

        let type_dir = self.archive_root
            .join(platform_str)
            .join("package")
            .join(&ver_str)
            .join(pkgtype.to_string());

        if !type_dir.is_dir() {
            return Ok(None);
        }

        let ids_path = type_dir.join("info.json");
        let ids_value = self.read_json(&ids_path)?;
        let all_ids: Vec<i64> = ids_value
            .as_array()
            .unwrap_or(&vec![])
            .iter()
            .filter_map(|v| v.as_i64())
            .collect();

        // Deduplicate and exclude, then sort numerically (same as Python's sorted(set(...).difference(...)))
        let mut ids: Vec<i64> = {
            let exclude_set: std::collections::HashSet<i64> = exclude.iter().copied().collect();
            let mut seen = std::collections::HashSet::new();
            all_ids.into_iter()
                .filter(|id| seen.insert(*id) && !exclude_set.contains(id))
                .collect()
        };
        ids.sort();

        let mut result = Vec::new();
        for pkgid in ids {
            let pkg_dir = type_dir.join(pkgid.to_string());
            let files_path = pkg_dir.join("infov2.json");
            let files_value = self.read_json(&files_path)?;

            let entries: Vec<FileEntry> = serde_json::from_value(files_value)
                .with_context(|| format!("Bad infov2.json in {}", pkg_dir.display()))?;

            for entry in entries {
                let full_path = pkg_dir.join(&entry.name);
                let url = self.path_to_url(&full_path);
                result.push(BatchDownloadInfoModel {
                    url,
                    size: entry.size,
                    checksums: ChecksumModel { md5: entry.md5, sha256: entry.sha256 },
                    package_id: pkgid,
                });
            }
        }

        Ok(Some(result))
    }

    /// Get download entries for a specific package type and ID.
    /// Returns `None` if not found.
    pub fn get_single_package(
        &self,
        pkgtype: u8,
        pkgid: i64,
        platform: usize,
    ) -> anyhow::Result<Option<Vec<DownloadInfoModel>>> {
        let latest = self.get_latest_version()?;
        let ver_str = version_string(latest);
        let platform_str = PLATFORM_MAP[platform];

        let pkg_dir = self.archive_root
            .join(platform_str)
            .join("package")
            .join(&ver_str)
            .join(pkgtype.to_string())
            .join(pkgid.to_string());

        if !pkg_dir.is_dir() {
            return Ok(None);
        }

        let files_path = pkg_dir.join("infov2.json");
        let files_value = self.read_json(&files_path)?;
        let entries: Vec<FileEntry> = serde_json::from_value(files_value)
            .with_context(|| format!("Bad infov2.json in {}", pkg_dir.display()))?;

        let result = entries
            .into_iter()
            .map(|entry| {
                let full_path = pkg_dir.join(&entry.name);
                let url = self.path_to_url(&full_path);
                DownloadInfoModel {
                    url,
                    size: entry.size,
                    checksums: ChecksumModel { md5: entry.md5, sha256: entry.sha256 },
                }
            })
            .collect();

        Ok(Some(result))
    }

    /// Read a pre-decrypted database file. Returns `None` if not found.
    pub fn get_database_file(&self, name: &str) -> anyhow::Result<Option<Vec<u8>>> {
        let pref = self.get_update_preference()?;
        let latest = self.get_latest_version()?;
        let ver_str = version_string(latest);
        let platform_str = PLATFORM_MAP[pref];

        // Sanitize: only alphanumeric and underscore
        let db_name: String = name
            .chars()
            .filter(|c| c.is_alphanumeric() || *c == '_')
            .collect();

        let db_path = self.archive_root
            .join(platform_str)
            .join("package")
            .join(&ver_str)
            .join("db")
            .join(format!("{db_name}.db_"));

        match std::fs::read(&db_path) {
            Ok(data) => Ok(Some(data)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Get the download info for a single micro-download file.
    /// Always returns a result (with empty checksums if file not found).
    pub fn get_microdl_file(
        &self,
        file_path: &str,
        platform: usize,
    ) -> anyhow::Result<DownloadInfoModel> {
        let latest = self.get_latest_version()?;
        let ver_str = version_string(latest);
        let platform_str = PLATFORM_MAP[platform];

        let common_path = format!("{platform_str}/package/{ver_str}/microdl");
        let base_path = self.archive_root.join(&common_path);

        // Sanitize path: remove ".." components and normalize
        let sanitized = sanitize_path(file_path);

        let url_path = format!("{common_path}/{sanitized}");

        // Default: empty file response
        let mut result = DownloadInfoModel {
            url: url_path.clone(),
            size: 0,
            checksums: ChecksumModel {
                md5: MD5_EMPTY.to_string(),
                sha256: SHA256_EMPTY.to_string(),
            },
        };

        // Try to read microdl/info.json
        let info_path = base_path.join("info.json");
        if let Ok(map_value) = self.read_json(&info_path) {
            if let Some(obj) = map_value.as_object() {
                if let Some(info) = obj.get(&sanitized) {
                    if let (Some(size), Some(md5), Some(sha256)) = (
                        info.get("size").and_then(|v| v.as_u64()),
                        info.get("md5").and_then(|v| v.as_str()),
                        info.get("sha256").and_then(|v| v.as_str()),
                    ) {
                        result.size = size;
                        result.checksums.md5 = md5.to_string();
                        result.checksums.sha256 = sha256.to_string();
                    }
                }
            }
        }

        Ok(result)
    }

    /// Convert an absolute path inside archive_root to a URL-path component
    /// relative to archive_root (with a leading `/`).
    fn path_to_url(&self, full_path: &Path) -> String {
        match full_path.strip_prefix(&self.archive_root) {
            Ok(rel) => format!("/{}", rel.to_string_lossy().replace('\\', "/")),
            Err(_) => format!("/{}", full_path.to_string_lossy().replace('\\', "/")),
        }
    }
}

/// Sanitize a micro-download file path:
/// - Remove `..` occurrences
/// - Collect only Normal path components (strips leading `/`, `.`, etc.)
/// - Re-join with `/`
fn sanitize_path(file_path: &str) -> String {
    let no_dotdot = file_path.replace("..", "");
    let mut components = Vec::new();
    for part in Path::new(&no_dotdot).components() {
        if let std::path::Component::Normal(s) = part {
            let s = s.to_string_lossy();
            if !s.is_empty() {
                components.push(s.into_owned());
            }
        }
    }
    components.join("/")
}
