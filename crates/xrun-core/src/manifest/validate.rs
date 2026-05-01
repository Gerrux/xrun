#![deny(unsafe_code)]

use super::types::{Manifest, Vendor};
use crate::error::ManifestError;

pub fn validate(manifest: &Manifest) -> Result<(), ManifestError> {
    validate_name(&manifest.name)?;
    validate_vendor_sections(manifest)?;
    let dst_is_host_native = matches!(manifest.vendor, Vendor::Local);
    if let Some(data) = &manifest.data {
        for source in data {
            // Local accepts host-native paths (Windows `C:\...`, Unix `/...`,
            // or relative). Cloud + ssh vendors place files into a Linux
            // container/box, so the dst is required to be absolute.
            if !dst_is_host_native && !source.dst.starts_with('/') {
                return Err(ManifestError::Validation(format!(
                    "data dst must start with '/': {}",
                    source.dst
                )));
            }
        }
    }
    if let Some(args) = &manifest.run.args {
        for key in args.keys() {
            if key.contains(' ') {
                return Err(ManifestError::Validation(format!(
                    "args key must not contain spaces: {:?}",
                    key
                )));
            }
        }
    }
    Ok(())
}

fn validate_name(name: &str) -> Result<(), ManifestError> {
    if name.is_empty() {
        return Err(ManifestError::Validation(
            "name must not be empty".to_string(),
        ));
    }
    let valid = name.chars().enumerate().all(|(i, c)| {
        if i == 0 {
            c.is_ascii_lowercase() || c.is_ascii_digit()
        } else {
            c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-'
        }
    });
    if !valid {
        return Err(ManifestError::Validation(format!(
            "name must match ^[a-z0-9][a-z0-9_-]*$: {:?}",
            name
        )));
    }
    Ok(())
}

fn validate_vendor_sections(manifest: &Manifest) -> Result<(), ManifestError> {
    match manifest.vendor {
        Vendor::Vast => {
            let vast = manifest.vast.as_ref().ok_or_else(|| {
                ManifestError::Validation("vendor=vast requires a [vast] section".to_string())
            })?;
            if manifest.kaggle.is_some() {
                return Err(ManifestError::Validation(
                    "vendor=vast must not have a [kaggle] section".to_string(),
                ));
            }
            if manifest.local.is_some() {
                return Err(ManifestError::Validation(
                    "vendor=vast must not have a [local] section".to_string(),
                ));
            }
            if manifest.ssh.is_some() {
                return Err(ManifestError::Validation(
                    "vendor=vast must not have an [ssh] section".to_string(),
                ));
            }
            if vast.gpu.count < 1 {
                return Err(ManifestError::Validation(
                    "vast.gpu.count must be >= 1".to_string(),
                ));
            }
        }
        Vendor::Kaggle => {
            if manifest.kaggle.is_none() {
                return Err(ManifestError::Validation(
                    "vendor=kaggle requires a [kaggle] section".to_string(),
                ));
            }
            if manifest.vast.is_some() {
                return Err(ManifestError::Validation(
                    "vendor=kaggle must not have a [vast] section".to_string(),
                ));
            }
            if manifest.local.is_some() {
                return Err(ManifestError::Validation(
                    "vendor=kaggle must not have a [local] section".to_string(),
                ));
            }
            if manifest.ssh.is_some() {
                return Err(ManifestError::Validation(
                    "vendor=kaggle must not have an [ssh] section".to_string(),
                ));
            }
        }
        Vendor::Local => {
            if manifest.vast.is_some() {
                return Err(ManifestError::Validation(
                    "vendor=local must not have a [vast] section".to_string(),
                ));
            }
            if manifest.kaggle.is_some() {
                return Err(ManifestError::Validation(
                    "vendor=local must not have a [kaggle] section".to_string(),
                ));
            }
            if manifest.ssh.is_some() {
                return Err(ManifestError::Validation(
                    "vendor=local must not have an [ssh] section".to_string(),
                ));
            }
            // [local] section is optional — defaults are sane.
        }
        Vendor::Ssh => {
            let ssh = manifest.ssh.as_ref().ok_or_else(|| {
                ManifestError::Validation("vendor=ssh requires an [ssh] section".to_string())
            })?;
            if ssh.host_alias.is_empty() {
                return Err(ManifestError::Validation(
                    "ssh.host_alias must not be empty".to_string(),
                ));
            }
            if manifest.vast.is_some() {
                return Err(ManifestError::Validation(
                    "vendor=ssh must not have a [vast] section".to_string(),
                ));
            }
            if manifest.kaggle.is_some() {
                return Err(ManifestError::Validation(
                    "vendor=ssh must not have a [kaggle] section".to_string(),
                ));
            }
            if manifest.local.is_some() {
                return Err(ManifestError::Validation(
                    "vendor=ssh must not have a [local] section".to_string(),
                ));
            }
        }
    }
    Ok(())
}
