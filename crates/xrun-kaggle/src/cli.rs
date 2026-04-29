#![deny(unsafe_code)]

use std::path::Path;

use serde::Deserialize;

use crate::error::KaggleError;

pub type KernelSlug = String;

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
}

/// Real implementation using the `kaggle` binary from PATH.
pub struct KaggleProcessReal;

impl KaggleProcess for KaggleProcessReal {
    fn push(&self, local_dir: &Path) -> Result<String, KaggleError> {
        let output = std::process::Command::new("kaggle")
            .args(["kernels", "push", "-p"])
            .arg(local_dir)
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

    fn status(&self, slug: &str) -> Result<String, KaggleError> {
        let output = std::process::Command::new("kaggle")
            .args(["kernels", "status", slug, "-m"])
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
        let out = std::process::Command::new("kaggle")
            .args(["kernels", "output", slug, "-p"])
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
}

pub struct KaggleCli {
    process: Box<dyn KaggleProcess>,
}

impl KaggleCli {
    pub fn new() -> Self {
        Self {
            process: Box::new(KaggleProcessReal),
        }
    }

    pub fn with_process(process: Box<dyn KaggleProcess>) -> Self {
        Self { process }
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
    // Look for pattern like "username/kernelname"
    let parts: Vec<&str> = line.split_whitespace().collect();
    for part in parts {
        if part.contains('/') && !part.starts_with("http") {
            return Some(part.trim_matches('"').to_string());
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
