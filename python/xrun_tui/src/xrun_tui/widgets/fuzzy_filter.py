"""Inline filter bar that mounts on top of a list view.

Toggles via `/` on the parent screen.
"""
from __future__ import annotations

from typing import Callable

from textual.app import ComposeResult
from textual.binding import Binding
from textual.containers import Horizontal
from textual.widget import Widget
from textual.widgets import Input, Static


class FilterBar(Widget):
    """Slim two-row bar: an Input plus a hint line."""

    DEFAULT_CSS = """
    FilterBar {
        height: 3;
        background: #1e2030;
        border-bottom: solid #2d3149;
        padding: 0 1;
        display: none;
    }
    FilterBar.-visible { display: block; }
    FilterBar Horizontal { height: 3; align: left middle; }
    FilterBar #filter-prefix {
        width: 3;
        color: #7aa2f7;
        content-align: right middle;
        padding-right: 1;
    }
    FilterBar Input {
        width: 1fr;
        background: #24283b;
        color: #c0caf5;
        border: tall #414868;
    }
    FilterBar Input:focus { border: tall #7aa2f7; }
    """

    BINDINGS = [
        Binding("escape", "cancel", show=False),
    ]

    def __init__(
        self,
        on_change: Callable[[str], None],
        on_close: Callable[[], None] | None = None,
        placeholder: str = "type to filter…",
        id: str | None = None,
    ) -> None:
        super().__init__(id=id)
        self._on_change = on_change
        self._on_close  = on_close
        self._placeholder = placeholder

    def compose(self) -> ComposeResult:
        with Horizontal():
            yield Static("›", id="filter-prefix")
            yield Input(placeholder=self._placeholder, id="filter-input")

    def show(self) -> None:
        self.add_class("-visible")
        try:
            self.query_one("#filter-input", Input).focus()
        except Exception:
            pass

    def hide(self) -> None:
        self.remove_class("-visible")
        try:
            self.query_one("#filter-input", Input).value = ""
        except Exception:
            pass
        self._on_change("")
        if self._on_close:
            self._on_close()

    def on_input_changed(self, event: Input.Changed) -> None:
        if event.input.id == "filter-input":
            self._on_change(event.value)

    def action_cancel(self) -> None:
        self.hide()

    @property
    def value(self) -> str:
        try:
            return self.query_one("#filter-input", Input).value
        except Exception:
            return ""
