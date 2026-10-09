#![deny(unsafe_code)]

//! The seam between the adapter and the `lightning-sdk` Python package.
//! [`LightningBridge`] is what the adapter needs; [`PyLightningBridge`] is the
//! production implementation over a persistent Python child, and `FakeBridge`
//! (tests / `mock` feature) is an in-memory stand-in.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Deserialize;
use serde_json::{json, Value};
use xrun_core::config::credentials::LightningCredentials;
use xrun_core::pybridge::PyBridge;

use crate::error::LightningError;

pub const BRIDGE_SCRIPT: &str = include_str!("bridge.py");
pub const SCRIPT_NAME: &str = "lightning_bridge";

pub const T_PING: Duration = Duration::from_secs(60);
pub const T_START: Duration = Duration::from_secs(20 * 60);
pub const T_SETUP: Duration = Duration::from_secs(2 * 3600);
pub const T_QUICK: Duration = Duration::from_secs(90);
pub const T_TRANSFER: Duration = Duration::from_secs(3600);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StudioRef {
    pub name: String,
    /// `owner/name`; `None` lets the bridge resolve the default teamspace.
    pub teamspace: Option<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct SdkInfo {
    pub sdk_version: String,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct WhoAmI {
    pub user: String,
    #[serde(default)]
    pub teamspace: Option<String>,
    /// `owner/name` slugs of every teamspace the user belongs to (default
    /// first); what `lightning.teamspace` may be set to.
    #[serde(default)]
    pub teamspaces: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartParams {
    pub machine: String,
    pub interruptible: bool,
    pub max_runtime: Option<u64>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct StudioInfo {
    pub studio_id: String,
    pub name: String,
    #[serde(default)]
    pub machine: String,
    #[serde(default)]
    pub status: String,
    /// Teamspace the SDK resolved (useful when none was given explicitly).
    #[serde(default)]
    pub teamspace: Option<String>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct RunOut {
    pub output: String,
    pub exit_code: i32,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
pub struct StudioEntry {
    pub name: String,
    pub status: String,
    #[serde(default)]
    pub machine: String,
}

pub trait LightningBridge: Send {
    fn ping(&self) -> Result<SdkInfo, LightningError>;
    fn whoami(&self, teamspace: Option<&str>) -> Result<WhoAmI, LightningError>;
    fn studio_start(
        &self,
        studio: &StudioRef,
        params: &StartParams,
    ) -> Result<StudioInfo, LightningError>;
    fn studio_status(&self, studio: &StudioRef) -> Result<String, LightningError>;
    fn studio_stop(&self, studio: &StudioRef) -> Result<(), LightningError>;
    /// Idempotent / probe command; the bridge may replay it after a child crash.
    fn run(
        &self,
        studio: &StudioRef,
        cmd: &str,
        timeout: Duration,
    ) -> Result<RunOut, LightningError>;
    /// Non-idempotent command (the training launch, the kill script): never
    /// replayed after the child died with the request already written.
    fn run_once(
        &self,
        studio: &StudioRef,
        cmd: &str,
        timeout: Duration,
    ) -> Result<RunOut, LightningError>;
    fn upload_file(
        &self,
        studio: &StudioRef,
        local: &Path,
        remote: &str,
    ) -> Result<(), LightningError>;
    fn upload_folder(
        &self,
        studio: &StudioRef,
        local: &Path,
        remote: &str,
    ) -> Result<(), LightningError>;
    fn download_file(
        &self,
        studio: &StudioRef,
        remote: &str,
        local: &Path,
    ) -> Result<(), LightningError>;
    fn list_studios(&self, teamspace: Option<&str>) -> Result<Vec<StudioEntry>, LightningError>;
}

// ---------------------------------------------------------------------------
// production implementation
// ---------------------------------------------------------------------------

/// Spawns one Python child on first use and keeps it for the adapter's life.
pub struct PyLightningBridge {
    env: Vec<(String, String)>,
    bridge: Mutex<Option<Arc<PyBridge>>>,
}

impl PyLightningBridge {
    pub fn new(creds: &LightningCredentials) -> Self {
        let mut env = vec![(
            "LIGHTNING_DISABLE_VERSION_CHECK".to_string(),
            "1".to_string(),
        )];
        fn set(s: &Option<String>) -> Option<&str> {
            s.as_deref().filter(|v| !v.trim().is_empty())
        }
        if let (Some(key), Some(user)) = (set(&creds.api_key), set(&creds.user_id)) {
            env.push(("LIGHTNING_API_KEY".into(), key.to_string()));
            env.push(("LIGHTNING_USER_ID".into(), user.to_string()));
        }
        if let Some(ts) = set(&creds.teamspace) {
            env.push(("LIGHTNING_TEAMSPACE".into(), ts.to_string()));
        }
        Self {
            env,
            bridge: Mutex::new(None),
        }
    }

    fn handle(&self) -> Result<Arc<PyBridge>, LightningError> {
        let mut slot = self
            .bridge
            .lock()
            .map_err(|_| LightningError::Bridge("bridge mutex poisoned".into()))?;
        if let Some(b) = slot.as_ref() {
            return Ok(Arc::clone(b));
        }
        let b = Arc::new(PyBridge::spawn(
            SCRIPT_NAME,
            BRIDGE_SCRIPT,
            self.env.clone(),
        )?);
        *slot = Some(Arc::clone(&b));
        Ok(b)
    }

    fn call(&self, req: Value, timeout: Duration) -> Result<Value, LightningError> {
        Ok(self.handle()?.call(req, timeout)?)
    }

    /// Never replayed after the request reached a child that then died.
    fn call_once(&self, req: Value, timeout: Duration) -> Result<Value, LightningError> {
        Ok(self.handle()?.call_opts(req, timeout, false)?)
    }

    fn call_as<T: for<'de> Deserialize<'de>>(
        &self,
        req: Value,
        timeout: Duration,
    ) -> Result<T, LightningError> {
        decode(self.call(req, timeout)?)
    }

    fn call_once_as<T: for<'de> Deserialize<'de>>(
        &self,
        req: Value,
        timeout: Duration,
    ) -> Result<T, LightningError> {
        decode(self.call_once(req, timeout)?)
    }
}

fn decode<T: for<'de> Deserialize<'de>>(v: Value) -> Result<T, LightningError> {
    serde_json::from_value(v).map_err(|e| LightningError::Protocol(e.to_string()))
}

fn target(op: &str, s: &StudioRef) -> serde_json::Map<String, Value> {
    let mut m = serde_json::Map::new();
    m.insert("op".into(), json!(op));
    m.insert("studio".into(), json!(s.name));
    m.insert("teamspace".into(), json!(s.teamspace));
    m
}

fn with(mut m: serde_json::Map<String, Value>, kv: &[(&str, Value)]) -> Value {
    for (k, v) in kv {
        m.insert((*k).to_string(), v.clone());
    }
    Value::Object(m)
}

fn path_str(p: &Path) -> String {
    p.display().to_string()
}

impl LightningBridge for PyLightningBridge {
    fn ping(&self) -> Result<SdkInfo, LightningError> {
        self.call_as(json!({"op": "ping"}), T_PING)
    }

    fn whoami(&self, teamspace: Option<&str>) -> Result<WhoAmI, LightningError> {
        self.call_as(json!({"op": "whoami", "teamspace": teamspace}), T_QUICK)
    }

    fn studio_start(
        &self,
        studio: &StudioRef,
        p: &StartParams,
    ) -> Result<StudioInfo, LightningError> {
        let req = with(
            target("studio_start", studio),
            &[
                ("machine", json!(p.machine)),
                ("interruptible", json!(p.interruptible)),
                ("max_runtime", json!(p.max_runtime)),
            ],
        );
        self.call_once_as(req, T_START)
    }

    fn studio_status(&self, studio: &StudioRef) -> Result<String, LightningError> {
        let v = self.call(Value::Object(target("studio_status", studio)), T_QUICK)?;
        v.get("status")
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| LightningError::Protocol(format!("studio_status: {v}")))
    }

    fn studio_stop(&self, studio: &StudioRef) -> Result<(), LightningError> {
        self.call_once(Value::Object(target("studio_stop", studio)), T_START)
            .map(|_| ())
    }

    fn run(
        &self,
        studio: &StudioRef,
        cmd: &str,
        timeout: Duration,
    ) -> Result<RunOut, LightningError> {
        let req = with(target("run", studio), &[("cmd", json!(cmd))]);
        self.call_as(req, timeout)
    }

    fn run_once(
        &self,
        studio: &StudioRef,
        cmd: &str,
        timeout: Duration,
    ) -> Result<RunOut, LightningError> {
        let req = with(target("run", studio), &[("cmd", json!(cmd))]);
        self.call_once_as(req, timeout)
    }

    fn upload_file(
        &self,
        studio: &StudioRef,
        local: &Path,
        remote: &str,
    ) -> Result<(), LightningError> {
        let req = with(
            target("upload_file", studio),
            &[("local", json!(path_str(local))), ("remote", json!(remote))],
        );
        self.call(req, T_TRANSFER).map(|_| ())
    }

    fn upload_folder(
        &self,
        studio: &StudioRef,
        local: &Path,
        remote: &str,
    ) -> Result<(), LightningError> {
        let req = with(
            target("upload_folder", studio),
            &[("local", json!(path_str(local))), ("remote", json!(remote))],
        );
        self.call(req, T_TRANSFER).map(|_| ())
    }

    fn download_file(
        &self,
        studio: &StudioRef,
        remote: &str,
        local: &Path,
    ) -> Result<(), LightningError> {
        let req = with(
            target("download_file", studio),
            &[("local", json!(path_str(local))), ("remote", json!(remote))],
        );
        self.call(req, T_TRANSFER).map(|_| ())
    }

    fn list_studios(&self, teamspace: Option<&str>) -> Result<Vec<StudioEntry>, LightningError> {
        #[derive(Deserialize)]
        struct Out {
            studios: Vec<StudioEntry>,
        }
        let out: Out = self.call_as(
            json!({"op": "list_studios", "teamspace": teamspace}),
            T_QUICK,
        )?;
        Ok(out.studios)
    }
}

// ---------------------------------------------------------------------------
// in-memory fake
// ---------------------------------------------------------------------------

#[cfg(any(test, feature = "mock"))]
pub use fake::{FakeBridge, FakeState};

#[cfg(any(test, feature = "mock"))]
mod fake {
    use std::collections::HashMap;

    use super::*;

    type RunFn = Box<dyn Fn(&str) -> RunOut + Send>;

    /// Everything the fake records and serves. Shared (`Arc<Mutex<_>>`) so a
    /// test keeps a handle after boxing the bridge into an adapter.
    pub struct FakeState {
        /// One line per call, e.g. `studio_start xrun-demo`, `run <cmd>`.
        pub calls: Vec<String>,
        /// Commands passed to `run`, in order.
        pub runs: Vec<String>,
        /// The subset of `runs` issued through `run_once` (no-replay).
        pub once_runs: Vec<String>,
        /// Answers `run`; default is empty output, exit 0.
        pub run_fn: RunFn,
        /// Remote path -> bytes served by `download_file`.
        pub remote_files: HashMap<String, Vec<u8>>,
        pub uploaded: Vec<(String, String)>,
        pub status: String,
        pub fail_start: bool,
    }

    #[derive(Clone)]
    pub struct FakeBridge {
        pub state: Arc<Mutex<FakeState>>,
    }

    impl Default for FakeBridge {
        fn default() -> Self {
            Self::new()
        }
    }

    impl FakeBridge {
        pub fn new() -> Self {
            Self {
                state: Arc::new(Mutex::new(FakeState {
                    calls: Vec::new(),
                    runs: Vec::new(),
                    once_runs: Vec::new(),
                    run_fn: Box::new(|_| RunOut {
                        output: String::new(),
                        exit_code: 0,
                    }),
                    remote_files: HashMap::new(),
                    uploaded: Vec::new(),
                    status: "running".to_string(),
                    fail_start: false,
                })),
            }
        }

        pub fn set_run_fn(&self, f: impl Fn(&str) -> RunOut + Send + 'static) {
            self.state.lock().unwrap().run_fn = Box::new(f);
        }

        pub fn calls(&self) -> Vec<String> {
            self.state.lock().unwrap().calls.clone()
        }

        fn rec(&self, line: String) {
            self.state.lock().unwrap().calls.push(line);
        }
    }

    impl LightningBridge for FakeBridge {
        fn ping(&self) -> Result<SdkInfo, LightningError> {
            self.rec("ping".into());
            Ok(SdkInfo {
                sdk_version: "fake".into(),
            })
        }

        fn whoami(&self, teamspace: Option<&str>) -> Result<WhoAmI, LightningError> {
            self.rec("whoami".into());
            Ok(WhoAmI {
                user: "tester".into(),
                teamspace: teamspace
                    .map(str::to_string)
                    .or(Some("tester/default".into())),
                teamspaces: vec!["tester/default".into()],
            })
        }

        fn studio_start(
            &self,
            studio: &StudioRef,
            p: &StartParams,
        ) -> Result<StudioInfo, LightningError> {
            self.rec(format!(
                "studio_start {} machine={} interruptible={} max_runtime={:?}",
                studio.name, p.machine, p.interruptible, p.max_runtime
            ));
            if self.state.lock().unwrap().fail_start {
                return Err(LightningError::Bridge("no capacity".into()));
            }
            Ok(StudioInfo {
                studio_id: "sid-1".into(),
                name: studio.name.clone(),
                machine: p.machine.clone(),
                status: "running".into(),
                teamspace: studio
                    .teamspace
                    .clone()
                    .or(Some("tester/default".to_string())),
            })
        }

        fn studio_status(&self, studio: &StudioRef) -> Result<String, LightningError> {
            self.rec(format!("studio_status {}", studio.name));
            Ok(self.state.lock().unwrap().status.clone())
        }

        fn studio_stop(&self, studio: &StudioRef) -> Result<(), LightningError> {
            self.rec(format!("studio_stop {}", studio.name));
            self.state.lock().unwrap().status = "stopped".into();
            Ok(())
        }

        fn run(
            &self,
            studio: &StudioRef,
            cmd: &str,
            _timeout: Duration,
        ) -> Result<RunOut, LightningError> {
            let mut st = self.state.lock().unwrap();
            st.calls.push(format!("run {} {cmd}", studio.name));
            st.runs.push(cmd.to_string());
            Ok((st.run_fn)(cmd))
        }

        fn run_once(
            &self,
            studio: &StudioRef,
            cmd: &str,
            timeout: Duration,
        ) -> Result<RunOut, LightningError> {
            self.state.lock().unwrap().once_runs.push(cmd.to_string());
            self.run(studio, cmd, timeout)
        }

        fn upload_file(
            &self,
            studio: &StudioRef,
            local: &Path,
            remote: &str,
        ) -> Result<(), LightningError> {
            let mut st = self.state.lock().unwrap();
            st.calls
                .push(format!("upload_file {} -> {remote}", studio.name));
            st.uploaded.push((path_str(local), remote.to_string()));
            Ok(())
        }

        fn upload_folder(
            &self,
            studio: &StudioRef,
            local: &Path,
            remote: &str,
        ) -> Result<(), LightningError> {
            let mut st = self.state.lock().unwrap();
            st.calls
                .push(format!("upload_folder {} -> {remote}", studio.name));
            st.uploaded.push((path_str(local), remote.to_string()));
            Ok(())
        }

        fn download_file(
            &self,
            studio: &StudioRef,
            remote: &str,
            local: &Path,
        ) -> Result<(), LightningError> {
            let bytes = {
                let mut st = self.state.lock().unwrap();
                st.calls
                    .push(format!("download_file {} {remote}", studio.name));
                st.remote_files.get(remote).cloned()
            };
            let bytes = bytes.ok_or_else(|| LightningError::NotFound(remote.to_string()))?;
            if let Some(parent) = local.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::write(local, bytes)?;
            Ok(())
        }

        fn list_studios(&self, _ts: Option<&str>) -> Result<Vec<StudioEntry>, LightningError> {
            self.rec("list_studios".into());
            Ok(Vec::new())
        }
    }
}
