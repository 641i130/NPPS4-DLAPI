// Port of clone.py — downloads a full archive from a remote NPPS4-DLAPI server.
//
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
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::{anyhow, Context};
use clap::Args;
use serde::{Deserialize, Serialize};
use zip::ZipArchive;

use crate::util::{hash_bytes, nat_cmp, parse_version, read_json_file, version_string, write_json_file};

const NEED_DLAPI_VERSION: (u32, u32) = (1, 1);
const MAX_RETRIES: u32 = 25;

// ── CLI args ───────────────────────────────────────────────────────────────────

#[derive(Debug, Args)]
pub struct CloneArgs {
    /// Destination directory to store the mirrored files.
    pub destination: PathBuf,

    /// URL to the NPPS4-DLAPI server (protocol v1.1+ required).
    ///
    /// Scheme defaults to https:// if omitted. A trailing slash is optional.
    pub mirror: String,

    /// Shared key to authenticate with the remote server.
    #[arg(long, default_value = "")]
    pub shared_key: String,

    /// Skip iOS file download.
    #[arg(long)]
    pub no_ios: bool,

    /// Skip Android file download.
    #[arg(long)]
    pub no_android: bool,

    /// Base SIF version for the update download (default "59.0").
    ///
    /// Updates are only fetched for versions newer than this baseline.
    #[arg(long, default_value = "59.0")]
    pub base_version: String,
}

// ── Resume file structures (replace Python pickle) ────────────────────────────

#[derive(Serialize, Deserialize)]
struct UpdateResume {
    version: String,
    expire: i64,
    files: Vec<ResumeUpdateEntry>,
}

#[derive(Serialize, Deserialize)]
struct ResumeUpdateEntry {
    url: String,
    size: u64,
    md5: String,
    sha256: String,
    version: String,
}

#[derive(Serialize, Deserialize)]
struct BatchResume {
    version: String,
    expire: i64,
    files: Vec<ResumeBatchEntry>,
}

#[derive(Serialize, Deserialize)]
struct ResumeBatchEntry {
    url: String,
    size: u64,
    md5: String,
    sha256: String,
    package_id: i64,
}

// ── HTTP error sentinel (never retried) ───────────────────────────────────────

#[derive(Debug)]
struct HttpError(u16, String);

impl std::fmt::Display for HttpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "HTTP {} from {}", self.0, self.1)
    }
}
impl std::error::Error for HttpError {}

// ── HTTP client ───────────────────────────────────────────────────────────────

struct ApiClient {
    client: reqwest::blocking::Client,
    api_base: String,
    shared_key: String,
}

impl ApiClient {
    fn new(api_base: String, shared_key: String) -> anyhow::Result<Self> {
        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()?;
        Ok(ApiClient { client, api_base, shared_key })
    }

    /// Call an API endpoint with optional JSON body; retry on network errors.
    fn call_api(&self, endpoint: &str, body: Option<serde_json::Value>) -> anyhow::Result<serde_json::Value> {
        let url = format!("{}{}", self.api_base, endpoint);
        let mut retry = 0u32;
        loop {
            match self.try_call_api(&url, body.as_ref()) {
                Ok(v) => return Ok(v),
                Err(e) => {
                    // HttpError means non-success HTTP status — never retry.
                    if e.downcast_ref::<HttpError>().is_some() {
                        return Err(e);
                    }
                    retry += 1;
                    if retry >= MAX_RETRIES {
                        return Err(e);
                    }
                    eprintln!("  Retrying API ({retry}/{MAX_RETRIES}): {e}");
                }
            }
        }
    }

    fn try_call_api(&self, url: &str, body: Option<&serde_json::Value>) -> anyhow::Result<serde_json::Value> {
        let mut builder = if body.is_some() {
            self.client.post(url)
        } else {
            self.client.get(url)
        };
        if !self.shared_key.is_empty() {
            builder = builder.header(
                "DLAPI-Shared-Key",
                urlencoding::encode(&self.shared_key).as_ref(),
            );
        }
        if let Some(b) = body {
            builder = builder.json(b);
        }
        let resp = builder.send()?;
        let status = resp.status();
        if !status.is_success() {
            return Err(HttpError(status.as_u16(), url.to_string()).into());
        }
        Ok(resp.json()?)
    }

    /// Download a file by URL; retry on network errors but not HTTP errors.
    fn download_file(&self, url: &str) -> anyhow::Result<Vec<u8>> {
        let mut retry = 0u32;
        loop {
            match self.try_download(url) {
                Ok(v) => return Ok(v),
                Err(e) => {
                    if e.downcast_ref::<HttpError>().is_some() {
                        return Err(e);
                    }
                    retry += 1;
                    if retry >= MAX_RETRIES {
                        return Err(e);
                    }
                    eprintln!("  Retrying download ({retry}/{MAX_RETRIES}): {e}");
                }
            }
        }
    }

    fn try_download(&self, url: &str) -> anyhow::Result<Vec<u8>> {
        let resp = self.client.get(url).send()?;
        let status = resp.status();
        if !status.is_success() {
            return Err(HttpError(status.as_u16(), url.to_string()).into());
        }
        Ok(resp.bytes()?.to_vec())
    }
}

// ── Helpers ────────────────────────────────────────────────────────────────────

fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Compute absolute expiry timestamp. Returns `i64::MAX` for no expiry (dt==0).
fn get_expiry_time(serve_time_limit: i64) -> i64 {
    if serve_time_limit == 0 { i64::MAX } else { now_unix() + serve_time_limit }
}

fn verify_hash(data: &[u8], md5_expected: &str, sha256_expected: &str) -> anyhow::Result<()> {
    let (md5, sha256) = hash_bytes(data);
    if md5 != md5_expected {
        return Err(anyhow!("MD5 mismatch: expected {md5_expected}, got {md5}"));
    }
    if sha256 != sha256_expected {
        return Err(anyhow!("SHA256 mismatch: expected {sha256_expected}, got {sha256}"));
    }
    Ok(())
}

fn remap_os(os_name: &str) -> anyhow::Result<u8> {
    match os_name {
        "iOS" => Ok(1),
        "Android" => Ok(2),
        _ => Err(anyhow!("Unknown OS: {os_name}")),
    }
}

/// Return the highest locally-stored game version across all platforms.
fn get_local_latest_version(root: &Path) -> Option<(u32, u32)> {
    for platform in &["iOS", "Android"] {
        let path = root.join(platform).join("package").join("info.json");
        if path.is_file() {
            if let Ok(versions) = read_json_file::<Vec<String>>(&path) {
                let mut parsed: Vec<(u32, u32)> =
                    versions.iter().filter_map(|s| parse_version(s)).collect();
                parsed.sort();
                if let Some(last) = parsed.last() {
                    return Some(*last);
                }
            }
        }
    }
    None
}

fn expiry_string(dt: i64) -> String {
    if dt == 0 {
        return "no expiration".to_string();
    }
    let hours = dt / 3600;
    let minutes = (dt % 3600) / 60;
    let seconds = dt % 60;
    let mut parts = Vec::new();
    if hours == 1 { parts.push("1 hour".to_string()); }
    else if hours > 1 { parts.push(format!("{hours} hours")); }
    if minutes == 1 { parts.push("1 minute".to_string()); }
    else if minutes > 1 { parts.push(format!("{minutes} minutes")); }
    if seconds == 1 { parts.push("1 second".to_string()); }
    else if seconds > 1 { parts.push(format!("{seconds} seconds")); }
    parts.join(" ")
}

// ── Resume/continue update ─────────────────────────────────────────────────────

fn continue_update(client: &ApiClient, platform_path: &Path) -> anyhow::Result<()> {
    let resume_path = platform_path.join("update.json");
    if !resume_path.is_file() {
        return Ok(());
    }
    let resume: UpdateResume = read_json_file(&resume_path)?;
    if now_unix() >= resume.expire {
        return Err(anyhow!(
            "Update links expired. Delete {} and try again.",
            resume_path.display()
        ));
    }

    println!("Resuming update download: {}", platform_path.display());

    // Group entries by version string
    let mut by_version: HashMap<String, Vec<&ResumeUpdateEntry>> = HashMap::new();
    for entry in &resume.files {
        by_version.entry(entry.version.clone()).or_default().push(entry);
    }

    let update_dir = platform_path.join("update");
    for (version, files) in &by_version {
        let ver_dir = update_dir.join(version);
        let info_path = ver_dir.join("info.json");
        fs::create_dir_all(&ver_dir)?;

        if !info_path.is_file() {
            let count = files.len();
            let mut info_map: HashMap<String, u64> = HashMap::new();
            for (i, entry) in files.iter().enumerate() {
                let name = format!("{}.zip", i + 1);
                let dest = ver_dir.join(&name);
                if !dest.is_file() {
                    println!("  Downloading update {}/{count} {}", i + 1, dest.display());
                    let data = client.download_file(&entry.url)?;
                    verify_hash(&data, &entry.md5, &entry.sha256)
                        .with_context(|| format!("Checksum failed for {}", entry.url))?;
                    fs::write(&dest, &data)?;
                }
                info_map.insert(name, entry.size);
            }
            write_json_file(&info_path, &info_map)?;
        }
    }

    // Append this version to the platform's version list
    let versionlist_path = update_dir.join("info.json");
    let mut versionlist: Vec<String> = if versionlist_path.is_file() {
        read_json_file(&versionlist_path).unwrap_or_default()
    } else {
        Vec::new()
    };
    if !versionlist.contains(&resume.version) {
        versionlist.push(resume.version.clone());
        write_json_file(&versionlist_path, &versionlist)?;
    }

    fs::remove_file(&resume_path)?;
    Ok(())
}

fn prepare_update(
    platform_path: &Path,
    target_version: &str,
    files: &[serde_json::Value],
    expire: i64,
) -> anyhow::Result<()> {
    let entries = files.iter().map(|f| ResumeUpdateEntry {
        url: f["url"].as_str().unwrap_or("").to_string(),
        size: f["size"].as_u64().unwrap_or(0),
        md5: f["checksums"]["md5"].as_str().unwrap_or("").to_string(),
        sha256: f["checksums"]["sha256"].as_str().unwrap_or("").to_string(),
        version: f["version"].as_str().unwrap_or("").to_string(),
    }).collect();

    write_json_file(
        &platform_path.join("update.json"),
        &UpdateResume { version: target_version.to_string(), expire, files: entries },
    )
}

// ── Resume/continue batch download ────────────────────────────────────────────

fn continue_batch_download(client: &ApiClient, pkg_root: &Path, pkg_type: u8) -> anyhow::Result<()> {
    let resume_path = pkg_root.join(format!("package_{pkg_type}.json"));
    if !resume_path.is_file() {
        return Ok(());
    }
    let resume: BatchResume = read_json_file(&resume_path)?;
    if now_unix() >= resume.expire {
        return Err(anyhow!(
            "Batch links expired. Delete {} and try again.",
            resume_path.display()
        ));
    }

    println!("Resuming batch download: type={pkg_type} {}", pkg_root.display());

    let ver_type_path = pkg_root.join(&resume.version).join(pkg_type.to_string());
    fs::create_dir_all(&ver_type_path)?;

    // Group entries by package_id
    let mut by_id: HashMap<i64, Vec<&ResumeBatchEntry>> = HashMap::new();
    for entry in &resume.files {
        by_id.entry(entry.package_id).or_default().push(entry);
    }

    for (&pkg_id, files) in &by_id {
        let id_dir = ver_type_path.join(pkg_id.to_string());
        let info_path = id_dir.join("info.json");
        fs::create_dir_all(&id_dir)?;

        if !info_path.is_file() {
            let count = files.len();
            let mut info_map: HashMap<String, u64> = HashMap::new();
            for (i, entry) in files.iter().enumerate() {
                let name = format!("{}.zip", i + 1);
                let dest = id_dir.join(&name);
                if !dest.is_file() {
                    println!(
                        "  Downloading package {pkg_type}/{pkg_id} {}/{count} {}",
                        i + 1,
                        dest.display()
                    );
                    let data = client.download_file(&entry.url)?;
                    verify_hash(&data, &entry.md5, &entry.sha256)
                        .with_context(|| format!("Checksum failed for {}", entry.url))?;
                    fs::write(&dest, &data)?;
                }
                info_map.insert(name, entry.size);
            }
            write_json_file(&info_path, &info_map)?;
        }
    }

    println!("  Building info.json for package type {pkg_type}");
    let mut ids: Vec<i64> = by_id.keys().copied().collect();
    ids.sort();
    write_json_file(&ver_type_path.join("info.json"), &ids)?;

    fs::remove_file(&resume_path)?;
    Ok(())
}

fn prepare_batch_download(
    pkg_root: &Path,
    target_version: &str,
    pkg_type: u8,
    files: &[serde_json::Value],
    expire: i64,
) -> anyhow::Result<()> {
    let entries = files.iter().map(|f| ResumeBatchEntry {
        url: f["url"].as_str().unwrap_or("").to_string(),
        size: f["size"].as_u64().unwrap_or(0),
        md5: f["checksums"]["md5"].as_str().unwrap_or("").to_string(),
        sha256: f["checksums"]["sha256"].as_str().unwrap_or("").to_string(),
        package_id: f["packageId"].as_i64().unwrap_or(0),
    }).collect();

    write_json_file(
        &pkg_root.join(format!("package_{pkg_type}.json")),
        &BatchResume { version: target_version.to_string(), expire, files: entries },
    )
}

// ── Microdl map ───────────────────────────────────────────────────────────────

/// Build `microdl_map.json` mapping each file within package-type-4 archives
/// to the archive path that contains it (most-recent version wins).
fn make_microdl_map(ver_dir: &Path) -> anyhow::Result<()> {
    let info_path = ver_dir.join("4").join("info.json");
    if !info_path.is_file() {
        return Ok(());
    }
    let ids: Vec<i64> = read_json_file(&info_path)?;
    let mut file_map: HashMap<String, String> = HashMap::new();

    for id in &ids {
        let id_dir = ver_dir.join("4").join(id.to_string());
        let archive_list: HashMap<String, u64> = read_json_file(&id_dir.join("info.json"))?;
        let mut names: Vec<String> = archive_list.keys().cloned().collect();
        names.sort_by(|a, b| nat_cmp(a, b));

        for name in names {
            let archive_path = id_dir.join(&name);
            println!("  Scanning {}", archive_path.display());
            let data = fs::read(&archive_path)?;
            let mut zip = ZipArchive::new(std::io::Cursor::new(data))?;
            for i in 0..zip.len() {
                let zf = zip.by_index(i)?;
                let filename = zf.name().to_string();
                // First occurrence wins (later archives in the sequence take precedence)
                file_map.entry(filename).or_insert_with(|| {
                    archive_path.to_string_lossy().into_owned()
                });
            }
        }
    }

    println!("  Writing microdl_map.json");
    write_json_file(&ver_dir.join("microdl_map.json"), &file_map)
}

// ── Main clone logic ──────────────────────────────────────────────────────────

pub fn run(args: CloneArgs) -> anyhow::Result<()> {
    let root = args.destination.clone();

    // Normalize mirror URL: ensure scheme and trailing slash
    let mut mirror = args.mirror.clone();
    if !mirror.starts_with("http://") && !mirror.starts_with("https://") {
        mirror = format!("https://{mirror}");
    }
    if !mirror.ends_with('/') {
        mirror.push('/');
    }

    // Determine platforms
    let mut oses: Vec<&str> = Vec::new();
    if !args.no_ios { oses.push("iOS"); }
    if !args.no_android { oses.push("Android"); }
    if oses.is_empty() {
        return Err(anyhow!("Nothing to download (both --no-ios and --no-android given)."));
    }

    let base_version = parse_version(&args.base_version)
        .ok_or_else(|| anyhow!("Invalid --base-version: {}", args.base_version))?;

    // Create platform dirs
    for os in &oses {
        fs::create_dir_all(root.join(os).join("package"))?;
    }

    let client = ApiClient::new(mirror.clone(), args.shared_key.clone())?;

    // Resume any interrupted downloads before talking to the server
    println!("Checking for interrupted downloads...");
    for os in &oses {
        continue_update(&client, &root.join(os))?;
    }
    for os in &oses {
        for pkg_type in 0u8..7 {
            continue_batch_download(&client, &root.join(os).join("package"), pkg_type)?;
        }
    }

    // Fetch server information
    println!("Calling public info API...");
    let public_info = client.call_api("api/publicinfo", None)?;

    let game_version_str = public_info["gameVersion"]
        .as_str()
        .ok_or_else(|| anyhow!("Missing gameVersion in publicinfo response"))?;
    let target_version = parse_version(game_version_str)
        .ok_or_else(|| anyhow!("Invalid gameVersion: {game_version_str}"))?;
    let target_version_str = version_string(target_version);

    let serve_time_limit = public_info["serveTimeLimit"].as_i64().unwrap_or(0);
    let dlapi_major = public_info["dlapiVersion"]["major"].as_u64().unwrap_or(0) as u32;
    let dlapi_minor = public_info["dlapiVersion"]["minor"].as_u64().unwrap_or(0) as u32;

    println!();
    println!("Mirror information");
    println!("  Base URL:          {mirror}");
    println!("  Protocol version:  {dlapi_major}.{dlapi_minor}");
    println!("  Latest game ver:   {target_version_str}");
    println!("  Link expiry:       {}", expiry_string(serve_time_limit));
    if let Some(app) = public_info["application"].as_object() {
        if !app.is_empty() {
            println!("  Additional server data:");
            for (k, v) in app {
                println!("    {k}: {v}");
            }
        }
    }
    println!();

    // Check protocol compatibility
    if dlapi_major != NEED_DLAPI_VERSION.0 || dlapi_minor < NEED_DLAPI_VERSION.1 {
        return Err(anyhow!(
            "Remote server provides protocol {dlapi_major}.{dlapi_minor}, \
             but {}.{} is required.",
            NEED_DLAPI_VERSION.0, NEED_DLAPI_VERSION.1
        ));
    }

    let expire = get_expiry_time(serve_time_limit);

    // Download updates if the remote has a newer version
    let latest_local = get_local_latest_version(&root).unwrap_or(base_version);
    if target_version > latest_local {
        for os in &oses {
            let from_ver = std::cmp::max(latest_local, base_version);
            println!("Fetching update list for {os} (from {})...", version_string(from_ver));
            let update_links = client.call_api(
                "api/v1/update",
                Some(serde_json::json!({
                    "version": version_string(from_ver),
                    "platform": remap_os(os)?,
                })),
            )?;
            let files = update_links
                .as_array()
                .ok_or_else(|| anyhow!("Expected JSON array from update API"))?;
            prepare_update(&root.join(os), &target_version_str, files, expire)?;
        }
        for os in &oses {
            continue_update(&client, &root.join(os))?;
        }
    }

    // Download all package types
    for os in &oses {
        for pkg_type in 0u8..7 {
            let os_int = remap_os(os)?;
            let pkg_root = root.join(os).join("package");
            let existing_info = pkg_root
                .join(&target_version_str)
                .join(pkg_type.to_string())
                .join("info.json");

            let exclude: Vec<i64> = if existing_info.is_file() {
                read_json_file(&existing_info).unwrap_or_default()
            } else {
                Vec::new()
            };

            let batch_links = client.call_api(
                "api/v1/batch",
                Some(serde_json::json!({
                    "package_type": pkg_type,
                    "platform": os_int,
                    "exclude": exclude,
                })),
            )?;
            let files = batch_links
                .as_array()
                .ok_or_else(|| anyhow!("Expected JSON array from batch API"))?;
            if !files.is_empty() {
                prepare_batch_download(&pkg_root, &target_version_str, pkg_type, files, expire)?;
            }
        }
    }

    for os in &oses {
        for pkg_type in 0u8..7 {
            continue_batch_download(&client, &root.join(os).join("package"), pkg_type)?;
        }
    }

    // Build the microdl file→archive map for each platform
    for os in &oses {
        let ver_dir = root.join(os).join("package").join(&target_version_str);
        if ver_dir.is_dir() {
            println!("Building microdl_map for {os}...");
            make_microdl_map(&ver_dir)?;
        }
    }

    // Update local version lists
    for os in &oses {
        let versionlist_path = root.join(os).join("package").join("info.json");
        let mut versionlist: Vec<String> = if versionlist_path.is_file() {
            read_json_file(&versionlist_path).unwrap_or_default()
        } else {
            Vec::new()
        };
        if !versionlist.contains(&target_version_str) {
            versionlist.push(target_version_str.clone());
            write_json_file(&versionlist_path, &versionlist)?;
        }
    }

    // Fetch and store release keys
    println!("Downloading release_info.json...");
    let release_keys = client.call_api("api/v1/release_info", None)?;
    write_json_file(&root.join("release_info.json"), &release_keys)?;

    println!("Done!");
    Ok(())
}
