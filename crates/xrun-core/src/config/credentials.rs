#![deny(unsafe_code)]

use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct VastCredentials {
    pub api_key: Option<String>,
}

/// One named SSH host. `host_alias` (the map key in `[vendors.ssh.<alias>]`)
/// is what the manifest references via `ssh.host_alias`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct SshHostCredentials {
    pub host: Option<String>,
    pub user: Option<String>,
    pub port: Option<u16>,
    /// Path to the SSH key (`~/.ssh/id_ed25519`-style; tilde expanded at use
    /// time). When unset, ssh falls back to the system default key.
    pub key: Option<String>,
    /// Optional default workdir for runs from this host. Override per-manifest
    /// via `ssh.workdir`.
    pub default_workdir: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct KaggleCredentials {
    /// New-style Bearer token (kaggle CLI ≥ 1.8.0 / kagglehub ≥ 0.4.1)
    pub token: Option<String>,
    /// Legacy username+key from kaggle.json
    pub username: Option<String>,
    pub key: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct MlflowCredentials {
    pub token: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct Credentials {
    pub vast: VastCredentials,
    pub kaggle: KaggleCredentials,
    pub mlflow: MlflowCredentials,
    /// SSH hosts keyed by alias. Manifests reference these via
    /// `ssh.host_alias`. Loaded from `[vendors.ssh.<alias>]` sections.
    #[serde(rename = "ssh", default)]
    pub ssh_hosts: HashMap<String, SshHostCredentials>,
}

impl Credentials {
    pub fn is_empty(&self) -> bool {
        self.vast.api_key.is_none()
            && self.kaggle.token.is_none()
            && self.kaggle.username.is_none()
            && self.kaggle.key.is_none()
            && self.mlflow.token.is_none()
            && self.ssh_hosts.is_empty()
    }

    pub fn vast_native_path() -> Option<PathBuf> {
        let base = directories::BaseDirs::new()?;
        Some(
            base.home_dir()
                .join(".config")
                .join("vastai")
                .join("vast_api_key"),
        )
    }

    /// Path to new-style token file (`~/.kaggle/access_token`).
    pub fn kaggle_access_token_path() -> Option<PathBuf> {
        let base = directories::BaseDirs::new()?;
        Some(base.home_dir().join(".kaggle").join("access_token"))
    }

    /// Path to legacy credentials file (`~/.kaggle/kaggle.json`).
    pub fn kaggle_native_path() -> Option<PathBuf> {
        let base = directories::BaseDirs::new()?;
        Some(base.home_dir().join(".kaggle").join("kaggle.json"))
    }

    /// Try to read `~/.kaggle/access_token`. Returns `Ok(Some(token))` if present.
    pub fn import_kaggle_access_token() -> Result<Option<String>, std::io::Error> {
        // Also honour KAGGLE_API_TOKEN env var (same precedence as the file).
        if let Ok(tok) = std::env::var("KAGGLE_API_TOKEN") {
            let tok = tok.trim().to_string();
            if !tok.is_empty() {
                return Ok(Some(tok));
            }
        }
        let Some(path) = Self::kaggle_access_token_path() else {
            return Ok(None);
        };
        if !path.exists() {
            return Ok(None);
        }
        let raw = std::fs::read_to_string(&path)?;
        let token = raw.trim().to_string();
        if token.is_empty() {
            return Ok(None);
        }
        Ok(Some(token))
    }

    /// Try to read `vastai`'s native key file. Returns `Ok(Some)` if the file
    /// exists and contains a non-empty token; `Ok(None)` if the file is absent.
    pub fn import_vast_native() -> Result<Option<String>, std::io::Error> {
        let Some(path) = Self::vast_native_path() else {
            return Ok(None);
        };
        if !path.exists() {
            return Ok(None);
        }
        let raw = std::fs::read_to_string(&path)?;
        let token = raw.trim().to_string();
        if token.is_empty() {
            return Ok(None);
        }
        Ok(Some(token))
    }

    /// Try to read `kaggle.json`. Returns `Ok(Some((username, key)))` on success.
    pub fn import_kaggle_native() -> Result<Option<(String, String)>, std::io::Error> {
        let Some(path) = Self::kaggle_native_path() else {
            return Ok(None);
        };
        if !path.exists() {
            return Ok(None);
        }
        let raw = std::fs::read_to_string(&path)?;
        let parsed: serde_json::Value = serde_json::from_str(&raw)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;
        let username = parsed
            .get("username")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let key = parsed
            .get("key")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        match (username, key) {
            (Some(u), Some(k)) if !u.is_empty() && !k.is_empty() => Ok(Some((u, k))),
            _ => Ok(None),
        }
    }
}
