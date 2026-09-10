from __future__ import annotations

from datetime import datetime, timezone
from typing import Any

from rich.text import Text
from textual.app import ComposeResult
from textual.binding import Binding
from textual.containers import Vertical
from textual.screen import ModalScreen
from textual.widgets import DataTable, Static

_SEV = {
    "error":       ("✗", "bold #f7768e"),
    "warning":     ("!", "bold #e0af68"),
    "information": ("·", "#7aa2f7"),
}

# Push-notification kinds (from `xrun notify kinds`) → in-app severity.
_KIND_SEV = {
    "run.done": "information",
    "test": "information",
    "manual": "information",
    "run.failed": "error",
    "run.idle": "warning",
    "budget.warn": "warning",
    "budget.auto_destroyed": "error",
    "budget.daily": "warning",
    "instance.cleanup_failed": "error",
    "instance.orphan": "error",
    "metric.anomaly": "warning",
    "poller.dead": "error",
}


def _journal_entries(rows: list[dict[str, Any]]) -> list[dict[str, Any]]:
    """Normalise `xrun notify log --json` rows to the in-app history shape.

    One row per (notification, channel); collapse to one entry per
    notification so a 3-channel setup doesn't show every event thrice.
    Failed deliveries keep their error so the user sees the channel is
    broken.
    """
    seen: dict[tuple[str, str], dict[str, Any]] = {}
    for r in rows:
        try:
            ts = datetime.fromisoformat(str(r.get("ts", "")).replace("Z", "+00:00"))
            ts_f = ts.astimezone(timezone.utc).timestamp()
        except Exception:
            ts_f = 0.0
        key = (str(r.get("dedupe_key", "")), str(r.get("ts", "")))
        entry = seen.get(key)
        if entry is None:
            entry = {
                "ts": ts_f,
                "message": str(r.get("title", "")),
                "severity": _KIND_SEV.get(str(r.get("kind", "")), "information"),
                "title": "push",
                "channels": [],
            }
            seen[key] = entry
        ch = str(r.get("channel", "?"))
        if r.get("ok"):
            entry["channels"].append(ch)
        else:
            entry["channels"].append(f"{ch}✗")
            entry["severity"] = "error"
            err = r.get("error")
            if err:
                entry["message"] = f"{entry['message']}  [{ch}: {err}]"
    out = list(seen.values())
    for e in out:
        chans = ",".join(e.pop("channels"))
        if chans:
            e["message"] = f"[{chans}] {e['message']}"
    return out


class NotificationsScreen(ModalScreen[None]):
    BINDINGS = [
        Binding("escape,q,n", "dismiss_modal", show=False),
        Binding("c",          "clear",         "Clear"),
        Binding("s",          "setup",         "Setup"),
    ]

    DEFAULT_CSS = """
    NotificationsScreen { align: center middle; }
    #notif-box {
        background: #24283b;
        border: round #7aa2f7;
        width: 100;
        height: 30;
        padding: 1 2;
    }
    #notif-title {
        color: #7aa2f7;
        text-style: bold;
        height: 1;
        padding-bottom: 1;
        border-bottom: solid #414868;
    }
    #notif-empty {
        height: 1fr;
        content-align: center middle;
        color: #414868;
        text-style: italic;
    }
    #notif-table { height: 1fr; }
    """

    def compose(self) -> ComposeResult:
        with Vertical(id="notif-box"):
            yield Static("Notifications history  [#565f89](s = setup push channels · c = clear · esc)[/]",
                        id="notif-title")
            yield DataTable(id="notif-table", cursor_type="row",
                            zebra_stripes=True)
            yield Static("[#414868]no notifications yet[/]", id="notif-empty")

    def on_mount(self) -> None:
        t = self.query_one("#notif-table", DataTable)
        t.add_columns(
            Text(" ",       style="#565f89"),
            Text("Time",    style="#565f89"),
            Text("Severity",style="#565f89"),
            Text("Message", style="#565f89"),
        )
        self._journal: list[dict[str, Any]] = []
        self._refresh_table()
        self.run_worker(self._load_journal(), exclusive=True)

    async def _load_journal(self) -> None:
        """Pull the daemon's push-notification journal so this screen shows
        what reached the phone, not only what the TUI itself toasted."""
        from xrun_tui.services import notify_log
        try:
            rows = await notify_log(limit=100)
        except Exception:
            rows = []
        self._journal = _journal_entries(rows)
        if self._journal and self.is_mounted:
            self._refresh_table()

    def _refresh_table(self) -> None:
        history = list(getattr(self.app, "_notif_history", []))
        history = sorted(
            history + list(getattr(self, "_journal", [])),
            key=lambda e: e.get("ts", 0.0),
        )
        table = self.query_one("#notif-table", DataTable)
        empty = self.query_one("#notif-empty", Static)
        table.clear()
        if not history:
            empty.display = True
            table.display = False
            return
        empty.display = False
        table.display = True
        for entry in reversed(history):
            sym, style = _SEV.get(entry["severity"], ("·", "#c0caf5"))
            ts = datetime.fromtimestamp(entry["ts"]).strftime("%H:%M:%S")
            sev_label = entry["severity"]
            if entry.get("title") == "push":
                sev_label = f"push/{sev_label}"
            table.add_row(
                Text(sym, style=style),
                Text(ts, style="#565f89"),
                Text(sev_label, style=style),
                Text(entry["message"][:200], style="#c0caf5"),
            )

    def action_dismiss_modal(self) -> None:
        self.dismiss(None)

    async def action_setup(self) -> None:
        """Jump to the channels screen. Dismiss first so the modal doesn't
        sit on top of the full-screen setup."""
        from xrun_tui.screens.notify_setup import NotifySetupScreen
        app = self.app
        self.dismiss(None)
        await app.push_screen(NotifySetupScreen())

    def action_clear(self) -> None:
        history = getattr(self.app, "_notif_history", None)
        if history is not None:
            history.clear()
        self._refresh_table()
