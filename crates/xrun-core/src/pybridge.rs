#![deny(unsafe_code)]

//! Persistent Python child driven over line-delimited JSON.
//!
//! Vendors whose only SDK is Python (Lightning AI, Google Colab) ship a
//! self-contained script that xrun spawns once per adapter instance. A
//! request is one JSON line on the child's stdin; the response is the first
//! stdout line starting with [`SENTINEL`] followed by a JSON object
//! `{"ok":true,"result":…}` or `{"ok":false,"error":"…","kind":"…"}`. Any
//! other stdout line (library chatter, stray prints) is ignored.
//!
//! The child is a long-lived process because a cold `import` of these SDKs
//! costs seconds. If it dies between or during a request, the bridge respawns
//! it once and replays the request; a second failure is reported together
//! with the stderr tail.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::Duration;

use serde_json::Value;
use sha2::{Digest, Sha256};

/// Prefix of the stdout line carrying a response.
pub const SENTINEL: &str = "<<<XRUN_BRIDGE>>>";

/// How much of the child's stderr (the last bytes) is kept for error messages.
const STDERR_TAIL_BYTES: usize = 4096;

/// How long `shutdown` waits for the pipe readers after killing the child.
const READER_JOIN_GRACE: Duration = Duration::from_secs(2);

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Machine-readable class of a failure reported by the Python side.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteKind {
    Auth,
    NotFound,
    Busy,
    Other,
}

impl RemoteKind {
    fn parse(s: &str) -> Self {
        match s {
            "auth" => RemoteKind::Auth,
            "not_found" => RemoteKind::NotFound,
            "busy" => RemoteKind::Busy,
            _ => RemoteKind::Other,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            RemoteKind::Auth => "auth",
            RemoteKind::NotFound => "not_found",
            RemoteKind::Busy => "busy",
            RemoteKind::Other => "other",
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum BridgeError {
    #[error("python interpreter not found (set XRUN_PYTHON, or install python3 / `py -3`)")]
    NoPython,
    #[error("bridge I/O error: {0}")]
    Io(String),
    #[error("bridge call timed out after {0:?}")]
    Timeout(Duration),
    #[error("bridge process exited unexpectedly{}", fmt_tail(.stderr))]
    Exited { stderr: String },
    #[error("bridge protocol error: {0}")]
    Protocol(String),
    #[error("{} error: {msg}", .kind.as_str())]
    Remote { kind: RemoteKind, msg: String },
}

fn fmt_tail(stderr: &str) -> String {
    let t = stderr.trim();
    if t.is_empty() {
        String::new()
    } else {
        format!(" (stderr tail: {t})")
    }
}

// ---------------------------------------------------------------------------
// interpreter discovery
// ---------------------------------------------------------------------------

/// Interpreter plus the args that must precede the script (`py -3`).
type PythonCmd = (PathBuf, Vec<String>);

fn base_command(program: &Path) -> Command {
    let cmd = Command::new(program);
    #[cfg(windows)]
    let cmd = {
        use std::os::windows::process::CommandExt;
        let mut cmd = cmd;
        cmd.creation_flags(CREATE_NO_WINDOW);
        cmd
    };
    cmd
}

fn works(program: &Path, args: &[String]) -> bool {
    base_command(program)
        .args(args)
        .args(["-c", "import sys"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn discover_python() -> Option<PythonCmd> {
    if let Some(p) = std::env::var_os("XRUN_PYTHON").filter(|p| !p.is_empty()) {
        let p = PathBuf::from(p);
        if works(&p, &[]) {
            return Some((p, Vec::new()));
        }
    }
    let candidates: [(&str, &[&str]); 3] = [("python", &[]), ("python3", &[]), ("py", &["-3"])];
    for (prog, args) in candidates {
        let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        let p = PathBuf::from(prog);
        if works(&p, &args) {
            return Some((p, args));
        }
    }
    None
}

fn python_cmd() -> Option<&'static PythonCmd> {
    static CACHE: OnceLock<Option<PythonCmd>> = OnceLock::new();
    CACHE.get_or_init(discover_python).as_ref()
}

/// Python interpreter xrun would use: `XRUN_PYTHON`, else the first of
/// `python`, `python3`, `py -3` that runs. The lookup runs once per process
/// and is cached. For the `py` launcher only the program is returned; the
/// bridge itself always adds `-3`.
pub fn find_python() -> Option<PathBuf> {
    python_cmd().map(|(p, _)| p.clone())
}

// ---------------------------------------------------------------------------
// script cache
// ---------------------------------------------------------------------------

fn materialize_script(name: &str, src: &str) -> io::Result<PathBuf> {
    // Unique per writer, also across threads of one process (two adapters
    // spawning the same script at once must not share a temp file).
    static TMP_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let digest = Sha256::digest(src.as_bytes());
    let sha16: String = digest.iter().take(8).map(|b| format!("{b:02x}")).collect();
    let dir = std::env::temp_dir().join("xrun-bridge");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{name}-{sha16}.py"));
    let up_to_date = std::fs::metadata(&path)
        .map(|m| m.len() == src.len() as u64)
        .unwrap_or(false);
    if !up_to_date {
        let seq = TMP_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let tmp = dir.join(format!("{name}-{sha16}.{}-{seq}.tmp", std::process::id()));
        std::fs::write(&tmp, src)?;
        if let Err(e) = std::fs::rename(&tmp, &path) {
            let _ = std::fs::remove_file(&tmp);
            // A concurrent writer may have won the race.
            if !path.is_file() {
                return Err(e);
            }
        }
    }
    Ok(path)
}

fn script_command(
    py: &PythonCmd,
    script: &Path,
    extra_args: &[String],
    env: &[(String, String)],
) -> Command {
    let mut cmd = base_command(&py.0);
    cmd.args(&py.1).arg(script).args(extra_args);
    cmd.env("PYTHONIOENCODING", "utf-8")
        .env("PYTHONUNBUFFERED", "1");
    for (k, v) in env {
        cmd.env(k, v);
    }
    cmd
}

/// Run a bridge script as a foreground interactive process (stdio inherited),
/// e.g. `python colab_bridge.py --login`. The caller is responsible for a TTY
/// check where one is required.
pub fn run_script_interactive(
    script_name: &str,
    script_src: &str,
    extra_args: &[String],
    env: &[(String, String)],
) -> io::Result<ExitStatus> {
    let py = python_cmd().ok_or_else(|| {
        io::Error::new(io::ErrorKind::NotFound, BridgeError::NoPython.to_string())
    })?;
    let script = materialize_script(script_name, script_src)?;
    script_command(py, &script, extra_args, env)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
}

// ---------------------------------------------------------------------------
// the bridge
// ---------------------------------------------------------------------------

/// One running child with its reader threads.
struct Proc {
    child: Child,
    stdin: ChildStdin,
    /// Response payloads (text after the sentinel), one per sentinel line.
    rx: Receiver<String>,
    stderr_tail: Arc<Mutex<Vec<u8>>>,
    threads: Vec<JoinHandle<()>>,
}

impl Proc {
    fn spawn(py: &PythonCmd, script: &Path, env: &[(String, String)]) -> Result<Proc, BridgeError> {
        let mut child = script_command(py, script, &[], env)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| BridgeError::Io(format!("spawn {}: {e}", py.0.display())))?;
        let stdin = child.stdin.take().expect("stdin piped");
        let stdout = child.stdout.take().expect("stdout piped");
        let stderr = child.stderr.take().expect("stderr piped");

        let (tx, rx) = mpsc::channel::<String>();
        let out_thread = std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            let mut buf = Vec::new();
            loop {
                buf.clear();
                match reader.read_until(b'\n', &mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {
                        let line = String::from_utf8_lossy(&buf);
                        if let Some(rest) = line.strip_prefix(SENTINEL) {
                            if tx.send(rest.trim().to_string()).is_err() {
                                break;
                            }
                        }
                    }
                }
            }
        });

        let tail = Arc::new(Mutex::new(Vec::<u8>::new()));
        let tail_w = Arc::clone(&tail);
        let err_thread = std::thread::spawn(move || {
            let mut stderr = stderr;
            let mut chunk = [0u8; 1024];
            loop {
                match stderr.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if let Ok(mut t) = tail_w.lock() {
                            t.extend_from_slice(&chunk[..n]);
                            if t.len() > STDERR_TAIL_BYTES {
                                let cut = t.len() - STDERR_TAIL_BYTES;
                                t.drain(..cut);
                            }
                        }
                    }
                }
            }
        });

        Ok(Proc {
            child,
            stdin,
            rx,
            stderr_tail: tail,
            threads: vec![out_thread, err_thread],
        })
    }

    /// Kill the child, join the readers, return the stderr tail.
    ///
    /// A grandchild the script spawned (an SDK helper, a browser opener) may
    /// have inherited the stdout/stderr pipes and keep them open after the
    /// child is gone; joining its reader would then block for that process's
    /// whole life. Readers still running after [`READER_JOIN_GRACE`] are left
    /// detached instead.
    fn shutdown(&mut self) -> String {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let deadline = std::time::Instant::now() + READER_JOIN_GRACE;
        while std::time::Instant::now() < deadline && self.threads.iter().any(|t| !t.is_finished())
        {
            std::thread::sleep(Duration::from_millis(20));
        }
        for t in self.threads.drain(..) {
            if t.is_finished() {
                let _ = t.join();
            }
        }
        let bytes = self
            .stderr_tail
            .lock()
            .map(|t| t.clone())
            .unwrap_or_default();
        String::from_utf8_lossy(&bytes).into_owned()
    }
}

impl Drop for Proc {
    fn drop(&mut self) {
        // Joining is skipped on drop paths already shut down (threads empty).
        if !self.threads.is_empty() {
            let _ = self.shutdown();
        }
    }
}

/// Outcome of one attempt to talk to the child.
enum Attempt {
    Done(Value),
    /// Pipe broke or the child went away: eligible for respawn-and-retry.
    Broken {
        /// The request line was fully written to a live child.
        sent: bool,
    },
    Timeout,
}

pub struct PyBridge {
    python: &'static PythonCmd,
    script: PathBuf,
    env: Vec<(String, String)>,
    proc: Mutex<Option<Proc>>,
}

impl PyBridge {
    /// Materialize `script_src` under `temp_dir()/xrun-bridge/` and start the
    /// child. `env` is added on top of `PYTHONIOENCODING=utf-8` and
    /// `PYTHONUNBUFFERED=1`.
    pub fn spawn(
        script_name: &str,
        script_src: &str,
        env: Vec<(String, String)>,
    ) -> Result<PyBridge, BridgeError> {
        let python = python_cmd().ok_or(BridgeError::NoPython)?;
        let script = materialize_script(script_name, script_src)
            .map_err(|e| BridgeError::Io(format!("write bridge script: {e}")))?;
        let proc = Proc::spawn(python, &script, &env)?;
        Ok(PyBridge {
            python,
            script,
            env,
            proc: Mutex::new(Some(proc)),
        })
    }

    /// Send one request and wait up to `timeout` for the response.
    ///
    /// Calls are serialized: the child handles one request at a time. A
    /// timeout kills the child (its state is unknown); the next call respawns
    /// it. A broken pipe / exit triggers one respawn and a replay.
    pub fn call(&self, req: Value, timeout: Duration) -> Result<Value, BridgeError> {
        self.call_opts(req, timeout, true)
    }

    /// Like [`PyBridge::call`], with control over replay.
    ///
    /// With `replay == false` (non-idempotent ops: allocating a runtime,
    /// starting a training) a request that may already have reached the
    /// child is never re-sent: if the child dies after the request was
    /// written, the child is respawned for the *next* call and
    /// [`BridgeError::Exited`] is returned now. A write that failed before
    /// any byte was sent is still retried once on a fresh child.
    pub fn call_opts(
        &self,
        req: Value,
        timeout: Duration,
        replay: bool,
    ) -> Result<Value, BridgeError> {
        let mut line = serde_json::to_string(&req)
            .map_err(|e| BridgeError::Protocol(format!("encode request: {e}")))?;
        line.push('\n');

        let mut guard = self
            .proc
            .lock()
            .map_err(|_| BridgeError::Io("bridge mutex poisoned".to_string()))?;

        let mut last_stderr = String::new();
        for _attempt in 0..2 {
            if guard.is_none() {
                *guard = Some(Proc::spawn(self.python, &self.script, &self.env)?);
            }
            let proc = guard.as_mut().expect("just ensured");
            match attempt(proc, &line, timeout) {
                Attempt::Done(v) => return decode_response(v),
                Attempt::Timeout => {
                    if let Some(mut p) = guard.take() {
                        let _ = p.shutdown();
                    }
                    return Err(BridgeError::Timeout(timeout));
                }
                Attempt::Broken { sent } => {
                    if let Some(mut p) = guard.take() {
                        last_stderr = p.shutdown();
                    }
                    if sent && !replay {
                        return Err(BridgeError::Exited {
                            stderr: last_stderr,
                        });
                    }
                }
            }
        }
        Err(BridgeError::Exited {
            stderr: last_stderr,
        })
    }
}

fn attempt(proc: &mut Proc, line: &str, timeout: Duration) -> Attempt {
    // A previous child may have exited between calls.
    if let Ok(Some(_)) = proc.child.try_wait() {
        return Attempt::Broken { sent: false };
    }
    if proc.stdin.write_all(line.as_bytes()).is_err() || proc.stdin.flush().is_err() {
        // The request never reached a live child.
        return Attempt::Broken { sent: false };
    }
    // Wait in slices so a child that died is noticed even when its stdout
    // pipe stays open (a grandchild inherited it, so the reader never sees
    // EOF and the channel never disconnects).
    let deadline = std::time::Instant::now() + timeout;
    loop {
        let now = std::time::Instant::now();
        if now >= deadline {
            return Attempt::Timeout;
        }
        let slice = (deadline - now).min(EXIT_POLL);
        match proc.rx.recv_timeout(slice) {
            Ok(text) => return Attempt::Done(parse_payload(&text)),
            Err(RecvTimeoutError::Disconnected) => return Attempt::Broken { sent: true },
            Err(RecvTimeoutError::Timeout) => {
                if let Ok(Some(_)) = proc.child.try_wait() {
                    // A response written just before the exit may still be
                    // in the reader's hands.
                    return match proc.rx.recv_timeout(EXIT_POLL) {
                        Ok(text) => Attempt::Done(parse_payload(&text)),
                        Err(_) => Attempt::Broken { sent: true },
                    };
                }
            }
        }
    }
}

/// Poll interval for noticing a dead child while waiting for its answer.
const EXIT_POLL: Duration = Duration::from_millis(250);

fn parse_payload(text: &str) -> Value {
    match serde_json::from_str::<Value>(text) {
        Ok(v) => v,
        Err(e) => serde_json::json!({
            "ok": false,
            "error": format!("unparseable bridge response: {e}"),
            "kind": "__protocol",
        }),
    }
}

fn decode_response(v: Value) -> Result<Value, BridgeError> {
    match v.get("ok").and_then(Value::as_bool) {
        Some(true) => Ok(v.get("result").cloned().unwrap_or(Value::Null)),
        Some(false) => {
            let msg = v
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("unknown error")
                .to_string();
            let kind = v.get("kind").and_then(Value::as_str).unwrap_or("other");
            if kind == "__protocol" {
                return Err(BridgeError::Protocol(msg));
            }
            Err(BridgeError::Remote {
                kind: RemoteKind::parse(kind),
                msg,
            })
        }
        None => Err(BridgeError::Protocol(format!(
            "response has no boolean `ok`: {v}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const ECHO: &str = r#"
import sys, json, time, os
S = "<<<XRUN_BRIDGE>>>"
for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    try:
        req = json.loads(line)
        op = req.get("op")
        if op == "ping":
            res = {"pong": True, "sdk_version": "test"}
        elif op == "echo":
            res = req
        elif op == "slow":
            time.sleep(req.get("secs", 5))
            res = {}
        elif op == "noise":
            print("garbage line before the answer", flush=True)
            print("<<<XRUN_BRIDGE_NOT>>> not a sentinel", flush=True)
            res = {"after": "noise"}
        elif op == "auth":
            print(S + json.dumps({"ok": False, "error": "bad key", "kind": "auth"}), flush=True)
            continue
        elif op == "die":
            sys.stderr.write("dying now\n")
            sys.stderr.flush()
            sys.exit(3)
        elif op == "orphan":
            import subprocess
            # Inherits stdout/stderr (where the OS passes them on) and
            # outlives this process.
            subprocess.Popen([sys.executable, "-c", "import time; time.sleep(20)"])
            sys.exit(5)
        elif op == "die_once":
            marker = req["marker"]
            if not os.path.exists(marker):
                open(marker, "w").write("x")
                sys.exit(4)
            res = {"survived": True}
        else:
            raise RuntimeError("boom")
        out = {"ok": True, "result": res}
    except Exception as e:
        out = {"ok": False, "error": str(e), "kind": "other"}
    print(S + json.dumps(out), flush=True)
"#;

    fn bridge() -> Option<PyBridge> {
        if find_python().is_none() {
            eprintln!("skipping: no python interpreter");
            return None;
        }
        Some(PyBridge::spawn("test-echo", ECHO, vec![]).expect("spawn"))
    }

    const T: Duration = Duration::from_secs(20);

    #[test]
    fn ping_returns_pong() {
        let Some(b) = bridge() else { return };
        let r = b.call(json!({"op": "ping"}), T).unwrap();
        assert_eq!(r["pong"], json!(true));
        // Persistent child: a second call reuses it.
        let r = b.call(json!({"op": "echo", "x": "привет"}), T).unwrap();
        assert_eq!(r["x"], json!("привет"));
    }

    #[test]
    fn remote_errors_are_typed() {
        let Some(b) = bridge() else { return };
        match b.call(json!({"op": "boom"}), T).unwrap_err() {
            BridgeError::Remote { kind, msg } => {
                assert_eq!(kind, RemoteKind::Other);
                assert!(msg.contains("boom"));
            }
            other => panic!("unexpected {other:?}"),
        }
        match b.call(json!({"op": "auth"}), T).unwrap_err() {
            BridgeError::Remote { kind, .. } => assert_eq!(kind, RemoteKind::Auth),
            other => panic!("unexpected {other:?}"),
        }
        // Still usable afterwards.
        assert!(b.call(json!({"op": "ping"}), T).is_ok());
    }

    #[test]
    fn non_sentinel_lines_are_ignored() {
        let Some(b) = bridge() else { return };
        let r = b.call(json!({"op": "noise"}), T).unwrap();
        assert_eq!(r["after"], json!("noise"));
    }

    #[test]
    fn timeout_kills_child_and_next_call_respawns() {
        let Some(b) = bridge() else { return };
        let err = b
            .call(
                json!({"op": "slow", "secs": 30}),
                Duration::from_millis(400),
            )
            .unwrap_err();
        assert!(matches!(err, BridgeError::Timeout(_)), "{err:?}");
        let r = b.call(json!({"op": "ping"}), T).unwrap();
        assert_eq!(r["pong"], json!(true));
    }

    #[test]
    fn exit_mid_request_respawns_and_retries_once() {
        let Some(b) = bridge() else { return };
        let dir = std::env::temp_dir().join(format!("xrun-bridge-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let marker = dir.join("marker");
        let _ = std::fs::remove_file(&marker);
        let r = b
            .call(
                json!({"op": "die_once", "marker": marker.to_string_lossy()}),
                T,
            )
            .unwrap();
        assert_eq!(r["survived"], json!(true));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn exit_mid_request_without_replay_fails_once_and_next_call_respawns() {
        let Some(b) = bridge() else { return };
        let dir = std::env::temp_dir().join(format!("xrun-bridge-norep-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let marker = dir.join("marker");
        let _ = std::fs::remove_file(&marker);
        let req = json!({"op": "die_once", "marker": marker.to_string_lossy()});
        let err = b.call_opts(req.clone(), T, false).unwrap_err();
        assert!(matches!(err, BridgeError::Exited { .. }), "{err:?}");
        // Not replayed: the marker exists (one execution), and the bridge
        // serves the next call from a fresh child.
        assert!(marker.exists());
        assert_eq!(b.call(req, T).unwrap()["survived"], json!(true));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn no_replay_still_retries_when_child_died_between_calls() {
        let Some(b) = bridge() else { return };
        // Kill the child via a dying request, then a no-replay call must
        // succeed: the dead child is noticed before anything is sent.
        let _ = b.call_opts(json!({"op": "die"}), T, false);
        std::thread::sleep(Duration::from_millis(100));
        let r = b.call_opts(json!({"op": "ping"}), T, false).unwrap();
        assert_eq!(r["pong"], json!(true));
    }

    #[test]
    fn persistent_failure_reports_stderr_tail() {
        let Some(b) = bridge() else { return };
        match b.call(json!({"op": "die"}), T).unwrap_err() {
            BridgeError::Exited { stderr } => assert!(stderr.contains("dying now"), "{stderr:?}"),
            other => panic!("unexpected {other:?}"),
        }
        // The bridge recovers on the next call.
        assert!(b.call(json!({"op": "ping"}), T).is_ok());
    }

    #[test]
    fn dead_child_with_inherited_pipes_is_noticed_without_waiting_out_the_timeout() {
        let Some(b) = bridge() else { return };
        let started = std::time::Instant::now();
        let err = b
            .call(json!({"op": "orphan"}), Duration::from_secs(60))
            .unwrap_err();
        assert!(matches!(err, BridgeError::Exited { .. }), "{err:?}");
        // Two attempts, each: exit noticed within a poll slice + reader grace.
        assert!(
            started.elapsed() < Duration::from_secs(20),
            "took {:?}",
            started.elapsed()
        );
        assert!(b.call(json!({"op": "ping"}), T).is_ok());
    }

    #[test]
    fn concurrent_writers_of_one_script_never_see_a_partial_file() {
        let src = format!("# {}\nprint(1)\n", "x".repeat(200_000));
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let src = src.clone();
                std::thread::spawn(move || materialize_script("cache-race-test", &src))
            })
            .collect();
        for h in handles {
            let p = h.join().unwrap().expect("materialize");
            assert_eq!(std::fs::read_to_string(&p).unwrap(), src);
        }
    }

    #[test]
    fn script_cache_is_content_addressed() {
        let a = materialize_script("cache-test", "print(1)\n").unwrap();
        let b = materialize_script("cache-test", "print(1)\n").unwrap();
        let c = materialize_script("cache-test", "print(2)\n").unwrap();
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_eq!(std::fs::read_to_string(&a).unwrap(), "print(1)\n");
    }

    #[test]
    fn decode_rejects_missing_ok() {
        assert!(matches!(
            decode_response(json!({"result": 1})),
            Err(BridgeError::Protocol(_))
        ));
    }
}
