"""Modal dialogs used by the wizard."""
from __future__ import annotations

from textual.app import ComposeResult
from textual.binding import Binding
from textual.containers import Horizontal, Vertical
from textual.screen import ModalScreen
from textual.widgets import Button, Label


class ConfirmSkip(ModalScreen[bool]):
    """Asks the user to confirm wizard skip. Returns True if confirmed."""

    BINDINGS = [
        Binding("escape", "dismiss(False)", "Cancel"),
        Binding("y",      "dismiss(True)",  "Yes"),
        Binding("n",      "dismiss(False)", "No"),
    ]

    DEFAULT_CSS = """
    ConfirmSkip {
        align: center middle;
    }
    ConfirmSkip > Vertical {
        width: 56;
        height: auto;
        padding: 1 2;
        background: #24283b;
        border: round #7aa2f7;
    }
    ConfirmSkip Horizontal {
        height: 3;
        align: center middle;
    }
    ConfirmSkip Button {
        margin: 0 1;
    }
    """

    def compose(self) -> ComposeResult:
        with Vertical():
            yield Label(
                "Skip the setup wizard?\n\n"
                "Your selections so far will be discarded.\n"
                "You can re-run it any time with [b]xrun init[/].",
                markup=True,
            )
            with Horizontal():
                yield Button("Yes, skip [Y]", id="confirm-yes", variant="warning")
                yield Button("No, keep editing [N]", id="confirm-no", variant="primary")

    def on_button_pressed(self, event: Button.Pressed) -> None:
        self.dismiss(event.button.id == "confirm-yes")
