#![deny(unsafe_code)]

//! Pure command-string builders. Kept free of side effects so they can be
//! unit-tested without spawning subprocesses.

use std::path::PathBuf;

/// Resolved connection info for one host alias. Consumers in `ssh.rs` turn
/// this into the actual ssh/rsync argv.
#[derive(Debug, Clone)]
pub struct SshConn {
    pub alias: String,
    pub host: String,
    pub user: String,
    pub port: u16,
    pub key: Option<PathBuf>,
}

impl SshConn {
    /// `-p <port> -o BatchMode=yes -o StrictHostKeyChecking=no [-i <key>]`
    /// — the leading argv prepended to every `ssh` call. BatchMode disables
    /// any password / passphrase prompt so a missing key fails loudly instead
    /// of hanging.
    pub fn ssh_options(&self) -> Vec<String> {
        let mut out = vec![
            "-p".to_string(),
            self.port.to_string(),
            "-o".to_string(),
            "BatchMode=yes".to_string(),
            "-o".to_string(),
            "StrictHostKeyChecking=no".to_string(),
        ];
        if let Some(k) = self.key.as_deref() {
            out.push("-i".to_string());
            out.push(k.display().to_string());
        }
        out
    }

    /// `user@host` for ssh / rsync.
    pub fn target(&self) -> String {
        format!("{}@{}", self.user, self.host)
    }

    /// `-e "ssh -p N -o BatchMode=yes ..."` argument for rsync.
    pub fn rsync_e_arg(&self) -> String {
        let mut parts = vec!["ssh".to_string()];
        parts.extend(self.ssh_options());
        parts.join(" ")
    }
}

/// Argv for `ssh <opts> user@host -- <remote_cmd>`.
pub fn ssh_argv(conn: &SshConn, remote_cmd: &str) -> Vec<String> {
    let mut argv = conn.ssh_options();
    argv.push(conn.target());
    argv.push("--".to_string());
    argv.push(remote_cmd.to_string());
    argv
}

/// Argv for `rsync -avz --partial -e "<ssh ...>" <src> user@host:<dst>`
/// (upload direction).
pub fn rsync_upload_argv(conn: &SshConn, src: &str, remote_dst: &str) -> Vec<String> {
    vec![
        "-avz".to_string(),
        "--partial".to_string(),
        "-e".to_string(),
        conn.rsync_e_arg(),
        src.to_string(),
        format!("{}:{remote_dst}", conn.target()),
    ]
}

/// Argv for `rsync -avz -e "<ssh ...>" user@host:<remote_pattern> <local_into>/`.
pub fn rsync_download_argv(conn: &SshConn, remote_pattern: &str, local_into: &str) -> Vec<String> {
    vec![
        "-avz".to_string(),
        "-e".to_string(),
        conn.rsync_e_arg(),
        format!("{}:{remote_pattern}", conn.target()),
        local_into.to_string(),
    ]
}

/// Build the remote shell snippet that backgrounds the user command, redirects
/// stdout/stderr to a per-run log file, and writes the PID to `<run_dir>/run.pid`.
/// Equivalent to the `nohup … & echo $! > pid` pattern xrun-vast already uses.
pub fn remote_launch_script(run_dir: &str, user_cmd: &str) -> String {
    let mkdir = shell_quote(run_dir);
    let log = shell_quote(&format!("{run_dir}/stdout.log"));
    let pid = shell_quote(&format!("{run_dir}/run.pid"));
    let escaped_cmd = user_cmd.replace('\'', "'\\''");
    format!(
        "set -e; mkdir -p {mkdir}; \
         (nohup bash -c '{escaped_cmd}' >{log} 2>&1 & echo $!) >{pid}"
    )
}

/// Build the ssh-side `wc -c < <file>` probe for the tail offset logic.
pub fn remote_size_script(file: &str) -> String {
    format!("wc -c < {f} 2>/dev/null || echo 0", f = shell_quote(file))
}

/// Build the ssh-side `tail -c +N <file>` snippet (1-indexed byte offset).
pub fn remote_tail_script(file: &str, offset: u64) -> String {
    format!(
        "tail -c +{} {f}",
        offset.saturating_add(1),
        f = shell_quote(file)
    )
}

/// Single-quote a string for embedding in a `bash -c '...'` shell line. Closes
/// and re-opens the quote around any literal single quote in the input.
pub fn shell_quote(s: &str) -> String {
    let escaped = s.replace('\'', "'\\''");
    format!("'{escaped}'")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn conn() -> SshConn {
        SshConn {
            alias: "ws".to_string(),
            host: "192.168.1.10".to_string(),
            user: "ubuntu".to_string(),
            port: 2222,
            key: Some(PathBuf::from("/home/me/.ssh/id_ed25519")),
        }
    }

    #[test]
    fn ssh_options_include_port_batch_and_key() {
        let c = conn();
        let o = c.ssh_options();
        assert!(o.contains(&"2222".to_string()));
        assert!(o.iter().any(|s| s == "BatchMode=yes"));
        assert!(o.iter().any(|s| s == "/home/me/.ssh/id_ed25519"));
    }

    #[test]
    fn ssh_argv_appends_dashdash_and_command() {
        let argv = ssh_argv(&conn(), "uname -a");
        assert_eq!(argv.last().unwrap(), "uname -a");
        assert!(argv.iter().any(|s| s == "--"));
        assert!(argv.iter().any(|s| s == "ubuntu@192.168.1.10"));
    }

    #[test]
    fn rsync_upload_uses_e_arg_with_ssh() {
        let argv = rsync_upload_argv(&conn(), "/local/src/", "/remote/dst/");
        assert!(argv.contains(&"-avz".to_string()));
        let e_idx = argv.iter().position(|s| s == "-e").expect("has -e");
        assert!(argv[e_idx + 1].starts_with("ssh "));
        assert!(argv.last().unwrap().starts_with("ubuntu@192.168.1.10:"));
    }

    #[test]
    fn shell_quote_escapes_single_quote() {
        let q = shell_quote("it's me");
        assert_eq!(q, r"'it'\''s me'");
    }

    #[test]
    fn remote_launch_script_redirects_and_records_pid() {
        let s = remote_launch_script("/tmp/xrun/abc", "python train.py --lr 5e-4");
        assert!(s.contains("nohup bash -c"), "script: {s}");
        assert!(s.contains("'/tmp/xrun/abc/stdout.log'"), "script: {s}");
        assert!(s.contains("'/tmp/xrun/abc/run.pid'"), "script: {s}");
        assert!(s.contains("echo $!"));
    }

    #[test]
    fn remote_tail_script_offsets_one_indexed() {
        assert_eq!(remote_tail_script("/tmp/file", 0), "tail -c +1 '/tmp/file'");
        assert_eq!(
            remote_tail_script("/tmp/file", 100),
            "tail -c +101 '/tmp/file'"
        );
    }
}
