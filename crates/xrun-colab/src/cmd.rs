#![deny(unsafe_code)]

//! Pure builders: the shell launch/kill scripts and the Python snippets the
//! adapter sends to the Colab kernel (through `exec`), plus the parsers for
//! the sentinel-marked lines they print. The snippets are plain Python so
//! they also run locally (the tests do exactly that).

use base64::Engine;
use serde::Deserialize;
use xrun_ssh::cmd::shell_quote;

use crate::error::ColabError;

pub const MARK_SH: &str = "<<<XRUN_SH>>>";
pub const MARK_SPAWN: &str = "<<<XRUN_SPAWN>>>";
pub const MARK_TAIL: &str = "<<<XRUN_TAIL>>>";
pub const MARK_GLOB: &str = "<<<XRUN_GLOB>>>";
pub const MARK_ALIVE: &str = "<<<XRUN_ALIVE>>>";

/// Largest slice a single `tail` call returns. It travels base64-encoded in
/// one IOPub message, a text transport where a truncated chunk would break
/// decoding forever, so it is kept small (512 KiB); the poller simply asks
/// again from the new offset.
pub const TAIL_CHUNK_BYTES: u64 = 512 * 1024;

/// Python string literal for `s`. JSON string syntax is a subset of Python's.
fn py_str(s: &str) -> String {
    serde_json::to_string(s).expect("string serialization is infallible")
}

fn fill(template: &str, pairs: &[(&str, String)]) -> String {
    let mut out = template.to_string();
    for (k, v) in pairs {
        out = out.replace(k, v);
    }
    out
}

// ---------------------------------------------------------------------------
// shell scripts
// ---------------------------------------------------------------------------

/// Detached launch script (run via [`spawn_snippet`]): `user_cmd` runs under
/// `setsid nohup bash -c` in `workdir`, stdout+stderr go to
/// `<run_dir>/stdout.log` and the PID to `<run_dir>/run.pid`. `run_dir` and
/// `workdir` must be absolute.
pub fn launch_script(run_dir: &str, workdir: &str, run_id: &str, user_cmd: &str) -> String {
    let inner = format!(
        "cd {wd} && export XRUN_RUN_ID={rid} XRUN_RUN_DIR={rd} \
         PYTHONUNBUFFERED=\"${{PYTHONUNBUFFERED:-1}}\" && {user_cmd}",
        wd = shell_quote(workdir),
        rid = shell_quote(run_id),
        rd = shell_quote(run_dir),
    );
    let escaped = inner.replace('\'', "'\\''");
    format!(
        "set -e; mkdir -p {rd}; \
         (setsid nohup bash -c '{escaped}' >{log} 2>&1 </dev/null & echo $!) >{pid}",
        rd = shell_quote(run_dir),
        log = shell_quote(&format!("{run_dir}/stdout.log")),
        pid = shell_quote(&format!("{run_dir}/run.pid")),
    )
}

/// TERM, then KILL, the process recorded in `<run_dir>/run.pid`; non-zero
/// exit when it cannot be stopped. A missing pid file is not an error.
///
/// The recorded pid is the `bash -c` that `setsid` made a session (and
/// process group) leader; the training (`cd … && export … && python …`) is
/// its child, so the whole group is signalled first, else the child would
/// outlive its shell.
pub fn kill_script(run_dir: &str) -> String {
    let pf = shell_quote(&format!("{run_dir}/run.pid"));
    format!(
        "if [ -f {pf} ]; then PID=$(cat {pf}); \
         case \"$PID\" in ''|*[!0-9]*) exit 1;; esac; \
         [ \"$PID\" -gt 1 ] || exit 1; \
         kill -TERM -- \"-$PID\" 2>/dev/null; kill -TERM \"$PID\" 2>/dev/null; sleep 1; \
         if kill -0 \"$PID\" 2>/dev/null; then \
         kill -KILL -- \"-$PID\" 2>/dev/null; kill -KILL \"$PID\" 2>/dev/null; sleep 1; fi; \
         if kill -0 \"$PID\" 2>/dev/null; then exit 1; fi; rm -f {pf}; fi"
    )
}

/// `mkdir -p` for several directories.
pub fn mkdir_script<S: AsRef<str>>(dirs: &[S]) -> String {
    let quoted: Vec<String> = dirs.iter().map(|d| shell_quote(d.as_ref())).collect();
    format!("mkdir -p {}", quoted.join(" "))
}

// ---------------------------------------------------------------------------
// kernel snippets
// ---------------------------------------------------------------------------

const SH_TPL: &str = r#"import base64, json, subprocess
def _xrun_sh():
    try:
        r = subprocess.run(["bash", "-lc", @CMD@], capture_output=True)
        o = {"code": r.returncode, "out": r.stdout[-65536:].decode("utf-8", "replace"),
             "err": r.stderr[-65536:].decode("utf-8", "replace")}
    except Exception as e:
        o = {"code": 127, "out": "", "err": "%s: %s" % (type(e).__name__, e)}
    print("\n@MARK@" + base64.b64encode(json.dumps(o).encode()).decode())
_xrun_sh()
"#;

/// Run `cmd` under `bash -lc` in the kernel host, print code/stdout/stderr.
pub fn sh_snippet(cmd: &str) -> String {
    fill(
        SH_TPL,
        &[("@CMD@", py_str(cmd)), ("@MARK@", MARK_SH.to_string())],
    )
}

const SPAWN_TPL: &str = r#"import json, subprocess
def _xrun_spawn():
    try:
        p = subprocess.Popen(["bash", "-c", @SCRIPT@], start_new_session=True,
                             stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                             stderr=subprocess.DEVNULL)
        try:
            rc = p.wait(timeout=30)
        except subprocess.TimeoutExpired:
            rc = None
        o = {"pid": p.pid, "code": rc, "err": ""}
    except Exception as e:
        o = {"pid": None, "code": 127, "err": "%s: %s" % (type(e).__name__, e)}
    print("\n@MARK@" + json.dumps(o))
_xrun_spawn()
"#;

/// Start `script` detached from the kernel (own session, no stdio) and print
/// the wrapper's pid and exit code.
pub fn spawn_snippet(script: &str) -> String {
    fill(
        SPAWN_TPL,
        &[
            ("@SCRIPT@", py_str(script)),
            ("@MARK@", MARK_SPAWN.to_string()),
        ],
    )
}

const TAIL_TPL: &str = r#"import base64, json, os
def _xrun_tail():
    path, offset, limit = @PATH@, @OFFSET@, @LIMIT@
    try:
        size = os.path.getsize(path)
    except OSError:
        size = 0
    data = b""
    if size > offset:
        with open(path, "rb") as f:
            f.seek(offset)
            data = f.read(limit)
    print("\n@MARK@" + json.dumps({"size": size, "data_b64": base64.b64encode(data).decode()}))
_xrun_tail()
"#;

/// Print the file size and up to [`TAIL_CHUNK_BYTES`] bytes from `offset`
/// (base64). A missing file reports size 0.
pub fn tail_snippet(path: &str, offset: u64) -> String {
    fill(
        TAIL_TPL,
        &[
            ("@PATH@", py_str(path)),
            ("@OFFSET@", offset.to_string()),
            ("@LIMIT@", TAIL_CHUNK_BYTES.to_string()),
            ("@MARK@", MARK_TAIL.to_string()),
        ],
    )
}

const GLOB_TPL: &str = r#"import glob, json, os
def _xrun_glob():
    pat = os.path.expanduser(@PATTERN@)
    files = sorted(os.path.abspath(p) for p in glob.glob(pat, recursive=True) if os.path.isfile(p))
    print("\n@MARK@" + json.dumps(files))
_xrun_glob()
"#;

/// Print the JSON list of regular files matching `pattern` (recursive glob,
/// `~` expanded).
pub fn glob_snippet(pattern: &str) -> String {
    fill(
        GLOB_TPL,
        &[
            ("@PATTERN@", py_str(pattern)),
            ("@MARK@", MARK_GLOB.to_string()),
        ],
    )
}

const ALIVE_TPL: &str = r#"import os
def _xrun_alive():
    try:
        with open(@PIDFILE@) as f:
            pid = int(f.read().strip())
    except (OSError, ValueError):
        return "no_pid"
    if pid <= 1:
        return "no_pid"
    if os.path.isdir("/proc/self"):
        try:
            with open("/proc/%d/stat" % pid) as f:
                state = f.read().rsplit(")", 1)[1].split()[0]
        except (OSError, IndexError):
            return "dead"
        return "dead" if state in ("Z", "X") else "alive"
    try:
        os.kill(pid, 0)
        return "alive"
    except OSError:
        return "dead"
print("\n@MARK@" + _xrun_alive())
"#;

/// Print `alive`, `dead` or `no_pid` for the process in `pidfile` (zombies
/// count as dead).
pub fn alive_snippet(pidfile: &str) -> String {
    fill(
        ALIVE_TPL,
        &[
            ("@PIDFILE@", py_str(pidfile)),
            ("@MARK@", MARK_ALIVE.to_string()),
        ],
    )
}

// ---------------------------------------------------------------------------
// parsing
// ---------------------------------------------------------------------------

/// Text after `marker` on the last stdout line that starts with it.
pub fn find_marker<'a>(stdout: &'a str, marker: &str) -> Option<&'a str> {
    stdout
        .lines()
        .rev()
        .find_map(|l| l.strip_prefix(marker))
        .map(str::trim)
}

fn bad(what: &'static str, detail: impl Into<String>) -> ColabError {
    ColabError::BadOutput {
        what,
        detail: detail.into(),
    }
}

fn payload<'a>(stdout: &'a str, marker: &str, what: &'static str) -> Result<&'a str, ColabError> {
    find_marker(stdout, marker).ok_or_else(|| {
        let tail: String = stdout
            .chars()
            .rev()
            .take(300)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        bad(
            what,
            format!("marker {marker} missing; output tail: {tail:?}"),
        )
    })
}

/// Result of a remote shell command.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ShOut {
    pub code: i64,
    #[serde(default)]
    pub out: String,
    #[serde(default)]
    pub err: String,
}

impl ShOut {
    /// `Ok(self)` when the exit code is 0, else [`ColabError::RemoteCommand`]
    /// with the stderr (or stdout) tail.
    pub fn ok(self) -> Result<ShOut, ColabError> {
        if self.code == 0 {
            return Ok(self);
        }
        let text = if self.err.trim().is_empty() {
            &self.out
        } else {
            &self.err
        };
        let t = text.trim();
        let tail: String = t
            .chars()
            .rev()
            .take(1500)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        Err(ColabError::RemoteCommand {
            code: self.code,
            detail: tail,
        })
    }
}

/// Encode what [`sh_snippet`] prints (used by the fake bridge).
pub fn encode_sh(code: i64, out: &str, err: &str) -> String {
    let json = serde_json::json!({"code": code, "out": out, "err": err}).to_string();
    format!(
        "{MARK_SH}{}\n",
        base64::engine::general_purpose::STANDARD.encode(json)
    )
}

pub fn parse_sh(stdout: &str) -> Result<ShOut, ColabError> {
    let b64 = payload(stdout, MARK_SH, "sh")?;
    let raw = base64::engine::general_purpose::STANDARD
        .decode(b64)
        .map_err(|e| bad("sh", format!("base64: {e}")))?;
    serde_json::from_slice(&raw).map_err(|e| bad("sh", format!("json: {e}")))
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Spawned {
    pub pid: Option<i64>,
    pub code: Option<i64>,
    #[serde(default)]
    pub err: String,
}

pub fn parse_spawn(stdout: &str) -> Result<Spawned, ColabError> {
    let p = payload(stdout, MARK_SPAWN, "spawn")?;
    let s: Spawned = serde_json::from_str(p).map_err(|e| bad("spawn", format!("json: {e}")))?;
    match (s.pid, s.code) {
        (Some(_), Some(0) | None) => Ok(s),
        _ => Err(ColabError::RemoteCommand {
            code: s.code.unwrap_or(127),
            detail: s.err.clone(),
        }),
    }
}

/// Parsed output of [`tail_snippet`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TailChunk {
    pub size: u64,
    pub data: Vec<u8>,
}

pub fn encode_tail(size: u64, data: &[u8]) -> String {
    format!(
        "{MARK_TAIL}{}\n",
        serde_json::json!({
            "size": size,
            "data_b64": base64::engine::general_purpose::STANDARD.encode(data),
        })
    )
}

pub fn parse_tail(stdout: &str) -> Result<TailChunk, ColabError> {
    #[derive(Deserialize)]
    struct Raw {
        size: u64,
        data_b64: String,
    }
    let p = payload(stdout, MARK_TAIL, "tail")?;
    let raw: Raw = serde_json::from_str(p).map_err(|e| bad("tail", format!("json: {e}")))?;
    let data = base64::engine::general_purpose::STANDARD
        .decode(raw.data_b64)
        .map_err(|e| bad("tail", format!("base64: {e}")))?;
    Ok(TailChunk {
        size: raw.size,
        data,
    })
}

pub fn encode_glob(files: &[&str]) -> String {
    format!("{MARK_GLOB}{}\n", serde_json::json!(files))
}

pub fn parse_glob(stdout: &str) -> Result<Vec<String>, ColabError> {
    let p = payload(stdout, MARK_GLOB, "glob")?;
    serde_json::from_str(p).map_err(|e| bad("glob", format!("json: {e}")))
}

/// `Some(true)` alive, `Some(false)` dead, `None` no pid file / unreadable.
pub fn parse_alive(stdout: &str) -> Result<Option<bool>, ColabError> {
    match payload(stdout, MARK_ALIVE, "alive")? {
        "alive" => Ok(Some(true)),
        "dead" => Ok(Some(false)),
        "no_pid" => Ok(None),
        other => Err(bad("alive", format!("unexpected state {other:?}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    use xrun_core::pybridge::find_python;

    /// Run a snippet in a local interpreter (it is plain Python); `None` when
    /// no interpreter is available.
    fn run_local(code: &str) -> Option<String> {
        let py = find_python()?;
        let mut cmd = Command::new(py);
        // `py -3` launchers resolve via PATH; plain `-c` works for all.
        let out = cmd.arg("-c").arg(code).output().ok()?;
        assert!(
            out.status.success(),
            "snippet failed: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        Some(String::from_utf8_lossy(&out.stdout).into_owned())
    }

    fn have_bash() -> bool {
        Command::new("bash")
            .arg("-c")
            .arg("exit 0")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    #[test]
    fn launch_script_shape() {
        let s = launch_script(
            "/content/xrun/R1",
            "/content/xrun/R1",
            "R1",
            "python t.py --lr 1",
        );
        assert!(s.starts_with("set -e; mkdir -p '/content/xrun/R1';"), "{s}");
        assert!(s.contains("setsid nohup bash -c '"), "{s}");
        assert!(s.contains(">'/content/xrun/R1/stdout.log' 2>&1"), "{s}");
        assert!(s.contains("echo $!) >'/content/xrun/R1/run.pid'"), "{s}");
        // Inner script is single-quoted, so its own quotes appear as '\''.
        assert!(s.contains("export XRUN_RUN_ID='\\''R1'\\''"), "{s}");
        assert!(s.contains("XRUN_RUN_DIR='\\''/content/xrun/R1'\\''"), "{s}");
        assert!(s.contains("PYTHONUNBUFFERED="), "{s}");
        assert!(!s.contains("CUDA_VISIBLE_DEVICES"), "{s}");
        assert!(s.contains("python t.py --lr 1"), "{s}");
    }

    #[test]
    fn launch_script_escapes_single_quotes() {
        let s = launch_script("/r", "/w", "R", "echo 'hi there'");
        assert!(s.contains("echo '\\''hi there'\\''"), "{s}");
    }

    /// The launch script needs `setsid` (util-linux): present on Colab, absent
    /// on macOS runners.
    fn have_setsid() -> bool {
        cfg!(unix)
            && Command::new("bash")
                .arg("-c")
                .arg("command -v setsid")
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false)
    }

    #[test]
    fn launch_script_runs_and_records_pid() {
        if !have_bash() || !have_setsid() {
            return;
        }
        let td = tempfile::tempdir().unwrap();
        let rd = td.path().join("run");
        let rd = rd.to_str().unwrap();
        let s = launch_script(
            rd,
            td.path().to_str().unwrap(),
            "R9",
            "echo hello-$XRUN_RUN_ID",
        );
        let st = Command::new("bash").arg("-c").arg(&s).status().unwrap();
        assert!(st.success());
        std::thread::sleep(std::time::Duration::from_millis(500));
        let log = std::fs::read_to_string(format!("{rd}/stdout.log")).unwrap();
        assert_eq!(log.trim(), "hello-R9");
        let pid = std::fs::read_to_string(format!("{rd}/run.pid")).unwrap();
        assert!(pid.trim().parse::<u32>().is_ok(), "pid {pid:?}");
    }

    #[test]
    fn kill_script_guards_pid() {
        let s = kill_script("/content/xrun/R1");
        assert!(s.contains("kill -TERM"));
        assert!(s.contains("kill -KILL"));
        assert!(s.contains("'/content/xrun/R1/run.pid'"));
        assert!(s.contains("-gt 1"));
        // The process group goes first: the training is a child of the
        // recorded shell.
        assert!(s.contains("kill -TERM -- \"-$PID\""), "{s}");
        assert!(s.contains("kill -KILL -- \"-$PID\""), "{s}");
    }

    #[test]
    fn kill_script_stops_the_training_child_too() {
        if !have_bash() || !have_setsid() {
            return;
        }
        let td = tempfile::tempdir().unwrap();
        let rd = td.path().join("run");
        let rd = rd.to_str().unwrap();
        let child = td.path().join("child.pid");
        let child = child.to_str().unwrap();
        // `; wait` keeps the shell as the parent instead of exec-ing sleep.
        let user_cmd = format!("sleep 60 & echo $! > {child}; wait");
        let s = launch_script(rd, td.path().to_str().unwrap(), "R1", &user_cmd);
        assert!(Command::new("bash")
            .arg("-c")
            .arg(&s)
            .status()
            .unwrap()
            .success());
        let mut child_pid = String::new();
        for _ in 0..50 {
            child_pid = std::fs::read_to_string(child).unwrap_or_default();
            if !child_pid.trim().is_empty() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        let child_pid = child_pid.trim().to_string();
        assert!(!child_pid.is_empty(), "child never started");
        let st = Command::new("bash")
            .arg("-c")
            .arg(kill_script(rd))
            .status()
            .unwrap();
        assert!(st.success());
        // Alive = exists and is not a zombie (an orphaned zombie may linger
        // until init reaps it).
        let alive = Command::new("bash")
            .arg("-c")
            .arg(format!(
                "s=$(ps -o stat= -p {child_pid} 2>/dev/null | tr -d ' '); \
                 [ -n \"$s\" ] && [ \"${{s#Z}}\" = \"$s\" ]"
            ))
            .status()
            .unwrap()
            .success();
        assert!(!alive, "training child {child_pid} survived kill_script");
    }

    #[test]
    fn mkdir_script_quotes_each_dir() {
        assert_eq!(mkdir_script(&["/a b", "/c"]), "mkdir -p '/a b' '/c'");
    }

    #[test]
    fn find_marker_takes_last_matching_line() {
        let out = "noise\n<<<XRUN_GLOB>>>[1]\nmore\n<<<XRUN_GLOB>>>[2]\ntrailing";
        assert_eq!(find_marker(out, MARK_GLOB), Some("[2]"));
        assert_eq!(find_marker("nothing", MARK_GLOB), None);
    }

    #[test]
    fn sh_roundtrip_and_error_mapping() {
        let enc = encode_sh(0, "out\n", "");
        let r = parse_sh(&format!("hello\n{enc}")).unwrap();
        assert_eq!(r.code, 0);
        assert_eq!(r.out, "out\n");
        assert!(r.ok().is_ok());
        let enc = encode_sh(3, "", "boom\n");
        match parse_sh(&enc).unwrap().ok() {
            Err(ColabError::RemoteCommand { code: 3, detail }) => assert_eq!(detail, "boom"),
            other => panic!("{other:?}"),
        }
        assert!(matches!(
            parse_sh("no marker here"),
            Err(ColabError::BadOutput { what: "sh", .. })
        ));
    }

    #[test]
    fn tail_glob_alive_parsers() {
        let c = parse_tail(&encode_tail(7, b"abc")).unwrap();
        assert_eq!(
            c,
            TailChunk {
                size: 7,
                data: b"abc".to_vec()
            }
        );
        assert_eq!(
            parse_glob(&encode_glob(&["/a/b.pt", "/a/c.pt"])).unwrap(),
            vec!["/a/b.pt", "/a/c.pt"]
        );
        assert_eq!(parse_alive("<<<XRUN_ALIVE>>>alive\n").unwrap(), Some(true));
        assert_eq!(parse_alive("<<<XRUN_ALIVE>>>dead\n").unwrap(), Some(false));
        assert_eq!(parse_alive("<<<XRUN_ALIVE>>>no_pid\n").unwrap(), None);
        assert!(parse_alive("<<<XRUN_ALIVE>>>??\n").is_err());
    }

    #[test]
    fn spawn_parse_rejects_failure() {
        assert!(parse_spawn("<<<XRUN_SPAWN>>>{\"pid\": 5, \"code\": 0, \"err\": \"\"}").is_ok());
        assert!(parse_spawn("<<<XRUN_SPAWN>>>{\"pid\": 5, \"code\": null, \"err\": \"\"}").is_ok());
        assert!(parse_spawn("<<<XRUN_SPAWN>>>{\"pid\": 5, \"code\": 2, \"err\": \"x\"}").is_err());
        assert!(
            parse_spawn("<<<XRUN_SPAWN>>>{\"pid\": null, \"code\": 127, \"err\": \"x\"}").is_err()
        );
    }

    #[test]
    fn snippets_embed_values_as_python_literals() {
        let s = sh_snippet("echo \"a\\b\" 'q'");
        assert!(s.contains(r#"["bash", "-lc", "echo \"a\\b\" 'q'"]"#), "{s}");
        assert!(tail_snippet("/x/y.log", 12).contains("\"/x/y.log\", 12,"));
    }

    #[test]
    fn sh_snippet_runs_locally() {
        if !have_bash() {
            return;
        }
        let Some(out) = run_local(&sh_snippet("echo hi; echo err >&2; exit 3")) else {
            return;
        };
        let r = parse_sh(&out).unwrap();
        assert_eq!((r.code, r.out.trim(), r.err.trim()), (3, "hi", "err"));
    }

    #[test]
    fn tail_chunk_is_capped_at_512_kib() {
        assert_eq!(TAIL_CHUNK_BYTES, 512 * 1024);
        assert!(tail_snippet("/x", 0).contains("0, 524288"));
        let td = tempfile::tempdir().unwrap();
        let f = td.path().join("big.log");
        std::fs::write(&f, vec![b'a'; 600 * 1024]).unwrap();
        let Some(out) = run_local(&tail_snippet(f.to_str().unwrap(), 0)) else {
            return;
        };
        let c = parse_tail(&out).unwrap();
        assert_eq!(c.size, 600 * 1024);
        assert_eq!(c.data.len() as u64, TAIL_CHUNK_BYTES);
    }

    #[test]
    fn tail_snippet_runs_locally() {
        let td = tempfile::tempdir().unwrap();
        let f = td.path().join("a.log");
        std::fs::write(&f, b"0123456789").unwrap();
        let p = f.to_str().unwrap();
        let Some(out) = run_local(&tail_snippet(p, 4)) else {
            return;
        };
        let c = parse_tail(&out).unwrap();
        assert_eq!((c.size, c.data.as_slice()), (10, &b"456789"[..]));
        let c = parse_tail(&run_local(&tail_snippet(p, 10)).unwrap()).unwrap();
        assert_eq!((c.size, c.data.len()), (10, 0));
        // size < offset is reported (the adapter turns it into Truncated).
        let c = parse_tail(&run_local(&tail_snippet(p, 99)).unwrap()).unwrap();
        assert_eq!((c.size, c.data.len()), (10, 0));
        let missing = td.path().join("none.log");
        let c =
            parse_tail(&run_local(&tail_snippet(missing.to_str().unwrap(), 0)).unwrap()).unwrap();
        assert_eq!(c.size, 0);
    }

    #[test]
    fn glob_snippet_runs_locally_files_only() {
        let td = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(td.path().join("ck/sub")).unwrap();
        std::fs::write(td.path().join("ck/best.pt"), b"x").unwrap();
        std::fs::write(td.path().join("ck/sub/best2.pt"), b"y").unwrap();
        std::fs::create_dir_all(td.path().join("ck/dir.pt")).unwrap();
        let root = td.path().to_str().unwrap().replace('\\', "/");
        let Some(out) = run_local(&glob_snippet(&format!("{root}/ck/**/*.pt"))) else {
            return;
        };
        let files = parse_glob(&out).unwrap();
        assert_eq!(files.len(), 2, "{files:?}");
        assert!(files
            .iter()
            .all(|f| f.ends_with(".pt") && !f.ends_with("dir.pt")));
    }

    #[test]
    fn alive_snippet_runs_locally() {
        let td = tempfile::tempdir().unwrap();
        let pf = td.path().join("run.pid");
        let p = pf.to_str().unwrap();
        let Some(out) = run_local(&alive_snippet(p)) else {
            return;
        };
        assert_eq!(parse_alive(&out).unwrap(), None);
        std::fs::write(&pf, "not-a-number").unwrap();
        assert_eq!(
            parse_alive(&run_local(&alive_snippet(p)).unwrap()).unwrap(),
            None
        );
        // Probing a live pid is only exercised where /proc exists (os.kill(pid, 0)
        // is not a probe on Windows).
        if cfg!(target_os = "linux") {
            std::fs::write(&pf, std::process::id().to_string()).unwrap();
            assert_eq!(
                parse_alive(&run_local(&alive_snippet(p)).unwrap()).unwrap(),
                Some(true)
            );
            std::fs::write(&pf, "4194999").unwrap();
            assert_eq!(
                parse_alive(&run_local(&alive_snippet(p)).unwrap()).unwrap(),
                Some(false)
            );
        }
    }

    #[test]
    fn spawn_snippet_runs_locally() {
        if !have_bash() {
            return;
        }
        let Some(out) = run_local(&spawn_snippet("exit 0")) else {
            return;
        };
        let s = parse_spawn(&out).unwrap();
        assert_eq!(s.code, Some(0));
        assert!(s.pid.unwrap() > 1);
    }
}
