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

use std::path::PathBuf;
use anyhow::{anyhow, Context};

const REQUIRE_GENERATION: (u32, u32) = (1, 1);

pub struct Config {
    pub main_public: bool,
    pub shared_key: Option<String>,
    pub archive_root: PathBuf,
    /// If set, used as the base URL for archive file links instead of
    /// deriving it from the incoming Host header.
    pub base_url: Option<String>,
    /// The nested [api.*] table from the TOML config, or empty table.
    pub api_publicness: toml::Value,
}

impl Config {
    pub fn load() -> anyhow::Result<Self> {
        let config_file = std::env::var("N4DLAPI_CONFIG_FILE")
            .unwrap_or_else(|_| "config.toml".to_string());

        let mut cfg = if std::path::Path::new(&config_file).is_file() {
            let raw = std::fs::read_to_string(&config_file)
                .with_context(|| format!("Failed to read config file: {config_file}"))?;
            let toml: toml::Value = raw.parse()
                .with_context(|| format!("Failed to parse TOML from: {config_file}"))?;
            Self::from_toml(toml)?
        } else {
            Self::defaults()
        };

        // Environment variable override for archive root
        if let Ok(env_root) = std::env::var("N4DLAPI_ARCHIVE_ROOT") {
            cfg.archive_root = PathBuf::from(env_root);
        }

        // Validate archive root
        if !cfg.archive_root.is_dir() {
            return Err(anyhow!(
                r#""{}" does not point to a valid directory"#,
                cfg.archive_root.display()
            ));
        }
        cfg.archive_root = cfg.archive_root.canonicalize()?;

        // Validate archive generation
        let gen_path = cfg.archive_root.join("generation.json");
        let gen: (u32, u32) = if gen_path.is_file() {
            let raw = std::fs::read_to_string(&gen_path)?;
            let v: serde_json::Value = serde_json::from_str(&raw)?;
            let major = v["major"].as_u64().unwrap_or(1) as u32;
            let minor = v["minor"].as_u64().unwrap_or(0) as u32;
            (major, minor)
        } else {
            (1, 0)
        };

        if gen < REQUIRE_GENERATION {
            return Err(anyhow!(
                "\"archive-root\" generation is too old ({}.{}). Forgot to run update script?",
                gen.0, gen.1
            ));
        }
        if gen.0 > REQUIRE_GENERATION.0 {
            return Err(anyhow!(
                "\"archive-root\" generation is too new ({}.{}).",
                gen.0, gen.1
            ));
        }

        Ok(cfg)
    }

    fn from_toml(toml: toml::Value) -> anyhow::Result<Self> {
        let main = toml.get("main")
            .ok_or_else(|| anyhow!("Missing [main] section in config"))?;

        let main_public = main.get("public")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);

        let shared_key_raw = main.get("shared_key")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let shared_key = if shared_key_raw.is_empty() {
            None
        } else {
            Some(shared_key_raw)
        };

        let archive_root_str = main.get("archive_root")
            .and_then(|v| v.as_str())
            .unwrap_or("archive-root")
            .to_string();
        let archive_root = PathBuf::from(archive_root_str);

        let base_url = main.get("base_url")
            .and_then(|v| v.as_str())
            .map(|s| s.trim_end_matches('/').to_string())
            .filter(|s| !s.is_empty());

        let api_publicness = toml.get("api")
            .cloned()
            .unwrap_or_else(|| toml::Value::Table(toml::Table::new()));

        Ok(Config {
            main_public,
            shared_key,
            archive_root,
            base_url,
            api_publicness,
        })
    }

    fn defaults() -> Self {
        Config {
            main_public: true,
            shared_key: None,
            archive_root: PathBuf::from("archive-root"),
            base_url: None,
            api_publicness: toml::Value::Table(toml::Table::new()),
        }
    }

    /// Check if an endpoint path (e.g. "/api/publicinfo") is publicly accessible
    /// based on the per-endpoint config overrides.
    pub fn is_endpoint_accessible(&self, endpoint: &str) -> bool {
        let stripped = endpoint.trim_start_matches('/');
        let parts: Vec<&str> = stripped.split('/').collect();

        if parts.first().copied() != Some("api") {
            return false;
        }

        let mut current = &self.api_publicness;
        for part in &parts[1..] {
            match current.get(part) {
                Some(v) => current = v,
                None => return self.main_public,
            }
        }

        match current.get("public") {
            Some(toml::Value::Boolean(b)) => *b,
            _ => self.main_public,
        }
    }

    /// Check if an endpoint is accessible given the provided shared key.
    pub fn is_accessible(&self, endpoint: &str, sk: Option<&str>) -> bool {
        if self.shared_key.is_none() {
            return true;
        }
        if self.is_endpoint_accessible(endpoint) {
            return true;
        }
        match (&self.shared_key, sk) {
            (Some(expected), Some(provided)) => expected == provided,
            _ => false,
        }
    }

    pub fn is_public_accessible(&self) -> bool {
        self.main_public
    }
}
