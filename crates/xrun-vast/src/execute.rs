#![deny(unsafe_code)]

use std::collections::HashMap;

use xrun_core::manifest::RunSpec;

use crate::{cli::InstanceId, error::VastError};

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Render RunSpec.args into a command-line string sorted alphabetically by key.
/// - bool true  → "--key" (bare flag, no value)
/// - bool false → omitted
/// - other      → "--key value" (string values are shell-quoted)
pub fn render_args(args: &HashMap<String, serde_json::Value>) -> String {
    let mut sorted: Vec<(&String, &serde_json::Value)> = args.iter().collect();
    sorted.sort_by_key(|(k, _)| k.as_str());

    let mut parts = Vec::new();
    for (key, val) in sorted {
        match val {
            serde_json::Value::Bool(true) => {
                parts.push(key.clone());
            }
            serde_json::Value::Bool(false) => {}
            serde_json::Value::Number(n) => {
                parts.push(key.clone());
                parts.push(n.to_string());
            }
            serde_json::Value::String(s) => {
                parts.push(key.clone());
                parts.push(shell_quote(s));
            }
            other => {
                parts.push(key.clone());
                parts.push(shell_quote(&other.to_string()));
            }
        }
    }
    parts.join(" ")
}

/// Build the nohup background command to launch the run on the remote instance.
/// Returns a shell command string that launches the training in the background
/// and echoes the PID so the caller can track it.
pub fn build_launch_command(run_spec: &RunSpec) -> String {
    let workdir = run_spec.workdir.as_deref().unwrap_or("/workspace");
    let cmd = run_spec
        .cmd
        .as_deref()
        .unwrap_or("echo 'no command specified'");
    let args_str = run_spec
        .args
        .as_ref()
        .filter(|a| !a.is_empty())
        .map(render_args)
        .unwrap_or_default();

    let main_cmd = if args_str.is_empty() {
        format!(
            "cd {} && XRUN_RUN_DIR=/workspace/run {} 2>&1 | tee /workspace/run/stdout.log",
            workdir, cmd
        )
    } else {
        format!(
            "cd {} && XRUN_RUN_DIR=/workspace/run {} {} 2>&1 | tee /workspace/run/stdout.log",
            workdir, cmd, args_str
        )
    };

    // Embed in nohup so vastai execute returns immediately with the background PID.
    // Single-quote escape: ' → '\''
    let escaped = main_cmd.replace('\'', "'\\''");
    format!(
        "mkdir -p /workspace/run && nohup sh -c '{}' >/dev/null 2>&1 & echo $!",
        escaped
    )
}

/// Run the setup command synchronously, then launch the main command in the background.
/// Returns the PID of the background process, if parseable from stdout.
pub(crate) async fn launch_run(
    instance_id: InstanceId,
    run_spec: &RunSpec,
) -> Result<Option<u64>, VastError> {
    if let Some(setup) = &run_spec.setup {
        let setup_cmd = format!(
            "mkdir -p /workspace/run && export XRUN_RUN_DIR=/workspace/run && ({})",
            setup
        );
        crate::cli::execute(instance_id, &setup_cmd).await?;
    }

    let launch_cmd = build_launch_command(run_spec);
    let out = crate::cli::execute(instance_id, &launch_cmd).await?;
    let pid: Option<u64> = String::from_utf8_lossy(&out).trim().parse().ok();
    Ok(pid)
}
