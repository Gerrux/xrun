#![deny(unsafe_code)]

//! Pure shell-string builders for the commands the adapter sends to a Studio
//! through `Studio.run_with_exit_code`. No side effects, unit-tested.
//!
//! Paths handed in are home-relative (`xrun/<run_id>`) or absolute; they are
//! turned into shell words with `absolute_shell_path`, so they stay correct
//! after a `cd` (the Studio's default cwd is not guaranteed to be `$HOME`).

use base64::Engine;
use xrun_core::manifest::Manifest;
use xrun_ssh::absolute_shell_path;
use xrun_ssh::cmd::shell_quote;

/// Longest slice of a log returned by one tail call. The slice travels as
/// base64 through the command-output API (a text transport): a truncated
/// chunk would break decoding forever, so it is kept small (512 KiB); the
/// poller simply takes the next chunk on the following tick.
pub const TAIL_CAP_BYTES: u64 = 512 * 1024;

/// Studio name for a manifest: `lightning.studio` (lowercased), else
/// `xrun-<manifest.name>` reduced to `[a-z0-9-]`, at most 40 chars.
pub fn studio_name_for(manifest: &Manifest) -> String {
    if let Some(s) = manifest
        .lightning
        .as_ref()
        .and_then(|l| l.studio.as_deref())
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        return s.to_lowercase();
    }
    sanitize_studio_name(&format!("xrun-{}", manifest.name))
}

pub fn sanitize_studio_name(raw: &str) -> String {
    let mut out = String::new();
    for c in raw.to_lowercase().chars() {
        let c = if c.is_ascii_lowercase() || c.is_ascii_digit() {
            c
        } else {
            '-'
        };
        if c == '-' && out.ends_with('-') {
            continue;
        }
        out.push(c);
    }
    let cut: String = out.chars().take(40).collect();
    let name = cut.trim_matches('-').to_string();
    if name.is_empty() {
        "xrun".to_string()
    } else {
        name
    }
}

/// `export XRUN_RUN_ID=… XRUN_RUN_DIR=… PYTHONUNBUFFERED=… [CUDA_VISIBLE_DEVICES=…] && `.
/// `export … &&` (not an inline prefix) so every command of a chained
/// `run.cmd` sees the variables. `XRUN_RUN_DIR` is absolute (`"$HOME"/…`)
/// because the command runs after `cd <workdir>`.
pub fn env_exports(run_id: &str, run_dir: &str, gpu: Option<&str>) -> String {
    let mut parts = vec![
        format!("XRUN_RUN_ID={}", shell_quote(run_id)),
        format!("XRUN_RUN_DIR={}", absolute_shell_path(run_dir)),
        "PYTHONUNBUFFERED=\"${PYTHONUNBUFFERED:-1}\"".to_string(),
    ];
    if let Some(gpu) = gpu {
        match gpu {
            "auto" | "" => {}
            "cpu" => parts.push("CUDA_VISIBLE_DEVICES=".to_string()),
            other => {
                let stripped = other.strip_prefix("cuda:").unwrap_or(other);
                parts.push(format!("CUDA_VISIBLE_DEVICES={}", shell_quote(stripped)));
            }
        }
    }
    format!("export {} && ", parts.join(" "))
}

pub fn mkdir_script(dir: &str) -> String {
    format!("mkdir -p {}", absolute_shell_path(dir))
}

/// `run.setup`, synchronously, inside the training directory.
pub fn setup_script(workdir: &str, setup: &str) -> String {
    let wd = absolute_shell_path(workdir);
    format!("mkdir -p {wd} && cd {wd} && ({setup})")
}

/// Detached launch of the training command. The inner `bash` writes its own
/// pid to `run.pid` (`$$`), so the pid is right whether or not `setsid`
/// forks; `setsid` (when present) makes the run a session leader, so
/// [`kill_script`] can signal the whole group. Prints the pid.
pub fn launch_script(run_dir: &str, workdir: &str, env: &str, user_cmd: &str) -> String {
    let rd = absolute_shell_path(run_dir);
    let pid = absolute_shell_path(&format!("{run_dir}/run.pid"));
    let log = absolute_shell_path(&format!("{run_dir}/stdout.log"));
    let wd = absolute_shell_path(workdir);
    let inner = shell_quote(&format!("echo $$ > {pid}; cd {wd} && {env}{user_cmd}"));
    format!(
        "mkdir -p {rd}; rm -f {pid}; \
         if command -v setsid >/dev/null 2>&1; then S=setsid; else S=; fi; \
         $S nohup bash -c {inner} >{log} 2>&1 </dev/null & \
         i=0; while [ ! -s {pid} ] && [ $i -lt 10 ]; do sleep 1; i=$((i+1)); done; \
         cat {pid}"
    )
}

/// Exit 0 alive, 1 dead, 2 no pid file.
pub fn alive_script(run_dir: &str) -> String {
    let pf = absolute_shell_path(&format!("{run_dir}/run.pid"));
    format!(
        "[ -s {pf} ] || exit 2; PID=$(cat {pf}); \
         case \"$PID\" in ''|*[!0-9]*) exit 2;; esac; \
         if kill -0 \"$PID\" 2>/dev/null; then exit 0; else exit 1; fi"
    )
}

/// TERM then KILL of the recorded pid (group first: `setsid` made it a leader).
/// Exit 0 when nothing is left running or there is no pid file, 1 otherwise.
pub fn kill_script(run_dir: &str) -> String {
    let pf = absolute_shell_path(&format!("{run_dir}/run.pid"));
    format!(
        "[ -f {pf} ] || exit 0; PID=$(cat {pf}); \
         case \"$PID\" in ''|*[!0-9]*) exit 1;; esac; \
         [ \"$PID\" -gt 1 ] || exit 1; \
         kill -TERM -- \"-$PID\" 2>/dev/null; kill -TERM \"$PID\" 2>/dev/null; sleep 1; \
         if kill -0 \"$PID\" 2>/dev/null; then \
         kill -KILL -- \"-$PID\" 2>/dev/null; kill -KILL \"$PID\" 2>/dev/null; sleep 1; fi; \
         if kill -0 \"$PID\" 2>/dev/null; then exit 1; fi; rm -f {pf}"
    )
}

/// First line: file size in bytes (0 when missing). Second line (only when
/// the file is longer than `offset`): at most [`TAIL_CAP_BYTES`] bytes from
/// byte `offset` on, base64 without line breaks (works with and without
/// `base64 -w0`).
pub fn tail_script(file: &str, offset: u64) -> String {
    format!(
        "f={f}; sz=$(wc -c < \"$f\" 2>/dev/null || echo 0); echo \"$sz\"; \
         if [ \"$sz\" -gt {offset} ]; then \
         tail -c +{start} \"$f\" 2>/dev/null | head -c {cap} | base64 | tr -d '\\n'; echo; fi",
        f = absolute_shell_path(file),
        start = offset.saturating_add(1),
        cap = TAIL_CAP_BYTES
    )
}

/// Parse [`tail_script`] output into `(size, bytes)`. Lines that precede the
/// size line (login-shell chatter that is not a number) are skipped.
pub fn parse_tail_output(out: &str) -> Result<(u64, Vec<u8>), String> {
    let mut lines = out.lines();
    let size = lines
        .by_ref()
        .find_map(|l| l.trim().parse::<u64>().ok())
        .ok_or_else(|| format!("no size line in tail output: {out:?}"))?;
    let b64: String = lines
        .flat_map(|l| l.split_whitespace())
        .collect::<Vec<_>>()
        .concat();
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(b64.as_bytes())
        .map_err(|e| format!("bad base64 in tail output: {e}"))?;
    Ok((size, bytes))
}

/// A glob as one shell word: glob metacharacters (`* ? [ ]`) stay live,
/// everything else is single-quoted (spaces, `$`, quotes are literal).
pub fn glob_word(pattern: &str) -> String {
    let mut out = String::new();
    let mut lit = String::new();
    for c in pattern.chars() {
        if matches!(c, '*' | '?' | '[' | ']') {
            if !lit.is_empty() {
                out.push_str(&shell_quote(&lit));
                lit.clear();
            }
            out.push(c);
        } else {
            lit.push(c);
        }
    }
    if !lit.is_empty() {
        out.push_str(&shell_quote(&lit));
    }
    out
}

/// Expand `pattern` (home-relative or absolute) with `globstar`, print the
/// matching regular files one per line, home-relative when under `$HOME`.
pub fn pull_script(pattern: &str) -> String {
    let body = format!(
        "shopt -s globstar nullglob; cd \"$HOME\" || exit 1; \
         for f in {g}; do [ -f \"$f\" ] && echo \"${{f#\"$HOME\"/}}\"; done; true",
        g = glob_word(pattern)
    );
    format!("bash -c {}", shell_quote(&body))
}

/// Lines of `pull_script` output that look like paths (drops shell chatter).
pub fn parse_pull_output(out: &str) -> Vec<String> {
    out.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(yaml_extra: &str, name: &str) -> Manifest {
        let y = format!("name: {name}\nvendor: lightning\n{yaml_extra}run:\n  cmd: echo hi\n");
        Manifest::from_yaml_str(&y).expect("parse")
    }

    #[test]
    fn studio_name_sanitized_and_prefixed() {
        assert_eq!(
            studio_name_for(&manifest("", "my_exp-v2")),
            "xrun-my-exp-v2"
        );
    }

    #[test]
    fn studio_name_truncated_to_40() {
        let n = studio_name_for(&manifest("", &"a".repeat(80)));
        assert_eq!(n.len(), 40);
        assert!(n.starts_with("xrun-aaa"));
        assert!(!n.ends_with('-'));
    }

    #[test]
    fn studio_name_explicit_wins() {
        let m = manifest("lightning:\n  studio: my-studio\n", "ignored");
        assert_eq!(studio_name_for(&m), "my-studio");
    }

    #[test]
    fn studio_name_explicit_is_lowercased() {
        let m = manifest("lightning:\n  studio: My-Studio\n", "ignored");
        assert_eq!(studio_name_for(&m), "my-studio");
    }

    #[test]
    fn sanitize_collapses_and_falls_back() {
        assert_eq!(sanitize_studio_name("A  b--C"), "a-b-c");
        assert_eq!(sanitize_studio_name("!!!"), "xrun");
    }

    #[test]
    fn env_run_dir_is_absolute_home_prefixed() {
        let e = env_exports("R1", "xrun/R1", None);
        assert!(e.contains("XRUN_RUN_DIR=\"$HOME\"/'xrun/R1'"), "{e}");
        assert!(e.contains("XRUN_RUN_ID='R1'"));
        assert!(e.ends_with(" && "));
        assert!(!e.contains("CUDA_VISIBLE_DEVICES"));
        let e = env_exports("R1", "/abs/R1", None);
        assert!(e.contains("XRUN_RUN_DIR='/abs/R1'"), "{e}");
    }

    #[test]
    fn env_cuda_hints_match_ssh() {
        assert!(!env_exports("R", "x/R", Some("auto")).contains("CUDA"));
        assert!(env_exports("R", "x/R", Some("cpu")).contains("CUDA_VISIBLE_DEVICES= "));
        assert!(env_exports("R", "x/R", Some("cuda:0")).contains("CUDA_VISIBLE_DEVICES='0'"));
        assert!(env_exports("R", "x/R", Some("1,2")).contains("CUDA_VISIBLE_DEVICES='1,2'"));
    }

    #[test]
    fn env_unbuffers_python_by_default() {
        assert!(
            env_exports("R", "x/R", None).contains("PYTHONUNBUFFERED=\"${PYTHONUNBUFFERED:-1}\"")
        );
    }

    #[test]
    fn launch_script_shape_and_quoting() {
        let env = env_exports("R1", "xrun/R1", None);
        let s = launch_script("xrun/R1", "xrun/R1", &env, "python t.py --name it's");
        assert!(s.contains("command -v setsid"), "{s}");
        assert!(s.contains("$S nohup bash -c '"), "{s}");
        assert!(
            s.contains(">\"$HOME\"/'xrun/R1/stdout.log' 2>&1 </dev/null &"),
            "{s}"
        );
        // Inside the quoted `bash -c` word the path quotes are escaped.
        assert!(
            s.contains("echo $$ > \"$HOME\"/'\\''xrun/R1/run.pid'\\''"),
            "{s}"
        );
        // The single quote of the user command is escaped inside the bash -c word.
        assert!(s.contains("it'\\\\''s") || s.contains("it'\\''s"), "{s}");
        assert!(s.trim_end().ends_with("cat \"$HOME\"/'xrun/R1/run.pid'"));
    }

    #[test]
    fn setup_script_cds_into_workdir() {
        let s = setup_script("xrun/R1", "pip install -r req.txt");
        assert!(s.starts_with("mkdir -p \"$HOME\"/'xrun/R1' && cd "));
        assert!(s.ends_with("(pip install -r req.txt)"));
    }

    #[test]
    fn kill_script_has_term_then_kill_and_guards() {
        let s = kill_script("xrun/R1");
        assert!(s.contains("kill -TERM"));
        assert!(s.contains("kill -KILL"));
        assert!(s.contains("-gt 1"));
        assert!(s.contains("rm -f"));
    }

    #[test]
    fn alive_script_uses_exit_codes() {
        let s = alive_script("xrun/R1");
        assert!(s.contains("exit 2") && s.contains("exit 1") && s.contains("kill -0"));
    }

    #[test]
    fn tail_script_offsets_and_cap() {
        let s = tail_script("xrun/R1/events.jsonl", 100);
        assert!(s.contains("tail -c +101"), "{s}");
        assert!(s.contains("-gt 100"), "{s}");
        assert!(s.contains("head -c 524288"));
        assert!(s.contains("base64 | tr -d '\\n'"));
        assert!(s.starts_with("f=\"$HOME\"/'xrun/R1/events.jsonl'"));
    }

    #[test]
    fn parse_tail_decodes_and_joins_wrapped_base64() {
        // "hello world" = aGVsbG8gd29ybGQ=
        let (size, bytes) = parse_tail_output("11\naGVsbG8g\nd29ybGQ=\n").unwrap();
        assert_eq!(size, 11);
        assert_eq!(bytes, b"hello world");
    }

    #[test]
    fn parse_tail_size_only_and_chatter() {
        let (size, bytes) = parse_tail_output("welcome to the studio\n  42\n").unwrap();
        assert_eq!(size, 42);
        assert!(bytes.is_empty());
        assert!(parse_tail_output("nothing numeric\n").is_err());
        assert!(parse_tail_output("5\n!!!notb64\n").is_err());
    }

    #[test]
    fn glob_word_keeps_wildcards_live_and_quotes_the_rest() {
        assert_eq!(
            glob_word("xrun/R1/checkpoints/best*.pt"),
            "'xrun/R1/checkpoints/best'*'.pt'"
        );
        assert_eq!(glob_word("a b/**/x?"), "'a b/'**'/x'?");
        assert_eq!(glob_word("it's"), "'it'\\''s'");
    }

    #[test]
    fn pull_script_enables_globstar_and_strips_home() {
        let s = pull_script("xrun/R1/ck/*.pt");
        assert!(s.starts_with("bash -c '"));
        assert!(s.contains("globstar nullglob"));
        assert!(s.contains("[ -f "));
        assert!(s.contains("xrun/R1/ck/"));
    }

    #[test]
    fn parse_pull_output_trims_blank_lines() {
        assert_eq!(
            parse_pull_output("a/b.pt\n\n  c.json \n"),
            vec!["a/b.pt".to_string(), "c.json".to_string()]
        );
    }
}
