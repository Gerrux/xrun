#![deny(unsafe_code)]

use std::path::Path;

use serde::Deserialize;

use crate::error::KaggleError;

pub type KernelSlug = String;

/// One entry from `kaggle datasets list --mine -m`.
#[derive(Debug, Clone, Deserialize, serde::Serialize)]
pub struct DatasetListItem {
    #[serde(rename = "ref")]
    pub slug_ref: String,
    pub title: Option<String>,
    pub size: Option<String>,
    #[serde(rename = "lastUpdated")]
    pub last_updated: Option<String>,
}

/// One entry from `kaggle kernels list --mine -m`.
#[derive(Debug, Clone, Deserialize)]
pub struct KernelListItem {
    #[serde(rename = "ref")]
    pub slug_ref: String,
    pub title: Option<String>,
    pub status: Option<String>,
    #[serde(rename = "totalRunningTimeSec")]
    pub run_seconds: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum KernelState {
    Queued,
    Running,
    Complete,
    Error,
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct KernelStatus {
    pub status: KernelState,
    #[serde(rename = "failureMessage")]
    pub error_message: Option<String>,
    #[serde(rename = "totalRunningTimeSec")]
    pub run_seconds: Option<u64>,
}

impl Default for KernelStatus {
    fn default() -> Self {
        Self {
            status: KernelState::Unknown,
            error_message: None,
            run_seconds: None,
        }
    }
}

/// Abstraction over the kaggle subprocess — enables testing without a real `kaggle` binary.
pub trait KaggleProcess: Send + Sync {
    /// Run `kaggle kernels push -p <dir>` and return stdout.
    fn push(&self, local_dir: &Path) -> Result<String, KaggleError>;
    /// Run `kaggle kernels status <slug> -m` and return stdout.
    fn status(&self, slug: &str) -> Result<String, KaggleError>;
    /// Run `kaggle kernels output <slug> -p <into_dir>` and return stdout.
    fn output(&self, slug: &str, into_dir: &Path) -> Result<String, KaggleError>;
    /// Run `kaggle kernels cancel <slug>` and return stdout.
    fn cancel(&self, slug: &str) -> Result<String, KaggleError>;
    /// Run `kaggle kernels list --mine -m` and return stdout.
    fn list_mine(&self) -> Result<String, KaggleError>;
    /// Run `kaggle config view` and return stdout.
    fn config_view(&self) -> Result<String, KaggleError>;
    /// Run `kaggle datasets status <slug> -m` and return stdout.
    fn datasets_status(&self, slug: &str) -> Result<String, KaggleError>;
    /// Run `kaggle datasets create -p <local_dir>` and return stdout.
    fn datasets_create(&self, local_dir: &Path) -> Result<String, KaggleError>;
    /// Run `kaggle datasets version -p <local_dir> -m <message>` and return stdout.
    fn datasets_version(&self, local_dir: &Path, message: &str) -> Result<String, KaggleError>;
    /// Run `kaggle datasets list --mine -m` and return stdout.
    fn datasets_list_mine(&self) -> Result<String, KaggleError>;

    /// Authenticate via the Python `kaggle` module (`KaggleApi().authenticate()`)
    /// and return the resolved username. The default implementation parses
    /// stdout of `config_view()` for backwards compatibility — this loses
    /// fidelity when the CLI prints a version banner before the YAML body,
    /// which is exactly what `kaggle config view` does today.
    /// `KaggleProcessReal` overrides this to call Python directly.
    fn authenticate_via_python(&self) -> Result<String, KaggleError> {
        let stdout = self.config_view()?;
        parse_username(&stdout)
    }
}

/// Real implementation using the `kaggle` binary from PATH.
/// Injects optional env vars (e.g. `KAGGLE_USERNAME`/`KAGGLE_KEY` or `KAGGLE_API_TOKEN`).
pub struct KaggleProcessReal {
    env: Vec<(String, String)>,
}

impl KaggleProcessReal {
    pub fn new() -> Self {
        Self { env: vec![] }
    }

    fn cmd(&self, args: &[&str]) -> std::process::Command {
        let mut cmd = std::process::Command::new("kaggle");
        cmd.args(args);
        for (k, v) in &self.env {
            cmd.env(k, v);
        }
        // Pin stdin to /dev/null. Without this, `kaggle` inherits our stdin
        // and a few subcommands (notably `kernels push` and `datasets
        // version` on a pre-existing target) prompt for confirmation —
        // freezing `xrun launch --detach` for as long as the user stays at
        // the keyboard. With `null` the prompt either gets EOF and aborts
        // fast, or proceeds with the default. Either way we never hang.
        cmd.stdin(std::process::Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x08000000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        cmd
    }
}

impl Default for KaggleProcessReal {
    fn default() -> Self {
        Self::new()
    }
}

/// Default watchdog for `kaggle kernels push`. Override with
/// `XRUN_KAGGLE_PUSH_TIMEOUT_SECS`. Picked to be longer than a slow 3 GB
/// dataset push but short enough to surface a wedged subprocess instead of
/// silently blocking `xrun launch --detach` forever.
const DEFAULT_PUSH_TIMEOUT_SECS: u64 = 600;
/// Watchdog for `kaggle datasets create|version`. Larger default because
/// dataset uploads are commonly multi-GB and the final commit step can
/// legitimately take several minutes on the Kaggle backend.
const DEFAULT_DATASET_TIMEOUT_SECS: u64 = 1800;
/// Watchdog for `kaggle kernels output`. Without this, a slow or wedged
/// download blocks `poll_completion` indefinitely the moment a kernel
/// flips to Complete — and `xrun fix-status` (called from the TUI with a
/// 60s timeout) gets killed before it ever gets to update the run status,
/// so a finished kernel sits in `running ⚠ stale` forever. Default is
/// large because output bundles can be multi-GB; the daemon path is fine
/// to wait, the watchdog is here to bound the worst case.
const DEFAULT_OUTPUT_TIMEOUT_SECS: u64 = 1800;

fn push_timeout() -> std::time::Duration {
    let secs = std::env::var("XRUN_KAGGLE_PUSH_TIMEOUT_SECS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(DEFAULT_PUSH_TIMEOUT_SECS);
    std::time::Duration::from_secs(secs)
}

fn dataset_timeout() -> std::time::Duration {
    let secs = std::env::var("XRUN_KAGGLE_DATASET_TIMEOUT_SECS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(DEFAULT_DATASET_TIMEOUT_SECS);
    std::time::Duration::from_secs(secs)
}

fn output_timeout() -> std::time::Duration {
    let secs = std::env::var("XRUN_KAGGLE_OUTPUT_TIMEOUT_SECS")
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(DEFAULT_OUTPUT_TIMEOUT_SECS);
    std::time::Duration::from_secs(secs)
}

/// Spawn `cmd`, wait up to `timeout` for it to finish, and on timeout kill
/// the child and return a clear error. stdout/stderr are captured.
///
/// Pipes are drained on background threads. Without this, a chatty subprocess
/// (e.g. `kaggle kernels push` with progress output) fills the OS pipe buffer,
/// blocks on write, and `try_wait` reports `None` forever — we'd then sit on
/// `--detach` until the watchdog fires, even though the kernel started fine on
/// the Kaggle side. Issue 3 in field-issues log.
fn run_with_timeout(
    mut cmd: std::process::Command,
    timeout: std::time::Duration,
    label: &str,
) -> Result<std::process::Output, KaggleError> {
    use std::io::Read;

    cmd.stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut child = cmd
        .spawn()
        .map_err(|e| KaggleError::NotFound(format!("spawn {label}: {e}")))?;

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let drain = |mut h: Option<std::process::ChildStdout>| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            if let Some(h) = h.as_mut() {
                let _ = h.read_to_end(&mut buf);
            }
            buf
        })
    };
    let drain_err = |mut h: Option<std::process::ChildStderr>| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            if let Some(h) = h.as_mut() {
                let _ = h.read_to_end(&mut buf);
            }
            buf
        })
    };
    let stdout_handle = drain(stdout);
    let stderr_handle = drain_err(stderr);

    let start = std::time::Instant::now();
    let poll = std::time::Duration::from_millis(200);
    let status = loop {
        match child
            .try_wait()
            .map_err(|e| KaggleError::Other(format!("wait {label}: {e}")))?
        {
            Some(s) => break s,
            None => {
                if start.elapsed() >= timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(KaggleError::Other(format!(
                        "{label} timed out after {}s — kaggle subprocess wedged. \
                         Override with XRUN_KAGGLE_PUSH_TIMEOUT_SECS=N.",
                        timeout.as_secs()
                    )));
                }
                std::thread::sleep(poll);
            }
        }
    };

    let stdout = stdout_handle.join().unwrap_or_default();
    let stderr = stderr_handle.join().unwrap_or_default();
    Ok(std::process::Output {
        status,
        stdout,
        stderr,
    })
}

impl KaggleProcess for KaggleProcessReal {
    fn push(&self, local_dir: &Path) -> Result<String, KaggleError> {
        let mut cmd = self.cmd(&["kernels", "push", "-p"]);
        cmd.arg(local_dir);
        let output = run_with_timeout(cmd, push_timeout(), "kaggle kernels push")?;

        if !output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout).to_string();
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            return Err(KaggleError::CliFailure {
                exit_code: output.status.code().unwrap_or(-1),
                stderr: annotate_kaggle_cli_failure(&format!("stdout: {stdout}\nstderr: {stderr}")),
            });
        }
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }

    fn status(&self, slug: &str) -> Result<String, KaggleError> {
        // Kaggle CLI 1.8.x dropped the `-m` (machine-readable JSON) flag from
        // `kernels status`; only plain text is available now.
        let output = self
            .cmd(&["kernels", "status", slug])
            .output()
            .map_err(|e| KaggleError::NotFound(e.to_string()))?;

        if !output.status.success() {
            return Err(KaggleError::CliFailure {
                exit_code: output.status.code().unwrap_or(-1),
                stderr: String::from_utf8_lossy(&output.stderr).to_string(),
            });
        }
        let raw = String::from_utf8_lossy(&output.stdout).to_string();
        tracing::debug!("kaggle kernels status {slug} raw output: {raw:?}");
        Ok(raw)
    }

    fn output(&self, slug: &str, into_dir: &Path) -> Result<String, KaggleError> {
        let mut cmd = self.cmd(&["kernels", "output", slug, "-p"]);
        cmd.arg(into_dir);
        let out = run_with_timeout(cmd, output_timeout(), "kaggle kernels output")?;

        if !out.status.success() {
            return Err(KaggleError::CliFailure {
                exit_code: out.status.code().unwrap_or(-1),
                stderr: String::from_utf8_lossy(&out.stderr).to_string(),
            });
        }
        Ok(String::from_utf8_lossy(&out.stdout).to_string())
    }

    fn cancel(&self, slug: &str) -> Result<String, KaggleError> {
        let out = self
            .cmd(&["kernels", "cancel", slug])
            .output()
            .map_err(|e| KaggleError::NotFound(e.to_string()))?;
        if !out.status.success() {
            return Err(KaggleError::CliFailure {
                exit_code: out.status.code().unwrap_or(-1),
                stderr: format!(
                    "stdout: {}\nstderr: {}",
                    String::from_utf8_lossy(&out.stdout),
                    String::from_utf8_lossy(&out.stderr)
                ),
            });
        }
        Ok(String::from_utf8_lossy(&out.stdout).to_string())
    }

    fn list_mine(&self) -> Result<String, KaggleError> {
        // `kaggle kernels list` does NOT support `--json`; the only structured
        // option is `-v/--csv`. The historical `-m` (mine) flag returned a
        // padded-column table that our JSON parser couldn't read, breaking
        // `xrun resume` whenever it fell through to `vendor_instances`.
        // Use CSV — note that `-m` here is `--mine` (the short alias) and is
        // unrelated to the `-m` machine-readable flag on `status`/`datasets`.
        let out = self
            .cmd(&["kernels", "list", "--mine", "--csv"])
            .output()
            .map_err(|e| KaggleError::NotFound(e.to_string()))?;
        if !out.status.success() {
            return Err(KaggleError::CliFailure {
                exit_code: out.status.code().unwrap_or(-1),
                stderr: String::from_utf8_lossy(&out.stderr).to_string(),
            });
        }
        Ok(String::from_utf8_lossy(&out.stdout).to_string())
    }

    fn config_view(&self) -> Result<String, KaggleError> {
        let out = self
            .cmd(&["config", "view"])
            .output()
            .map_err(|e| KaggleError::NotFound(e.to_string()))?;
        if !out.status.success() {
            return Err(KaggleError::CliFailure {
                exit_code: out.status.code().unwrap_or(-1),
                stderr: String::from_utf8_lossy(&out.stderr).to_string(),
            });
        }
        Ok(String::from_utf8_lossy(&out.stdout).to_string())
    }

    fn datasets_status(&self, slug: &str) -> Result<String, KaggleError> {
        // The `-m` (machine-readable) flag was dropped from `datasets status`
        // in kaggle CLI 1.7.x; passing it now produces
        // `unrecognized arguments: -m` and breaks `xrun doctor`. The plain
        // text output ("<slug> has status: ready") is still parseable by
        // `parse_dataset_ready`, which only looks for the word "ready".
        let out = self
            .cmd(&["datasets", "status", slug])
            .output()
            .map_err(|e| KaggleError::NotFound(e.to_string()))?;
        if !out.status.success() {
            return Err(KaggleError::CliFailure {
                exit_code: out.status.code().unwrap_or(-1),
                stderr: String::from_utf8_lossy(&out.stderr).to_string(),
            });
        }
        Ok(String::from_utf8_lossy(&out.stdout).to_string())
    }

    fn datasets_create(&self, local_dir: &Path) -> Result<String, KaggleError> {
        let mut cmd = self.cmd(&["datasets", "create", "-p"]);
        cmd.arg(local_dir).arg("--dir-mode").arg("tar");
        let out = run_with_timeout(cmd, dataset_timeout(), "kaggle datasets create")?;
        if !out.status.success() {
            return Err(KaggleError::CliFailure {
                exit_code: out.status.code().unwrap_or(-1),
                stderr: format!(
                    "stdout: {}\nstderr: {}",
                    String::from_utf8_lossy(&out.stdout),
                    String::from_utf8_lossy(&out.stderr)
                ),
            });
        }
        Ok(String::from_utf8_lossy(&out.stdout).to_string())
    }

    fn datasets_version(&self, local_dir: &Path, message: &str) -> Result<String, KaggleError> {
        let mut cmd = self.cmd(&["datasets", "version", "-p"]);
        cmd.arg(local_dir)
            .arg("--dir-mode")
            .arg("tar")
            .arg("-m")
            .arg(message);
        let out = run_with_timeout(cmd, dataset_timeout(), "kaggle datasets version")?;
        if !out.status.success() {
            return Err(KaggleError::CliFailure {
                exit_code: out.status.code().unwrap_or(-1),
                stderr: format!(
                    "stdout: {}\nstderr: {}",
                    String::from_utf8_lossy(&out.stdout),
                    String::from_utf8_lossy(&out.stderr)
                ),
            });
        }
        Ok(String::from_utf8_lossy(&out.stdout).to_string())
    }

    fn datasets_list_mine(&self) -> Result<String, KaggleError> {
        let out = self
            .cmd(&["datasets", "list", "--mine", "-m"])
            .output()
            .map_err(|e| KaggleError::NotFound(e.to_string()))?;
        if !out.status.success() {
            return Err(KaggleError::CliFailure {
                exit_code: out.status.code().unwrap_or(-1),
                stderr: String::from_utf8_lossy(&out.stderr).to_string(),
            });
        }
        Ok(String::from_utf8_lossy(&out.stdout).to_string())
    }

    /// Use the Python `kaggle` module to authenticate, sidestepping the
    /// stdout-parsing brittleness of `kaggle config view` (which prints a
    /// version-update banner before the YAML body in newer CLIs and breaks
    /// the regex). Returns the username on success.
    fn authenticate_via_python(&self) -> Result<String, KaggleError> {
        // The kaggle module prints an "outdated version" banner to STDOUT
        // (not stderr) on import for many published versions. Wrapping our
        // own write in an unmistakable sentinel lets us strip the noise
        // even when it lands in the same stream.
        //
        // The script also enforces a strict priority: when the caller passed
        // a `KAGGLE_API_TOKEN` env var (which `with_credentials()` does for
        // token-auth setups), we use that token DIRECTLY via introspect_token
        // and refuse to fall through to `~/.kaggle/kaggle.json` or
        // `~/.kaggle/access_token` if introspection fails. The default kaggle
        // module path silently falls through, which means a stale token cache
        // on disk would mask a bad/expired env token and return the wrong
        // username — the exact bug reported when changing the API key in TUI
        // didn't change the resolved nickname.
        let script = r#"
import os, sys

# CRITICAL: snapshot env BEFORE importing kaggle. The kaggle module's
# import-time code calls authenticate() and pops KAGGLE_API_TOKEN out of
# os.environ as a side-effect, so reading it after the import sees None.
# That used to silently fall through to ~/.kaggle/access_token (cached
# from a previous account) and return the wrong username.
env_token = os.environ.get("KAGGLE_API_TOKEN")

try:
    from kaggle.api.kaggle_api_extended import KaggleApi
except Exception as e:
    sys.stderr.write(f"import_error: {e}\n")
    sys.exit(2)

api = KaggleApi()

if env_token and not os.path.exists(env_token):
    # Env token wins. Bypass api.authenticate()'s fallback chain by going
    # straight to _introspect_token, so a bad token errors out instead of
    # silently using cached on-disk creds from another account.
    try:
        user = api._introspect_token(env_token)
    except Exception as e:
        sys.stderr.write(f"env_token_introspect_failed: {e}\n")
        sys.exit(5)
    if not user:
        sys.stderr.write("env_token_no_username\n")
        sys.exit(4)
    sys.stdout.write(f"\n<<<XRUN_KAGGLE_USER:{user}>>>\n")
    sys.exit(0)

# No env token -> let kaggle module pick from its standard sources.
try:
    api.authenticate()
except Exception as e:
    sys.stderr.write(f"auth_error: {e}\n")
    sys.exit(3)
user = (
    getattr(api, "config_values", {}).get("username")
    or api.read_config_environment().get("username", "")
)
if not user:
    sys.stderr.write("auth_ok_no_username\n")
    sys.exit(4)
sys.stdout.write(f"\n<<<XRUN_KAGGLE_USER:{user}>>>\n")
"#;
        let pythons: &[&str] = &["python", "python3", "py"];
        let mut last_err: Option<KaggleError> = None;
        for py in pythons {
            let mut cmd = std::process::Command::new(py);
            cmd.args(["-c", script]);
            for (k, v) in &self.env {
                cmd.env(k, v);
            }
            cmd.stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped());
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                const CREATE_NO_WINDOW: u32 = 0x08000000;
                cmd.creation_flags(CREATE_NO_WINDOW);
            }
            let out = match cmd.output() {
                Ok(o) => o,
                Err(_) => continue, // try next interpreter
            };
            if out.status.success() {
                let stdout = String::from_utf8_lossy(&out.stdout);
                // Sentinel-extract: kaggle's import-time outdated-version
                // banner shares stdout with our payload, so a naïve trim
                // would yield "Warning: …\nactual-user". Look for the
                // bracketed marker we wrote and pull the username out.
                let user = stdout
                    .lines()
                    .find_map(|l| {
                        l.strip_prefix("<<<XRUN_KAGGLE_USER:")
                            .and_then(|s| s.strip_suffix(">>>"))
                            .map(|s| s.trim().to_string())
                    })
                    .unwrap_or_default();
                if user.is_empty() {
                    return Err(KaggleError::ParseError(
                        "kaggle Python module authenticated but returned no username".to_string(),
                    ));
                }
                return Ok(user);
            }
            let code = out.status.code().unwrap_or(-1);
            let stderr = String::from_utf8_lossy(&out.stderr).to_string();
            // Code 2 = python module missing → try fallback below.
            if code == 2 {
                last_err = Some(KaggleError::NotFound(format!(
                    "kaggle Python module not importable: {stderr}"
                )));
                continue;
            }
            return Err(KaggleError::CliFailure {
                exit_code: code,
                stderr: format!("kaggle.api authenticate failed: {stderr}"),
            });
        }
        // No Python interpreter or module — fall back to the legacy CLI parse.
        let stdout = match self.config_view() {
            Ok(s) => s,
            Err(cli_err) => {
                let msg = last_err
                    .map(|e| e.to_string())
                    .unwrap_or_else(|| cli_err.to_string());
                return Err(KaggleError::NotFound(format!(
                    "neither python kaggle module nor `kaggle config view` available: {msg}"
                )));
            }
        };
        parse_username(&stdout)
    }
}

pub struct KaggleCli {
    process: Box<dyn KaggleProcess>,
}

impl KaggleCli {
    pub fn new() -> Self {
        Self {
            process: Box::new(KaggleProcessReal::new()),
        }
    }

    pub fn with_process(process: Box<dyn KaggleProcess>) -> Self {
        Self { process }
    }

    /// Override env vars injected into the kaggle subprocess (credentials etc.).
    /// Only works when using the real `KaggleProcessReal` backend.
    pub fn with_env(mut self, env: Vec<(String, String)>) -> Self {
        self.process = Box::new(KaggleProcessReal { env });
        self
    }

    /// Push a kernel directory and return the slug (`<user>/<slug>`).
    pub fn push(&self, local_dir: &Path) -> Result<KernelSlug, KaggleError> {
        let stdout = self.process.push(local_dir)?;
        parse_push_slug(&stdout)
    }

    /// Get the current status of a kernel.
    pub fn status(&self, slug: &str) -> Result<KernelStatus, KaggleError> {
        let stdout = self.process.status(slug)?;
        parse_status(&stdout)
    }

    /// Download kernel output to `into_dir`.
    pub fn output(&self, slug: &str, into_dir: &Path) -> Result<(), KaggleError> {
        self.process.output(slug, into_dir)?;
        Ok(())
    }

    /// Try to cancel / interrupt a running kernel. Returns `Ok(())` on success
    /// or if the cancel command is unsupported (fallback: kernel auto-terminates).
    pub fn cancel(&self, slug: &str) -> Result<(), KaggleError> {
        self.process.cancel(slug)?;
        Ok(())
    }

    /// List the authenticated user's kernels.
    pub fn list_mine(&self) -> Result<Vec<KernelListItem>, KaggleError> {
        let stdout = self.process.list_mine()?;
        parse_kernel_list(&stdout)
    }

    /// Return the username from `kaggle config view`.
    pub fn username(&self) -> Result<String, KaggleError> {
        let stdout = self.process.config_view()?;
        parse_username(&stdout)
    }

    /// Authenticate via the Python `kaggle` module rather than parsing
    /// `kaggle config view` stdout. Returns the resolved username.
    /// Use this from doctor/wizard probes — it's resilient to CLI
    /// version-banners and dropped flags.
    pub fn authenticate(&self) -> Result<String, KaggleError> {
        self.process.authenticate_via_python()
    }

    /// Return `true` when the dataset is in `ready` state.
    pub fn is_dataset_ready(&self, slug: &str) -> Result<bool, KaggleError> {
        let stdout = self.process.datasets_status(slug)?;
        Ok(parse_dataset_ready(&stdout))
    }

    /// Push a local directory as a Kaggle dataset.
    ///
    /// Ensures `dataset-metadata.json` exists in `local_dir` (generates one from
    /// `slug` if absent), then calls `kaggle datasets create`. On "already exists"
    /// conflict (exit non-zero + "already" in stderr), falls back to
    /// `kaggle datasets version`. Transient errors (timeout, 5xx, connection
    /// reset on the final `CreateDatasetVersion` commit) are retried up to
    /// `XRUN_KAGGLE_DATASET_RETRIES` times (default 2 — total 3 attempts).
    pub fn dataset_push(
        &self,
        local_dir: &Path,
        slug: &str,
        message: Option<&str>,
    ) -> Result<(), KaggleError> {
        ensure_dataset_metadata(local_dir, slug)?;
        let msg = message.unwrap_or("Updated via xrun");

        let max_retries: u32 = std::env::var("XRUN_KAGGLE_DATASET_RETRIES")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(2);

        let mut attempt = 0u32;
        loop {
            let result = match self.process.datasets_create(local_dir) {
                Ok(_) => Ok(()),
                Err(KaggleError::CliFailure { ref stderr, .. })
                    if stderr.to_lowercase().contains("already") =>
                {
                    self.process.datasets_version(local_dir, msg).map(|_| ())
                }
                Err(e) => Err(e),
            };

            match result {
                Ok(()) => return Ok(()),
                Err(e) if is_transient_kaggle_error(&e) && attempt < max_retries => {
                    let base_secs: u64 = std::env::var("XRUN_KAGGLE_DATASET_BACKOFF_BASE_SECS")
                        .ok()
                        .and_then(|s| s.parse().ok())
                        .unwrap_or(10);
                    let backoff = std::time::Duration::from_secs(base_secs * (1 << attempt));
                    tracing::warn!(
                        "kaggle dataset push transient error ({}/{max_retries}): {e}. \
                         Retrying in {:?}…",
                        attempt + 1,
                        backoff
                    );
                    std::thread::sleep(backoff);
                    attempt += 1;
                }
                Err(e) => return Err(e),
            }
        }
    }

    /// List datasets owned by the authenticated user.
    pub fn dataset_list_mine(&self) -> Result<Vec<DatasetListItem>, KaggleError> {
        let stdout = self.process.datasets_list_mine()?;
        parse_dataset_list(&stdout)
    }

    /// Return raw status string for a dataset slug.
    pub fn dataset_status_raw(&self, slug: &str) -> Result<String, KaggleError> {
        self.process.datasets_status(slug)
    }
}

impl Default for KaggleCli {
    fn default() -> Self {
        Self::new()
    }
}

/// Heuristic for "is this likely a transient network/backend hiccup worth
/// retrying?" Conservative — we'd rather miss a retry than retry a hard
/// permission/auth error and burn another 3 GB of upload bandwidth.
fn is_transient_kaggle_error(err: &KaggleError) -> bool {
    let s = err.to_string().to_lowercase();
    // Watchdog-killed subprocess from run_with_timeout
    if s.contains("timed out") || s.contains("wedged") {
        return true;
    }
    match err {
        KaggleError::CliFailure { stderr, .. } => {
            let lower = stderr.to_lowercase();
            lower.contains("timeout")
                || lower.contains("timed out")
                || lower.contains("connection reset")
                || lower.contains("connection aborted")
                || lower.contains("connection refused")
                || lower.contains("eof occurred")
                || lower.contains("temporarily unavailable")
                || lower.contains(" 502")
                || lower.contains(" 503")
                || lower.contains(" 504")
        }
        _ => false,
    }
}

/// Append an actionable hint to a `kaggle CLI failed` body when the message
/// matches a known kaggle-side bug, so operators don't have to web-search
/// the cryptic native error.
///
/// Currently handles the kaggle CLI 1.8.x path where the CLI's own
/// outdated-version warning gets fed through its own `json.loads` and
/// produces `Expecting value: line 1 column 1 (char 0)` from a perfectly
/// reasonable kernel push.
pub(crate) fn annotate_kaggle_cli_failure(body: &str) -> String {
    if body.contains("Expecting value: line 1 column 1 (char 0)") {
        return format!(
            "{body}\n\nhint: this matches a known kaggle CLI 1.8.x bug where the CLI's \
             own outdated-version warning corrupts JSON parsing. \
             Run `pip install --upgrade kaggle` and retry. If the upgrade \
             isn't available in your environment, retry the launch — kaggle \
             only emits the warning intermittently."
        );
    }
    body.to_string()
}

/// Drop kaggle CLI noise lines (`Warning:` banners, literal-template version
/// notices) from a stdout buffer before our JSON / line parsers see it.
///
/// kaggle CLI 1.8.3 emits a literal-template warning to **stdout** like
/// `Warning: Looks like you're using an outdated `kaggle`` version
/// (installed: {current_version}), …` — note the un-substituted `{…}`
/// placeholders, which is a kaggle bug. When we then `serde_json::from_str`
/// the buffer, we get the cryptic `Expecting value: line 1 column 1
/// (char 0)` from a wedged-looking line. Stripping these lines up front
/// keeps the parser path clean and gives users a real error if something
/// else goes wrong.
pub(crate) fn strip_kaggle_cli_noise(stdout: &str) -> String {
    stdout
        .lines()
        .filter(|line| {
            let trimmed = line.trim_start();
            // Discard top-level warning banners. We deliberately keep
            // anything indented (those are part of structured output —
            // e.g. YAML body lines from `config view` that happen to
            // contain the substring "warning").
            if trimmed.len() == line.len() && trimmed.starts_with("Warning:") {
                return false;
            }
            // The literal-template variant in 1.8.3 leaks the un-substituted
            // `{current_version}` placeholder; match defensively so we
            // don't depend on the "Warning:" prefix specifically.
            if trimmed.contains("outdated `kaggle`") {
                return false;
            }
            true
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn parse_push_slug(stdout: &str) -> Result<KernelSlug, KaggleError> {
    let stdout = strip_kaggle_cli_noise(stdout);
    let stdout = stdout.as_str();
    // Expected: "Kernel pushed: <user>/<slug>" or "Kernel already exists, new version pushed: ..."
    for line in stdout.lines() {
        if let Some(rest) = line.strip_prefix("Kernel pushed: ") {
            return Ok(rest.trim().to_string());
        }
        if let Some(rest) = line.strip_prefix("Kernel already exists, new version pushed: ") {
            return Ok(rest.trim().to_string());
        }
        // Also handle "Kernel version X successfully pushed to ..."
        if line.contains("successfully pushed") {
            // Try to find user/slug pattern
            if let Some(slug) = extract_slug_from_line(line) {
                return Ok(slug);
            }
        }
    }
    Err(KaggleError::ParseError(format!(
        "could not find kernel slug in push output: {stdout}"
    )))
}

fn extract_slug_from_line(line: &str) -> Option<String> {
    // First try: extract from Kaggle URL pattern "/code/<user>/<slug>"
    // e.g. "https://www.kaggle.com/code/kartaviychert/my-kernel"
    if let Some(pos) = line.find("/code/") {
        let after = &line[pos + "/code/".len()..];
        let slug_part: String = after
            .split_whitespace()
            .next()
            .unwrap_or("")
            .trim_end_matches('.')
            .to_string();
        // Must contain exactly one '/'
        if slug_part.matches('/').count() == 1 {
            return Some(slug_part);
        }
    }
    // Fallback: look for bare "username/kernelname" token (no http prefix)
    for part in line.split_whitespace() {
        let clean = part.trim_matches('"').trim_end_matches('.');
        if clean.contains('/') && !clean.starts_with("http") && clean.matches('/').count() == 1 {
            return Some(clean.to_string());
        }
    }
    None
}

fn parse_status(stdout: &str) -> Result<KernelStatus, KaggleError> {
    let cleaned = strip_kaggle_cli_noise(stdout);
    let trimmed = cleaned.trim();
    if trimmed.is_empty() {
        return Err(KaggleError::ParseError("empty status output".to_string()));
    }

    // Older Kaggle CLI versions emitted JSON via `-m`. Tolerate that path
    // first so callers built against an old kaggle-api still work.
    if trimmed.starts_with('{') {
        if let Ok(parsed) = serde_json::from_str::<KernelStatus>(trimmed) {
            return Ok(parsed);
        }
    }

    // Kaggle CLI 1.8.x text format. Examples:
    //   "<slug> has status \"KernelWorkerStatus.RUNNING\""
    //   "<slug> has status \"KernelWorkerStatus.COMPLETE\""
    //   "<slug> has status \"KernelWorkerStatus.ERROR\"
    //    Failure message: \"Your notebook tried to allocate more memory than is available.\""
    //   "<slug> has status \"KernelWorkerStatus.QUEUED\""
    //   "<slug> has status \"KernelWorkerStatus.CANCEL_ACKNOWLEDGED\""
    //
    // Older versions emitted "Kernel is currently running" or short tokens
    // like "complete" / "error" — we still tolerate those for forward-compat.
    let lower = trimmed.to_lowercase();
    let state = if lower.contains("kernelworkerstatus.running")
        || lower.contains("currently running")
        || lower.contains("\"running\"")
    {
        KernelState::Running
    } else if lower.contains("kernelworkerstatus.complete")
        || lower.contains("\"complete\"")
        || lower.contains("has completed")
    {
        KernelState::Complete
    } else if lower.contains("kernelworkerstatus.error")
        || lower.contains("\"error\"")
        || lower.contains("\"failed\"")
    {
        KernelState::Error
    } else if lower.contains("kernelworkerstatus.queued")
        || lower.contains("\"queued\"")
        || lower.contains("is queued")
    {
        KernelState::Queued
    } else if lower.contains("kernelworkerstatus.cancel") || lower.contains("\"cancel") {
        // Cancelled by user / cancel acknowledged → terminal failure so the
        // run doesn't sit in `running` forever.
        KernelState::Error
    } else {
        KernelState::Unknown
    };

    let is_error = matches!(state, KernelState::Error);
    Ok(KernelStatus {
        status: state,
        error_message: if is_error {
            Some(extract_failure_message(trimmed).unwrap_or_else(|| trimmed.to_string()))
        } else {
            None
        },
        run_seconds: None,
    })
}

/// Pull the quoted Failure message: line out of the kaggle status text so the
/// stored `auto_destroyed_reason` / event message stays short and readable.
fn extract_failure_message(stdout: &str) -> Option<String> {
    for line in stdout.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("Failure message:") {
            let trimmed = rest.trim().trim_matches('"');
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }
    None
}

fn parse_kernel_list(stdout: &str) -> Result<Vec<KernelListItem>, KaggleError> {
    let cleaned = strip_kaggle_cli_noise(stdout);
    let trimmed = cleaned.trim();
    if trimmed.is_empty() || trimmed == "[]" {
        return Ok(vec![]);
    }
    // Newer call sites pass `--csv`; older mocks/tests still feed JSON, so
    // try JSON first and fall through to CSV when the body doesn't start
    // like JSON. Reduces churn in the existing test suite while letting
    // production paths swallow real kaggle output.
    if trimmed.starts_with('[') || trimmed.starts_with('{') {
        return serde_json::from_str(trimmed).map_err(|e| {
            KaggleError::ParseError(format!(
                "failed to parse kernel list JSON: {e}\nInput: {trimmed}"
            ))
        });
    }
    parse_kernel_list_csv(trimmed)
}

/// Parse `kaggle kernels list --csv` output.
///
/// Layout (kaggle CLI 1.6+):
/// ```text
/// ref,title,author,lastRunTime,totalVotes
/// user/kernel-a,My Kernel,user,2026-05-05T12:00:00Z,3
/// ```
///
/// `kernels list` doesn't expose status or runtime, so callers downstream of
/// `vendor_instances` see `status: None` / `run_seconds: None` for every
/// kaggle row. That's accurate to what the CLI tells us — better than
/// silently fabricating a status. `xrun gc` and `xrun fix-status` only need
/// the slug ref to detect orphans, so the missing fields are harmless there;
/// `xrun resume` no longer reaches this path because `poll_completion`
/// returns `Some` for healthy kernels.
fn parse_kernel_list_csv(body: &str) -> Result<Vec<KernelListItem>, KaggleError> {
    let mut lines = body.lines().filter(|l| !l.trim().is_empty());
    let header = match lines.next() {
        Some(h) => h,
        None => return Ok(vec![]),
    };
    let cols: Vec<&str> = header.split(',').map(str::trim).collect();
    let ref_idx = cols.iter().position(|c| *c == "ref");

    let mut out = Vec::new();
    for row in lines {
        let fields = split_csv_row(row);
        let slug = match ref_idx.and_then(|i| fields.get(i)) {
            Some(s) if !s.is_empty() => s.clone(),
            // Permissive fallback: when the header is absent or unrecognised,
            // pick the first non-empty field that contains `/` — kaggle slugs
            // are always `<owner>/<name>`. Avoids dropping rows on schema
            // drift.
            _ => match fields.iter().find(|f| f.contains('/')) {
                Some(s) => s.clone(),
                None => continue,
            },
        };
        let title = cols
            .iter()
            .position(|c| *c == "title")
            .and_then(|i| fields.get(i))
            .filter(|s| !s.is_empty())
            .cloned();
        out.push(KernelListItem {
            slug_ref: slug,
            title,
            status: None,
            run_seconds: None,
        });
    }
    Ok(out)
}

/// Split a CSV row, respecting double-quoted fields. Kaggle escapes embedded
/// commas (e.g. in titles) by quoting; without quote handling we'd shift all
/// downstream columns.
fn split_csv_row(row: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut buf = String::new();
    let mut in_quote = false;
    let mut chars = row.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if in_quote && chars.peek() == Some(&'"') => {
                // Escaped double-quote inside a quoted field.
                buf.push('"');
                chars.next();
            }
            '"' => in_quote = !in_quote,
            ',' if !in_quote => {
                out.push(std::mem::take(&mut buf));
            }
            _ => buf.push(c),
        }
    }
    out.push(buf);
    for f in &mut out {
        *f = f.trim().to_string();
    }
    out
}

fn parse_username(stdout: &str) -> Result<String, KaggleError> {
    let cleaned = strip_kaggle_cli_noise(stdout);
    let trimmed = cleaned.trim();
    // Try JSON format: {"username": "..."}
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(trimmed) {
        if let Some(u) = v.get("username").and_then(|u| u.as_str()) {
            return Ok(u.to_string());
        }
    }
    // Plain-text format. Tolerant of:
    //   "username: foo"
    //   "- username: foo"      (yaml-list style)
    //   "  username: foo"      (indented)
    //   "Warning: ..." / version-banner lines before the real content.
    for line in trimmed.lines() {
        let stripped = line.trim_start().trim_start_matches('-').trim_start();
        if let Some(rest) = stripped.strip_prefix("username:") {
            let value = rest.trim().trim_matches('"').trim_matches('\'');
            if !value.is_empty() {
                return Ok(value.to_string());
            }
        }
    }
    Err(KaggleError::ParseError(format!(
        "could not find username in config view output: {trimmed}"
    )))
}

fn parse_dataset_ready(stdout: &str) -> bool {
    let cleaned = strip_kaggle_cli_noise(stdout);
    let trimmed = cleaned.trim().to_lowercase();
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&trimmed) {
        let status = v
            .get("status")
            .or_else(|| v.get("datasetStatus"))
            .and_then(|s| s.as_str())
            .unwrap_or("");
        return status == "ready";
    }
    trimmed.contains("ready")
}

fn parse_dataset_list(stdout: &str) -> Result<Vec<DatasetListItem>, KaggleError> {
    let cleaned = strip_kaggle_cli_noise(stdout);
    let trimmed = cleaned.trim();
    if trimmed.is_empty() || trimmed == "[]" {
        return Ok(vec![]);
    }
    serde_json::from_str(trimmed).map_err(|e| {
        KaggleError::ParseError(format!(
            "failed to parse dataset list JSON: {e}\nInput: {trimmed}"
        ))
    })
}

/// Write `dataset-metadata.json` into `local_dir` if not already present.
fn ensure_dataset_metadata(local_dir: &Path, slug: &str) -> Result<(), KaggleError> {
    let meta_path = local_dir.join("dataset-metadata.json");
    if meta_path.exists() {
        return Ok(());
    }
    let title = slug.rsplit('/').next().unwrap_or(slug).replace('-', " ");
    let meta = serde_json::json!({
        "title": title,
        "id": slug,
        "licenses": [{"name": "CC0-1.0"}]
    });
    let content = serde_json::to_string_pretty(&meta)
        .map_err(|e| KaggleError::ParseError(format!("failed to serialize metadata: {e}")))?;
    std::fs::write(&meta_path, content)?;
    Ok(())
}

#[cfg(test)]
mod cli_unit_tests {
    use super::{annotate_kaggle_cli_failure, parse_kernel_list, strip_kaggle_cli_noise};

    #[test]
    fn strip_drops_leading_warning_line() {
        let input = "Warning: Looks like you're using an outdated `kaggle` version, \
                     please consider upgrading.\nuser/slug has status \"Running\"\n";
        let cleaned = strip_kaggle_cli_noise(input);
        assert!(!cleaned.contains("Warning:"));
        assert!(cleaned.contains("Running"));
    }

    #[test]
    fn strip_drops_literal_template_variant() {
        // The 1.8.3 bug: curly-brace placeholders are emitted unsubstituted.
        let input = "Warning: Looks like you're using an outdated `kaggle`` version \
                     (installed: {current_version}), please consider upgrading to the \
                     latest version ({latest_version_str})\n[]\n";
        let cleaned = strip_kaggle_cli_noise(input);
        assert_eq!(cleaned.trim(), "[]");
    }

    #[test]
    fn strip_keeps_legitimate_content_with_warning_substring() {
        // Indented `Warning:` substrings inside structured output (eg yaml
        // body line) must not be discarded.
        let input = "  message: \"Warning: low disk\"\n";
        let cleaned = strip_kaggle_cli_noise(input);
        assert!(cleaned.contains("Warning: low disk"));
    }

    #[test]
    fn annotate_adds_hint_for_known_kaggle_bug() {
        let body = "stdout: \nstderr: ... Expecting value: line 1 column 1 (char 0)";
        let annotated = annotate_kaggle_cli_failure(body);
        assert!(annotated.contains("known kaggle CLI 1.8.x bug"));
        assert!(annotated.contains("pip install --upgrade kaggle"));
    }

    #[test]
    fn annotate_passes_through_other_failures() {
        let body = "stderr: 403 Forbidden";
        let annotated = annotate_kaggle_cli_failure(body);
        assert_eq!(annotated, body);
    }

    // Bonus #3 regression: kaggle CLI emits CSV (not JSON) for `kernels list`.
    // Previously `parse_kernel_list` only knew JSON and crashed `xrun resume`
    // with "Expecting value: line 1 column 1 (char 0)".

    #[test]
    fn parse_kernel_list_handles_csv() {
        let csv = "ref,title,author,lastRunTime,totalVotes\n\
                   user/treetop3d-v9-skipalpha-d,Treetop 3D,user,2026-05-05T12:00:00Z,0\n\
                   other/abc-kernel,ABC,other,2026-05-04T10:00:00Z,1\n";
        let items = parse_kernel_list(csv).expect("CSV must parse");
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].slug_ref, "user/treetop3d-v9-skipalpha-d");
        assert_eq!(items[0].title.as_deref(), Some("Treetop 3D"));
        assert!(items[0].status.is_none(), "CSV doesn't carry status");
        assert_eq!(items[1].slug_ref, "other/abc-kernel");
    }

    #[test]
    fn parse_kernel_list_csv_quoted_field_with_comma() {
        let csv = "ref,title,author\n\
                   user/k1,\"My, kernel\",user\n";
        let items = parse_kernel_list(csv).expect("must respect quoted commas");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].slug_ref, "user/k1");
        assert_eq!(items[0].title.as_deref(), Some("My, kernel"));
    }

    #[test]
    fn parse_kernel_list_csv_with_kaggle_warning_prefix() {
        // The 1.8.3 outdated-version banner often lands on top of CSV output.
        let csv = "Warning: Looks like you're using an outdated `kaggle`` version \
                   (installed: {current_version}), please consider upgrading...\n\
                   ref,title,author\n\
                   user/k1,Title,user\n";
        let items = parse_kernel_list(csv).expect("warning prefix must not break CSV parse");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].slug_ref, "user/k1");
    }

    #[test]
    fn parse_kernel_list_still_accepts_json() {
        // Backward compat: the existing test mocks feed JSON.
        let json =
            r#"[{"ref":"user/k1","title":"K1","status":"running","totalRunningTimeSec":42}]"#;
        let items = parse_kernel_list(json).expect("JSON path must still work");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].status.as_deref(), Some("running"));
        assert_eq!(items[0].run_seconds, Some(42));
    }
}
