from __future__ import annotations

from datetime import datetime

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


class NotificationsScreen(ModalScreen[None]):
    BINDINGS = [
        Binding("escape,q,n", "dismiss_modal", show=False),
        Binding("c",          "clear",         "Clear"),
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
            yield Static("Notifications history  [#565f89](press n / esc to close, c to clear)[/]",
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
        self._render()

    def _render(self) -> None:
        history = list(getattr(self.app, "_notif_history", []))
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
            table.add_row(
                Text(sym, style=style),
                Text(ts, style="#565f89"),
                Text(entry["severity"], style=style),
                Text(entry["message"][:200], style="#c0caf5"),
            )

    def action_dismiss_modal(self) -> None:
        self.dismiss(None)

    def action_clear(self) -> None:
        history = getattr(self.app, "_notif_history", None)
        if history is not None:
            history.clear()
        self._render()
