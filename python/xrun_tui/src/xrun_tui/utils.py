from __future__ import annotations

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
# stopped writing events. This typically happens after a binary upgrade on
# Windows (the OS won't let cargo replace the open .exe, so the daemon dies
# silently). The user can recover with `xrun fix-status` — TUI surfaces this
# directly so they don't have to discover the command from logs.
STALE_THRESHOLD_SECS = 30 * 60


def _parse_iso(dt_str: str | None) -> datetime | None:
    if not dt_str:
        return None
    try:
        return datetime.fromisoformat(dt_str.replace("Z", "+00:00"))
    except Exception:
        return None


def is_stale(run: dict, threshold_secs: int = STALE_THRESHOLD_SECS) -> bool:
    """A `running`/`provisioning`/`uploading` run with no recent event."""
    if run.get("status") not in ("running", "provisioning", "uploading"):
        return False
    last = _parse_iso(run.get("last_event_ts")) or _parse_iso(
        run.get("started_at")
    ) or _parse_iso(run.get("created_at"))
    if last is None:
        return False
    age = (datetime.now(timezone.utc) - last).total_seconds()
    return age > threshold_secs


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
