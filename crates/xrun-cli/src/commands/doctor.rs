#![deny(unsafe_code)]

use std::path::Path;

use anyhow::Result;
use xrun_core::{manifest::Manifest, Store};
use xrun_kaggle::KaggleAdapter;

use crate::cli::DoctorArgs;

struct Check {
    name: &'static str,
    ok: bool,
    /// If true the check is advisory only — shown as WARN, does not cause exit 1.
    warn_only: bool,
    detail: String,
}

pub fn run(args: &DoctorArgs, config_dir: &Path, db_path: Option<&Path>) -> Result<()> {
    let mut checks: Vec<Check> = Vec::new();

    // --- required checks ---

    let config_ok = dir_writable(config_dir);
    checks.push(Check {
        name: "config_dir",
        ok: config_ok,
        warn_only: false,
        detail: config_dir.display().to_string(),
    });

    let vastai_ok = binary_available("vastai");
    checks.push(Check {
        name: "vastai_binary",
        ok: vastai_ok,
        warn_only: false,
        detail: if vastai_ok {
            "found in PATH".to_string()
        } else {
            "not found in PATH".to_string()
        },
    });

    let kaggle_ok = binary_available("kaggle");
    checks.push(Check {
        name: "kaggle_binary",
        ok: kaggle_ok,
        warn_only: false,
        detail: if kaggle_ok {
            "found in PATH".to_string()
        } else {
            "not found in PATH".to_string()
        },
    });

    let db_ok = match db_path {
        Some(p) => Store::open(p).is_ok(),
        None => false,
    };
    checks.push(Check {
        name: "db_access",
        ok: db_ok,
        warn_only: false,
        detail: db_path
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "path unavailable".to_string()),
    });

    // --- advisory / warn-only checks ---

    // vastai SSH key: only check when the binary exists
    if vastai_ok {
        let (ssh_ok, ssh_detail) = check_vastai_ssh_key();
        checks.push(Check {
            name: "vastai_ssh_key",
            ok: ssh_ok,
            warn_only: true,
            detail: ssh_detail,
        });
    }

    // rsync: needed for manifest data sources with mode: rsync
    let rsync_ok = binary_available("rsync");
    checks.push(Check {
        name: "rsync_binary",
        ok: rsync_ok,
        warn_only: true,
        detail: if rsync_ok {
            "found in PATH".to_string()
        } else {
            "not found in PATH (only needed for data.mode: rsync)".to_string()
        },
    });

    // Manifest validation (--manifest path), one check per file. Fatal on
    // failure since the user explicitly asked us to validate.
    for path in &args.manifests {
        match std::fs::read_to_string(path) {
            Ok(yaml) => match Manifest::from_yaml_str(&yaml) {
                Ok(m) => {
                    checks.push(Check {
                        name: "manifest",
                        ok: true,
                        warn_only: false,
                        detail: format!("{} (vendor={:?})", path.display(), m.vendor),
                    });
                    // For Kaggle manifests, run additional checks.
                    if matches!(m.vendor, xrun_core::manifest::Vendor::Kaggle) {
                        kaggle_manifest_checks(&m, config_dir, &mut checks);
                    }
                }
                Err(e) => checks.push(Check {
                    name: "manifest",
                    ok: false,
                    warn_only: false,
                    detail: format!("{}: {e}", path.display()),
                }),
            },
            Err(e) => checks.push(Check {
                name: "manifest",
                ok: false,
                warn_only: false,
                detail: format!("{}: read failed: {e}", path.display()),
            }),
        }
    }

    // python3 xrun_hook: installed on training instance; optional locally
    let hook_ok = check_python_hook();
    checks.push(Check {
        name: "python_xrun_hook",
        ok: hook_ok,
        warn_only: true,
        detail: if hook_ok {
            "importable".to_string()
        } else {
            "not importable (installed on instance automatically; optional locally)".to_string()
        },
    });

    let any_fail = checks.iter().any(|c| !c.ok && !c.warn_only);

    if args.json {
        let out: Vec<_> = checks
            .iter()
            .map(|c| {
                serde_json::json!({
                    "check": c.name,
                    "status": check_status_str(c),
                    "detail": c.detail,
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&out).unwrap_or_default());
    } else {
        println!("{:<24}  {:<6}  detail", "check", "status");
        println!("{}", "-".repeat(70));
        for c in &checks {
            println!("{:<24}  {:<6}  {}", c.name, check_status_str(c), c.detail);
        }
    }

    if any_fail {
        std::process::exit(1);
    }

    Ok(())
}

fn check_status_str(c: &Check) -> &'static str {
    if c.ok {
        "OK"
    } else if c.warn_only {
        "WARN"
    } else {
        "FAIL"
    }
}

/// Run `vastai show user --raw` and check for a non-empty ssh_key field.
fn check_vastai_ssh_key() -> (bool, String) {
    let result = std::process::Command::new("vastai")
        .args(["show", "user", "--raw"])
        .output();

    match result {
        Ok(out) if out.status.success() => {
            let text = String::from_utf8_lossy(&out.stdout);
            match serde_json::from_str::<serde_json::Value>(text.trim()) {
                Ok(v) => {
                    let has_key = v
                        .get("ssh_key")
                        .and_then(|k| k.as_str())
                        .map(|s| !s.is_empty())
                        .unwrap_or(false);
                    if has_key {
                        (true, "SSH key registered on account".to_string())
                    } else {
                        (
                            false,
                            "no SSH key — add one at https://cloud.vast.ai/account/".to_string(),
                        )
                    }
                }
                Err(_) => (false, "could not parse vastai show user output".to_string()),
            }
        }
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            let detail = if stderr.contains("unauthorized") || stderr.contains("api_key") {
                "not authenticated — run `vastai set api-key <KEY>`".to_string()
            } else {
                let first_line = stderr.lines().next().unwrap_or("unknown error");
                let truncated: String = first_line.chars().take(120).collect();
                format!("vastai show user failed: {}", truncated)
            };
            (false, detail)
        }
        Err(e) => (false, format!("could not run vastai: {e}")),
    }
}

/// Check if `python3 -c "import xrun_hook"` succeeds.
fn check_python_hook() -> bool {
    std::process::Command::new("python3")
        .args(["-c", "import xrun_hook"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn dir_writable(path: &Path) -> bool {
    if std::fs::create_dir_all(path).is_err() {
        return false;
    }
    let probe = path.join(".xrun_doctor_probe");
    let ok = std::fs::write(&probe, b"").is_ok();
    let _ = std::fs::remove_file(&probe);
    ok
}

/// Kaggle-specific checks for manifests with `vendor: kaggle`.
fn kaggle_manifest_checks(manifest: &Manifest, config_dir: &Path, checks: &mut Vec<Check>) {
    let kaggle_spec = match &manifest.kaggle {
        Some(s) => s,
        None => {
            checks.push(Check {
                name: "kaggle_spec",
                ok: false,
                warn_only: false,
                detail: "vendor is kaggle but no [kaggle] section found in manifest".to_string(),
            });
            return;
        }
    };

    // Check kernel_slug consistency with manifest.name.
    // Kaggle slugifies names by lowercasing and replacing spaces/special chars with hyphens.
    let expected_slug_suffix = manifest
        .name
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '-' { c } else { '-' })
        .collect::<String>();
    let actual_suffix = kaggle_spec
        .kernel_slug
        .split('/')
        .last()
        .unwrap_or(&kaggle_spec.kernel_slug);
    let slug_ok = actual_suffix == expected_slug_suffix;
    checks.push(Check {
        name: "kaggle_kernel_slug",
        ok: slug_ok,
        warn_only: true,
        detail: if slug_ok {
            format!("kernel_slug '{}' matches manifest name", kaggle_spec.kernel_slug)
        } else {
            format!(
                "kernel_slug suffix '{}' differs from slugified manifest name '{}' — \
                 Kaggle may create the kernel at the wrong slug",
                actual_suffix, expected_slug_suffix
            )
        },
    });

    // Build adapter to access KaggleCli.
    let creds = resolve_kaggle_credentials(config_dir);
    let adapter = KaggleAdapter::new().with_credentials(creds);
    let cli = adapter.cli();

    // Check Kaggle credentials via `kaggle config view`.
    let creds_ok = match cli.username() {
        Ok(u) => {
            checks.push(Check {
                name: "kaggle_credentials",
                ok: true,
                warn_only: false,
                detail: format!("authenticated as {u}"),
            });
            true
        }
        Err(e) => {
            checks.push(Check {
                name: "kaggle_credentials",
                ok: false,
                warn_only: false,
                detail: format!("could not authenticate: {e}"),
            });
            false
        }
    };

    // Check each dataset slug for readiness (skip if not authenticated).
    if creds_ok {
        let mut all_slugs: Vec<&str> = Vec::new();
        if let Some(s) = &kaggle_spec.dataset {
            all_slugs.push(s.as_str());
        }
        for s in &kaggle_spec.datasets {
            all_slugs.push(s.as_str());
        }
        for slug in all_slugs {
            let (ok, warn_only, detail) = match cli.is_dataset_ready(slug) {
                Ok(true) => (true, false, format!("dataset '{slug}' is ready")),
                Ok(false) => (
                    false,
                    true,
                    format!(
                        "dataset '{slug}' exists but is not ready yet — \
                         training will fail until it finishes processing"
                    ),
                ),
                Err(e) => (
                    false,
                    false,
                    format!("dataset '{slug}' check failed: {e}"),
                ),
            };
            checks.push(Check {
                name: "kaggle_dataset",
                ok,
                warn_only,
                detail,
            });
        }
    }
}

fn resolve_kaggle_credentials(
    config_dir: &Path,
) -> xrun_core::config::credentials::KaggleCredentials {
    use xrun_core::Credentials;
    if let Ok(creds) = Credentials::load(config_dir) {
        if creds.kaggle.token.is_some()
            || (creds.kaggle.username.is_some() && creds.kaggle.key.is_some())
        {
            return creds.kaggle;
        }
    }
    if let Ok(Some((username, key))) = Credentials::import_kaggle_native() {
        return xrun_core::config::credentials::KaggleCredentials {
            token: None,
            username: Some(username),
            key: Some(key),
        };
    }
    if let Ok(Some(token)) = Credentials::import_kaggle_access_token() {
        return xrun_core::config::credentials::KaggleCredentials {
            token: Some(token),
            username: None,
            key: None,
        };
    }
    xrun_core::config::credentials::KaggleCredentials::default()
}

fn binary_available(name: &str) -> bool {
    let Some(paths) = std::env::var_os("PATH") else {
        return false;
    };
    for dir in std::env::split_paths(&paths) {
        if dir.join(name).is_file() {
            return true;
        }
        #[cfg(windows)]
        {
            if dir.join(format!("{name}.exe")).is_file()
                || dir.join(format!("{name}.cmd")).is_file()
                || dir.join(format!("{name}.bat")).is_file()
            {
                return true;
            }
        }
    }
    false
}
