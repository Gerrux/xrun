"""Every screen's text must have a row to be drawn on.

Textual heights are border-box: `height: 1` plus a border or vertical padding
leaves the content zero rows, and the widget shows only its border. That hid
the screen titles, stats bars, section headers and the status bar on every
screen without failing anything else.
"""
from __future__ import annotations

import asyncio
from pathlib import Path

import pytest
from rich.segment import Segment
from rich.style import Style
from textual.app import App
from textual.color import Color
from textual.widgets import Button, Label, Static

from xrun_tui import themes
from xrun_tui.screens.registry import iter_screens

_CSS = Path(themes.__file__).parent / "tokyo_night.tcss"


class _EmptyDB:
    """Answers every query with nothing; layout does not depend on data.

    An empty dict serves callers that expect a list (iterates, len 0) and the
    ones that expect a mapping alike.
    """

    def __getattr__(self, name: str):
        async def _empty(*args, **kwargs):
            return {}
        return _empty


class _Host(App):
    CSS_PATH = str(_CSS)

    def __init__(self) -> None:
        super().__init__()
        self.db = _EmptyDB()
        self._vast_status_cache: dict = {}
        self._kaggle_status_cache: dict = {}
        self._exp_dir = None
        self._notif_history: list = []
        self._compare_selection: list = []
        self.theme_name = "tokyo-night"


def _collapsed(screen) -> list[str]:
    """Text widgets, and containers with visible children, left zero rows."""
    found = []
    for w in screen.walk_children():
        if not w.display or w.region.height <= 0 or w.content_region.height > 0:
            continue
        is_text = isinstance(w, (Static, Label, Button))
        has_children = any(c.display for c in w.children)
        if is_text or has_children:
            found.append(f"{type(w).__name__}#{w.id} .{' .'.join(w.classes)}")
    return found


def _run_detail():
    from xrun_tui.screens.run_detail import RunDetailScreen
    return RunDetailScreen("01TESTRUN")


# Registered screens plus the ones reached only from a row.
_FACTORIES = {e.slug: e.factory for e in iter_screens()} | {
    "run_detail": _run_detail,
}


@pytest.mark.parametrize("slug", list(_FACTORIES))
def test_no_text_widget_is_collapsed_by_its_border(slug, tmp_path, monkeypatch) -> None:
    monkeypatch.setenv("XRUN_CONFIG_DIR", str(tmp_path))
    monkeypatch.setenv("XRUN_DATA_DIR", str(tmp_path))

    async def scenario() -> list[str]:
        app = _Host()
        async with app.run_test(size=(140, 42)) as pilot:
            await app.push_screen(_FACTORIES[slug]())
            await pilot.pause(0.2)
            return _collapsed(app.screen)

    assert asyncio.run(scenario()) == []


@pytest.mark.parametrize("slug,empty_id", [
    ("runs", "#runs-empty"), ("watch", "#watch-empty"),
])
def test_screen_empty_state_still_fills_the_body(
    slug, empty_id, tmp_path, monkeypatch,
) -> None:
    # The dashboard's 3-row empty state must not leak into the shared class.
    monkeypatch.setenv("XRUN_CONFIG_DIR", str(tmp_path))
    monkeypatch.setenv("XRUN_DATA_DIR", str(tmp_path))

    async def scenario() -> int:
        app = _Host()
        async with app.run_test(size=(140, 42)) as pilot:
            await app.push_screen(_FACTORIES[slug]())
            await pilot.pause(0.3)
            empty = app.screen.query_one(empty_id)
            assert empty.display
            return empty.region.height

    assert asyncio.run(scenario()) > 10


def test_runs_no_match_message_shows_filter_text_literally(tmp_path, monkeypatch) -> None:
    # The filter is user input inside markup: "[/]" raised MarkupError.
    monkeypatch.setenv("XRUN_CONFIG_DIR", str(tmp_path))
    monkeypatch.setenv("XRUN_DATA_DIR", str(tmp_path))

    async def scenario() -> str:
        app = _Host()
        async with app.run_test(size=(140, 42)) as pilot:
            await app.push_screen(_FACTORIES["runs"]())
            await pilot.pause(0.2)
            app.screen._on_filter_change("[/]x[b]")
            await pilot.pause()
            return str(app.screen.query_one("#runs-empty", Static).render())

    assert "No runs match '[/]x[b]'" in asyncio.run(scenario())


def test_splash_version_line_is_not_collapsed(tmp_path, monkeypatch) -> None:
    # Not in the registry, so the parametrized test above never sees it.
    from xrun_tui.screens.splash import SplashScreen

    monkeypatch.setenv("XRUN_CONFIG_DIR", str(tmp_path))
    monkeypatch.setenv("XRUN_DATA_DIR", str(tmp_path))

    async def _idle(self) -> None:
        return None

    monkeypatch.setattr(SplashScreen, "_init_sequence", _idle)

    async def _never_done() -> None:
        return None

    async def scenario() -> list[str]:
        app = _Host()
        async with app.run_test(size=(140, 42)) as pilot:
            await app.push_screen(SplashScreen(_never_done, version="9.9"))
            await pilot.pause(0.2)
            return _collapsed(app.screen)

    assert asyncio.run(scenario()) == []


def test_button_key_hints_are_escaped() -> None:
    # Button labels are markup: "Stop  [s]" drew "Stop" struck through and
    # "Rerun [r]" reversed, and [enter] / [Esc] vanished. `[Ctrl+S]` survives
    # only because "+" is not a tag character, so escape every hint.
    import re

    import xrun_tui

    root = Path(xrun_tui.__file__).parent
    pattern = re.compile(r'Button\(\s*f?"[^"]*?(?<!\\)\[[A-Za-z_]+\]')
    hits = [
        f"{path.relative_to(root)}: {m.group(0)}"
        for path in root.rglob("*.py")
        for m in pattern.finditer(path.read_text(encoding="utf-8"))
    ]
    assert hits == []


# ── Palette filter ─────────────────────────────────────────────────────────

def test_palette_filter_recolours_inline_tokyo_colours() -> None:
    f =themes.palette_filter("catppuccin-mocha")
    assert f is not None
    out = f.apply(
        [Segment("x", Style.parse("bold #565f89 on #1a1b26")),
         Segment("y", Style.parse("#123456")),
         Segment("z", None)],
        Color(0, 0, 0),
    )
    assert out[0].style.color.triplet.hex == "#9399b2"
    assert out[0].style.bgcolor.triplet.hex == "#1e1e2e"
    assert out[0].style.bold
    assert out[1].style == Style.parse("#123456")
    assert out[2].style is None


def test_palette_filter_follows_a_runtime_theme_switch(tmp_path, monkeypatch) -> None:
    # Settings assigns `app.theme_name`; the filter must not keep the old map.
    monkeypatch.setenv("XRUN_CONFIG_DIR", str(tmp_path))
    from xrun_tui.app import XrunApp

    app = XrunApp()
    app.theme_name = "gruvbox-dark"
    mine = [f for f in app.get_line_filters() if isinstance(f, themes.PaletteFilter)]
    assert len(mine) == 1
    assert mine[0].apply(
        [Segment("x", Style.parse("#565f89"))], Color(0, 0, 0)
    )[0].style.color.triplet.hex == themes.GRUVBOX_DARK["#565f89"]
    app.theme_name = "tokyo-night"
    assert not [f for f in app.get_line_filters()
                if isinstance(f, themes.PaletteFilter)]


def test_tokyo_night_needs_no_filter() -> None:
    assert themes.palette_filter("tokyo-night") is None
    assert themes.palette_filter("no-such-theme") is None


@pytest.mark.parametrize("name", [n for n in themes.PALETTES if n != "tokyo-night"])
def test_remapped_colours_are_not_remapped_twice(name) -> None:
    # The stylesheet is already recoloured as text; the filter then sees its
    # output. A target that is itself a Tokyo source would shift twice.
    palette = themes.PALETTES[name]
    sources = {s for s, d in palette.items() if s != d}
    targets = {d for s, d in palette.items() if s != d}
    assert not sources & targets
