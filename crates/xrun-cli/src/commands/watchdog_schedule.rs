#![deny(unsafe_code)]

//! `xrun watchdog schedule` — register / remove / inspect the OS scheduler
//! entry that runs `xrun watchdog` every few minutes.
//!
//! Backends: `schtasks` on Windows, the user crontab elsewhere (macOS
//! included — launchd would be nicer but cron is universally present and
//! trivially inspectable). The entry always points at the *current* binary
//! by absolute path so it works when `xrun` is not on the scheduler's PATH.
//!
//! Nothing here runs unless the user asks for `--install` / `--remove`;
//! the default is a read-only `--status`.

use std::path::PathBuf;
use std::process::{Command, Stdio};

use anyhow::{bail, Context, Result};
use clap::Args;
use serde::Serialize;

pub const TASK_NAME: &str = "xrun-watchdog";
#[cfg(not(windows))]
const CRON_MARKER: &str = "# xrun-watchdog";

#[derive(Args)]
pub struct ScheduleArgs {
    /// Register the scheduler entry (idempotent: replaces an existing one).
    #[arg(long, conflicts_with = "remove")]
    pub install: bool,
    /// Remove the scheduler entry.
    #[arg(long, conflicts_with = "install")]
    pub remove: bool,
    /// Interval in minutes (with --install).
    #[arg(long, default_value = "5", value_name = "MIN")]
    pub every_min: u32,
    /// Output as JSON
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Serialize)]
pub struct Status {
    pub installed: bool,
    pub backend: &'static str,
    /// Human-readable description of the entry (or of what would be created).
    pub entry: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub every_min: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action: Option<&'static str>,
}

fn exe() -> Result<PathBuf> {
    std::env::current_exe().context("cannot resolve the xrun binary path")
}

fn quiet(mut cmd: Command) -> Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd.stdin(Stdio::null());
    cmd
}

pub fn run(args: &ScheduleArgs) -> Result<()> {
    if args.every_min == 0 {
        bail!("--every-min must be at least 1");
    }
    let status = if args.install {
        install(args.every_min)?
    } else if args.remove {
        remove()?
    } else {
        status()?
    };
    if args.json {
        println!("{}", serde_json::to_string_pretty(&status)?);
        return Ok(());
    }
    let verb = match status.action {
        Some("installed") => "installed",
        Some("removed") => "removed",
        _ => {
            if status.installed {
                "installed"
            } else {
                "not installed"
            }
        }
    };
    println!("watchdog schedule: {verb} ({})", status.backend);
    println!("  {}", status.entry);
    if !status.installed && status.action.is_none() {
        println!("  register with: xrun watchdog schedule --install [--every-min 5]");
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Windows: schtasks
// ---------------------------------------------------------------------------

#[cfg(windows)]
fn status() -> Result<Status> {
    let out = quiet(Command::new("schtasks"))
        .args(["/Query", "/TN", TASK_NAME, "/FO", "LIST", "/V"])
        .output()
        .context("schtasks not available")?;
    let installed = out.status.success();
    let text = String::from_utf8_lossy(&out.stdout);
    let entry = if installed {
        text.lines()
            .find(|l| l.trim_start().starts_with("Task To Run"))
            .map(|l| l.trim().to_string())
            .unwrap_or_else(|| format!("task {TASK_NAME}"))
    } else {
        format!(
            "schtasks task `{TASK_NAME}` → \"{}\" watchdog",
            exe()?.display()
        )
    };
    Ok(Status {
        installed,
        backend: "schtasks",
        entry,
        every_min: None,
        action: None,
    })
}

#[cfg(windows)]
fn install(every_min: u32) -> Result<Status> {
    let exe = exe()?;
    let tr = format!("\"{}\" watchdog", exe.display());
    let out = quiet(Command::new("schtasks"))
        .args([
            "/Create",
            "/SC",
            "MINUTE",
            "/MO",
            &every_min.to_string(),
            "/TN",
            TASK_NAME,
            "/TR",
            &tr,
            "/F",
        ])
        .output()
        .context("schtasks not available")?;
    if !out.status.success() {
        bail!(
            "schtasks /Create failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(Status {
        installed: true,
        backend: "schtasks",
        entry: format!("task `{TASK_NAME}` every {every_min} min → {tr}"),
        every_min: Some(every_min),
        action: Some("installed"),
    })
}

#[cfg(windows)]
fn remove() -> Result<Status> {
    let out = quiet(Command::new("schtasks"))
        .args(["/Delete", "/TN", TASK_NAME, "/F"])
        .output()
        .context("schtasks not available")?;
    // A missing task is not an error for --remove.
    let entry = if out.status.success() {
        format!("task `{TASK_NAME}` deleted")
    } else {
        format!("task `{TASK_NAME}` was not registered")
    };
    Ok(Status {
        installed: false,
        backend: "schtasks",
        entry,
        every_min: None,
        action: Some("removed"),
    })
}

// ---------------------------------------------------------------------------
// Unix: user crontab
// ---------------------------------------------------------------------------

#[cfg(not(windows))]
fn read_crontab() -> Result<Vec<String>> {
    let out = quiet(Command::new("crontab"))
        .arg("-l")
        .output()
        .context("crontab not available")?;
    if !out.status.success() {
        // "no crontab for <user>" exits 1 — treat as empty.
        return Ok(Vec::new());
    }
    Ok(String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::to_string)
        .collect())
}

#[cfg(not(windows))]
fn write_crontab(lines: &[String]) -> Result<()> {
    use std::io::Write;
    let mut child = Command::new("crontab")
        .arg("-")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .context("crontab not available")?;
    {
        let stdin = child.stdin.as_mut().context("crontab stdin")?;
        let mut body = lines.join("\n");
        body.push('\n');
        stdin.write_all(body.as_bytes())?;
    }
    let out = child.wait_with_output()?;
    if !out.status.success() {
        bail!(
            "crontab - failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}

#[cfg(not(windows))]
fn cron_line(every_min: u32) -> Result<String> {
    Ok(format!(
        "*/{every_min} * * * * \"{}\" watchdog >/dev/null 2>&1 {CRON_MARKER}",
        exe()?.display()
    ))
}

#[cfg(not(windows))]
fn status() -> Result<Status> {
    let lines = read_crontab()?;
    let existing = lines.iter().find(|l| l.contains(CRON_MARKER));
    Ok(Status {
        installed: existing.is_some(),
        backend: "crontab",
        entry: existing.cloned().unwrap_or(cron_line(5)?),
        every_min: None,
        action: None,
    })
}

#[cfg(not(windows))]
fn install(every_min: u32) -> Result<Status> {
    let mut lines: Vec<String> = read_crontab()?
        .into_iter()
        .filter(|l| !l.contains(CRON_MARKER))
        .collect();
    let line = cron_line(every_min)?;
    lines.push(line.clone());
    write_crontab(&lines)?;
    Ok(Status {
        installed: true,
        backend: "crontab",
        entry: line,
        every_min: Some(every_min),
        action: Some("installed"),
    })
}

#[cfg(not(windows))]
fn remove() -> Result<Status> {
    let before = read_crontab()?;
    let after: Vec<String> = before
        .iter()
        .filter(|l| !l.contains(CRON_MARKER))
        .cloned()
        .collect();
    let had = after.len() != before.len();
    if had {
        write_crontab(&after)?;
    }
    Ok(Status {
        installed: false,
        backend: "crontab",
        entry: if had {
            "crontab entry removed".into()
        } else {
            "no crontab entry was registered".into()
        },
        every_min: None,
        action: Some("removed"),
    })
}
