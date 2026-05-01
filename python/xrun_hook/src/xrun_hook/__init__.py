"""xrun_hook — minimal training hook for structured event and metric logging."""

import os
import sys
from datetime import datetime, timezone
from typing import Any

from . import _log_streamer, _paths, _writer

__all__ = ["stage", "metric", "epoch", "fail", "done"]

# ---------------------------------------------------------------------------
# Module-level lazy state
# ---------------------------------------------------------------------------

_events: "_writer.JsonlWriter | _writer.StdoutWriter | None" = None
_metrics: "_writer.JsonlWriter | _writer.StdoutWriter | None" = None


def _get_events() -> "_writer.JsonlWriter | _writer.StdoutWriter":
    global _events
    if _events is None:
        _events = _make_writer("events.jsonl")
    return _events


def _get_metrics() -> "_writer.JsonlWriter | _writer.StdoutWriter":
    global _metrics
    if _metrics is None:
        _metrics = _make_writer("metrics.jsonl")
    return _metrics


def _make_writer(filename: str) -> "_writer.JsonlWriter | _writer.StdoutWriter":
    run_dir = _paths.find_run_dir()
    if run_dir is not None:
        return _writer.JsonlWriter(run_dir / filename)
    return _writer.StdoutWriter()


def _reset() -> None:
    """Reset module state. For testing only."""
    global _events, _metrics
    for w in (_events, _metrics):
        if w is not None:
            try:
                w.close()
            except Exception:
                pass
    _events = None
    _metrics = None


# ---------------------------------------------------------------------------
# Timestamp
# ---------------------------------------------------------------------------


def _now_iso() -> str:
    dt = datetime.now(timezone.utc)
    ms = dt.microsecond // 1000
    return dt.strftime(f"%Y-%m-%dT%H:%M:%S.{ms:03d}Z")


# ---------------------------------------------------------------------------
# Internal write helpers
# ---------------------------------------------------------------------------


def _write_event(
    name: str,
    status: str,
    msg: "str | None" = None,
    extra: "dict | None" = None,
) -> None:
    record: dict[str, Any] = {"ts": _now_iso(), "stage": name, "status": status}
    if msg is not None:
        record["msg"] = msg
    cleaned = _writer.sanitize_extra(extra)
    if cleaned is not None:
        record["extra"] = cleaned
    _get_events().append(record)


def _write_metric(key: str, value: float, step: int) -> None:
    record = {"ts": _now_iso(), "step": step, "key": key, "value": value}
    _get_metrics().append(record)


# ---------------------------------------------------------------------------
# Public API
# ---------------------------------------------------------------------------


class stage:
    """Write a start event immediately; acts as a context manager to close with ok/fail."""

    def __init__(
        self,
        name: str,
        status: str = "start",
        msg: "str | None" = None,
        extra: "dict | None" = None,
    ) -> None:
        self._name = name
        _write_event(name, status, msg, extra)

    def __enter__(self) -> "stage":
        return self

    def __exit__(
        self,
        exc_type: "type | None",
        exc_val: "BaseException | None",
        exc_tb: Any,
    ) -> bool:
        if exc_type is None:
            _write_event(self._name, "ok")
        else:
            _write_event(self._name, "fail", extra={"error": repr(exc_val)})
        return False  # do not suppress exceptions


def metric(key: str, value: float, step: int) -> None:
    """Write one metric data point."""
    _write_metric(key, value, step)


def epoch(idx: int, extra: "dict | None" = None) -> None:
    """Write an epoch-ok event."""
    merged = {"epoch": idx}
    if extra:
        merged.update(extra)
    _write_event("epoch", "ok", extra=merged)


def fail(msg: str, extra: "dict | None" = None) -> None:
    """Write a failure event, close all writers, and exit with code 1."""
    global _events, _metrics
    _write_event("error", "fail", msg=msg, extra=extra)
    _reset()
    sys.exit(1)


def done() -> None:
    """Write the terminal done event and close all writers."""
    global _events, _metrics
    _write_event("done", "ok")
    _events_w = _get_events()
    _metrics_w = _get_metrics()
    _events_w.close()
    if _metrics_w is not _events_w:
        _metrics_w.close()
    _events = None
    _metrics = None


# ---------------------------------------------------------------------------
# Auto-install excepthook on import
# ---------------------------------------------------------------------------


def _excepthook(exc_type: type, exc_val: BaseException, exc_tb: Any) -> None:
    _write_event("error", "fail", msg=repr(exc_val))
    sys.__excepthook__(exc_type, exc_val, exc_tb)


if os.environ.get("XRUN_HOOK_INSTALL_EXCEPTHOOK", "1") != "0":
    sys.excepthook = _excepthook

# Best-effort log streaming: no-op when MLflow / XRUN_RUN_ID env vars are absent.
_log_streamer.start_if_configured()
