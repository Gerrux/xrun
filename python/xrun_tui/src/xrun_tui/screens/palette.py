from __future__ import annotations

from textual.app import ComposeResult
from textual.binding import Binding
from textual.containers import Vertical
from textual.screen import ModalScreen
from textual.widgets import Input, OptionList
from textual.widgets.option_list import Option

from xrun_tui.screens.registry import by_slug, iter_screens


# Each entry: (label, target action key)
# Target keys are interpreted by `run_target` below. Screen destinations come
# from the registry; only the non-navigation actions are listed by hand.
PALETTE_COMMANDS: list[tuple[str, str]] = [
    (f"{'Show' if e.slug == 'help' else 'Go'}: {e.description}", e.target)
    for e in iter_screens()
] + [
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
    if target == "act:quit":
        app.exit()
        return
    if target == "act:refresh":
        scr = app.screen
        if hasattr(scr, "action_refresh"):
            await scr.action_refresh()  # type: ignore[func-returns-value]
        return
    entry = by_slug(target[3:]) if target.startswith("go:") else None
    if entry is None:
        return
    cls = entry.load()
    # One instance per destination. Pushing a fresh screen on every `g …`
    # grew the stack without bound — each visit left a mounted copy behind —
    # so going somewhere already open unwinds back to it instead.
    stack = list(app.screen_stack)
    for depth, screen in enumerate(stack):
        if type(screen) is cls:
            await _unwind_to(app, screen, stack[depth + 1:])
            return
    await app.push_screen(cls())


async def _unwind_to(app, screen, dropped: list) -> None:
    """Go back to `screen`, dropping the screens above it. A config form among
    them gets the same protection as on Esc: unwinding used to throw away
    unsaved edits without a word, and could cut a running save short."""
    from xrun_tui.widgets.form import FormGuard

    forms = [s for s in dropped if isinstance(s, FormGuard)]
    if any(f._saving for f in forms):
        app.notify("Save in progress — wait for it to finish",
                   severity="warning")
        return
    if not any(f.form_dirty() for f in forms):
        # One batched update: the screens in between are dropped
        # without each being repainted on the way down.
        screen.pop_until_active()
        return

    from xrun_tui.screens.confirm import ConfirmScreen

    def _after(discard: bool | None) -> None:
        if discard:
            screen.pop_until_active()

    await app.push_screen(
        ConfirmScreen("Discard unsaved changes?", default_no=True), _after
    )
