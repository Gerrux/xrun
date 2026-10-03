"""The screen registry is the only list of navigable screens: chords, the
palette and the help screen all derive from it, so they must agree."""
from __future__ import annotations

import asyncio

import pytest
from textual.app import App
from textual.screen import Screen

from xrun_tui.app import _CHORDS, XrunApp
from xrun_tui.screens import registry
from xrun_tui.screens.help import _HELP
from xrun_tui.screens.palette import PALETTE_COMMANDS, run_target


def test_slugs_are_unique_and_lookups_agree() -> None:
    slugs = [e.slug for e in registry.iter_screens()]
    assert len(slugs) == len(set(slugs))
    for e in registry.iter_screens():
        assert registry.by_slug(e.slug) is e
        if e.chord:
            assert registry.by_chord(e.chord) is e
    assert registry.by_slug("nope") is None
    assert registry.by_chord("?") is None


def test_every_factory_returns_a_screen(tmp_path, monkeypatch) -> None:
    # Several screens read config / credentials in __init__: keep them off
    # the developer's real config dir.
    monkeypatch.setenv("XRUN_CONFIG_DIR", str(tmp_path))
    monkeypatch.setenv("XRUN_DATA_DIR", str(tmp_path / "data"))
    for e in registry.iter_screens():
        screen = e.factory()
        assert isinstance(screen, Screen), e.slug
        assert type(screen) is e.load(), e.slug


def test_chord_keys_are_unique_and_clear_of_priority_keys() -> None:
    chords = [e.chord for e in registry.iter_screens() if e.chord]
    assert len(chords) == len(set(chords))
    # Keys the app owns outright (`n` is priority too, but `g n` is the
    # notifications-setup chord and is handled in action_open_notifications).
    reserved = {"g", "?", "question_mark", "ctrl+p", "ctrl+o"}
    assert not reserved & set(chords)
    priority = {k for b in XrunApp.BINDINGS if b.priority
                for k in b.key.split(",")}
    for e in registry.iter_screens():
        if e.chord in priority:
            assert (e.slug, e.chord) == ("notify", "n")


def test_chord_table_is_the_registry() -> None:
    assert set(_CHORDS) == {"g"}
    assert _CHORDS["g"] == {
        e.chord: e.slug for e in registry.iter_screens() if e.chord
    }
    for slug in _CHORDS["g"].values():
        assert registry.by_slug(slug) is not None


def test_every_palette_target_resolves() -> None:
    targets = [t for _, t in PALETTE_COMMANDS]
    assert len(targets) == len(set(targets))
    for target in targets:
        if target.startswith("go:"):
            assert registry.by_slug(target[3:]) is not None, target
        else:
            assert target in {"act:refresh", "act:quit"}
    for e in registry.iter_screens():
        assert e.target in targets  # nothing navigable is missing


def test_help_navigation_lists_every_chord() -> None:
    nav = dict(_HELP)["Go to screen (g, then a key)"]
    rows = {keys: desc for keys, desc in nav}
    for e in registry.iter_screens():
        if e.chord:
            assert rows.get(f"g {e.chord}") == e.label


class _Host(App):
    """Just a screen stack; run_target needs push_screen / screen_stack."""


def test_run_target_pushes_once_and_unwinds_to_an_open_screen() -> None:
    async def scenario() -> None:
        app = _Host()
        async with app.run_test() as pilot:
            await run_target(app, "go:help")
            await pilot.pause()
            depth = len(app.screen_stack)
            assert type(app.screen) is registry.by_slug("help").load()
            await run_target(app, "go:help")
            await pilot.pause()
            assert len(app.screen_stack) == depth
            await run_target(app, "go:no-such-screen")  # ignored
            await run_target(app, "bogus")
            assert len(app.screen_stack) == depth

    asyncio.run(scenario())


def test_navigating_away_from_an_edited_form_asks_first() -> None:
    """Esc on a dirty form asks; going elsewhere through the palette or a
    chord used to drop the form, and its edits, without asking."""
    from textual.widgets import Input

    from xrun_tui.screens.confirm import ConfirmScreen
    from xrun_tui.widgets.form import FormGuard

    class _Form(FormGuard, Screen):
        def compose(self):
            yield Input("old", id="field")

    async def scenario() -> None:
        app = _Host()
        async with app.run_test() as pilot:
            await run_target(app, "go:help")
            await pilot.pause()
            help_screen = app.screen
            form = _Form()
            await app.push_screen(form)
            await pilot.pause()
            form.snapshot_form()

            # Untouched form: unwinds straight away.
            await run_target(app, "go:help")
            await pilot.pause()
            assert app.screen is help_screen

            form = _Form()
            await app.push_screen(form)
            await pilot.pause()
            form.snapshot_form()
            form.query_one("#field", Input).value = "new"

            await run_target(app, "go:help")
            await pilot.pause()
            assert isinstance(app.screen, ConfirmScreen)
            await pilot.press("enter")  # default No: stay on the form
            await pilot.pause()
            assert app.screen is form

            form._saving = True
            await run_target(app, "go:help")
            await pilot.pause()
            assert app.screen is form  # refused while a save is running
            form._saving = False

            await run_target(app, "go:help")
            await pilot.pause()
            await pilot.press("y")
            await pilot.pause()
            assert app.screen is help_screen

    asyncio.run(scenario())


def test_question_mark_toggles_help(tmp_path, monkeypatch) -> None:
    """The help footer says "press ? to close"; the app's priority `?`
    binding used to swallow the key while help was open."""
    from xrun_tui.screens.help import HelpScreen

    monkeypatch.setenv("XRUN_CONFIG_DIR", str(tmp_path))
    monkeypatch.setenv("XRUN_DATA_DIR", str(tmp_path / "data"))
    monkeypatch.setenv("XRUN_TUI_NO_RESUME", "1")

    async def _noop(self) -> None:
        return None

    monkeypatch.setattr(XrunApp, "on_mount", _noop)
    monkeypatch.setattr(XrunApp, "on_unmount", _noop)

    async def scenario() -> None:
        app = XrunApp()
        async with app.run_test() as pilot:
            base = app.screen
            await pilot.press("question_mark")
            await pilot.pause()
            assert isinstance(app.screen, HelpScreen)
            await pilot.press("question_mark")
            await pilot.pause()
            assert app.screen is base

    asyncio.run(scenario())


@pytest.mark.parametrize("slug", ["notifications", "help"])
def test_chordless_entries_stay_chordless(slug: str) -> None:
    # Reached by bare keys (`n`, `?`), which are app priority bindings.
    assert registry.by_slug(slug).chord is None
