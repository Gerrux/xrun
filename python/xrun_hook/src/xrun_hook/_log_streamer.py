"""Best-effort tailer that pushes stdout chunks to an MLflow tracking server.

Used by the kaggle adapter to bridge the air gap: Kaggle exposes no live log
API, so xrun_hook tails its own log file from inside the kernel and PUTs new
bytes as MLflow artifacts. The local poller then GETs them via `tail()`.

Stdlib-only on purpose — Kaggle base images don't ship `mlflow` and we don't
want to make xrun_hook fat. All HTTP goes through `urllib.request`.

Activation contract (env vars set by the launching adapter):
    MLFLOW_TRACKING_URI       — required. Otherwise streamer is inert.
    XRUN_RUN_ID               — required. Used as MLflow tag for poller lookup.
    XRUN_LOG_STREAM_FILE      — log file to tail. Default: __xrun_stdout.log
                                 (resolved against CWD when relative).
    XRUN_LOG_STREAM_INTERVAL  — seconds between flushes. Default: 5.
    XRUN_LOG_STREAM_EXPERIMENT — MLflow experiment name. Default: xrun-logs.
    MLFLOW_TRACKING_USERNAME  — optional Basic auth user.
    MLFLOW_TRACKING_PASSWORD  — optional Basic auth password.
    MLFLOW_TRACKING_TOKEN     — optional Bearer token (takes precedence over
                                 username/password).

Failure mode: streaming is best-effort; the run continues even if MLflow is
unreachable. A failed MLflow init at start is retried with backoff for the
whole run — one network blip at kernel start used to switch telemetry off
for all ten hours of it. Status lines go to stderr and, when it exists, to
the tailed log file, so they reach `xrun logs` too.
"""

from __future__ import annotations

import atexit
import base64
import itertools
import json
import logging
import os
import sys
import threading
import time
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path
from typing import Callable

_log = logging.getLogger(__name__)

DEFAULT_LOG_FILE = "__xrun_stdout.log"
DEFAULT_INTERVAL_SEC = 5.0
DEFAULT_EXPERIMENT = "xrun-logs"
ARTIFACT_PREFIX = "logs"
EVENTS_PREFIX = "events"
METRICS_PREFIX = "metrics"
HTTP_TIMEOUT_SEC = 15
# Upper bound of one chunk PUT. A backlog (MLflow down for hours) goes out
# as several chunks instead of one unbounded upload.
MAX_CHUNK_BYTES = 4 * 1024 * 1024
# Chunks per file per tick (the final drain at exit is not bounded).
MAX_CHUNKS_PER_TICK = 8
# Ceiling for the pause between MLflow init retries.
INIT_RETRY_MAX_SEC = 120.0

# Tag keys the poller searches by.
TAG_RUN_ID = "xrun_run_id"
TAG_STREAM_MARKER = "xrun_log_stream"


def _is_rank_zero() -> bool:
    """DDP guard: only rank 0 streams unless explicitly overridden."""
    return int(os.environ.get("RANK", "0")) == 0 or os.environ.get("XRUN_HOOK_ALL_RANKS") == "1"


def _say(msg: str, log_path: "Path | None" = None) -> None:
    """Status line to stderr and, if it already exists, the tailed log.

    On Kaggle stderr only reaches the kernel's own log, which no API returns
    while the kernel runs; the tailed file is what `xrun logs` shows. Never
    creates the file — other vendors may not tail one.
    """
    line = f"[xrun_hook] {msg}\n"
    try:
        sys.stderr.write(line)
    except Exception:
        pass
    if log_path is not None and log_path.exists():
        try:
            with log_path.open("a", encoding="utf-8") as f:
                f.write(line)
        except OSError:
            pass


# ---------------------------------------------------------------------------
# HTTP helpers (stdlib only)
# ---------------------------------------------------------------------------


def _auth_headers() -> dict:
    """Pick auth from env. Bearer wins over Basic when both present."""
    token = os.environ.get("MLFLOW_TRACKING_TOKEN")
    if token:
        return {"Authorization": f"Bearer {token}"}
    user = os.environ.get("MLFLOW_TRACKING_USERNAME")
    pwd = os.environ.get("MLFLOW_TRACKING_PASSWORD")
    if user and pwd:
        raw = f"{user}:{pwd}".encode("utf-8")
        return {"Authorization": "Basic " + base64.b64encode(raw).decode("ascii")}
    return {}


def _http_request(
    method: str,
    url: str,
    body: "bytes | None" = None,
    headers: "dict | None" = None,
    timeout: float = HTTP_TIMEOUT_SEC,
) -> "tuple[int, bytes]":
    """Minimal HTTP wrapper over urllib. Returns (status, body)."""
    h = dict(_auth_headers())
    if headers:
        h.update(headers)
    req = urllib.request.Request(url, data=body, method=method, headers=h)
    try:
        with urllib.request.urlopen(req, timeout=timeout) as resp:
            return resp.status, resp.read()
    except urllib.error.HTTPError as e:
        return e.code, e.read() if hasattr(e, "read") else b""


def _post_json(url: str, payload: dict) -> "tuple[int, dict]":
    body = json.dumps(payload).encode("utf-8")
    status, raw = _http_request(
        "POST", url, body=body, headers={"Content-Type": "application/json"}
    )
    try:
        parsed = json.loads(raw.decode("utf-8")) if raw else {}
    except json.JSONDecodeError:
        parsed = {}
    return status, parsed


def _get_json(url: str) -> "tuple[int, dict]":
    status, raw = _http_request("GET", url)
    try:
        parsed = json.loads(raw.decode("utf-8")) if raw else {}
    except json.JSONDecodeError:
        parsed = {}
    return status, parsed


# ---------------------------------------------------------------------------
# MLflow REST client (just the four endpoints we need)
# ---------------------------------------------------------------------------


class _MlflowClient:
    def __init__(self, base_url: str) -> None:
        self.base = base_url.rstrip("/")

    def get_or_create_experiment(self, name: str) -> str:
        # GET-by-name first
        url = (
            f"{self.base}/api/2.0/mlflow/experiments/get-by-name"
            f"?experiment_name={urllib.parse.quote(name)}"
        )
        status, body = _get_json(url)
        if status == 200 and "experiment" in body:
            return body["experiment"]["experiment_id"]

        # Fall through to create on 404 (or any non-200)
        status, body = _post_json(
            f"{self.base}/api/2.0/mlflow/experiments/create", {"name": name}
        )
        if status == 200 and "experiment_id" in body:
            return body["experiment_id"]
        raise RuntimeError(
            f"could not get/create MLflow experiment {name!r}: HTTP {status} {body!r}"
        )

    def create_run(self, experiment_id: str, tags: "list[dict]") -> "tuple[str, str]":
        """Returns (run_id, artifact_path). artifact_path is the proxy-relative
        storage prefix (e.g. `1/<run_id>/artifacts`) parsed out of MLflow's
        `mlflow-artifacts:/...` URI."""
        ts = int(time.time() * 1000)
        status, body = _post_json(
            f"{self.base}/api/2.0/mlflow/runs/create",
            {"experiment_id": experiment_id, "start_time": ts, "tags": tags},
        )
        if status == 200 and "run" in body:
            run_id = body["run"]["info"]["run_id"]
            uri = body["run"]["info"].get("artifact_uri", "")
            # `mlflow-artifacts:/1/<run_id>/artifacts` → `1/<run_id>/artifacts`
            artifact_path = uri.split(":", 1)[1].lstrip("/") if ":" in uri else ""
            return run_id, artifact_path
        raise RuntimeError(f"could not create MLflow run: HTTP {status} {body!r}")

    def update_run(self, run_id: str, status_str: str) -> None:
        end_time = int(time.time() * 1000)
        _post_json(
            f"{self.base}/api/2.0/mlflow/runs/update",
            {"run_id": run_id, "status": status_str, "end_time": end_time},
        )

    def log_batch(self, run_id: str, metrics: "list[dict]") -> None:
        """Native MLflow metric logging via /api/2.0/mlflow/runs/log-batch.

        We *also* PUT the JSONL artifact (so the local poller can ingest
        it into the xrun DB), but without this call the MLflow UI's
        Metrics tab stays empty — it doesn't read artifacts. MLflow
        caps batches at 1000 entries; callers are expected to chunk.
        """
        if not metrics:
            return
        status, body = _post_json(
            f"{self.base}/api/2.0/mlflow/runs/log-batch",
            {"run_id": run_id, "metrics": metrics},
        )
        if status >= 400:
            raise RuntimeError(f"log-batch failed: HTTP {status} {body!r}")

    def put_artifact(
        self, artifact_path: str, remote_path: str, content: bytes
    ) -> None:
        # MLflow's artifact proxy treats `?run_id=` as advisory only — the
        # storage location comes from the URL path. We must include the run's
        # `<exp_id>/<run_id>/artifacts` prefix or every run shares one bucket
        # at `/mlflow/artifacts/<remote_path>`.
        full_path = (
            f"{artifact_path.rstrip('/')}/{remote_path}" if artifact_path else remote_path
        )
        url = f"{self.base}/api/2.0/mlflow-artifacts/artifacts/{full_path}"
        status, _ = _http_request(
            "PUT",
            url,
            body=content,
            headers={"Content-Type": "application/octet-stream"},
        )
        if status >= 400:
            raise RuntimeError(f"PUT artifact {remote_path} failed: HTTP {status}")


# ---------------------------------------------------------------------------
# LogStreamer
# ---------------------------------------------------------------------------


class _Tailer:
    """One file, one chunk-prefix, one byte offset. Pure plumbing for the
    streamer thread — no I/O scheduling here.

    The streamer holds a list of these and asks each to flush new bytes to
    MLflow under the configured prefix on every tick. Same shape as the
    original log-only flow; we just added events/metrics on the side.
    """

    def __init__(self, path: Path, prefix: str, ext: str) -> None:
        self.path = path
        self.prefix = prefix
        self.ext = ext
        self.offset = 0
        self.chunk_seq = 0
        # Chunk read but not yet uploaded. Retried byte-for-byte under the
        # same seq until the PUT succeeds: a failed upload must not leave a
        # hole in the stream (the poller's offsets, and post-run ingest,
        # assume the chunks are the file front to back), and a retry that
        # overwrites an attempt which did land server-side must not change
        # its bytes.
        self._pending: "bytes | None" = None

    def read_new(self) -> bytes:
        """The pending chunk, else up to MAX_CHUNK_BYTES past `offset`.
        Does not advance anything — `commit_chunk()` does, after the PUT."""
        if self._pending is not None:
            return self._pending
        if not self.path.exists():
            return b""
        size = self.path.stat().st_size
        if size <= self.offset:
            # Rotated/truncated. Reset and re-read from start on the next tick.
            if size < self.offset:
                self.offset = 0
            return b""
        want = size - self.offset
        with self.path.open("rb") as f:
            f.seek(self.offset)
            data = f.read(min(want, MAX_CHUNK_BYTES))
        if len(data) < want:
            # Capped: end the chunk on a line boundary so a JSONL record is
            # never split between two chunks the poller may fetch apart.
            nl = data.rfind(b"\n")
            if nl >= 0:
                data = data[: nl + 1]
        if data:
            self._pending = data
        return data

    def next_remote(self) -> str:
        """Remote path of the pending chunk; the seq advances on commit."""
        seq = self.chunk_seq + 1
        return f"{self.prefix}/{self.prefix.rstrip('s')}_{seq:06d}.{self.ext}"

    def commit_chunk(self) -> None:
        """The pending chunk is uploaded: move past it."""
        if self._pending is None:
            return
        self.offset += len(self._pending)
        self.chunk_seq += 1
        self._pending = None


class LogStreamer:
    """Tails files in a background thread, pushes new chunks to MLflow.

    Always streams the stdout log (`logs/log_NNNNNN.txt`). When the writer
    has resolved a run dir, also streams `events.jsonl` and `metrics.jsonl`
    as `events/events_NNNNNN.jsonl` / `metrics/metrics_NNNNNN.jsonl`. The
    poller reassembles each prefix the same way (sort by N, concatenate)
    and ingests the events/metrics into the local DB so live telemetry on
    Kaggle stops being a black box (Issue 8 in field-issues log).
    """

    def __init__(
        self,
        client: _MlflowClient,
        run_id: "str | None",
        log_path: Path,
        interval_sec: float,
        artifact_path: str = "",
        run_dir: "Path | None" = None,
        init: "Callable[[], tuple[str, str]] | None" = None,
    ) -> None:
        """`run_id=None` + `init` starts the streamer without an MLflow run:
        every tick calls `init()` (with backoff) until it returns
        `(run_id, artifact_path)`. Files are not read meanwhile, so the
        backlog goes out in full once the run exists."""
        self._client = client
        self._run_id = run_id
        self._artifact_path = artifact_path
        self._init = init
        self._init_failures = 0
        self._next_init_at = 0.0
        self._log_path = log_path
        # Tests pass tiny intervals; production callers go through
        # start_if_configured() which clamps the env-var floor at 0.5 s.
        self._interval = max(0.01, interval_sec)
        self._tailers: list[_Tailer] = [_Tailer(log_path, ARTIFACT_PREFIX, "txt")]
        if run_dir is not None:
            self._tailers.append(_Tailer(run_dir / "events.jsonl", EVENTS_PREFIX, "jsonl"))
            self._tailers.append(_Tailer(run_dir / "metrics.jsonl", METRICS_PREFIX, "jsonl"))
        self._stop = threading.Event()
        self._thread = threading.Thread(
            target=self._loop, name="xrun-log-streamer", daemon=True
        )
        self._started = False
        self._lock = threading.Lock()

    @property
    def mlflow_run_id(self) -> "str | None":
        return self._run_id

    def _ensure_run(self, force: bool = False) -> bool:
        """True once an MLflow run exists. `force` ignores the backoff —
        the final drain at exit gets one last attempt."""
        if self._run_id is not None:
            return True
        if self._init is None:
            return False
        now = time.monotonic()
        if not force and now < self._next_init_at:
            return False
        try:
            run_id, artifact_path = self._init()
        except Exception as e:  # noqa: BLE001 — best-effort
            self._init_failures += 1
            delay = min(INIT_RETRY_MAX_SEC, self._interval * 2 ** min(self._init_failures, 8))
            self._next_init_at = now + delay
            # First failure is reported by start_if_configured(); then every
            # 10th, so a ten-hour outage leaves a trail without flooding.
            if self._init_failures % 10 == 0:
                _say(
                    f"MLflow init still failing after {self._init_failures} attempts ({e}); "
                    f"retrying every {delay:.0f}s",
                    self._log_path,
                )
            return False
        self._run_id = run_id
        self._artifact_path = artifact_path
        _say(
            f"log streamer active → MLflow run {run_id} "
            f"(init succeeded on retry {self._init_failures + 1})",
            self._log_path,
        )
        return True

    def start(self) -> None:
        if self._started:
            return
        self._started = True
        self._thread.start()

    def stop(self, timeout: float = 5.0) -> None:
        if not self._started:
            return
        self._stop.set()
        # Final drain on the calling thread so atexit gets the last bytes
        # even if the worker is mid-sleep.
        try:
            self._flush_once(force_init=True)
        except Exception as e:
            _log.warning("xrun_hook log streamer final flush failed: %s", e)
        self._thread.join(timeout=timeout)

    def _loop(self) -> None:
        while not self._stop.wait(self._interval):
            try:
                self._flush_once()
            except Exception as e:  # noqa: BLE001 — best-effort
                # Single warning per session keeps logs tidy.
                _log.warning("xrun_hook log streamer flush failed: %s", e)

    def _flush_once(self, force_init: bool = False) -> None:
        with self._lock:
            if not self._ensure_run(force=force_init):
                return
            for t in self._tailers:
                # Several capped chunks per tick: a backlog (e.g. after an
                # MLflow outage) drains over a few ticks, and the final drain
                # at exit (`force_init`) leaves nothing behind. Bounded on
                # normal ticks so a chatty log can't starve events/metrics.
                for _ in itertools.count() if force_init else range(MAX_CHUNKS_PER_TICK):
                    new_bytes = t.read_new()
                    if not new_bytes:
                        break
                    # Raises on failure → the chunk stays pending for the
                    # next tick.
                    self._client.put_artifact(
                        self._artifact_path, t.next_remote(), new_bytes
                    )
                    t.commit_chunk()
                    # Mirror metric records to MLflow's native log-batch so
                    # the Metrics tab in the UI plots them. Artifact JSONL is
                    # the source of truth for the local poller; this is
                    # purely for the human-facing dashboard.
                    if t.prefix == METRICS_PREFIX:
                        try:
                            self._mirror_metrics_to_mlflow(new_bytes)
                        except Exception as e:  # noqa: BLE001 — best-effort
                            _log.warning("xrun_hook log-batch mirror failed: %s", e)

    def _mirror_metrics_to_mlflow(self, raw: bytes) -> None:
        """Parse a metrics-chunk byte buffer and POST in ≤1000-entry batches."""
        run_id = self._run_id
        if run_id is None:
            return
        batch: list[dict] = []
        for line in raw.decode("utf-8", errors="replace").splitlines():
            line = line.strip()
            if not line:
                continue
            try:
                rec = json.loads(line)
            except json.JSONDecodeError:
                continue
            ts_ms = _iso_to_ms(rec.get("ts")) or int(time.time() * 1000)
            try:
                batch.append({
                    "key": str(rec["key"]),
                    "value": float(rec["value"]),
                    "step": int(rec.get("step", 0)),
                    "timestamp": ts_ms,
                })
            except (KeyError, TypeError, ValueError):
                continue
            if len(batch) >= 1000:
                self._client.log_batch(run_id, batch)
                batch = []
        if batch:
            self._client.log_batch(run_id, batch)


# ---------------------------------------------------------------------------
# Auto-init on import
# ---------------------------------------------------------------------------

_streamer: "LogStreamer | None" = None


def start_if_configured() -> "LogStreamer | None":
    """Start a streamer iff env vars + rank gate allow. Idempotent."""
    global _streamer
    if _streamer is not None:
        return _streamer
    if not _is_rank_zero():
        return None
    if os.environ.get("XRUN_LOG_STREAM_DISABLE") == "1":
        return None

    base = os.environ.get("MLFLOW_TRACKING_URI")
    run_id = os.environ.get("XRUN_RUN_ID")
    if not base or not run_id:
        return None

    log_file = os.environ.get("XRUN_LOG_STREAM_FILE", DEFAULT_LOG_FILE)
    log_path = Path(log_file)
    if not log_path.is_absolute():
        log_path = Path.cwd() / log_path

    interval = max(
        0.5,
        _parse_float(
            os.environ.get("XRUN_LOG_STREAM_INTERVAL"), DEFAULT_INTERVAL_SEC
        ),
    )
    experiment = os.environ.get("XRUN_LOG_STREAM_EXPERIMENT", DEFAULT_EXPERIMENT)

    client = _MlflowClient(base)
    tags = [
        {"key": TAG_RUN_ID, "value": run_id},
        {"key": TAG_STREAM_MARKER, "value": "true"},
    ]

    def init() -> "tuple[str, str]":
        exp_id = client.get_or_create_experiment(experiment)
        return client.create_run(exp_id, tags)

    # A failed init no longer disables streaming for the whole run: the
    # streamer starts without a run and keeps retrying in its thread.
    mlflow_run_id: "str | None" = None
    artifact_path = ""
    try:
        mlflow_run_id, artifact_path = init()
    except Exception as e:  # noqa: BLE001 — best-effort
        _say(
            f"MLflow init failed ({e}); streaming is paused and will retry "
            "in the background",
            log_path,
        )

    # Resolve the run dir the writer is using so events/metrics streaming
    # tails the same files xrun_hook.metric/.epoch/.done write to.
    run_dir: "Path | None" = None
    try:
        from . import _paths

        run_dir = _paths.find_run_dir()
    except Exception:
        run_dir = None

    streamer = LogStreamer(
        client,
        mlflow_run_id,
        log_path,
        interval,
        artifact_path=artifact_path,
        run_dir=run_dir,
        init=init,
    )
    streamer.start()
    atexit.register(_atexit_drain, streamer, client)
    _streamer = streamer
    if mlflow_run_id is not None:
        _say(
            f"log streamer active → MLflow run {mlflow_run_id} "
            f"(experiment={experiment}, interval={interval}s)",
            log_path,
        )
    return streamer


def _atexit_drain(streamer: LogStreamer, client: _MlflowClient) -> None:
    try:
        streamer.stop()
    finally:
        run_id = streamer.mlflow_run_id
        if run_id is not None:
            try:
                client.update_run(run_id, "FINISHED")
            except Exception:
                pass


def _iso_to_ms(s: "str | None") -> "int | None":
    """Parse a JSONL `ts` string into milliseconds since epoch.

    xrun_hook writes either `2026-05-05T08:33:22.930Z` (writer) or RFC3339
    with a numeric offset. We accept both. Returns None on parse failure
    so callers can fall back to wall-clock time.
    """
    if not s:
        return None
    try:
        # Python 3.11+ accepts the trailing Z; older versions need it
        # rewritten to +00:00.
        from datetime import datetime

        clean = s[:-1] + "+00:00" if s.endswith("Z") else s
        return int(datetime.fromisoformat(clean).timestamp() * 1000)
    except (ValueError, TypeError):
        return None


def _parse_float(raw: "str | None", default: float) -> float:
    if raw is None or raw.strip() == "":
        return default
    try:
        return float(raw)
    except ValueError:
        return default


def _reset_for_test() -> None:
    """Test-only: drop the module-global streamer reference."""
    global _streamer
    if _streamer is not None:
        try:
            _streamer.stop(timeout=1.0)
        except Exception:
            pass
    _streamer = None
