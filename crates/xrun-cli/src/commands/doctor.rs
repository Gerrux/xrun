#![deny(unsafe_code)]

use std::path::Path;

use anyhow::Result;
use xrun_core::Store;

use crate::cli::DoctorArgs;

struct Check {
    name: &'static str,
    ok: bool,
    detail: String,
}

pub fn run(args: &DoctorArgs, config_dir: &Path, db_path: Option<&Path>) -> Result<()> {
    let mut checks: Vec<Check> = Vec::new();

    let config_ok = dir_writable(config_dir);
    checks.push(Check {
        name: "config_dir",
        ok: config_ok,
        detail: config_dir.display().to_string(),
    });

    let vastai_ok = binary_available("vastai");
    checks.push(Check {
        name: "vastai_binary",
        ok: vastai_ok,
        detail: if vastai_ok { "found in PATH" } else { "not found in PATH" }.to_string(),
    });

    let kaggle_ok = binary_available("kaggle");
    checks.push(Check {
        name: "kaggle_binary",
        ok: kaggle_ok,
        detail: if kaggle_ok { "found in PATH" } else { "not found in PATH" }.to_string(),
    });

    let db_ok = match db_path {
        Some(p) => Store::open(p).is_ok(),
        None => false,
    };
    checks.push(Check {
        name: "db_access",
        ok: db_ok,
        detail: db_path
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "path unavailable".to_string()),
    });

    let any_fail = checks.iter().any(|c| !c.ok);

    if args.json {
        let out: Vec<_> = checks
            .iter()
            .map(|c| {
                serde_json::json!({
                    "check": c.name,
                    "status": if c.ok { "OK" } else { "FAIL" },
                    "detail": c.detail,
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&out).unwrap_or_default());
    } else {
        println!("{:<20}  {:<6}  detail", "check", "status");
        println!("{}", "-".repeat(60));
        for c in &checks {
            println!(
                "{:<20}  {:<6}  {}",
                c.name,
                if c.ok { "OK" } else { "FAIL" },
                c.detail
            );
        }
    }

    if any_fail {
        std::process::exit(1);
    }

    Ok(())
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
