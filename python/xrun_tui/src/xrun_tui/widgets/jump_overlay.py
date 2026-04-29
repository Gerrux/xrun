"""Jump Mode overlay — press ctrl+o, then a letter to navigate."""
from __future__ import annotations

from textual import events
from textual.app import ComposeResult
from textual.binding import Binding
from textual.geometry import Offset
from textual.screen import ModalScreen
from textual.widgets import Label

# widget-id → (jump_key, palette_target)
JUMP_TARGETS: dict[str, tuple[str, str]] = {
    "kpi-active":  ("l", "go:launch"),
    "kpi-done":    ("r", "go:runs"),
    "kpi-failed":  ("i", "go:instances"),
    "kpi-spent":   ("v", "go:vendors"),
    "dash-active": ("d", "go:doctor"),
    "dash-recent": ("s", "go:settings"),
}

_KEY_LABEL: dict[str, str] = {
    "l": "[b]l[/] Launch",
    "r": "[b]r[/] Runs",
    "i": "[b]i[/] Instances",
    "v": "[b]v[/] Vendors",
    "d": "[b]d[/] Doctor",
    "s": "[b]s[/] Settings",
}


class JumpOverlay(ModalScreen[str | None]):
    """Semi-transparent overlay; press a letter to navigate."""

    DEFAULT_CSS = """
    JumpOverlay {
        background: $background 40%;
    }
    .jump-label {
        background: $accent;
        color: $background;
        padding: 0 1;
        width: auto;
    }
    #jump-hint {
        dock: bottom;
        width: 100%;
        content-align: center middle;
        color: $text-muted;
        height: 1;
    }
    """

    BINDINGS = [Binding("escape", "dismiss_none", show=False)]

    def __init__(self) -> None:
        super().__init__()
        self._key_to_target: dict[str, str] = {
            k: t for _, (k, t) in JUMP_TARGETS.items()
        }

    def compose(self) -> ComposeResult:
        screen = self.app.screen
        for widget_id, (key, _target) in JUMP_TARGETS.items():
            try:
                widget = screen.query_one(f"#{widget_id}")
                ox, oy = screen.get_offset(widget)
                offset = Offset(ox, oy)
            except Exception:
                continue
            label = Label(_KEY_LABEL[key], classes="jump-label")
            label.styles.margin = (offset.y, 0, 0, offset.x)
            yield label
        yield Label("[b]ESC[/] dismiss", id="jump-hint")

    def on_key(self, event: events.Key) -> None:
        event.stop()
        event.prevent_default()
        target = self._key_to_target.get(event.key)
        if target:
            self.dismiss(target)
        elif event.key == "escape":
            self.dismiss(None)

    def action_dismiss_none(self) -> None:
        self.dismiss(None)
