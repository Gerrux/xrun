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
