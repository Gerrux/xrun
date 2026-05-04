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

/// Spawn `cmd`, wait up to `timeout` for it to finish, and on timeout kill
/// the child and return a clear error. stdout/stderr are captured.
fn run_with_timeout(
    mut cmd: std::process::Command,
    timeout: std::time::Duration,
    label: &str,
) -> Result<std::process::Output, KaggleError> {
    cmd.stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut child = cmd
        .spawn()
        .map_err(|e| KaggleError::NotFound(format!("spawn {label}: {e}")))?;

    let start = std::time::Instant::now();
    let poll = std::time::Duration::from_millis(200);
    loop {
        match child
            .try_wait()
            .map_err(|e| KaggleError::Other(format!("wait {label}: {e}")))?
        {
            Some(_status) => break,
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
    }
    child
        .wait_with_output()
        .map_err(|e| KaggleError::Other(format!("collect {label}: {e}")))
}

impl KaggleProcess for KaggleProcessReal {
    fn push(&self, local_dir: &Path) -> Result<String, KaggleError> {
        let mut cmd = self.cmd(&["kernels", "push", "-p"]);
        cmd.arg(local_dir);
        let output = run_with_timeout(cmd, push_timeout(), "kaggle kernels push")?;

        if !output.status.success() {
            return Err(KaggleError::CliFailure {
                exit_code: output.status.code().unwrap_or(-1),
                stderr: format!(
                    "stdout: {}\nstderr: {}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                ),
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
        let out = self
            .cmd(&["kernels", "output", slug, "-p"])
            .arg(into_dir)
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
        let out = self
            .cmd(&["kernels", "list", "--mine", "-m"])
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

fn parse_push_slug(stdout: &str) -> Result<KernelSlug, KaggleError> {
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
    let trimmed = stdout.trim();
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
    let trimmed = stdout.trim();
    if trimmed.is_empty() || trimmed == "[]" {
        return Ok(vec![]);
    }
    serde_json::from_str(trimmed).map_err(|e| {
        KaggleError::ParseError(format!(
            "failed to parse kernel list JSON: {e}\nInput: {trimmed}"
        ))
    })
}

fn parse_username(stdout: &str) -> Result<String, KaggleError> {
    let trimmed = stdout.trim();
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
    let trimmed = stdout.trim().to_lowercase();
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
    let trimmed = stdout.trim();
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
