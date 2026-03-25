// Port of update_v1.1.py — upgrades an archive-root from generation 1.0 to 1.1.
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
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
};

use anyhow::{anyhow, Context};
use clap::Args;
use serde::{Deserialize, Serialize};
use zip::ZipArchive;

use crate::models::FileEntry;
use crate::util::{hash_bytes, nat_cmp, parse_version, read_json_file, version_string, write_json_file};

const PLATFORMS: &[&str] = &["iOS", "Android"];
const GENERATION_VERSION: (u32, u32) = (1, 1);

// ── CLI args ───────────────────────────────────────────────────────────────────

#[derive(Debug, Args)]
pub struct UpgradeArgs {
    /// Path to the archive-root directory to upgrade to generation 1.1.
    pub archive_root: PathBuf,
}

// ── Generation file ────────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize)]
struct Generation {
    major: u32,
    minor: u32,
}

// ── Entry point ────────────────────────────────────────────────────────────────

pub fn run(args: UpgradeArgs) -> anyhow::Result<()> {
    let root = &args.archive_root;
    if !root.is_dir() {
        return Err(anyhow!("Not a directory: {}", root.display()));
    }

    let gen_path = root.join("generation.json");
    let current_gen: (u32, u32) = if gen_path.is_file() {
        let g: Generation = read_json_file(&gen_path)?;
        (g.major, g.minor)
    } else {
        (1, 0)
    };

    if current_gen == GENERATION_VERSION {
        println!("Archive is already up-to-date (generation {}.{}).", current_gen.0, current_gen.1);
        return Ok(());
    }
    if current_gen > GENERATION_VERSION {
        return Err(anyhow!(
            "Archive generation {}.{} is newer than this tool supports ({}.{}).",
            current_gen.0, current_gen.1,
            GENERATION_VERSION.0, GENERATION_VERSION.1
        ));
    }

    for platform in PLATFORMS {
        let plat_dir = root.join(platform);
        if !plat_dir.is_dir() {
            continue;
        }
        println!("===== OS: {platform} =====");
        println!("Building update version list...");
        build_new_update_info(root, platform)?;
        println!("Hashing update archives...");
        prehash_update(root, platform)?;
        println!("Hashing package archives...");
        prehash_packages(root, platform)?;
    }

    write_json_file(
        &gen_path,
        &Generation { major: GENERATION_VERSION.0, minor: GENERATION_VERSION.1 },
    )?;
    println!(
        "Done. Archive is now generation {}.{}.",
        GENERATION_VERSION.0, GENERATION_VERSION.1
    );
    Ok(())
}

// ── Update version list ────────────────────────────────────────────────────────

/// Scan the update directory for version subdirectories and write infov2.json
/// listing them in sorted order.
fn build_new_update_info(root: &Path, platform: &str) -> anyhow::Result<()> {
    let path = root.join(platform).join("update");
    let mut versions: Vec<(u32, u32)> = Vec::new();
    for entry in fs::read_dir(&path).with_context(|| format!("Cannot read {}", path.display()))? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            if let Some(ver) = parse_version(&entry.file_name().to_string_lossy()) {
                versions.push(ver);
            }
        }
    }
    versions.sort();
    let strs: Vec<String> = versions.iter().map(|v| version_string(*v)).collect();
    write_json_file(&path.join("infov2.json"), &strs)
}

/// Read a version list JSON file and return sorted `(major, minor)` tuples.
fn get_versions(path: &Path) -> anyhow::Result<Vec<(u32, u32)>> {
    let strs: Vec<String> = read_json_file(path)?;
    let mut versions: Vec<(u32, u32)> = strs.iter().filter_map(|s| parse_version(s)).collect();
    versions.sort();
    Ok(versions)
}

// ── Update hashing ─────────────────────────────────────────────────────────────

/// For each update version in infov2.json, read its info.json (filename→size),
/// nat-sort the files, hash each archive, and write infov2.json.
fn prehash_update(root: &Path, platform: &str) -> anyhow::Result<()> {
    let path = root.join(platform).join("update");
    let versions = get_versions(&path.join("infov2.json"))?;
    for ver in &versions {
        let ver_str = version_string(*ver);
        println!("  Hashing update {ver_str}");
        let ver_dir = path.join(&ver_str);
        let info: HashMap<String, u64> = read_json_file(&ver_dir.join("info.json"))?;
        let mut entries: Vec<(String, u64)> = info.into_iter().collect();
        entries.sort_by(|(a, _), (b, _)| nat_cmp(a, b));

        let mut infov2: Vec<FileEntry> = Vec::new();
        for (name, size) in &entries {
            let data = fs::read(ver_dir.join(name))
                .with_context(|| format!("Cannot read {name} in {}", ver_dir.display()))?;
            let (md5, sha256) = hash_bytes(&data);
            infov2.push(FileEntry { name: name.clone(), size: *size, md5, sha256 });
        }
        write_json_file(&ver_dir.join("infov2.json"), &infov2)?;
    }
    Ok(())
}

// ── DB extraction from update archives ────────────────────────────────────────

/// Extract all `db/*.db_` files from update archives for versions up to
/// (and including) `up_to_version`.
fn get_db_from_update(
    root: &Path,
    platform: &str,
    up_to_version: (u32, u32),
) -> anyhow::Result<HashMap<String, Vec<u8>>> {
    let path = root.join(platform).join("update");
    let versions = get_versions(&path.join("infov2.json"))?;
    let mut dbfiles: HashMap<String, Vec<u8>> = HashMap::new();

    for ver in versions.iter().filter(|&&v| v <= up_to_version) {
        let ver_str = version_string(*ver);
        println!("  Scanning update {ver_str} for db files");
        let ver_dir = path.join(&ver_str);
        let infov2: Vec<FileEntry> = read_json_file(&ver_dir.join("infov2.json"))?;
        for entry in &infov2 {
            let data = fs::read(ver_dir.join(&entry.name))
                .with_context(|| format!("Cannot read update archive {}", entry.name))?;
            let mut zip = ZipArchive::new(std::io::Cursor::new(data))
                .with_context(|| format!("Bad ZIP: {}", entry.name))?;
            for i in 0..zip.len() {
                let mut zf = zip.by_index(i)?;
                let name = zf.name().to_string();
                if name.starts_with("db/") && name.ends_with(".db_") {
                    let basename = Path::new(&name)
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    println!("    Found db: {basename}");
                    let mut buf = Vec::new();
                    zf.read_to_end(&mut buf)?;
                    dbfiles.insert(basename, buf);
                }
            }
        }
    }
    Ok(dbfiles)
}

// ── Package hashing and microdl extraction ────────────────────────────────────

/// Hash all archives for a single package type/ID set.
///
/// - If `dbfiles` is `Some`, collect `db/*.db_` entries into it (used for type 0).
/// - If `extract_to` is `Some`, extract all zip contents into that directory
///   and write `info.json` (used for type 4 / microdl).
fn prehash_package_type(
    root: &Path,
    platform: &str,
    version: (u32, u32),
    pkgtype: u8,
    mut dbfiles: Option<&mut HashMap<String, Vec<u8>>>,
    extract_to: Option<&Path>,
) -> anyhow::Result<()> {
    let ver_str = version_string(version);
    let type_dir = root
        .join(platform)
        .join("package")
        .join(&ver_str)
        .join(pkgtype.to_string());

    if !type_dir.is_dir() {
        return Ok(());
    }

    let ids: Vec<i64> = read_json_file(&type_dir.join("info.json"))?;
    let mut microdl: HashMap<String, serde_json::Value> = HashMap::new();

    for &id in &ids {
        println!("  Hashing package {pkgtype}/{id}");
        let id_dir = type_dir.join(id.to_string());
        let info: HashMap<String, u64> = read_json_file(&id_dir.join("info.json"))?;
        let mut entries: Vec<(String, u64)> = info.into_iter().collect();
        entries.sort_by(|(a, _), (b, _)| nat_cmp(a, b));

        let mut infov2: Vec<FileEntry> = Vec::new();
        for (name, size) in &entries {
            let data = fs::read(id_dir.join(name))
                .with_context(|| format!("Cannot read package archive {name}"))?;

            // Collect db files from type-0 packages
            if let Some(ref mut db) = dbfiles {
                let mut zip = ZipArchive::new(std::io::Cursor::new(&data))
                    .with_context(|| format!("Bad ZIP: {name}"))?;
                for i in 0..zip.len() {
                    let mut zf = zip.by_index(i)?;
                    let zname = zf.name().to_string();
                    if zname.starts_with("db/") && zname.ends_with(".db_") {
                        let basename = Path::new(&zname)
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_default();
                        println!("    Collecting db: {basename}");
                        let mut buf = Vec::new();
                        zf.read_to_end(&mut buf)?;
                        db.insert(basename, buf);
                    }
                }
            }

            let (md5, sha256) = hash_bytes(&data);
            infov2.push(FileEntry { name: name.clone(), size: *size, md5, sha256 });
        }
        write_json_file(&id_dir.join("infov2.json"), &infov2)?;
    }

    // Extract microdl from type-4 packages.
    // Process IDs in reverse (lower IDs are later/override earlier), and archives
    // within each ID in reverse order (later archives have newer data).
    // First entry wins (skips duplicates), matching the Python logic.
    if let Some(dest) = extract_to {
        fs::create_dir_all(dest)?;
        for &id in ids.iter().rev() {
            println!("  Extracting microdl from package {pkgtype}/{id}");
            let id_dir = type_dir.join(id.to_string());
            let infov2: Vec<FileEntry> = read_json_file(&id_dir.join("infov2.json"))?;
            for entry in infov2.iter().rev() {
                let data = fs::read(id_dir.join(&entry.name))?;
                let mut zip = ZipArchive::new(std::io::Cursor::new(data))?;
                for i in 0..zip.len() {
                    let mut zf = zip.by_index(i)?;
                    let filename = zf.name().to_string();
                    if microdl.contains_key(&filename) {
                        continue; // Already have a newer version
                    }
                    let mut buf = Vec::new();
                    zf.read_to_end(&mut buf)?;
                    let (md5, sha256) = hash_bytes(&buf);
                    println!("    Extracting: {filename}");
                    microdl.insert(
                        filename.clone(),
                        serde_json::json!({ "size": buf.len() as u64, "md5": md5, "sha256": sha256 }),
                    );
                    let out_path = dest.join(&filename);
                    if let Some(parent) = out_path.parent() {
                        fs::create_dir_all(parent)?;
                    }
                    fs::write(&out_path, &buf)?;
                }
            }
        }
        println!("  Writing microdl/info.json");
        write_json_file(&dest.join("info.json"), &microdl)?;
    }

    Ok(())
}

/// Hash all package types for every package version, extract microdl and
/// decrypt db files.
fn prehash_packages(root: &Path, platform: &str) -> anyhow::Result<()> {
    let pkg_root = root.join(platform).join("package");
    let versions = get_versions(&pkg_root.join("info.json"))?;

    for ver in &versions {
        let ver_str = version_string(*ver);
        println!("Prehashing packages for version {ver_str}");

        // Collect encrypted db files from update archives up to this version
        let mut dbfiles = get_db_from_update(root, platform, *ver)?;

        let microdl_dest = pkg_root.join(&ver_str).join("microdl");

        for pkgtype in 0u8..7 {
            let db_opt = if pkgtype == 0 { Some(&mut dbfiles) } else { None };
            let microdl_opt = if pkgtype == 4 { Some(microdl_dest.as_path()) } else { None };
            prehash_package_type(root, platform, *ver, pkgtype, db_opt, microdl_opt)?;
        }

        // Decrypt and write db files
        let db_dest = pkg_root.join(&ver_str).join("db");
        fs::create_dir_all(&db_dest)?;
        for (name, encrypted) in &dbfiles {
            println!("Decrypting db: {name}");
            match decrypt_db(name, encrypted) {
                Ok(decrypted) => fs::write(db_dest.join(name), &decrypted)
                    .with_context(|| format!("Cannot write decrypted {name}"))?,
                Err(e) => eprintln!("Warning: could not decrypt {name}: {e}"),
            }
        }
    }
    Ok(())
}

// ── Database decryption ────────────────────────────────────────────────────────

/// Decrypt a SIF database file.
/// Tries `honoka2 -b {basename} - -` (stdin→stdout) first, then falls back to
/// `python -m honkypy` / `python3 -m honkypy` using a temp directory.
fn decrypt_db(basename: &str, encrypted: &[u8]) -> anyhow::Result<Vec<u8>> {
    if let Ok(result) = try_honoka2(basename, encrypted) {
        return Ok(result);
    }
    decrypt_via_honkypy(basename, encrypted)
}

fn try_honoka2(basename: &str, data: &[u8]) -> anyhow::Result<Vec<u8>> {
    let mut child = Command::new("honoka2")
        .args(["-b", basename, "-", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .context("honoka2 not found")?;

    // Write stdin in a separate thread to avoid deadlock with stdout.
    let data_owned = data.to_vec();
    let mut stdin = child.stdin.take().unwrap();
    let writer = std::thread::spawn(move || stdin.write_all(&data_owned));

    let out = child.wait_with_output().context("honoka2 wait failed")?;
    writer.join().map_err(|_| anyhow!("stdin writer thread panicked"))??;

    if !out.status.success() {
        return Err(anyhow!("honoka2 exited with status {}", out.status));
    }
    Ok(out.stdout)
}

fn decrypt_via_honkypy(basename: &str, data: &[u8]) -> anyhow::Result<Vec<u8>> {
    use tempfile::tempdir;

    // honkypy derives the decryption key from the filename, so the temp input
    // file must have the correct basename.
    let dir = tempdir()?;
    let in_path = dir.path().join(basename);
    let out_path = dir.path().join(format!("{basename}.out"));
    fs::write(&in_path, data)?;

    let args = [
        "-m",
        "honkypy",
        in_path.to_str().unwrap(),
        out_path.to_str().unwrap(),
    ];

    let status = Command::new("python")
        .args(args)
        .status()
        .or_else(|_| Command::new("python3").args(args).status())
        .context("python -m honkypy not found; install honkypy or provide honoka2 binary")?;

    if !status.success() {
        return Err(anyhow!("honkypy decryption failed for {basename}"));
    }
    Ok(fs::read(&out_path)?)
}
