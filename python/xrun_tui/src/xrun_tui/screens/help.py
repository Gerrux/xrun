from __future__ import annotations

from textual.app import ComposeResult
from textual.binding import Binding
from textual.containers import Vertical, VerticalScroll
from textual.screen import ModalScreen
from textual.widgets import Static

# Section → list of (keys, description)
_HELP: list[tuple[str, list[tuple[str, str]]]] = [
    ("Global", [
        ("?",          "This help"),
        ("ctrl+p",     "Command palette"),
        ("g d",        "Dashboard"),
        ("g r",        "Runs"),
        ("g i",        "Instances"),
        ("g v",        "Vendors"),
        ("g h",        "Doctor (health)"),
        ("g l",        "Launch"),
        (",",          "Settings"),
        ("ctrl+r / F5","Refresh current screen"),
        ("q / ctrl+c", "Quit"),
    ]),
    ("Lists & tables", [
        ("j / k or ↑ ↓", "Move cursor"),
        ("enter",         "Open detail"),
        ("f or /",        "Fuzzy filter"),
    ]),
    ("Runs", [
        ("s",      "Stop selected run"),
        ("r",      "Rerun selected run"),
        ("p",      "Pull latest artifacts"),
        ("c",      "Toggle compare selection"),
        ("C",      "Open compare (2 selected)"),
        ("G",      "Toggle grouping by manifest"),
        ("e",      "Export visible runs (JSON)"),
        ("f or /", "Fuzzy filter"),
    ]),
    ("Run detail", [
        ("1 / 2 / 3 / 4", "Stages / Logs / Manifest / Metrics"),
        ("a",              "Artifacts browser"),
        ("Relaunch btn",   "Launch from same manifest"),
        ("esc",            "Back"),
    ]),
    ("Instances", [
        ("x",      "Destroy selected remote instance"),
        ("ctrl+r", "Refresh"),
    ]),
    ("Forms", [
        ("ctrl+s", "Save"),
        ("ctrl+t", "Test connection"),
        ("esc",    "Cancel / back"),
    ]),
]


class HelpScreen(ModalScreen[None]):
    BINDINGS = [
        Binding("escape,q,?", "dismiss", show=False),
    ]

    DEFAULT_CSS = """
    HelpScreen { align: center middle; }
    #help-box {
        background: #24283b;
        border: round #7aa2f7;
        width: 70;
        height: 32;
        padding: 1 2;
    }
    #help-title {
        color: #7aa2f7;
        text-style: bold;
        height: 1;
        padding-bottom: 1;
        border-bottom: solid #414868;
    }
    .help-section-title {
        color: #bb9af7;
        text-style: bold;
        padding: 1 0 0 0;
        height: auto;
    }
    .help-row { height: 1; padding-left: 2; }
    #help-footer {
        color: #565f89;
        height: 1;
        padding-top: 1;
        border-top: solid #414868;
        content-align: center middle;
    }
    """

    def compose(self) -> ComposeResult:
        with Vertical(id="help-box"):
            yield Static("xrun  ·  Keyboard reference", id="help-title")
            with VerticalScroll():
                for section, rows in _HELP:
                    yield Static(section, classes="help-section-title")
                    for keys, desc in rows:
                        yield Static(
                            f"  [bold #7dcfff]{keys:<14}[/] [#c0caf5]{desc}[/]",
                            classes="help-row",
                        )
            yield Static("press [#7aa2f7]?[/] or [#7aa2f7]esc[/] to close",
                        id="help-footer")

    def action_dismiss(self) -> None:
        self.dismiss(None)
