//! The real `bridge.py` against a stub `colab_cli` package that keeps the
//! library contract the bridge depends on (google-colab-cli 0.7.4):
//! `State.sync_sessions()` caches the local session store on its first call
//! and only re-lists the server assignments afterwards. No network, no OAuth;
//! skipped without a Python.

use std::path::PathBuf;
use std::time::Duration;

use serde_json::{json, Value};
use xrun_colab::BRIDGE_PY;
use xrun_core::pybridge::{find_python, PyBridge};

const T: Duration = Duration::from_secs(30);

const STUB_COMMON: &str = r#"
import json, os


class _Named:
    def __init__(self, v):
        self.value = v
        self.name = v


class _Assignment:
    def __init__(self, endpoint):
        self.endpoint = endpoint
        self.accelerator = _Named("T4")
        self.variant = _Named("GPU")


class _Session:
    def __init__(self, name, endpoint):
        self.name = name
        self.endpoint = endpoint


def _store():
    with open(os.environ["STUB_SESSIONS"]) as f:
        return json.load(f)


class State:
    def __init__(self):
        self._sessions = None

    def sync_sessions(self):
        # Same caching as colab_cli.common.State.sync_sessions.
        if self._sessions is not None:
            return self._sessions, [_Assignment(e) for e in _store().values()]
        self._sessions = {n: _Session(n, e) for n, e in _store().items()}
        return self._sessions, [_Assignment(e) for e in _store().values()]


state = State()
"#;

struct Stub {
    _td: tempfile::TempDir,
    root: PathBuf,
    sessions: PathBuf,
}

impl Stub {
    fn new() -> Stub {
        let td = tempfile::tempdir().unwrap();
        let root = td.path().to_path_buf();
        let pkg = root.join("colab_cli");
        std::fs::create_dir_all(&pkg).unwrap();
        for m in [
            "__init__", "client", "runtime", "contents", "state", "utils",
        ] {
            std::fs::write(pkg.join(format!("{m}.py")), "").unwrap();
        }
        std::fs::write(pkg.join("common.py"), STUB_COMMON).unwrap();
        let token = root.join("token.json");
        std::fs::write(&token, "{}").unwrap();
        std::fs::write(
            pkg.join("auth.py"),
            format!("TOKEN_CONFIG_PATH = {:?}\n", token.display().to_string()),
        )
        .unwrap();
        let sessions = root.join("sessions.json");
        Stub {
            _td: td,
            root,
            sessions,
        }
    }

    fn set_sessions(&self, v: Value) {
        std::fs::write(&self.sessions, v.to_string()).unwrap();
    }

    fn bridge(&self) -> PyBridge {
        PyBridge::spawn(
            "colab_bridge_stubtest",
            BRIDGE_PY,
            vec![
                ("PYTHONPATH".to_string(), self.root.display().to_string()),
                (
                    "STUB_SESSIONS".to_string(),
                    self.sessions.display().to_string(),
                ),
            ],
        )
        .expect("spawn bridge")
    }
}

fn names(v: &Value) -> Vec<String> {
    let mut n: Vec<String> = v
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["name"].as_str().unwrap().to_string())
        .collect();
    n.sort();
    n
}

#[test]
fn sessions_listing_sees_sessions_created_after_the_first_call() {
    if find_python().is_none() {
        eprintln!("skip: no python interpreter");
        return;
    }
    let stub = Stub::new();
    stub.set_sessions(json!({"xrun-a": "ep-a"}));
    let b = stub.bridge();
    let first = b.call(json!({"op": "sessions"}), T).expect("sessions");
    assert_eq!(names(&first), vec!["xrun-a"]);

    // A later run of the same long-lived bridge (launch, then poll-daemon
    // ticks) adds a session: it must be named, not "?".
    stub.set_sessions(json!({"xrun-a": "ep-a", "xrun-b": "ep-b"}));
    let second = b.call(json!({"op": "sessions"}), T).expect("sessions");
    assert_eq!(names(&second), vec!["xrun-a", "xrun-b"]);
}
