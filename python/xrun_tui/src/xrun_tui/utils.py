from __future__ import annotations

import os
import sys
from datetime import datetime, timezone

from rich.text import Text

STATUS_DOT: dict[str, tuple[str, str]] = {
    "running":      ("●", "bold #9ece6a"),
    "done":         ("●", "#565f89"),
    "failed":       ("●", "bold #f7768e"),
    "cancelled":    ("●", "#bb9af7"),
    "provisioning": ("◌", "#e0af68"),
    "uploading":    ("⟳", "#e0af68"),
}

STATUS_LABEL: dict[str, tuple[str, str]] = {
    "running":      ("running",      "bold #9ece6a"),
    "done":         ("done",         "#565f89"),
    "failed":       ("failed",       "bold #f7768e"),
    "cancelled":    ("cancelled",    "#bb9af7"),
    "provisioning": ("provisioning", "#e0af68"),
    "uploading":    ("uploading",    "#e0af68"),
}

EVENT_STATUS_STYLE: dict[str, str] = {
    "ok":    "bold #9ece6a",
    "error": "bold #f7768e",
    "start": "#7dcfff",
    "warn":  "#e0af68",
    "info":  "#c0caf5",
}


def status_dot(status: str) -> Text:
    sym, style = STATUS_DOT.get(status, ("●", "#565f89"))
    return Text(sym, style=style)


def status_label(status: str) -> Text:
    lbl, style = STATUS_LABEL.get(status, (status, "#c0caf5"))
    return Text(lbl, style=style)


def rel_time(dt_str: str | None) -> str:
    if not dt_str:
        return "—"
    try:
        dt = datetime.fromisoformat(dt_str.replace("Z", "+00:00"))
        s = int((datetime.now(timezone.utc) - dt).total_seconds())
        if s < 60:
            return f"{s}s ago"
        if s < 3600:
            return f"{s // 60}m ago"
        if s < 86400:
            return f"{s // 3600}h {(s % 3600) // 60}m ago"
        return f"{s // 86400}d ago"
    except Exception:
        return dt_str[:10]


def duration(run: dict) -> str:
    if not run.get("started_at"):
        return "—"
    try:
        start = datetime.fromisoformat(run["started_at"].replace("Z", "+00:00"))
        end_str = run.get("ended_at")
        end = (
            datetime.fromisoformat(end_str.replace("Z", "+00:00"))
            if end_str else datetime.now(timezone.utc)
        )
        s = int((end - start).total_seconds())
        if s < 60:
            return f"{s}s"
        if s < 3600:
            return f"{s // 60}m {s % 60:02d}s"
        return f"{s // 3600}h {(s % 3600) // 60:02d}m"
    except Exception:
        return "—"


def cost(run: dict) -> str:
    if (c := run.get("cost_usd")) is not None:
        return f"${c:.2f}"
    if (e := run.get("cost_usd_estimate")) is not None:
        return f"~${e:.2f}"
    return "—"


# A run is "stale" when its DB row says it's still running but the poll-daemon
# is no longer running. This typically happens after a binary upgrade on
# Windows (the OS won't let cargo replace the open .exe, so the daemon dies
# silently). The user can recover with `xrun fix-status` — TUI surfaces this
# directly so they don't have to discover the command from logs.
#
# Authoritative signal: the recorded `poller_pid` is dead. Event silence is
# NOT a stale signal — Kaggle runs emit no events between `running:start` and
# `done:ok`, so a long-but-healthy training would otherwise look "stale".
# When `poller_pid` is missing (synchronous launch, or detached launch where
# spawn_daemon never reached because vendor.execute() blocked), we don't have
# a reliable signal — better to under-report than to cry wolf on healthy runs.


def _process_alive(pid: int) -> bool:
    if pid <= 0:
        return False
    if sys.platform == "win32":
        import ctypes
        from ctypes import wintypes

        PROCESS_QUERY_LIMITED_INFORMATION = 0x1000
        STILL_ACTIVE = 259
        kernel32 = ctypes.windll.kernel32
        handle = kernel32.OpenProcess(
            PROCESS_QUERY_LIMITED_INFORMATION, False, pid
        )
        if not handle:
            return False
        try:
            exit_code = wintypes.DWORD()
            if not kernel32.GetExitCodeProcess(handle, ctypes.byref(exit_code)):
                return False
            return exit_code.value == STILL_ACTIVE
        finally:
            kernel32.CloseHandle(handle)
    try:
        os.kill(pid, 0)
        return True
    except ProcessLookupError:
        return False
    except PermissionError:
        return True
    except OSError:
        return False


def is_stale(run: dict) -> bool:
    """A `running`/`provisioning`/`uploading` run whose poll-daemon is dead."""
    if run.get("status") not in ("running", "provisioning", "uploading"):
        return False
    pid = run.get("poller_pid")
    if not pid:
        return False
    return not _process_alive(int(pid))


def status_label_for(run: dict) -> Text:
    """Status with an inline ⚠ marker for stale runs."""
    base = status_label(run.get("status") or "")
    if is_stale(run):
        return Text.assemble(base, Text("  ⚠ stale", style="bold #e0af68"))
    return base


def status_dot_for(run: dict) -> Text:
    """Status dot, swapped for a warning symbol on stale runs."""
    if is_stale(run):
        return Text("⚠", style="bold #e0af68")
    return status_dot(run.get("status") or "")


# ── Sparkline + metric-key helpers ────────────────────────────────────────────

# Prefer canonical training metrics. The order encodes user intent: a run that
# logs both `loss` and `val_loss` is almost certainly more interested in the
# training loss curve at-a-glance, with val showing on detail screens.
_METRIC_KEY_PRIORITY: tuple[str, ...] = (
    "loss", "train_loss", "train/loss",
    "val_loss", "val/loss",
    "accuracy", "acc", "val_acc", "val_accuracy",
    "f1", "val_f1",
)

# Keys we never show on the dashboard sparkline — they're system-y or sparse.
_METRIC_KEY_SKIP_PREFIXES: tuple[str, ...] = (
    "system/", "sys/", "_", "step", "epoch",
)


def pick_metric_key(keys: list[str]) -> str | None:
    """Pick the most useful metric key for an at-a-glance sparkline.

    Priority list first; otherwise first non-system key; otherwise None.
    """
    if not keys:
        return None
    key_set = set(keys)
    for pri in _METRIC_KEY_PRIORITY:
        if pri in key_set:
            return pri
    for k in keys:
        if not any(k.startswith(p) for p in _METRIC_KEY_SKIP_PREFIXES):
            return k
    return None


_SPARK_BARS = "▁▂▃▄▅▆▇█"


def render_sparkline(values: list[float], width: int = 10) -> str:
    """Render a list of floats as unicode bars. Empty input → empty string.

    Downsamples if there are more values than `width` slots so the line
    stays readable on a 28-col KPI card. Uniform input renders as a flat
    mid-height bar so users can still see "data exists, just not changing".
    """
    if not values:
        return ""
    if len(values) > width:
        step = len(values) / width
        sampled = [values[min(int(i * step), len(values) - 1)]
                   for i in range(width)]
    else:
        sampled = values
    lo = min(sampled)
    hi = max(sampled)
    if hi == lo:
        return _SPARK_BARS[3] * len(sampled)
    rng = hi - lo
    n_bars = len(_SPARK_BARS)
    return "".join(
        _SPARK_BARS[min(int((v - lo) / rng * (n_bars - 1)), n_bars - 1)]
        for v in sampled
    )


def fmt_metric_value(v: float) -> str:
    """Compact float formatter for KPI/table cells. Picks fixed vs scientific
    so a metric column never overflows: 0.4231 stays human, 1.2e-7 stays
    short, 12345 stays an integer."""
    av = abs(v)
    if av == 0:
        return "0"
    if av >= 1000 or av < 0.001:
        return f"{v:.1e}"
    if av >= 100:
        return f"{v:.1f}"
    if av >= 10:
        return f"{v:.2f}"
    return f"{v:.3f}"
