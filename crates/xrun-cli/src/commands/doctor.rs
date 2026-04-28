#![deny(unsafe_code)]

use std::path::Path;

use anyhow::Result;
use xrun_core::Store;

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
                format!("vastai show user failed: {}", stderr.trim())
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
