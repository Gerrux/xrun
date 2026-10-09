"""xrun <-> Google Colab bridge (embedded in the xrun-colab crate).

Serve mode (default): one JSON request per stdin line, one response line
"<<<XRUN_BRIDGE>>>{json}" on stdout. Everything else printed by libraries goes
to stderr. `--login` mode: interactive copy-paste OAuth flow with inherited
stdio, then "login ok".

Uses only the colab_cli library modules (never colab_cli.cli / console), so it
also runs on Windows. Mirrors colab_cli commands/session.py (new/stop/sessions)
and commands/execution.py (exec).
"""
import importlib
import json
import os
import sys
import uuid

SENTINEL = "<<<XRUN_BRIDGE>>>"
PRELUDE = "import os; os.makedirs('/content', exist_ok=True); os.chdir('/content')"
LOGIN_HINT = "not logged in to Google Colab: run `xrun config login colab` in a terminal"
PKG = "google-colab-cli"


class BridgeError(Exception):
    def __init__(self, msg, kind="other"):
        Exception.__init__(self, msg)
        self.kind = kind


def _token_path():
    try:
        from colab_cli.auth import TOKEN_CONFIG_PATH

        return TOKEN_CONFIG_PATH
    except Exception:
        return os.path.expanduser("~/.config/colab-cli/token.json")


_L = {}


def _lib():
    if not _L:
        try:
            _L["client"] = importlib.import_module("colab_cli.client")
            _L["runtime"] = importlib.import_module("colab_cli.runtime")
            _L["contents"] = importlib.import_module("colab_cli.contents")
            _L["sstate"] = importlib.import_module("colab_cli.state")
            _L["utils"] = importlib.import_module("colab_cli.utils")
            _L["state"] = importlib.import_module("colab_cli.common").state
        except Exception as e:  # ImportError and anything a broken install raises
            _L.clear()
            raise BridgeError("%s is not importable (run `xrun install sdk colab`, or pip install %s): %s" % (PKG, PKG, e))
    return _L


def _require_login():
    if not os.path.exists(_token_path()):
        raise BridgeError(LOGIN_HINT, "auth")


def _session(name):
    _require_login()
    s = _lib()["state"].get_session(name, ignore_missing_session=True)
    if s is None:
        raise BridgeError("colab session '%s' not found" % name, "not_found")
    return s


def op_ping(req):
    _lib()
    try:
        from importlib import metadata

        ver = metadata.version(PKG)
    except Exception:
        ver = "unknown"
    return {"pong": True, "sdk_version": ver}


def op_whoami(req):
    if not os.path.exists(_token_path()):
        return {"logged_in": False, "usage": None}
    from colab_cli.consumption import format_consumption_status

    info = _lib()["state"].client.get_consumption_user_info()
    return {"logged_in": True, "usage": format_consumption_status(info)}


def op_session_new(req):
    _require_login()
    L = _lib()
    c, state, sstate = L["client"], L["state"], L["sstate"]
    name = req["name"]
    gpu = (req.get("gpu") or "cpu").strip().lower()
    high_mem = bool(req.get("high_mem"))
    if gpu in ("cpu", "none", ""):
        variant, acc = c.Variant.DEFAULT, c.Accelerator.NONE
    else:
        try:
            acc = c.Accelerator[gpu.upper()]
        except KeyError:
            raise BridgeError("unknown colab gpu %r" % gpu)
        variant = c.Variant.GPU
    shape = c.resolve_assign_shape(acc, high_mem=high_mem)
    try:
        res = state.client.assign(uuid.uuid4(), variant=variant, accelerator=acc, shape=shape)
    except c.TooManyAssignmentsError:
        raise BridgeError(
            "Colab allocation refused (precondition failed): too many active sessions, or a "
            "temporary usage/capacity limit for this runtime. Stop a session, wait and retry, "
            "or try another accelerator.",
            "busy",
        )
    except c.ColabRequestError as e:
        if L["utils"].get_status_code(e) == 400 and acc != c.Accelerator.NONE:
            raise BridgeError(
                "Colab rejected accelerator '%s': no quota or entitlement for it on this "
                "account. Try another gpu (T4) or gpu: cpu." % acc.value
            )
        raise
    info = getattr(res, "runtime_proxy_info", None)
    s = sstate.SessionState(
        name=name,
        token=info.token if info is not None else getattr(res, "runtime_proxy_token", ""),
        url=info.url if info is not None else "",
        endpoint=res.endpoint,
        token_expires_at=info.expires_at() if info is not None else None,
        variant=variant.value,
        accelerator=acc.value,
        machine_shape=(c.Shape.HIGH_RAM.name if shape == c.Shape.HIGH_RAM else c.Shape.STANDARD.name),
    )
    state.store.add(s)
    return {"endpoint": s.endpoint, "accelerator": s.accelerator, "variant": s.variant}


def op_session_stop(req):
    _require_login()
    L = _lib()
    state = L["state"]
    name = req["name"]
    s = state.get_session(name, ignore_missing_session=True)
    if s is None:
        return {"stopped": False}
    try:
        rt = L["runtime"].ColabRuntime(s.url, s.token, kernel_id=s.kernel_id)
        rt.stop(shutdown_kernel=True)
    except Exception:
        pass
    state.client.unassign(s.endpoint)
    state.store.remove(name)
    return {"stopped": True}


def op_sessions(req):
    _require_login()
    state = _lib()["state"]
    # State.sync_sessions() caches the local store on first use and never
    # re-reads it; in this long-lived process that would hide sessions
    # created (or pruned) after the first listing.
    state._sessions = None
    sessions, assignments = state.sync_sessions()
    by_endpoint = {s.endpoint: s.name for s in sessions.values()}
    return [
        {
            "name": by_endpoint.get(a.endpoint, "?"),
            "endpoint": a.endpoint,
            "accelerator": a.accelerator.value,
            "variant": a.variant.name,
        }
        for a in assignments
    ]


def op_exec(req):
    s = _session(req["name"])
    L = _lib()
    state = L["state"]

    def on_kernel(kid):
        s.kernel_id = kid
        state.store.add(s)

    def on_session(sid):
        s.session_id = sid
        state.store.add(s)

    rt = L["runtime"].ColabRuntime(
        s.url,
        s.token,
        kernel_id=s.kernel_id,
        session_id=s.session_id,
        on_kernel_started=on_kernel,
        on_session_started=on_session,
    )
    try:
        try:
            rt.execute_code(PRELUDE)
        except Exception as e:
            if L["utils"].is_terminal_error(e):
                state.prune_session(req["name"])
                raise BridgeError("colab session '%s' is gone (404/401)" % req["name"], "not_found")
            raise
        timeout = req.get("timeout")
        outputs = rt.execute_code(
            req["code"],
            output_hook=lambda o: None,
            timeout=float(timeout) if timeout else None,
        )
    finally:
        rt.stop()
    out, err, error = [], [], None
    for o in outputs or []:
        kind = o.get("output_type")
        if kind == "stream":
            (err if o.get("name") == "stderr" else out).append(o.get("text") or "")
        elif kind == "error":
            error = "%s: %s" % (o.get("ename") or "Error", o.get("evalue") or "")
    return {"stdout": "".join(out), "stderr": "".join(err), "error": error}


def _remote(path):
    # The Jupyter contents root is "/", the CLI addresses files as "content/...".
    return path.lstrip("/")


def op_upload(req):
    s = _session(req["name"])
    if not os.path.isfile(req["local"]):
        raise BridgeError("local file not found: %s" % req["local"], "not_found")
    _lib()["contents"].ContentsClient(s).upload(req["local"], _remote(req["remote"]))
    return {"ok": True}


def op_download(req):
    s = _session(req["name"])
    try:
        _lib()["contents"].ContentsClient(s).download(_remote(req["remote"]), req["local"])
    except FileNotFoundError as e:
        raise BridgeError(str(e), "not_found")
    return {"ok": True}


OPS = {
    "ping": op_ping,
    "whoami": op_whoami,
    "session_new": op_session_new,
    "session_stop": op_session_stop,
    "sessions": op_sessions,
    "exec": op_exec,
    "upload": op_upload,
    "download": op_download,
}


def _classify(e):
    if isinstance(e, BridgeError):
        return e.kind
    if type(e).__name__ in ("RefreshError", "DefaultCredentialsError", "TransportError"):
        return "auth"
    try:
        code = _lib()["utils"].get_status_code(e)
    except Exception:
        code = None
    if code in (401, 403):
        return "auth"
    if code == 404:
        return "not_found"
    return "other"


def serve():
    import builtins

    out = sys.stdout
    sys.stdout = sys.stderr  # library chatter (typer.echo) must not look like a response

    def no_input(prompt=""):
        raise BridgeError(LOGIN_HINT, "auth")

    builtins.input = no_input
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            req = json.loads(line)
            fn = OPS.get(req.get("op"))
            if fn is None:
                raise BridgeError("unknown op %r" % req.get("op"))
            resp = {"ok": True, "result": fn(req)}
        except (Exception, SystemExit) as e:
            resp = {"ok": False, "error": "%s: %s" % (type(e).__name__, e) if not isinstance(e, BridgeError) else str(e), "kind": _classify(e)}
        out.write(SENTINEL + json.dumps(resp) + "\n")
        out.flush()


def login():
    from colab_cli.common import state

    state.client  # first access runs the interactive OAuth flow
    print("login ok")


if __name__ == "__main__":
    if "--login" in sys.argv[1:]:
        try:
            login()
        except BaseException as e:  # incl. SystemExit from the auth flow
            print("login failed: %s" % e, file=sys.stderr)
            sys.exit(1)
    else:
        serve()
