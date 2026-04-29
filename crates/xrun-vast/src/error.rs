#![deny(unsafe_code)]

use thiserror::Error;

#[derive(Debug, Error)]
pub enum VastError {
    #[error("vastai binary not found in PATH: {0}")]
    NotFound(String),

    #[error("vastai CLI failed (exit {exit_code}): {stderr}")]
    CliFailure { exit_code: i32, stderr: String },

    #[error("failed to parse vastai output: {0}")]
    ParseError(String),

    #[error("no offers available matching query — REST POST /bundles/ returned 0 rows.\n\
             query body: {0}\n\
             hint: try `vastai search offers '<filter>'` from the CLI to narrow it down; \
             default filters include verified=true, rentable=true, external=false, rented=false")]
    NoOffersAvailable(String),

    #[error("price cap too low: cheapest ${cheapest:.4}/h, cap ${cap:.4}/h")]
    PriceCapTooLow { cheapest: f64, cap: f64 },

    #[error("instance provision timed out")]
    InstanceLossOnProvision,

    #[error("file truncated on instance: {file} was {was} bytes, now {now}")]
    FileTruncated { file: String, was: u64, now: u64 },

    #[error("rsync not found in PATH")]
    RsyncNotFound,

    #[error("already polling this run")]
    AlreadyPolling,

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}
