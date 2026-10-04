use std::io::{self, BufRead, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use chrono::{DateTime, Utc};
use clap::Args;
use serde::{Deserialize, Serialize};

const LATEST_RELEASE_URL: &str = "https://api.github.com/repos/gerrux/xrun/releases/latest";

/// The installer from the release being installed, not from `master`: the
/// binary, the TUI and the installer's checksum check all come from one tag.
#[cfg(not(windows))]
fn unix_installer_url(tag: &str) -> String {
    format!("https://raw.githubusercontent.com/gerrux/xrun/{tag}/install.sh")
}

#[cfg(windows)]
fn windows_installer_url(tag: &str) -> String {
    format!("https://raw.githubusercontent.com/gerrux/xrun/{tag}/install.ps1")
}

#[derive(Args)]
pub struct UpdateArgs {
    /// Only check whether an update is available
    #[arg(long)]
    pub check: bool,
    /// Install without asking for confirmation
    #[arg(long, short = 'y')]
    pub yes: bool,
    /// Update the CLI only; skip the Python TUI package
    #[arg(long)]
    pub no_tui: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateInfo {
    pub current: String,
    pub latest: String,
    pub url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GitHubRelease {
    tag_name: String,
    html_url: Option<String>,
}

pub fn run(args: &UpdateArgs) -> Result<()> {
    match latest_update()? {
        Some(info) => {
            println!("xrun update available: {} -> {}", info.current, info.latest);
            if let Some(url) = &info.url {
                println!("{url}");
            }
            if args.check {
                return Ok(());
            }
            if !args.yes && !confirm_update(&info)? {
                println!("update skipped");
                return Ok(());
            }
            install_update(&info.latest, args.no_tui)?;
        }
        None => {
            println!("xrun is up to date ({})", current_version());
        }
    }
    Ok(())
}

/// Returns true when the caller should exit instead of continuing startup.
pub fn maybe_prompt_on_startup() -> Result<bool> {
    if std::env::var_os("XRUN_NO_UPDATE_CHECK").is_some() {
        return Ok(false);
    }
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Ok(false);
    }

    let Some(info) = latest_update_quiet()? else {
        return Ok(false);
    };

    eprintln!();
    eprintln!("xrun update available: {} -> {}", info.current, info.latest);
    if let Some(url) = &info.url {
        eprintln!("{url}");
    }
    if confirm_update(&info)? {
        install_update(&info.latest, false)?;
        return Ok(true);
    }
    eprintln!("Continuing with xrun {}.", info.current);
    eprintln!();
    Ok(false)
}

pub fn latest_update() -> Result<Option<UpdateInfo>> {
    let release = fetch_latest_release().context("failed to check latest xrun release")?;
    Ok(update_info_from_release(current_version(), release))
}

fn latest_update_quiet() -> Result<Option<UpdateInfo>> {
    match latest_update() {
        Ok(info) => Ok(info),
        Err(e) => {
            eprintln!("[update] skipped: {e:#}");
            Ok(None)
        }
    }
}

/// Last background lookup, next to the DB. The TUI runs `xrun watchdog`
/// every 60 s; without this it would spend GitHub's 60/h unauthenticated
/// quota on its own.
#[derive(Debug, Default, Serialize, Deserialize)]
struct CheckState {
    /// Last successful lookup; the next one is a day later.
    checked_at: Option<DateTime<Utc>>,
    /// Last failed lookup; retried an hour later.
    failed_at: Option<DateTime<Utc>>,
    latest: Option<String>,
    url: Option<String>,
}

pub fn check_state_path(db_path: &Path) -> PathBuf {
    db_path
        .parent()
        .unwrap_or(Path::new("."))
        .join("update_check.json")
}

/// Release lookup for `xrun watchdog`: at most one GitHub call a day (an
/// hour after a failure); in between the stored result is compared with
/// the running version. `Ok(None)` = up to date or nothing known yet.
pub fn check_daily(state_path: &Path, now: DateTime<Utc>) -> Result<Option<UpdateInfo>> {
    check_daily_with(state_path, now, current_version(), fetch_latest_release)
}

fn check_daily_with(
    state_path: &Path,
    now: DateTime<Utc>,
    current: &str,
    fetch: impl FnOnce() -> Result<GitHubRelease>,
) -> Result<Option<UpdateInfo>> {
    let mut state: CheckState = std::fs::read_to_string(state_path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default();
    // A timestamp in the future (clock ran ahead, then got corrected) is
    // stale, not "fresh for another year".
    let within = |t: Option<DateTime<Utc>>, window: chrono::Duration| {
        t.is_some_and(|t| t <= now && now - t < window)
    };
    let fresh = within(state.checked_at, chrono::Duration::hours(24));
    let backing_off = within(state.failed_at, chrono::Duration::hours(1));
    if !fresh && !backing_off {
        let fetched = fetch();
        match &fetched {
            Ok(release) => {
                state.checked_at = Some(now);
                state.failed_at = None;
                state.latest = Some(release.tag_name.clone());
                state.url = release.html_url.clone();
            }
            Err(_) => state.failed_at = Some(now),
        }
        if let Ok(json) = serde_json::to_string(&state) {
            let _ = std::fs::write(state_path, json);
        }
        fetched.context("failed to check latest xrun release")?;
    }
    Ok(state.latest.and_then(|tag| {
        update_info_from_release(
            current,
            GitHubRelease {
                tag_name: tag,
                html_url: state.url,
            },
        )
    }))
}

fn fetch_latest_release() -> Result<GitHubRelease> {
    let url = std::env::var("XRUN_UPDATE_CHECK_URL").unwrap_or_else(|_| LATEST_RELEASE_URL.into());
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("failed to create update-check runtime")?;
    rt.block_on(async move {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(2))
            .user_agent(concat!("xrun/", env!("CARGO_PKG_VERSION")))
            .build()
            .context("failed to build update-check client")?;
        let resp = client
            .get(url)
            .send()
            .await
            .context("failed to query GitHub releases")?
            .error_for_status()
            .context("GitHub releases returned an error")?;
        resp.json::<GitHubRelease>()
            .await
            .context("failed to parse GitHub release response")
    })
}

fn update_info_from_release(current: &str, release: GitHubRelease) -> Option<UpdateInfo> {
    if version_gt(&release.tag_name, current) {
        Some(UpdateInfo {
            current: current.to_string(),
            latest: release.tag_name,
            url: release.html_url,
        })
    } else {
        None
    }
}

fn confirm_update(info: &UpdateInfo) -> Result<bool> {
    if !io::stdin().is_terminal() {
        bail!("update requires confirmation; re-run with `xrun update --yes`");
    }

    eprintln!();
    eprintln!("Update xrun now?");
    eprintln!("  current: {}", info.current);
    eprintln!("  latest:  {}", info.latest);
    eprintln!("  This runs the official installer and also updates xrun-tui.");
    print!("Install update? [y/N]: ");
    io::stdout().flush().ok();

    let stdin = io::stdin();
    let mut line = String::new();
    stdin.lock().read_line(&mut line)?;
    let answer = line.trim().to_ascii_lowercase();
    Ok(answer == "y" || answer == "yes")
}

fn install_update(version: &str, no_tui: bool) -> Result<()> {
    #[cfg(windows)]
    {
        install_update_windows(version, no_tui)
    }
    #[cfg(not(windows))]
    {
        install_update_unix(version, no_tui)
    }
}

#[cfg(not(windows))]
fn install_update_unix(version: &str, no_tui: bool) -> Result<()> {
    let mut script = format!(
        "curl -sSfL {} | sh -s -- --version {version}",
        unix_installer_url(version)
    );
    if no_tui {
        script.push_str(" --no-tui");
    }
    let status = Command::new("sh")
        .arg("-c")
        .arg(script)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .context("failed to run xrun installer")?;
    if !status.success() {
        bail!("xrun installer failed with status {status}");
    }
    eprintln!("Update complete. Restart xrun to use {version}.");
    Ok(())
}

#[cfg(windows)]
fn install_update_windows(version: &str, no_tui: bool) -> Result<()> {
    let mut command = format!(
        "& ([scriptblock]::Create((irm '{}'))) -Version {version}",
        windows_installer_url(version)
    );
    if no_tui {
        command.push_str(" -NoTui");
    }

    Command::new("powershell")
        .args([
            "-NoProfile",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            &format!("Start-Sleep -Seconds 1; {command}"),
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .spawn()
        .context("failed to start xrun updater")?;

    eprintln!("Updater started. xrun will exit so xrun.exe can be replaced.");
    Ok(())
}

fn current_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

fn version_gt(candidate: &str, current: &str) -> bool {
    let candidate = parse_version(candidate);
    let current = parse_version(current);
    candidate > current
}

fn parse_version(raw: &str) -> Vec<u64> {
    let mut parts: Vec<u64> = raw
        .trim_start_matches('v')
        .split(['.', '-'])
        .take(3)
        .map(|part| part.parse::<u64>().unwrap_or(0))
        .collect();
    parts.resize(3, 0);
    parts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_compare_handles_v_prefix() {
        assert!(version_gt("v0.8.0", "0.7.0"));
        assert!(version_gt("0.7.1", "v0.7.0"));
        assert!(!version_gt("v0.7.0", "0.7.0"));
        assert!(!version_gt("v0.7", "0.7.0"));
        assert!(!version_gt("v0.6.9", "0.7.0"));
    }

    #[test]
    fn release_maps_only_when_newer() {
        let newer = GitHubRelease {
            tag_name: "v9.0.0".into(),
            html_url: Some("https://example.test/release".into()),
        };
        assert!(update_info_from_release("0.7.0", newer).is_some());

        let same = GitHubRelease {
            tag_name: "v0.7.0".into(),
            html_url: None,
        };
        assert!(update_info_from_release("0.7.0", same).is_none());
    }

    fn release(tag: &str) -> Result<GitHubRelease> {
        Ok(GitHubRelease {
            tag_name: tag.into(),
            html_url: Some(format!("https://example.test/{tag}")),
        })
    }

    #[test]
    fn daily_check_calls_github_once_a_day() {
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join("update_check.json");
        let t0 = Utc::now();

        let info = check_daily_with(&path, t0, "0.9.0", || release("v0.9.1"))
            .unwrap()
            .unwrap();
        assert_eq!(info.latest, "v0.9.1");
        assert_eq!(info.url.as_deref(), Some("https://example.test/v0.9.1"));

        // Within the day: answered from the file, no fetch.
        let later = t0 + chrono::Duration::hours(23);
        let info = check_daily_with(&path, later, "0.9.0", || panic!("fetched")).unwrap();
        assert_eq!(info.unwrap().latest, "v0.9.1");

        // After updating, the stored release is no longer newer.
        assert!(
            check_daily_with(&path, later, "0.9.1", || panic!("fetched"))
                .unwrap()
                .is_none()
        );

        // A day later: fetched again.
        let next = t0 + chrono::Duration::hours(25);
        let info = check_daily_with(&path, next, "0.9.0", || release("v0.9.2")).unwrap();
        assert_eq!(info.unwrap().latest, "v0.9.2");
    }

    #[test]
    fn failed_check_retries_after_an_hour_and_keeps_the_last_answer() {
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join("update_check.json");
        let t0 = Utc::now();
        check_daily_with(&path, t0, "0.9.0", || release("v0.9.1")).unwrap();

        let t1 = t0 + chrono::Duration::hours(25);
        let err = check_daily_with(&path, t1, "0.9.0", || bail!("offline"));
        assert!(err.is_err());

        // Backing off: no fetch, the previous answer still stands.
        let t2 = t1 + chrono::Duration::minutes(30);
        let info = check_daily_with(&path, t2, "0.9.0", || panic!("fetched")).unwrap();
        assert_eq!(info.unwrap().latest, "v0.9.1");

        let t3 = t1 + chrono::Duration::minutes(61);
        let info = check_daily_with(&path, t3, "0.9.0", || release("v0.9.2")).unwrap();
        assert_eq!(info.unwrap().latest, "v0.9.2");
    }

    #[test]
    fn timestamps_in_the_future_do_not_suppress_the_lookup() {
        // A clock that ran ahead once (or a copied state file) must not
        // silence the check until real time catches up.
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join("update_check.json");
        let now = Utc::now();
        let ahead = now + chrono::Duration::days(365);
        check_daily_with(&path, ahead, "0.9.0", || release("v0.9.1")).unwrap();
        let info = check_daily_with(&path, now, "0.9.0", || release("v0.9.2")).unwrap();
        assert_eq!(info.unwrap().latest, "v0.9.2");

        let td = tempfile::tempdir().unwrap();
        let path = td.path().join("update_check.json");
        let _ = check_daily_with(&path, ahead, "0.9.0", || bail!("offline"));
        let info = check_daily_with(&path, now, "0.9.0", || release("v0.9.2")).unwrap();
        assert_eq!(info.unwrap().latest, "v0.9.2");
    }

    #[test]
    fn corrupt_state_file_is_a_fresh_start() {
        let td = tempfile::tempdir().unwrap();
        let path = td.path().join("update_check.json");
        std::fs::write(&path, "{not json").unwrap();
        let info = check_daily_with(&path, Utc::now(), "0.9.0", || release("v0.9.1")).unwrap();
        assert_eq!(info.unwrap().latest, "v0.9.1");
    }
}
