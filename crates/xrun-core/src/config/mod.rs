#![deny(unsafe_code)]

pub mod credentials;
pub mod global;

pub use credentials::Credentials;
pub use global::GlobalConfig;

use crate::error::ConfigError;
use std::path::Path;

impl GlobalConfig {
    pub fn load(config_dir: &Path) -> Result<Self, ConfigError> {
        let path = config_dir.join("config.toml");
        if !path.exists() {
            return Ok(Self::default());
        }
        let content = std::fs::read_to_string(&path)?;
        let config: Self = toml::from_str(&content)?;
        Ok(config)
    }

    pub fn save(&self, config_dir: &Path) -> Result<(), ConfigError> {
        std::fs::create_dir_all(config_dir)?;
        let path = config_dir.join("config.toml");
        let content = toml::to_string_pretty(self)?;
        std::fs::write(path, content)?;
        Ok(())
    }
}

impl Credentials {
    pub fn load(config_dir: &Path) -> Result<Self, ConfigError> {
        let path = config_dir.join("credentials.toml");
        if !path.exists() {
            return Ok(Self::default());
        }
        let content = std::fs::read_to_string(&path)?;
        let creds: Self = toml::from_str(&content)?;
        Ok(creds)
    }

    pub fn save(&self, config_dir: &Path) -> Result<(), ConfigError> {
        std::fs::create_dir_all(config_dir)?;
        let path = config_dir.join("credentials.toml");
        let content = toml::to_string_pretty(self)?;
        std::fs::write(&path, content)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::Permissions::from_mode(0o600);
            std::fs::set_permissions(&path, perms)?;
        }
        Ok(())
    }
}

pub struct ConfigStore;

pub struct InitResult {
    pub config_existed: bool,
    pub creds_existed: bool,
}

impl ConfigStore {
    pub fn init(config_dir: &Path) -> Result<InitResult, ConfigError> {
        std::fs::create_dir_all(config_dir)?;

        let config_path = config_dir.join("config.toml");
        let creds_path = config_dir.join("credentials.toml");

        let config_existed = config_path.exists();
        let creds_existed = creds_path.exists();

        if !config_existed {
            GlobalConfig::default().save(config_dir)?;
        }
        if !creds_existed {
            Credentials::default().save(config_dir)?;
        }

        Ok(InitResult {
            config_existed,
            creds_existed,
        })
    }
}
