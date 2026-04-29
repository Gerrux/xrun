#![deny(unsafe_code)]

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct VastCredentials {
    pub api_key: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct KaggleCredentials {
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
}

impl Credentials {
    pub fn is_empty(&self) -> bool {
        self.vast.api_key.is_none()
            && self.kaggle.username.is_none()
            && self.kaggle.key.is_none()
            && self.mlflow.token.is_none()
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

    pub fn kaggle_native_path() -> Option<PathBuf> {
        let base = directories::BaseDirs::new()?;
        Some(base.home_dir().join(".kaggle").join("kaggle.json"))
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
