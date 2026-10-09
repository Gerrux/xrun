#![deny(unsafe_code)]

use super::types::{
    Manifest, Vendor, COLAB_GPU_VALUES, ON_DONE_VALUES, ON_STAGE_FAILED_VALUES, PULL_ON_VALUES,
};
use crate::error::ManifestError;

pub fn validate(manifest: &Manifest) -> Result<(), ManifestError> {
    validate_name(&manifest.name)?;
    validate_vendor_sections(manifest)?;
    if let Some(v) = manifest.policy.as_ref().and_then(|p| p.on_done.as_deref()) {
        if !ON_DONE_VALUES.contains(&v) {
            return Err(ManifestError::Validation(format!(
                "policy.on_done must be one of {}: {:?}",
                ON_DONE_VALUES.join(" | "),
                v
            )));
        }
    }
    if let Some(v) = manifest
        .policy
        .as_ref()
        .and_then(|p| p.on_stage_failed.as_deref())
    {
        if !ON_STAGE_FAILED_VALUES.contains(&v) {
            return Err(ManifestError::Validation(format!(
                "policy.on_stage_failed must be one of {}: {:?}",
                ON_STAGE_FAILED_VALUES.join(" | "),
                v
            )));
        }
    }
    if let Some(v) = manifest
        .artifacts
        .as_ref()
        .and_then(|a| a.pull_on.as_deref())
    {
        if !PULL_ON_VALUES.contains(&v) {
            return Err(ManifestError::Validation(format!(
                "artifacts.pull_on must be one of {}: {:?}",
                PULL_ON_VALUES.join(" | "),
                v
            )));
        }
    }
    let dst_is_host_native = matches!(manifest.vendor, Vendor::Local);
    // Lightning uploads land relative to the Studio home (the SDK's
    // `upload_file(remote_path)` has no other root), so the dst is
    // home-relative there: `data/x.h5` or `~/data/x.h5`, never `/...`.
    let dst_is_home_relative = matches!(manifest.vendor, Vendor::Lightning);
    if let Some(data) = &manifest.data {
        for source in data {
            if dst_is_home_relative {
                if source.dst.starts_with('/') {
                    return Err(ManifestError::Validation(format!(
                        "vendor=lightning: data dst must be relative to the Studio home \
                         (e.g. `data/train.h5` or `~/data/train.h5`), got: {}",
                        source.dst
                    )));
                }
                continue;
            }
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

/// Reject every vendor section other than the one belonging to
/// `manifest.vendor`. Message style: "vendor=ssh must not have an [ssh] section".
fn reject_foreign_sections(manifest: &Manifest) -> Result<(), ManifestError> {
    let sections: [(Vendor, bool, &str); 6] = [
        (Vendor::Vast, manifest.vast.is_some(), "a [vast]"),
        (Vendor::Kaggle, manifest.kaggle.is_some(), "a [kaggle]"),
        (Vendor::Local, manifest.local.is_some(), "a [local]"),
        (Vendor::Ssh, manifest.ssh.is_some(), "an [ssh]"),
        (
            Vendor::Lightning,
            manifest.lightning.is_some(),
            "a [lightning]",
        ),
        (Vendor::Colab, manifest.colab.is_some(), "a [colab]"),
    ];
    for (vendor, present, label) in sections {
        if present && vendor != manifest.vendor {
            return Err(ManifestError::Validation(format!(
                "vendor={} must not have {} section",
                manifest.vendor.as_str(),
                label
            )));
        }
    }
    Ok(())
}

/// `^[a-z0-9][a-z0-9-]{0,39}$`
fn is_valid_studio_name(s: &str) -> bool {
    let len = s.chars().count();
    if len == 0 || len > 40 {
        return false;
    }
    s.chars().enumerate().all(|(i, c)| {
        if i == 0 {
            c.is_ascii_lowercase() || c.is_ascii_digit()
        } else {
            c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'
        }
    })
}

fn validate_vendor_sections(manifest: &Manifest) -> Result<(), ManifestError> {
    match manifest.vendor {
        Vendor::Vast => {
            let vast = manifest.vast.as_ref().ok_or_else(|| {
                ManifestError::Validation("vendor=vast requires a [vast] section".to_string())
            })?;
            reject_foreign_sections(manifest)?;
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
            reject_foreign_sections(manifest)?;
        }
        Vendor::Local => {
            reject_foreign_sections(manifest)?;
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
            reject_foreign_sections(manifest)?;
        }
        Vendor::Lightning => {
            reject_foreign_sections(manifest)?;
            // [lightning] section is optional — defaults are sane.
            if let Some(l) = &manifest.lightning {
                if let Some(w) = l.workdir.as_deref() {
                    if w.starts_with('/') {
                        return Err(ManifestError::Validation(format!(
                            "lightning.workdir must be relative to the studio home (no leading '/'): {w:?}"
                        )));
                    }
                }
                if let Some(t) = l
                    .teamspace
                    .as_deref()
                    .map(str::trim)
                    .filter(|t| !t.is_empty())
                {
                    let mut parts = t.split('/');
                    let ok = matches!(
                        (parts.next(), parts.next(), parts.next()),
                        (Some(o), Some(n), None) if !o.is_empty() && !n.is_empty()
                    );
                    if !ok {
                        return Err(ManifestError::Validation(format!(
                            "lightning.teamspace must be `owner/name`: {t:?}"
                        )));
                    }
                }
                if let Some(s) = l.studio.as_deref() {
                    if !is_valid_studio_name(&s.to_lowercase()) {
                        return Err(ManifestError::Validation(format!(
                            "lightning.studio must match ^[a-z0-9][a-z0-9-]{{0,39}}$: {s:?}"
                        )));
                    }
                }
            }
        }
        Vendor::Colab => {
            reject_foreign_sections(manifest)?;
            // A relative run.workdir would be anchored at `~/w` (/root/w) by
            // pull / the done-policy while the kernel's cwd is /content.
            if let Some(w) = manifest.run.workdir.as_deref() {
                if !w.trim().is_empty() && !w.trim().starts_with('/') {
                    return Err(ManifestError::Validation(
                        "vendor=colab: run.workdir must be absolute (e.g. /content/proj)"
                            .to_string(),
                    ));
                }
            }
            // [colab] section is optional — defaults are sane.
            if let Some(c) = &manifest.colab {
                if let Some(w) = c.workdir.as_deref() {
                    if !w.starts_with('/') {
                        return Err(ManifestError::Validation(format!(
                            "colab.workdir must be an absolute path (start with '/'): {w:?}"
                        )));
                    }
                }
                if let Some(g) = c.gpu.as_deref() {
                    if !COLAB_GPU_VALUES.contains(&g.to_lowercase().as_str()) {
                        return Err(ManifestError::Validation(format!(
                            "colab.gpu must be one of T4 | L4 | A100 | H100 | G4 | cpu: {g:?}"
                        )));
                    }
                }
            }
        }
    }
    Ok(())
}
