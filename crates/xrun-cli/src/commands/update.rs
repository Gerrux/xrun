use std::io::{self, BufRead, IsTerminal, Write};
use std::process::{Command, Stdio};
use std::time::Duration;

use anyhow::{bail, Context, Result};
use clap::Args;
use serde::Deserialize;

const LATEST_RELEASE_URL: &str = "https://api.github.com/repos/gerrux/xrun/releases/latest";
#[cfg(not(windows))]
const UNIX_INSTALLER_URL: &str = "https://raw.githubusercontent.com/gerrux/xrun/master/install.sh";
#[cfg(windows)]
const WINDOWS_INSTALLER_URL: &str =
    "https://raw.githubusercontent.com/gerrux/xrun/master/install.ps1";

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
    let mut script = format!("curl -sSfL {UNIX_INSTALLER_URL} | sh -s -- --version {version}");
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
    let mut command =
        format!("& ([scriptblock]::Create((irm '{WINDOWS_INSTALLER_URL}'))) -Version {version}");
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
}
