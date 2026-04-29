#![deny(unsafe_code)]

use std::path::Path;

use anyhow::{bail, Context, Result};
use xrun_core::Credentials;
use xrun_vast::transfer::{resolve_ssh, transfer, SshConn, TransferEndpoint};

#[derive(Debug, clap::Args)]
pub struct CpArgs {
    /// Source: INSTANCE_ID:/remote/path  or  /local/path
    pub src: String,
    /// Destination: INSTANCE_ID:/remote/path  or  /local/path
    pub dst: String,
    /// Print each file as it is transferred (passes -v to tar)
    #[arg(short, long)]
    pub verbose: bool,
}

pub fn run(args: &CpArgs, config_dir: &Path) -> Result<()> {
    let api_key = load_api_key(config_dir)?;

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("failed to build tokio runtime")?;

    eprintln!("resolving instances…");
    let src = rt.block_on(resolve_endpoint(&args.src, &api_key))?;
    let dst = rt.block_on(resolve_endpoint(&args.dst, &api_key))?;

    eprintln!("transferring {} → {}", args.src, args.dst);

    transfer(&src, &dst, args.verbose).context("transfer failed")?;

    eprintln!("done.");
    Ok(())
}

fn load_api_key(config_dir: &Path) -> Result<String> {
    if let Ok(creds) = Credentials::load(config_dir) {
        if let Some(key) = creds.vast.api_key {
            return Ok(key);
        }
    }
    if let Ok(Some(key)) = Credentials::import_vast_native() {
        return Ok(key);
    }
    bail!(
        "vast API key not found — run `xrun config set vast.api_key <KEY>` \
         or `vastai set api-key <KEY>`"
    )
}

enum RawEndpoint {
    Local(String),
    Remote { id: u64, path: String },
}

/// Parse "INSTANCE_ID:PATH" as remote, anything else as local.
fn parse_endpoint(s: &str) -> Result<RawEndpoint> {
    if let Some(colon) = s.find(':') {
        let prefix = &s[..colon];
        // Windows drive letters are single chars — skip them ("C:/" etc.)
        if prefix.len() > 1 {
            if let Ok(id) = prefix.parse::<u64>() {
                return Ok(RawEndpoint::Remote {
                    id,
                    path: s[colon + 1..].to_string(),
                });
            }
        }
    }
    Ok(RawEndpoint::Local(s.to_string()))
}

async fn resolve_endpoint(s: &str, api_key: &str) -> Result<TransferEndpoint> {
    match parse_endpoint(s)? {
        RawEndpoint::Local(path) => Ok(TransferEndpoint::Local(path)),
        RawEndpoint::Remote { id, path } => {
            let conn: SshConn = resolve_ssh(id, api_key)
                .await
                .with_context(|| format!("failed to resolve SSH for instance {id}"))?;
            Ok(TransferEndpoint::Remote(conn, path))
        }
    }
}
