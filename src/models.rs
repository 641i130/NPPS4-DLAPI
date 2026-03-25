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

use std::collections::HashMap;
use serde::{Deserialize, Serialize};

// ── Response models ────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct VersionModel {
    pub major: u32,
    pub minor: u32,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PublicInfoModel {
    pub public_api: bool,
    pub dlapi_version: VersionModel,
    pub serve_time_limit: u32,
    pub game_version: String,
    pub application: HashMap<String, String>,
}

#[derive(Debug, Serialize, Clone)]
pub struct ChecksumModel {
    pub md5: String,
    pub sha256: String,
}

#[derive(Debug, Serialize, Clone)]
pub struct DownloadInfoModel {
    pub url: String,
    pub size: u64,
    pub checksums: ChecksumModel,
}

#[derive(Debug, Serialize, Clone)]
pub struct DownloadUpdateModel {
    pub url: String,
    pub size: u64,
    pub checksums: ChecksumModel,
    pub version: String,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct BatchDownloadInfoModel {
    pub url: String,
    pub size: u64,
    pub checksums: ChecksumModel,
    pub package_id: i64,
}

#[derive(Debug, Serialize)]
pub struct ErrorResponseModel {
    pub detail: String,
}

// ── Request models ─────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct UpdateRequest {
    pub version: String,
    pub platform: u8,
}

#[derive(Debug, Deserialize)]
pub struct BatchDownloadRequest {
    pub package_type: u8,
    pub platform: u8,
    #[serde(default)]
    pub exclude: Vec<i64>,
}

#[derive(Debug, Deserialize)]
pub struct DownloadRequest {
    pub package_type: u8,
    pub package_id: i64,
    pub platform: u8,
}

#[derive(Debug, Deserialize)]
pub struct MicroDownloadRequest {
    pub files: Vec<String>,
    pub platform: u8,
}

// ── Internal file metadata (read from infov2.json) ────────────────────────────

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct FileEntry {
    pub name: String,
    pub size: u64,
    pub md5: String,
    pub sha256: String,
}

// Empty MD5/SHA256 (checksums of empty data)
pub const MD5_EMPTY: &str = "d41d8cd98f00b204e9800998ecf8427e";
pub const SHA256_EMPTY: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
