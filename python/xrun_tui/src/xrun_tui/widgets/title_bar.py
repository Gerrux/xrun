from __future__ import annotations

from textual.app import ComposeResult
from textual.containers import Horizontal
from textual.events import Click
from textual.widgets import Static


class _MenuBtn(Static):
    DEFAULT_CSS = """
    _MenuBtn {
        width: auto;
        height: 1;
        color: #7aa2f7;
        padding: 0 1;
    }
    _MenuBtn:hover { background: #2d3149; }
    """

    def on_click(self, event: Click) -> None:
        event.stop()
        self.run_worker(self.app.action_open_palette(), exclusive=True)  # type: ignore[attr-defined]


class TitleBar(Horizontal):
    """Shared title bar: [⊞ Menu]  xrun — <subtitle>."""

    DEFAULT_CSS = """
    TitleBar {
        dock: top;
        height: 1;
        background: #24283b;
        padding: 0 0;
        align: left middle;
    }
    TitleBar #tb-title {
        width: 1fr;
        content-align: center middle;
        color: #c0caf5;
    }
    """

    def __init__(self, subtitle: str = "") -> None:
        super().__init__()
        self._subtitle = subtitle

    def compose(self) -> ComposeResult:
        yield _MenuBtn("⊞ Menu")
        label = f"xrun  —  {self._subtitle}" if self._subtitle else "xrun"
        yield Static(label, id="tb-title")
