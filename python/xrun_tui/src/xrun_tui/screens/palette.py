from __future__ import annotations

from typing import Awaitable, Callable

from textual.app import ComposeResult
from textual.binding import Binding
from textual.containers import Vertical
from textual.screen import ModalScreen
from textual.widgets import Input, OptionList
from textual.widgets.option_list import Option


# Each entry: (label, target action key)
# Target keys are interpreted by `_run_target` below.
PALETTE_COMMANDS: list[tuple[str, str]] = [
    ("Go: Dashboard",                "go:dashboard"),
    ("Go: Runs",                     "go:runs"),
    ("Go: Watch  (live active runs)","go:watch"),
    ("Go: Budget & Spend",           "go:budget"),
    ("Go: Sweep results",            "go:sweep"),
    ("Go: Instances",                "go:instances"),
    ("Go: Vendors",                  "go:vendors"),
    ("Go: Sinks  (metrics & logs)",  "go:sinks"),
    ("Go: Doctor (system health)",   "go:doctor"),
    ("Go: Launch manifest",          "go:launch"),
    ("Go: Settings",                 "go:settings"),
    ("Go: Notifications",            "go:notifications"),
    ("Show: Keyboard help",          "go:help"),
    ("Refresh current screen",       "act:refresh"),
    ("Quit xrun TUI",                "act:quit"),
]


class CommandPalette(ModalScreen[str | None]):
    BINDINGS = [
        Binding("escape", "dismiss_none", show=False),
    ]

    DEFAULT_CSS = """
    CommandPalette { align: center top; }
    #palette-box {
        background: #24283b;
        border: round #7aa2f7;
        width: 70;
        height: auto;
        max-height: 24;
        padding: 1 1;
        margin-top: 4;
    }
    #palette-input {
        background: #1a1b26;
        color: #c0caf5;
        border: tall #414868;
    }
    #palette-input:focus { border: tall #7aa2f7; }
    OptionList {
        background: #24283b;
        color: #c0caf5;
        border: none;
        height: auto;
        max-height: 18;
    }
    OptionList > .option-list--option-highlighted {
        background: #3d59a1;
        color: #c0caf5;
    }
    """

    def compose(self) -> ComposeResult:
        with Vertical(id="palette-box"):
            yield Input(placeholder="› type to filter commands…",
                        id="palette-input")
            yield OptionList(*[Option(lbl, id=key)
                               for lbl, key in PALETTE_COMMANDS],
                             id="palette-list")

    def on_mount(self) -> None:
        self.query_one("#palette-input", Input).focus()

    def on_input_changed(self, event: Input.Changed) -> None:
        q = event.value.lower().strip()
        olist = self.query_one("#palette-list", OptionList)
        olist.clear_options()
        for lbl, key in PALETTE_COMMANDS:
            if not q or q in lbl.lower():
                olist.add_option(Option(lbl, id=key))

    def on_input_submitted(self, event: Input.Submitted) -> None:
        olist = self.query_one("#palette-list", OptionList)
        if olist.option_count == 0:
            return
        first = olist.get_option_at_index(0)
        if first.id:
            self.dismiss(first.id)

    def on_option_list_option_selected(
        self, event: OptionList.OptionSelected
    ) -> None:
        if event.option.id:
            self.dismiss(event.option.id)

    def action_dismiss_none(self) -> None:
        self.dismiss(None)


async def run_target(app, target: str) -> None:
    """Resolve a palette target into a screen-push or app action."""
    from xrun_tui.screens.dashboard  import DashboardScreen
    from xrun_tui.screens.runs       import RunsScreen
    from xrun_tui.screens.instances  import InstancesScreen
    from xrun_tui.screens.vendors    import VendorsScreen
    from xrun_tui.screens.doctor     import DoctorScreen
    from xrun_tui.screens.launch     import LaunchScreen
    from xrun_tui.screens.settings   import SettingsScreen
    from xrun_tui.screens.help       import HelpScreen

    if target == "act:quit":
        app.exit()
        return
    if target == "act:refresh":
        scr = app.screen
        if hasattr(scr, "action_refresh"):
            await scr.action_refresh()  # type: ignore[func-returns-value]
        return
    if target == "go:help":
        await app.push_screen(HelpScreen())
        return
    if target == "go:notifications":
        from xrun_tui.screens.notifications import NotificationsScreen
        await app.push_screen(NotificationsScreen())
        return

    from xrun_tui.screens.watch  import WatchScreen
    from xrun_tui.screens.budget import BudgetScreen
    from xrun_tui.screens.sweep  import SweepScreen

    from xrun_tui.screens.sinks import SinksScreen

    factories: dict[str, Callable[[], Any]] = {
        "go:dashboard":  DashboardScreen,
        "go:runs":       RunsScreen,
        "go:instances":  InstancesScreen,
        "go:vendors":    VendorsScreen,
        "go:sinks":      SinksScreen,
        "go:doctor":     DoctorScreen,
        "go:launch":     LaunchScreen,
        "go:settings":   SettingsScreen,
        "go:watch":      WatchScreen,
        "go:budget":     BudgetScreen,
        "go:sweep":      SweepScreen,
    }
    factory = factories.get(target)
    if factory is None:
        return
    await app.push_screen(factory())


# Re-export for typing
from typing import Any  # noqa: E402
