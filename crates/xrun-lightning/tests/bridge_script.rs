//! The real `bridge.py` against a stub `lightning_sdk` package that mimics the
//! SDK contracts the bridge relies on (verified against lightning-sdk
//! 2026.10.1): `User()` without a name raises unless a username is configured,
//! `Studio(create_ok=False)` raises for a missing studio, `stop()` raises on a
//! studio that is not Running/Pending, `start()` refuses a Pending/Stopping
//! one, and `Studio.upload_file` normalises the remote path with the host's
//! `os.path`. No network, no credentials; skipped without a Python.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{json, Value};
use xrun_core::pybridge::{find_python, BridgeError, PyBridge, RemoteKind};
use xrun_lightning::bridge::BRIDGE_SCRIPT;

const T: Duration = Duration::from_secs(30);

const STUB_INIT: &str = r#"
import enum, json, os

__version__ = "0.0-stub"


def _log(*a):
    with open(os.environ["STUB_LOG"], "a") as f:
        f.write(json.dumps(a) + "\n")


def _statuses():
    return json.loads(os.environ.get("STUB_STATUSES", "{}"))


class Status(enum.Enum):
    NotCreated = "NotCreated"
    Pending = "Pending"
    Running = "Running"
    Stopping = "Stopping"
    Stopped = "Stopped"


_BY_NAME = {s.value.lower(): s for s in Status}


class Machine:
    def __init__(self, name):
        self.name = name

    @classmethod
    def from_str(cls, name):
        return cls(name)


class _Owner:
    def __init__(self, name):
        self.name = name


class Teamspace:
    def __init__(self, name, owner):
        self.name = name
        self.owner = _Owner(owner)
        self.id = "ts-" + name

    @property
    def studios(self):
        return []


class User:
    def __init__(self, name=None):
        if name is None:
            raise ValueError(
                "Neither name is provided nor can the user be inferred from the environment variable!"
            )
        _log("user", name)
        self.name = name

    @property
    def teamspaces(self):
        return [Teamspace("main", self.name)]


class _Api:
    def upload_file(self, **kw):
        _log("api_upload", kw["remote_path"], kw["file_path"])


class ApiException(Exception):
    """Shape of lightning_cloud's ApiException: `status`, `body`, and a str()
    that dumps the response headers (which mention `Authorization`)."""

    def __init__(self, status, body):
        super().__init__(status)
        self.status = status
        self.body = body

    def __str__(self):
        return (
            "(%s)\nReason: Bad Request\nHTTP response headers: HTTPHeaderDict({"
            "'access-control-allow-headers': 'Content-Type,Authorization', "
            "'Set-Cookie': 'session-id=abc'})\nHTTP response body: %r" % (self.status, self.body)
        )


NO_BALANCE_BODY = (
    b'{"code":3, "message":"creating cloud space instance: insufficient balance '
    b'to start the cloud space, top up and try again", "details":[]}'
)


class Studio:
    def __init__(self, name=None, teamspace=None, create_ok=True):
        _log("init", name, teamspace, create_ok)
        if teamspace is None:
            raise ValueError("Couldn't resolve teamspace from the provided name, org, or user")
        known = _statuses()
        if name not in known and not create_ok:
            raise ValueError("Studio '%s' does not exist." % name)
        self._seq = list(known.get(name, ["stopped"]))
        self.name = name
        self.id = "sid-" + name
        owner, ts = teamspace.split("/")
        self.teamspace = Teamspace(ts, owner)
        self.cloud_account = "ca"
        self._studio_api = _Api()
        self.machine = None

    @property
    def status(self):
        s = self._seq[0]
        if len(self._seq) > 1:
            self._seq.pop(0)
        return _BY_NAME[s]

    def start(self, machine, interruptible=None, max_runtime=None):
        st = self.status
        if st != Status.Stopped:
            raise RuntimeError("Cannot start a Studio that is not stopped. Studio is %s." % st)
        if machine.name == "NOBALANCE":
            raise ApiException(400, NO_BALANCE_BODY)
        _log("start", machine.name, interruptible, max_runtime)
        self._seq = ["running"]
        self.machine = machine

    def stop(self):
        if self.status not in (Status.Running, Status.Pending):
            raise RuntimeError("Cannot stop a studio that is not running.")
        _log("stop", self.name)
        self._seq = ["stopped"]

    def upload_file(self, file_path, remote_path=None, progress_bar=True):
        _log("upload_file", os.path.normpath(remote_path), file_path)

    def run_with_exit_code(self, *cmds):
        _log("run", list(cmds))
        return "ok", 0
"#;

const STUB_RESOLVE: &str = r#"
from lightning_sdk import User


def _get_authed_user():
    return User(name="alice")
"#;

struct Stub {
    _td: tempfile::TempDir,
    root: PathBuf,
    log: PathBuf,
}

impl Stub {
    fn new() -> Stub {
        let td = tempfile::tempdir().unwrap();
        let root = td.path().to_path_buf();
        let pkg = root.join("lightning_sdk");
        std::fs::create_dir_all(pkg.join("utils")).unwrap();
        std::fs::write(pkg.join("__init__.py"), STUB_INIT).unwrap();
        std::fs::write(pkg.join("utils").join("__init__.py"), "").unwrap();
        std::fs::write(pkg.join("utils").join("resolve.py"), STUB_RESOLVE).unwrap();
        let log = root.join("calls.jsonl");
        Stub { _td: td, root, log }
    }

    /// Bridge over the stub. `authed=false` hides every credential source.
    fn bridge(&self, statuses: Value, authed: bool) -> PyBridge {
        let mut env = vec![
            ("PYTHONPATH".to_string(), self.root.display().to_string()),
            ("STUB_LOG".to_string(), self.log.display().to_string()),
            ("STUB_STATUSES".to_string(), statuses.to_string()),
            ("XRUN_LIGHTNING_POLL_SECS".to_string(), "0.01".to_string()),
            ("LIGHTNING_TEAMSPACE".to_string(), String::new()),
            ("LIGHTNING_AUTH_TOKEN".to_string(), String::new()),
            (
                "LIGHTNING_CREDENTIAL_PATH".to_string(),
                self.root
                    .join("no-such-credentials.json")
                    .display()
                    .to_string(),
            ),
        ];
        env.push((
            "LIGHTNING_API_KEY".to_string(),
            if authed { "test-key-abc" } else { "" }.to_string(),
        ));
        PyBridge::spawn("lightning_bridge_stubtest", BRIDGE_SCRIPT, env).expect("spawn bridge")
    }

    fn calls(&self) -> Vec<Value> {
        std::fs::read_to_string(&self.log)
            .unwrap_or_default()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    fn calls_named(&self, name: &str) -> Vec<Value> {
        self.calls()
            .into_iter()
            .filter(|c| c[0] == json!(name))
            .collect()
    }
}

fn have_python() -> bool {
    if find_python().is_none() {
        eprintln!("skip: no python interpreter");
        return false;
    }
    true
}

fn req(op: &str, studio: &str) -> Value {
    json!({"op": op, "studio": studio, "teamspace": "alice/main"})
}

#[test]
fn whoami_and_default_teamspace_work_without_a_configured_username() {
    if !have_python() {
        return;
    }
    let stub = Stub::new();
    let b = stub.bridge(json!({}), true);
    let w = b.call(json!({"op": "whoami"}), T).expect("whoami");
    assert_eq!(w["user"], json!("alice"));
    assert_eq!(w["teamspace"], json!("alice/main"));

    // No teamspace anywhere: the bridge resolves the user's first one.
    let r = b
        .call(
            json!({"op": "studio_start", "studio": "s1", "teamspace": null, "machine": "T4"}),
            T,
        )
        .expect("start");
    assert_eq!(r["status"], json!("running"));
    assert_eq!(r["teamspace"], json!("alice/main"));
    let inits = stub.calls_named("init");
    assert_eq!(inits.last().unwrap()[2], json!("alice/main"));
}

#[test]
fn only_studio_start_may_create_a_studio() {
    if !have_python() {
        return;
    }
    let stub = Stub::new();
    let b = stub.bridge(json!({}), true);
    match b.call(req("studio_status", "ghost"), T).unwrap_err() {
        BridgeError::Remote { kind, msg } => {
            assert_eq!(kind, RemoteKind::NotFound, "{msg}");
        }
        other => panic!("unexpected {other:?}"),
    }
    assert_eq!(stub.calls_named("init")[0][3], json!(false));
    b.call(json!({"op": "studio_start", "studio": "fresh", "teamspace": "alice/main", "machine": "T4"}), T)
        .expect("start creates");
    assert_eq!(stub.calls_named("init")[1][3], json!(true));
}

#[test]
fn stop_of_an_already_stopped_studio_is_ok() {
    if !have_python() {
        return;
    }
    let stub = Stub::new();
    let b = stub.bridge(json!({"idle": ["stopped"], "busy": ["running"]}), true);
    let r = b.call(req("studio_stop", "idle"), T).expect("stop idle");
    assert_eq!(r["stopped"], json!(false));
    let r = b.call(req("studio_stop", "busy"), T).expect("stop busy");
    assert_eq!(r["stopped"], json!(true));
    let stops = stub.calls_named("stop");
    assert_eq!(stops.len(), 1);
    assert_eq!(stops[0][1], json!("busy"));
}

#[test]
fn insufficient_balance_is_not_an_auth_error_and_keeps_only_the_body_message() {
    if !have_python() {
        return;
    }
    let stub = Stub::new();
    let b = stub.bridge(json!({"s": ["stopped"]}), true);
    let mut r = req("studio_start", "s");
    r["machine"] = json!("NOBALANCE");
    match b.call(r, T).unwrap_err() {
        BridgeError::Remote { kind, msg } => {
            assert_eq!(kind, RemoteKind::Other, "{msg}");
            assert!(
                msg.contains("HTTP 400: creating cloud space instance: insufficient balance"),
                "{msg}"
            );
            assert!(
                !msg.contains("Set-Cookie") && !msg.contains("HTTPHeaderDict"),
                "{msg}"
            );
        }
        other => panic!("unexpected: {other:?}"),
    }
}

#[test]
fn start_waits_for_a_stopping_studio_to_settle() {
    if !have_python() {
        return;
    }
    let stub = Stub::new();
    let b = stub.bridge(
        json!({"s": ["stopping", "stopping", "stopping", "stopped"]}),
        true,
    );
    let r = b
        .call(
            json!({"op": "studio_start", "studio": "s", "teamspace": "alice/main",
                   "machine": "L4", "interruptible": true, "max_runtime": 600}),
            T,
        )
        .expect("start after settle");
    assert_eq!(r["status"], json!("running"));
    let starts = stub.calls_named("start");
    assert_eq!(starts.len(), 1);
    assert_eq!(starts[0][1], json!("L4"));
}

#[test]
fn uploads_use_posix_remote_paths() {
    if !have_python() {
        return;
    }
    let stub = Stub::new();
    let data = stub.root.join("data");
    std::fs::create_dir_all(data.join("sub")).unwrap();
    std::fs::write(data.join("a.txt"), "a").unwrap();
    std::fs::write(data.join("sub").join("b.txt"), "b").unwrap();
    let b = stub.bridge(json!({"s": ["running"]}), true);

    let mut folder = req("upload_folder", "s");
    folder["local"] = json!(path_str(&data));
    folder["remote"] = json!("xrun/R1/data");
    b.call(folder, T).expect("upload folder");

    let mut file = req("upload_file", "s");
    file["local"] = json!(path_str(&data.join("a.txt")));
    file["remote"] = json!("");
    b.call(file, T).expect("upload file without remote");

    let mut remotes: Vec<String> = stub
        .calls()
        .into_iter()
        .filter(|c| c[0] == json!("api_upload") || c[0] == json!("upload_file"))
        .map(|c| c[1].as_str().unwrap().to_string())
        .collect();
    remotes.sort();
    assert_eq!(
        remotes,
        vec!["a.txt", "xrun/R1/data/a.txt", "xrun/R1/data/sub/b.txt"]
    );
}

#[test]
fn without_credentials_ops_fail_fast_as_auth_but_ping_works() {
    if !have_python() {
        return;
    }
    let stub = Stub::new();
    let b = stub.bridge(json!({"s": ["running"]}), false);
    let pong = b
        .call(json!({"op": "ping"}), T)
        .expect("ping needs no auth");
    assert_eq!(pong["sdk_version"], json!("0.0-stub"));
    match b.call(json!({"op": "whoami"}), T).unwrap_err() {
        BridgeError::Remote { kind, .. } => assert_eq!(kind, RemoteKind::Auth),
        other => panic!("unexpected {other:?}"),
    }
    // The SDK was never asked to authenticate.
    assert!(stub.calls_named("user").is_empty());
    assert!(stub.calls_named("init").is_empty());
}

fn path_str(p: &Path) -> String {
    p.display().to_string()
}
