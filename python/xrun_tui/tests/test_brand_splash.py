"""The mark on the splash: its frames, and the boot animation never holding
the dashboard back."""
from __future__ import annotations

import asyncio

import pytest
from textual.app import App
from textual.widgets import Static

from xrun_tui import brand
from xrun_tui.screens import splash
from xrun_tui.screens.splash import SplashScreen


def _px(size: int, curve: float = 1.0, dot: float = 1.0):
    data = brand.mask(size, curve, dot).tobytes()
    return lambda x, y: data[y * size + x]


def test_dot_stays_detached_at_splash_size() -> None:
    # Merged with the curve, the dot reads as a bent tail and the mark as
    # "L" (docs/brand.md). Some column between the curve's end and the dot
    # must stay mostly tile on every row the dot spans.
    px = _px(splash._MARK_PX)
    gap = max(min(px(x, y) for y in (15, 16, 17)) for x in range(14, 19))
    assert gap >= 64
    assert px(19, 16) < 32  # the dot itself is cut
    assert px(6, 12) < 32  # and so is the curve


def test_uncut_tile_has_no_hole() -> None:
    px = _px(24, curve=0.0, dot=0.0)
    assert min(px(x, y) for x in range(4, 20) for y in range(4, 20)) == 255


def test_timeline_order() -> None:
    assert brand.frame_at(0.0) == (0.0, 0.0, 0.0)
    assert brand.frame_at(1.0) == (1.0, 1.0, 1.0)
    steps = [brand.frame_at(i / 100) for i in range(101)]
    curves = [c for c, _, _ in steps]
    assert curves == sorted(curves)
    for curve, dot, alpha in steps:
        # The curve's start cap would flash on a half-faded tile.
        assert curve == 0 or alpha == 1
        # The dot lands last, once the curve is cut through.
        assert dot == 0 or curve == 1


def test_mask_needs_pillow_only() -> None:
    # scripts/brand.py renders the PNGs from mask() and promises "Pillow
    # only": importing the module must not drag in rich.
    import subprocess
    import sys
    from pathlib import Path

    src = Path(brand.__file__).resolve().parent.parent
    code = (
        "import sys; sys.modules['rich'] = None; "
        f"sys.path.insert(0, {str(src)!r}); "
        "from xrun_tui.brand import mask; mask(16)"
    )
    res = subprocess.run([sys.executable, "-c", code], capture_output=True, text=True)
    assert res.returncode == 0, res.stderr


def test_cells_are_half_blocks() -> None:
    lines = brand.cells(24).plain.split("\n")
    assert len(lines) == 12
    assert all(line == "▀" * 24 for line in lines)


class _Host(App):
    def __init__(self, animation: str = "full") -> None:
        super().__init__()
        self.animation_level = animation  # type: ignore[assignment]


async def _never_done() -> None:
    return None


async def _idle(self) -> None:
    return None


@pytest.fixture
def idle_init(monkeypatch):
    monkeypatch.setattr(SplashScreen, "_init_sequence", _idle)


def _styled(text) -> list[tuple[int, int, str]]:
    # Every frame is the same run of "▀"; only the colours tell them apart.
    return [(s.start, s.end, str(s.style)) for s in text.spans]


def _mark_text(screen) -> list[tuple[int, int, str]]:
    return _styled(screen.query_one("#splash-mark", Static).content)


def _final() -> list[tuple[int, int, str]]:
    return _styled(brand.cells(splash._MARK_PX))


async def _loaded(pilot, screen) -> None:
    for _ in range(50):
        if screen._mark_final is not None:
            return
        await pilot.pause(0.05)
    raise AssertionError("mark never loaded")


def test_finishing_boot_cuts_the_animation(idle_init, monkeypatch) -> None:
    # An animation long enough to be caught mid-way.
    monkeypatch.setattr(splash, "_MARK_ANIM_S", 60.0)

    async def scenario() -> tuple[bool, list, bool, list]:
        app = _Host()
        async with app.run_test(size=(100, 42)) as pilot:
            screen = SplashScreen(_never_done)
            await app.push_screen(screen)
            await _loaded(pilot, screen)
            await pilot.pause(0.1)
            running, mid = screen._mark_timer is not None, _mark_text(screen)
            screen._finish_mark()
            await pilot.pause()
            return running, mid, screen._mark_timer is None, _mark_text(screen)

    running, mid, stopped, end = asyncio.run(scenario())
    assert running and mid != _final()
    assert stopped and end == _final()


def test_real_init_sequence_cuts_the_animation(monkeypatch) -> None:
    # The real checklist, with its I/O stubbed: by the time it hands over to
    # the dashboard the mark must already be the finished frame.
    from xrun_tui import config, services

    async def _none(*args, **kwargs):
        return None

    async def _no_config(*args, **kwargs):
        return False, {}, ""

    monkeypatch.setattr(splash, "_MARK_ANIM_S", 60.0)
    monkeypatch.setattr(config, "read_credentials", lambda: {})
    monkeypatch.setattr(config, "get_vast_api_key", lambda: None)
    monkeypatch.setattr(services, "xrun_version", _none)
    monkeypatch.setattr(services, "config_show", _no_config)
    monkeypatch.setattr(services, "discover_manifests", lambda *a, **k: [])

    async def scenario() -> tuple[bool, list]:
        seen: list[tuple[bool, list]] = []
        app = _Host()
        app.db = type("Db", (), {"_conn": object()})()  # type: ignore[attr-defined]
        screen: SplashScreen

        async def _on_done() -> None:
            seen.append((screen._mark_timer is None, _mark_text(screen)))

        async with app.run_test(size=(100, 42)) as pilot:
            screen = SplashScreen(_on_done)
            # Load the mark first so the init sequence meets a running
            # animation rather than racing the Pillow import.
            orig = SplashScreen._init_sequence

            async def _late_init(self) -> None:
                await _loaded(pilot, self)
                await orig(self)

            monkeypatch.setattr(SplashScreen, "_init_sequence", _late_init)
            await app.push_screen(screen)
            for _ in range(60):
                if seen:
                    break
                await pilot.pause(0.05)
        assert seen, "init sequence never finished"
        return seen[0]

    assert asyncio.run(scenario()) == (True, _final())


def test_mark_after_boot_is_shown_finished(idle_init) -> None:
    # Init done before Pillow finished importing: no animation at all.
    async def scenario() -> tuple[bool, list]:
        app = _Host()
        async with app.run_test(size=(100, 42)) as pilot:
            screen = SplashScreen(_never_done)
            screen._booted = True
            await app.push_screen(screen)
            await _loaded(pilot, screen)
            await pilot.pause()
            return screen._mark_timer is None, _mark_text(screen)

    assert asyncio.run(scenario()) == (True, _final())


def test_no_animation_level_shows_final_frame(idle_init) -> None:
    async def scenario() -> tuple[bool, list]:
        app = _Host(animation="none")
        async with app.run_test(size=(100, 42)) as pilot:
            screen = SplashScreen(_never_done)
            await app.push_screen(screen)
            await _loaded(pilot, screen)
            await pilot.pause()
            return screen._mark_timer is None, _mark_text(screen)

    assert asyncio.run(scenario()) == (True, _final())


def test_mark_blends_over_the_theme_background(idle_init) -> None:
    # The theme filter remaps only exact Tokyo colours; an edge blended over
    # Tokyo's #1a1b26 would keep a Tokyo-tinted fringe under Catppuccin.
    async def scenario() -> str:
        app = _Host(animation="none")
        app.theme_name = "catppuccin-mocha"  # type: ignore[attr-defined]
        async with app.run_test(size=(100, 42)) as pilot:
            screen = SplashScreen(_never_done)
            await app.push_screen(screen)
            await _loaded(pilot, screen)
            text = screen.query_one("#splash-mark", Static).content
            # Cell (0, 0) is outside the tile's rounded corner: pure background.
            return str(text.spans[0].style.bgcolor.triplet.hex)

    assert asyncio.run(scenario()) == "#1e1e2e"


@pytest.mark.parametrize("height, shown", [(splash._MARK_MIN_H - 1, False), (42, True)])
def test_mark_needs_room(idle_init, height, shown) -> None:
    async def scenario() -> bool:
        app = _Host()
        async with app.run_test(size=(100, height)) as pilot:
            screen = SplashScreen(_never_done)
            await app.push_screen(screen)
            await pilot.pause()
            return screen.query_one("#splash-mark", Static).display

    assert asyncio.run(scenario()) is shown


def test_brand_load_failure_drops_the_mark(idle_init, monkeypatch) -> None:
    def _broken(*args, **kwargs):
        raise ImportError("no Pillow")

    monkeypatch.setattr(brand, "cells", _broken)

    async def scenario() -> bool:
        app = _Host()
        async with app.run_test(size=(100, 42)) as pilot:
            screen = SplashScreen(_never_done)
            await app.push_screen(screen)
            for _ in range(50):
                if not screen._mark_ok:
                    break
                await pilot.pause(0.05)
            return screen.query_one("#splash-mark", Static).display

    assert asyncio.run(scenario()) is False
