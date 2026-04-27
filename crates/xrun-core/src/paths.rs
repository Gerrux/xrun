#![deny(unsafe_code)]

use directories::ProjectDirs;
use std::path::PathBuf;

use crate::error::ConfigError;

fn project_dirs() -> Result<ProjectDirs, ConfigError> {
    ProjectDirs::from("", "", "xrun")
        .ok_or_else(|| ConfigError::NoPaths("cannot determine home directory".to_string()))
}

pub fn config_dir() -> Result<PathBuf, ConfigError> {
    Ok(project_dirs()?.config_dir().to_path_buf())
}

pub fn data_dir() -> Result<PathBuf, ConfigError> {
    Ok(project_dirs()?.data_dir().to_path_buf())
}

pub fn runs_dir() -> Result<PathBuf, ConfigError> {
    Ok(data_dir()?.join("runs"))
}
