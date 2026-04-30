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
        cmd
    }
}

impl Default for KaggleProcessReal {
    fn default() -> Self {
        Self::new()
    }
}

impl KaggleProcess for KaggleProcessReal {
    fn push(&self, local_dir: &Path) -> Result<String, KaggleError> {
        let output = self
            .cmd(&["kernels", "push", "-p"])
            .arg(local_dir)
            .output()
            .map_err(|e| KaggleError::NotFound(e.to_string()))?;

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
        let output = self
            .cmd(&["kernels", "status", slug, "-m"])
            .output()
            .map_err(|e| KaggleError::NotFound(e.to_string()))?;

        if !output.status.success() {
            return Err(KaggleError::CliFailure {
                exit_code: output.status.code().unwrap_or(-1),
                stderr: String::from_utf8_lossy(&output.stderr).to_string(),
            });
        }
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
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
        let out = self
            .cmd(&["datasets", "status", slug, "-m"])
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
        let out = self
            .cmd(&["datasets", "create", "-p"])
            .arg(local_dir)
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

    fn datasets_version(&self, local_dir: &Path, message: &str) -> Result<String, KaggleError> {
        let out = self
            .cmd(&["datasets", "version", "-p"])
            .arg(local_dir)
            .arg("-m")
            .arg(message)
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
    /// `kaggle datasets version`.
    pub fn dataset_push(
        &self,
        local_dir: &Path,
        slug: &str,
        message: Option<&str>,
    ) -> Result<(), KaggleError> {
        ensure_dataset_metadata(local_dir, slug)?;
        let msg = message.unwrap_or("Updated via xrun");

        match self.process.datasets_create(local_dir) {
            Ok(_) => Ok(()),
            Err(KaggleError::CliFailure { ref stderr, .. })
                if stderr.to_lowercase().contains("already") =>
            {
                self.process.datasets_version(local_dir, msg)?;
                Ok(())
            }
            Err(e) => Err(e),
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
    // kaggle kernels status -m returns JSON
    let trimmed = stdout.trim();
    if trimmed.is_empty() {
        return Err(KaggleError::ParseError("empty status output".to_string()));
    }
    serde_json::from_str(trimmed).map_err(|e| {
        KaggleError::ParseError(format!(
            "failed to parse kernel status JSON: {e}\nInput: {trimmed}"
        ))
    })
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
    // Plain-text format: "username: kartaviychert"
    for line in trimmed.lines() {
        if let Some(rest) = line.strip_prefix("username:") {
            return Ok(rest.trim().to_string());
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
    let title = slug.split('/').last().unwrap_or(slug).replace('-', " ");
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
