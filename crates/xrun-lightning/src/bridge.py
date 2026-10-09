"""xrun <-> Lightning AI bridge (Python 3.9+).

Persistent child driven by xrun_core::pybridge: one JSON request per stdin
line, one `<<<XRUN_BRIDGE>>>{json}` answer on stdout. Studio objects are
cached per (name, teamspace) for the life of the process.
"""
import json
import os
import posixpath
import sys
import time

SENTINEL = "<<<XRUN_BRIDGE>>>"
_studios = {}
# How long studio_start waits for a Pending/Stopping studio to settle.
_SETTLE_SECS = float(os.environ.get("XRUN_LIGHTNING_SETTLE_SECS", "600"))
_POLL_SECS = float(os.environ.get("XRUN_LIGHTNING_POLL_SECS", "5"))
# Ops that may run without Lightning credentials.
_NO_AUTH_OPS = ("ping",)


def _require_auth():
    """Without an API key / token / login file the SDK starts a browser login
    flow (local web server + webbrowser.open) and blocks. A headless bridge
    must fail fast instead. Only existence is checked, never content."""
    if os.environ.get("LIGHTNING_API_KEY") or os.environ.get("LIGHTNING_AUTH_TOKEN"):
        return
    path = os.environ.get("LIGHTNING_CREDENTIAL_PATH") or os.path.join(
        os.path.expanduser("~"), ".lightning", "credentials.json"
    )
    if os.path.exists(path):
        return
    raise PermissionError(
        "not authenticated to Lightning AI: no API key and no ~/.lightning/credentials.json"
    )


def _user():
    """The authenticated user. `User()` without a name only works when
    LIGHTNING_USERNAME or the `user.name` config key is set, which an
    api_key + user_id setup does not have."""
    from lightning_sdk import User

    try:
        return User()
    except ValueError:
        pass
    try:
        from lightning_sdk.utils.resolve import _get_authed_user

        return _get_authed_user()
    except ImportError:
        from lightning_sdk.api.user_api import UserApi

        return User(name=UserApi()._client.auth_service_get_user().username)


def _classify(exc):
    msg = str(exc).lower()
    if any(w in msg for w in ("authenticat", "api key", "api_key", "401", "403", "unauthorized", "credentials")):
        return "auth"
    if "not found" in msg or "404" in msg or "does not exist" in msg:
        return "not_found"
    return "other"


def _status(obj):
    s = obj.status
    return str(getattr(s, "name", s)).split(".")[-1].lower()


def _ts_str(t):
    owner = getattr(getattr(t, "owner", None), "name", None)
    return "%s/%s" % (owner, t.name) if owner else str(t.name)


def _default_teamspace():
    """Explicit/env teamspaces are handled by the SDK; this is the last resort."""
    teamspaces = list(_user().teamspaces)
    if not teamspaces:
        raise RuntimeError("no Lightning teamspace found for this user")
    return _ts_str(teamspaces[0])


def _studio(a, create_ok=False):
    """Studio for a request. Only `studio_start` may create one: every other
    op on a missing studio (stale handle) is a not-found error, not a new
    empty Studio."""
    from lightning_sdk import Studio

    name, ts = a["studio"], a.get("teamspace") or None
    key = (name, ts)
    if key not in _studios:
        try:
            _studios[key] = Studio(name=name, teamspace=ts, create_ok=create_ok)
        except ValueError as exc:
            # Nothing configured anywhere (SDK: "Couldn't resolve teamspace
            # ..."). A configured-but-wrong teamspace must surface, not
            # silently fall back to another one.
            if ts is None and "couldn't resolve teamspace" in str(exc).lower():
                _studios[key] = Studio(
                    name=name, teamspace=_default_teamspace(), create_ok=create_ok
                )
            else:
                raise
    return _studios[key]


def _upload_one(st, local, remote):
    """`Studio.upload_file` runs `os.path.normpath` on the remote path, which
    on a Windows host turns `xrun/R/a.txt` into `xrun\\R\\a.txt` and the blob
    lands under a backslash name. Send a POSIX path through the studio API
    directly when the host separator is not `/`."""
    remote = posixpath.normpath(remote.replace("\\", "/")).strip("/") if remote else ""
    if not remote or remote == ".":
        remote = os.path.basename(local)
    api = getattr(st, "_studio_api", None)
    if os.sep != "/" and api is not None and hasattr(api, "upload_file"):
        api.upload_file(
            studio_id=st.id,
            teamspace_id=st.teamspace.id,
            cloud_account=st.cloud_account,
            file_path=local,
            remote_path=remote,
            progress_bar=False,
        )
    else:
        st.upload_file(local, remote, progress_bar=False)


def _machine(name):
    from lightning_sdk import Machine

    if hasattr(Machine, "from_str"):
        return Machine.from_str(name)
    return getattr(Machine, name, name)


def _ts_of(st):
    ts = getattr(st, "teamspace", None)
    return _ts_str(ts) if ts is not None else None


def op_ping(a):
    import lightning_sdk  # no auth involved

    return {"pong": True, "sdk_version": str(getattr(lightning_sdk, "__version__", "unknown"))}


def op_whoami(a):
    user = _user()
    want = a.get("teamspace") or os.environ.get("LIGHTNING_TEAMSPACE") or None
    if want is None:
        teamspaces = list(user.teamspaces)
        want = _ts_str(teamspaces[0]) if teamspaces else None
    return {"user": str(user.name), "teamspace": want}


def op_studio_start(a):
    st = _studio(a, create_ok=True)
    # `start` refuses a Pending / Stopping studio (e.g. a stop from the
    # previous run still in flight): wait for it to settle first.
    deadline = time.time() + _SETTLE_SECS
    while _status(st) in ("pending", "stopping") and time.time() < deadline:
        time.sleep(_POLL_SECS)
    if _status(st) != "running":
        kw = {}
        if a.get("interruptible") is not None:
            kw["interruptible"] = bool(a["interruptible"])
        if a.get("max_runtime"):
            kw["max_runtime"] = int(a["max_runtime"])
        st.start(_machine(a.get("machine") or "T4"), **kw)
    return {
        "studio_id": str(st.id),
        "name": str(st.name),
        "machine": str(getattr(st.machine, "name", st.machine)),
        "status": _status(st),
        "teamspace": _ts_of(st),
    }


def op_studio_status(a):
    return {"status": _status(_studio(a))}


def op_studio_stop(a):
    st = _studio(a)
    status = _status(st)
    # The SDK raises on a studio that is not Running / Pending; an already
    # stopped one (interruptible reclaim, 4 h restart, manual stop) is the
    # goal state, not an error.
    if status not in ("running", "pending"):
        return {"stopped": False, "status": status}
    st.stop()
    return {"stopped": True, "status": status}


def op_run(a):
    out, code = _studio(a).run_with_exit_code(a["cmd"])
    return {"output": out or "", "exit_code": int(code)}


def op_upload_file(a):
    _upload_one(_studio(a), a["local"], a.get("remote") or "")
    return {}


def op_upload_folder(a):
    st = _studio(a)
    folder = os.path.normpath(a["local"])
    if not os.path.isdir(folder):
        raise NotADirectoryError("not a directory: %s" % folder)
    root = (a.get("remote") or "").replace("\\", "/").strip("/")
    for dirpath, _dirs, files in os.walk(folder):
        for fname in sorted(files):
            path = os.path.join(dirpath, fname)
            rel = os.path.relpath(path, folder).replace(os.sep, "/")
            _upload_one(st, path, posixpath.join(root, rel) if root else rel)
    return {}


def op_download_file(a):
    parent = os.path.dirname(os.path.abspath(a["local"]))
    os.makedirs(parent, exist_ok=True)
    _studio(a).download_file(a["remote"], a["local"])
    return {}


def op_list_studios(a):
    want = a.get("teamspace") or os.environ.get("LIGHTNING_TEAMSPACE") or None
    out = []
    for t in _user().teamspaces:
        if want and want not in (_ts_str(t), str(t.name)):
            continue
        for s in t.studios:
            try:
                machine = str(getattr(s.machine, "name", s.machine))
            except Exception:  # noqa: BLE001
                machine = ""
            out.append({"name": str(s.name), "status": _status(s), "machine": machine})
    return {"studios": out}


OPS = {k[3:]: v for k, v in dict(globals()).items() if k.startswith("op_")}


def serve():
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            req = json.loads(line)
            fn = OPS.get(req.get("op"))
            if fn is None:
                raise ValueError("unknown op: %s" % req.get("op"))
            if req.get("op") not in _NO_AUTH_OPS:
                _require_auth()
            resp = {"ok": True, "result": fn(req)}
        except Exception as exc:  # noqa: BLE001
            resp = {"ok": False, "error": "%s: %s" % (type(exc).__name__, exc), "kind": _classify(exc)}
        print(SENTINEL + json.dumps(resp), flush=True)


if __name__ == "__main__":
    os.environ.setdefault("LIGHTNING_DISABLE_VERSION_CHECK", "1")
    serve()
