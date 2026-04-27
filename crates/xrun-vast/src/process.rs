#![deny(unsafe_code)]

use std::sync::OnceLock;
use std::time::Duration;

use tokio::process::Command;
use tokio::sync::Semaphore;
use tokio::time::sleep;

use crate::error::VastError;

static VASTAI_SEMAPHORE: OnceLock<Semaphore> = OnceLock::new();

fn semaphore() -> &'static Semaphore {
    VASTAI_SEMAPHORE.get_or_init(|| Semaphore::new(4))
}

async fn run_vastai_inner(args: Vec<String>) -> Result<Vec<u8>, VastError> {
    let _permit = semaphore().acquire().await.expect("semaphore never closes");

    let output = Command::new("vastai")
        .args(&args)
        .output()
        .await
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                VastError::NotFound("vastai binary not found in PATH".to_string())
            } else {
                VastError::Io(e)
            }
        })?;

    if output.status.success() {
        Ok(output.stdout)
    } else {
        Err(VastError::CliFailure {
            exit_code: output.status.code().unwrap_or(-1),
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        })
    }
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
