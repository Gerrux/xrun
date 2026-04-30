#![deny(unsafe_code)]

use std::sync::{OnceLock, RwLock};
use std::time::Duration;

use tokio::process::Command;
use tokio::sync::Semaphore;
use tokio::time::sleep;

use crate::error::VastError;

static VASTAI_SEMAPHORE: OnceLock<Semaphore> = OnceLock::new();

fn semaphore() -> &'static Semaphore {
    VASTAI_SEMAPHORE.get_or_init(|| Semaphore::new(4))
}

/// Process-wide override for the vastai api key. Set by adapters that hold
/// fresh credentials; `run_vastai_inner` injects it as `--api-key` so vastai
/// doesn't fall back to its native ~/.config/vastai/vast_api_key file.
static API_KEY_OVERRIDE: OnceLock<RwLock<Option<String>>> = OnceLock::new();

fn api_key_slot() -> &'static RwLock<Option<String>> {
    API_KEY_OVERRIDE.get_or_init(|| RwLock::new(None))
}

pub fn set_api_key_override(key: Option<String>) {
    if let Ok(mut g) = api_key_slot().write() {
        *g = key;
    }
}

fn current_api_key() -> Option<String> {
    api_key_slot().read().ok().and_then(|g| g.clone())
}

/// User-visible label for a vastai invocation: the caller's args minus
/// `--raw`, which is just rendering noise. Caller args do not contain
/// `--api-key`; that is prepended later from the process-wide override.
fn cmd_label(args: &[String]) -> String {
    args.iter()
        .filter(|a| a.as_str() != "--raw")
        .cloned()
        .collect::<Vec<_>>()
        .join(" ")
}

async fn run_vastai_inner(args: Vec<String>) -> Result<Vec<u8>, VastError> {
    let _permit = semaphore().acquire().await.expect("semaphore never closes");

    let mut full_args: Vec<String> = Vec::with_capacity(args.len() + 2);
    if let Some(key) = current_api_key() {
        full_args.push("--api-key".to_string());
        full_args.push(key);
    }
    full_args.extend(args.clone());

    let label = cmd_label(&args);

    let mut vastai_cmd = Command::new("vastai");
    vastai_cmd.args(&full_args);
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        vastai_cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let output = vastai_cmd
        .output()
        .await
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                VastError::NotFound("vastai binary not found in PATH".to_string())
            } else {
                VastError::Io(e)
            }
        })?;

    if !output.status.success() {
        let stderr_text = String::from_utf8_lossy(&output.stderr).into_owned();
        let stdout_text = String::from_utf8_lossy(&output.stdout).into_owned();
        // Prefer stderr for the message, fall back to stdout if stderr is empty.
        let msg = if stderr_text.trim().is_empty() {
            stdout_text.trim().to_string()
        } else {
            stderr_text.trim().to_string()
        };
        return Err(VastError::CliFailure {
            exit_code: output.status.code().unwrap_or(-1),
            stderr: format!("vastai {} → {}", label, msg),
        });
    }

    // vastai often prints in-band errors with a clean exit code, notably for
    // auth problems ("failed with error 403: This action requires login.") and
    // for backend regressions that surface as 4xx ("failed with error 400:
    // owner: Extra inputs are not permitted"). Some versions write to stdout,
    // others to stderr — check both. Surface as CliFailure so callers don't
    // try to parse the message as JSON.
    if let Some(msg) =
        detect_inline_error(&output.stdout).or_else(|| detect_inline_error(&output.stderr))
    {
        return Err(VastError::CliFailure {
            exit_code: 0,
            stderr: format!("vastai {} → {}", label, msg),
        });
    }

    // Empty stdout with a clean exit code usually means vastai printed its
    // diagnostic on stderr and silently moved on. Surface that text instead of
    // returning empty bytes that would later fail JSON parsing with the
    // unhelpful "expected value at line 1 column 1".
    if output.stdout.iter().all(|b| b.is_ascii_whitespace()) && !output.stderr.is_empty() {
        let stderr_text = String::from_utf8_lossy(&output.stderr).trim().to_string();
        if !stderr_text.is_empty() {
            return Err(VastError::CliFailure {
                exit_code: 0,
                stderr: format!("vastai {} → {}", label, stderr_text),
            });
        }
    }

    Ok(output.stdout)
}

fn detect_inline_error(buf: &[u8]) -> Option<String> {
    let head = String::from_utf8_lossy(buf);
    let trimmed = head.trim_start();
    let lower = trimmed.to_ascii_lowercase();
    // Match any "failed with error <N>:" (covers 400/401/403/500/…), plus a
    // few other well-known phrases the legacy CLI uses.
    let looks_like_error = lower.starts_with("failed with error")
        || lower.starts_with("error:")
        || lower.contains("requires login")
        || lower.contains("traceback (most recent call last)");
    if looks_like_error {
        return Some(trimmed.lines().next().unwrap_or("").trim().to_string());
    }
    None
}

pub async fn run_vastai(args: &[&str]) -> Result<Vec<u8>, VastError> {
    run_vastai_inner(args.iter().map(|s| s.to_string()).collect()).await
}

#[derive(Debug, Clone)]
pub struct RetryPolicy {
    pub max_attempts: u32,
    pub base_delay_ms: u64,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 4,
            base_delay_ms: 1000,
        }
    }
}

/// Retry an async operation with exponential backoff.
///
/// Attempts are made up to `policy.max_attempts` times.
/// Delays: base_delay_ms, 2×, 4×, ...
pub async fn retry_op<F, Fut, T>(policy: &RetryPolicy, mut f: F) -> Result<T, VastError>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, VastError>>,
{
    let mut attempt = 0u32;
    let mut delay = Duration::from_millis(policy.base_delay_ms);
    loop {
        match f().await {
            Ok(out) => return Ok(out),
            Err(e) => {
                attempt += 1;
                if attempt >= policy.max_attempts {
                    return Err(e);
                }
                sleep(delay).await;
                delay = delay.saturating_mul(2);
            }
        }
    }
}

pub async fn run_vastai_with_retry(
    args: &[&str],
    policy: &RetryPolicy,
) -> Result<Vec<u8>, VastError> {
    let owned: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    retry_op(policy, || run_vastai_inner(owned.clone())).await
}
