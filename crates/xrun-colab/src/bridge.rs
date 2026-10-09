#![deny(unsafe_code)]

//! The Colab bridge: a trait the adapter is generic over, the production
//! implementation on top of [`xrun_core::pybridge::PyBridge`] and an
//! in-memory fake for tests.

use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

use serde::Deserialize;
use serde_json::{json, Value};
use xrun_core::pybridge::{self, BridgeError, PyBridge};

use crate::error::ColabError;

/// The embedded Python side (also run as `--login`).
pub const BRIDGE_PY: &str = include_str!("bridge.py");
pub const SCRIPT_NAME: &str = "colab_bridge";

pub const PING_TIMEOUT: Duration = Duration::from_secs(60);
pub const SESSION_NEW_TIMEOUT: Duration = Duration::from_secs(600);
pub const SESSION_OTHER_TIMEOUT: Duration = Duration::from_secs(120);
pub const FILE_TIMEOUT: Duration = Duration::from_secs(30 * 60);
/// Slack added to an `exec` timeout for the bridge round trip itself
/// (kernel start, prelude).
const EXEC_SLACK: Duration = Duration::from_secs(120);

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct PingInfo {
    pub sdk_version: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct WhoAmI {
    pub logged_in: bool,
    #[serde(default)]
    pub usage: Option<String>,
}

/// A Colab runtime assignment.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SessionInfo {
    /// Local session name; `"?"` for a server-side assignment xrun does not track.
    #[serde(default)]
    pub name: String,
    pub endpoint: String,
    pub accelerator: String,
    pub variant: String,
}

/// Result of one `exec` call in the session kernel.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct ExecOutput {
    #[serde(default)]
    pub stdout: String,
    #[serde(default)]
    pub stderr: String,
    /// `ename: evalue` when the cell raised.
    #[serde(default)]
    pub error: Option<String>,
}

impl ExecOutput {
    pub fn stdout(s: impl Into<String>) -> Self {
        ExecOutput {
            stdout: s.into(),
            ..Default::default()
        }
    }
}

pub trait ColabBridge {
    /// Check the bridge and the `google-colab-cli` import; never touches auth.
    fn ping(&self) -> Result<PingInfo, ColabError>;
    /// Login state (token file exists) and the consumption summary; never
    /// starts the interactive login.
    fn whoami(&self) -> Result<WhoAmI, ColabError>;
    fn session_new(&self, name: &str, gpu: &str, high_mem: bool)
        -> Result<SessionInfo, ColabError>;
    /// Unassign and forget a session; `Ok(false)` when it did not exist.
    fn session_stop(&self, name: &str) -> Result<bool, ColabError>;
    fn sessions(&self) -> Result<Vec<SessionInfo>, ColabError>;
    /// Probe / idempotent snippet; the bridge may replay it after a child crash.
    fn exec(&self, name: &str, code: &str, timeout: Duration) -> Result<ExecOutput, ColabError>;
    /// Non-idempotent snippet (spawn, setup, kill): never replayed after the
    /// child died with the request already written.
    fn exec_once(
        &self,
        name: &str,
        code: &str,
        timeout: Duration,
    ) -> Result<ExecOutput, ColabError>;
    fn upload(&self, name: &str, local: &Path, remote: &str) -> Result<(), ColabError>;
    fn download(&self, name: &str, remote: &str, local: &Path) -> Result<(), ColabError>;
}

/// Production bridge: lazily spawns one persistent Python child.
#[derive(Default)]
pub struct PyColabBridge {
    inner: Mutex<Option<PyBridge>>,
}

impl PyColabBridge {
    pub fn new() -> Self {
        Self::default()
    }

    fn call(&self, req: Value, timeout: Duration) -> Result<Value, ColabError> {
        self.call_opts(req, timeout, true)
    }

    fn call_opts(&self, req: Value, timeout: Duration, replay: bool) -> Result<Value, ColabError> {
        let mut guard = self
            .inner
            .lock()
            .map_err(|_| BridgeError::Io("colab bridge mutex poisoned".into()))?;
        if guard.is_none() {
            *guard = Some(PyBridge::spawn(SCRIPT_NAME, BRIDGE_PY, Vec::new())?);
        }
        Ok(guard
            .as_ref()
            .expect("just spawned")
            .call_opts(req, timeout, replay)?)
    }

    fn call_as<T: for<'de> Deserialize<'de>>(
        &self,
        req: Value,
        timeout: Duration,
    ) -> Result<T, ColabError> {
        self.call_as_opts(req, timeout, true)
    }

    fn call_as_opts<T: for<'de> Deserialize<'de>>(
        &self,
        req: Value,
        timeout: Duration,
        replay: bool,
    ) -> Result<T, ColabError> {
        let v = self.call_opts(req, timeout, replay)?;
        serde_json::from_value(v)
            .map_err(|e| BridgeError::Protocol(format!("unexpected bridge result: {e}")).into())
    }
}

impl ColabBridge for PyColabBridge {
    fn ping(&self) -> Result<PingInfo, ColabError> {
        self.call_as(json!({"op": "ping"}), PING_TIMEOUT)
    }

    fn whoami(&self) -> Result<WhoAmI, ColabError> {
        self.call_as(json!({"op": "whoami"}), SESSION_OTHER_TIMEOUT)
    }

    fn session_new(
        &self,
        name: &str,
        gpu: &str,
        high_mem: bool,
    ) -> Result<SessionInfo, ColabError> {
        // A replay would assign a second runtime.
        self.call_as_opts(
            json!({"op": "session_new", "name": name, "gpu": gpu, "high_mem": high_mem}),
            SESSION_NEW_TIMEOUT,
            false,
        )
    }

    fn session_stop(&self, name: &str) -> Result<bool, ColabError> {
        #[derive(Deserialize)]
        struct R {
            stopped: bool,
        }
        let r: R = self.call_as_opts(
            json!({"op": "session_stop", "name": name}),
            SESSION_OTHER_TIMEOUT,
            false,
        )?;
        Ok(r.stopped)
    }

    fn sessions(&self) -> Result<Vec<SessionInfo>, ColabError> {
        self.call_as(json!({"op": "sessions"}), SESSION_OTHER_TIMEOUT)
    }

    fn exec(&self, name: &str, code: &str, timeout: Duration) -> Result<ExecOutput, ColabError> {
        self.call_as(
            json!({"op": "exec", "name": name, "code": code, "timeout": timeout.as_secs_f64()}),
            timeout + EXEC_SLACK,
        )
    }

    fn exec_once(
        &self,
        name: &str,
        code: &str,
        timeout: Duration,
    ) -> Result<ExecOutput, ColabError> {
        self.call_as_opts(
            json!({"op": "exec", "name": name, "code": code, "timeout": timeout.as_secs_f64()}),
            timeout + EXEC_SLACK,
            false,
        )
    }

    fn upload(&self, name: &str, local: &Path, remote: &str) -> Result<(), ColabError> {
        let _: Value = self.call(
            json!({"op": "upload", "name": name, "local": local.to_string_lossy(), "remote": remote}),
            FILE_TIMEOUT,
        )?;
        Ok(())
    }

    fn download(&self, name: &str, remote: &str, local: &Path) -> Result<(), ColabError> {
        let _: Value = self.call(
            json!({"op": "download", "name": name, "remote": remote, "local": local.to_string_lossy()}),
            FILE_TIMEOUT,
        )?;
        Ok(())
    }
}

/// Interactive `colab login` (copy-paste OAuth): runs the bridge script with
/// `--login` and inherited stdio. The caller checks for a TTY first.
pub fn run_login() -> std::io::Result<std::process::ExitStatus> {
    pybridge::run_script_interactive(SCRIPT_NAME, BRIDGE_PY, &["--login".to_string()], &[])
}

#[cfg(any(test, feature = "mock"))]
pub use fake::{FakeBridge, FakeCall};

#[cfg(any(test, feature = "mock"))]
mod fake {
    use super::*;
    use crate::cmd;
    use std::collections::{HashMap, VecDeque};
    use std::sync::{Arc, Mutex};

    /// One recorded bridge call.
    #[derive(Debug, Clone, PartialEq)]
    pub enum FakeCall {
        Ping,
        Whoami,
        SessionNew {
            name: String,
            gpu: String,
            high_mem: bool,
        },
        SessionStop {
            name: String,
        },
        Sessions,
        Exec {
            name: String,
            code: String,
            timeout: Duration,
        },
        Upload {
            name: String,
            local: String,
            remote: String,
        },
        Download {
            name: String,
            remote: String,
            local: String,
        },
    }

    #[derive(Default)]
    struct Inner {
        calls: Vec<FakeCall>,
        /// Snippets sent through `exec_once` (also present in `calls`).
        once: Vec<String>,
        /// (substring of the snippet, scripted answers); the last answer repeats.
        rules: Vec<(String, VecDeque<Result<ExecOutput, String>>)>,
        /// Remote files served by the default `tail` answer and written by `download`.
        files: HashMap<String, Vec<u8>>,
        /// Answer of the default `glob` response.
        glob: Vec<String>,
        alive: String,
        sessions: Vec<SessionInfo>,
        logged_in: bool,
        fail_session_new: Option<String>,
    }

    /// Records every call and answers `exec` from scripted rules, falling back
    /// to a plausible answer per snippet kind (sh ok, spawn pid 4242, tail from
    /// `set_file`, glob from `set_glob`, alive from `set_alive`). Clones share
    /// state, so a test keeps one handle and gives the adapter another.
    #[derive(Clone)]
    pub struct FakeBridge(Arc<Mutex<Inner>>);

    impl Default for FakeBridge {
        fn default() -> Self {
            Self::new()
        }
    }

    impl FakeBridge {
        pub fn new() -> Self {
            FakeBridge(Arc::new(Mutex::new(Inner {
                alive: "alive".into(),
                logged_in: true,
                ..Default::default()
            })))
        }

        fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
            self.0.lock().unwrap()
        }

        pub fn calls(&self) -> Vec<FakeCall> {
            self.lock().calls.clone()
        }

        /// `exec` snippets sent so far, in order.
        pub fn exec_codes(&self) -> Vec<String> {
            self.calls()
                .into_iter()
                .filter_map(|c| match c {
                    FakeCall::Exec { code, .. } => Some(code),
                    _ => None,
                })
                .collect()
        }

        /// Snippets sent through the no-replay `exec_once`, in order.
        pub fn exec_once_codes(&self) -> Vec<String> {
            self.lock().once.clone()
        }

        /// Answer snippets containing `needle` with `out` (repeatable: later
        /// answers queue up, the last one sticks).
        pub fn script_exec(&self, needle: &str, out: ExecOutput) {
            self.push_rule(needle, Ok(out));
        }

        pub fn script_exec_err(&self, needle: &str, msg: &str) {
            self.push_rule(needle, Err(msg.to_string()));
        }

        fn push_rule(&self, needle: &str, r: Result<ExecOutput, String>) {
            let mut g = self.lock();
            if let Some((_, q)) = g.rules.iter_mut().find(|(n, _)| n == needle) {
                q.push_back(r);
            } else {
                g.rules.push((needle.to_string(), VecDeque::from([r])));
            }
        }

        pub fn set_file(&self, path: &str, data: &[u8]) {
            self.lock().files.insert(path.to_string(), data.to_vec());
        }

        pub fn set_glob(&self, files: &[&str]) {
            self.lock().glob = files.iter().map(|s| s.to_string()).collect();
        }

        /// `alive`, `dead` or `no_pid`.
        pub fn set_alive(&self, state: &str) {
            self.lock().alive = state.to_string();
        }

        pub fn set_sessions(&self, s: Vec<SessionInfo>) {
            self.lock().sessions = s;
        }

        pub fn set_logged_in(&self, v: bool) {
            self.lock().logged_in = v;
        }

        pub fn fail_session_new(&self, msg: &str) {
            self.lock().fail_session_new = Some(msg.to_string());
        }
    }

    fn remote_err(msg: &str) -> ColabError {
        ColabError::Bridge(BridgeError::Remote {
            kind: pybridge::RemoteKind::Other,
            msg: msg.to_string(),
        })
    }

    /// The path literal right after `path, offset, limit = ` in a tail snippet.
    fn tail_args(code: &str) -> Option<(String, u64)> {
        let rest = code.split("path, offset, limit = ").nth(1)?;
        let line = rest.lines().next()?;
        let (path, rest) = line.split_once("\", ")?;
        let offset = rest.split(',').next()?.trim().parse().ok()?;
        Some((path.trim_start_matches('"').to_string(), offset))
    }

    impl ColabBridge for FakeBridge {
        fn ping(&self) -> Result<PingInfo, ColabError> {
            self.lock().calls.push(FakeCall::Ping);
            Ok(PingInfo {
                sdk_version: "fake".into(),
            })
        }

        fn whoami(&self) -> Result<WhoAmI, ColabError> {
            let mut g = self.lock();
            g.calls.push(FakeCall::Whoami);
            Ok(WhoAmI {
                logged_in: g.logged_in,
                usage: g
                    .logged_in
                    .then(|| "Current balance: 100 compute units".into()),
            })
        }

        fn session_new(
            &self,
            name: &str,
            gpu: &str,
            high_mem: bool,
        ) -> Result<SessionInfo, ColabError> {
            let mut g = self.lock();
            g.calls.push(FakeCall::SessionNew {
                name: name.into(),
                gpu: gpu.into(),
                high_mem,
            });
            if let Some(m) = g.fail_session_new.clone() {
                return Err(remote_err(&m));
            }
            let info = SessionInfo {
                name: name.into(),
                endpoint: format!("gpu-{name}-ep"),
                accelerator: gpu.to_uppercase(),
                variant: "GPU".into(),
            };
            g.sessions.push(info.clone());
            Ok(info)
        }

        fn session_stop(&self, name: &str) -> Result<bool, ColabError> {
            let mut g = self.lock();
            g.calls.push(FakeCall::SessionStop { name: name.into() });
            let before = g.sessions.len();
            g.sessions.retain(|s| s.name != name);
            Ok(g.sessions.len() != before)
        }

        fn sessions(&self) -> Result<Vec<SessionInfo>, ColabError> {
            let mut g = self.lock();
            g.calls.push(FakeCall::Sessions);
            Ok(g.sessions.clone())
        }

        fn exec_once(
            &self,
            name: &str,
            code: &str,
            timeout: Duration,
        ) -> Result<ExecOutput, ColabError> {
            self.lock().once.push(code.to_string());
            self.exec(name, code, timeout)
        }

        fn exec(
            &self,
            name: &str,
            code: &str,
            timeout: Duration,
        ) -> Result<ExecOutput, ColabError> {
            let mut g = self.lock();
            g.calls.push(FakeCall::Exec {
                name: name.into(),
                code: code.into(),
                timeout,
            });
            if let Some((_, q)) = g.rules.iter_mut().find(|(n, _)| code.contains(n.as_str())) {
                let r = if q.len() > 1 {
                    q.pop_front().unwrap()
                } else {
                    q.front().cloned().unwrap()
                };
                return r.map_err(|m| remote_err(&m));
            }
            let out = if code.contains(cmd::MARK_SH) {
                cmd::encode_sh(0, "", "")
            } else if code.contains(cmd::MARK_SPAWN) {
                format!(
                    "{}{{\"pid\": 4242, \"code\": 0, \"err\": \"\"}}\n",
                    cmd::MARK_SPAWN
                )
            } else if code.contains(cmd::MARK_TAIL) {
                let (path, offset) = tail_args(code).unwrap_or_default();
                let data = g.files.get(&path).cloned().unwrap_or_default();
                let size = data.len() as u64;
                let slice: &[u8] = if size > offset {
                    &data[offset as usize..]
                } else {
                    &[]
                };
                cmd::encode_tail(size, slice)
            } else if code.contains(cmd::MARK_GLOB) {
                let files: Vec<&str> = g.glob.iter().map(String::as_str).collect();
                cmd::encode_glob(&files)
            } else if code.contains(cmd::MARK_ALIVE) {
                format!("{}{}\n", cmd::MARK_ALIVE, g.alive)
            } else {
                String::new()
            };
            Ok(ExecOutput::stdout(out))
        }

        fn upload(&self, name: &str, local: &Path, remote: &str) -> Result<(), ColabError> {
            let mut g = self.lock();
            g.calls.push(FakeCall::Upload {
                name: name.into(),
                local: local.to_string_lossy().into_owned(),
                remote: remote.into(),
            });
            let data = std::fs::read(local)?;
            g.files.insert(remote.to_string(), data);
            Ok(())
        }

        fn download(&self, name: &str, remote: &str, local: &Path) -> Result<(), ColabError> {
            let mut g = self.lock();
            g.calls.push(FakeCall::Download {
                name: name.into(),
                remote: remote.into(),
                local: local.to_string_lossy().into_owned(),
            });
            let data = g
                .files
                .get(remote)
                .cloned()
                .unwrap_or_else(|| format!("content of {remote}").into_bytes());
            std::fs::write(local, data)?;
            Ok(())
        }
    }
}
