"""Dashboard empty states and column fit, budget bars, status bar clock."""
from __future__ import annotations

import asyncio
from pathlib import Path

import pytest
from textual.app import App
from textual.widgets import DataTable, Static

from xrun_tui import themes
from xrun_tui.screens import dashboard
from xrun_tui.screens.budget import _TRACK, _TRACK_COLOR, _bar_markup, _bar_parts
from xrun_tui.screens.dashboard import DashboardScreen, name_width
from xrun_tui.widgets.status_bar import StatusBar

_CSS = Path(themes.__file__).parent / "tokyo_night.tcss"


class _FakeDB:
    def __init__(self, runs=None) -> None:
        self.runs_data = runs or []

    async def runs(self, status=None, limit=300):
        return self.runs_data

    async def instances(self):
        return []

    async def metric_keys(self, run_id):
        return []

    async def metrics_for_key(self, run_id, key):
        return []

    async def spend_by_day(self, days):
        return []


class _Host(App):
    CSS_PATH = str(_CSS)

    def __init__(self, db: _FakeDB) -> None:
        super().__init__()
        self.db = db
        self._vast_status_cache: dict = {}
        self._kaggle_status_cache: dict = {}
        self.theme_name = "tokyo-night"


def _run(i: int, status: str) -> dict:
    return {
        "id": f"01TESTRUN{i:03d}",
        "name": "a-very-long-experiment-name-" * 3 + str(i),
        "vendor": "kaggle",
        "status": status,
        "created_at": "2026-01-01T00:00:00Z",
        "cost_usd": None,
    }


@pytest.fixture(autouse=True)
def _no_health(monkeypatch) -> None:
    # Skip the slow doctor probe; it is irrelevant here. Patched per test so
    # the stub does not leak into other modules' dashboard tests.
    async def _skip(self) -> None:
        return None

    monkeypatch.setattr(DashboardScreen, "_refresh_health", _skip)


async def _mount_dashboard(app: _Host, pilot):
    await app.push_screen(DashboardScreen())
    await pilot.pause(0.3)
    return app.screen


# ── Budget bars ────────────────────────────────────────────────────────────

def test_zero_bar_is_all_track_in_faint_colour() -> None:
    filled, empty = _bar_parts(0.0, 18)
    assert filled == "" and empty == _TRACK * 18
    markup = _bar_markup(0.0, "#e0af68", 18)
    assert markup == f"[{_TRACK_COLOR}]{_TRACK * 18}[/]"
    assert "#e0af68" not in markup


def test_partial_bar_colours_only_the_filled_part() -> None:
    markup = _bar_markup(0.5, "#e0af68", 18)
    assert markup.startswith("[#e0af68]█")
    assert f"[{_TRACK_COLOR}]{_TRACK}" in markup
    assert _bar_parts(0.5, 18) == ("█" * 9, _TRACK * 9)


def test_full_bar_has_no_track() -> None:
    assert _bar_markup(1.0, "#7aa2f7", 18) == f"[#7aa2f7]{'█' * 18}[/]"


# ── Dashboard ──────────────────────────────────────────────────────────────

def test_name_width_is_bounded_and_leaves_no_overflow() -> None:
    fixed = dashboard._RECENT_FIXED
    assert name_width(0, fixed) == 24
    for avail in (60, 100, 114, 130, 400):
        nw = name_width(avail, fixed)
        assert dashboard._NAME_MIN <= nw <= dashboard._NAME_MAX
        total = sum(w + 2 for w in fixed) + nw + 2
        if avail - 2 >= total:  # not clamped up by the minimum
            assert total <= avail


def test_empty_dashboard_shows_messages_not_fake_rows() -> None:
    async def scenario() -> None:
        app = _Host(_FakeDB())
        async with app.run_test(size=(140, 42)) as pilot:
            screen = await _mount_dashboard(app, pilot)
            for sel in ("#dash-active", "#dash-recent"):
                assert screen.query_one(sel, DataTable).display is False
                assert screen.query_one(sel, DataTable).row_count == 0
                assert screen.query_one(f"{sel}-empty", Static).display is True
            assert "press" in str(screen.query_one("#dash-active-empty", Static).render())
            # The recent message fills its column; the active one stays short.
            assert screen.query_one("#dash-active-empty").region.height == 3
            assert screen.query_one("#dash-recent-empty").region.height > 3
            # Enter / refresh with no rows must not raise.
            await pilot.press("ctrl+r")
            await pilot.pause(0.3)
            assert app.screen is screen

    asyncio.run(scenario())


def test_table_returns_when_rows_appear_and_fits_the_width() -> None:
    async def scenario() -> None:
        for size in ((140, 42), (120, 36)):
            db = _FakeDB()
            app = _Host(db)
            async with app.run_test(size=size) as pilot:
                screen = await _mount_dashboard(app, pilot)
                assert screen.query_one("#dash-recent", DataTable).display is False

                db.runs_data = [_run(i, "done") for i in range(3)] + [
                    _run(9, "running")
                ]
                await screen._refresh()
                await pilot.pause(0.2)

                for sel in ("#dash-active", "#dash-recent"):
                    t = screen.query_one(sel, DataTable)
                    assert t.display is True and t.row_count >= 1
                    assert screen.query_one(f"{sel}-empty", Static).display is False
                    # No horizontal overflow: every column fits in the table.
                    assert t.virtual_size.width <= t.size.width, (size, sel)
                    assert t.max_scroll_x == 0

                # and back to the empty state
                db.runs_data = []
                await screen._refresh()
                await pilot.pause(0.2)
                assert screen.query_one("#dash-recent", DataTable).display is False

    asyncio.run(scenario())


def test_refresh_keeps_the_cursor_on_the_same_run() -> None:
    async def scenario() -> None:
        db = _FakeDB([_run(i, "done") for i in range(6)])
        app = _Host(db)
        async with app.run_test(size=(140, 42)) as pilot:
            screen = await _mount_dashboard(app, pilot)
            t = screen.query_one("#dash-recent", DataTable)

            def under_cursor() -> str:
                return t.coordinate_to_cell_key(t.cursor_coordinate).row_key.value

            t.move_cursor(row=3, animate=False)
            picked = under_cursor()

            # The 5 s tick refills the table; a new run lands on top.
            db.runs_data = [_run(99, "done")] + db.runs_data
            await screen._refresh()
            await pilot.pause(0.2)
            assert under_cursor() == picked
            assert t.cursor_row == 4

            # The run is gone: no crash, the cursor lands on some other row.
            db.runs_data = [r for r in db.runs_data if r["id"] != picked]
            await screen._refresh()
            await pilot.pause(0.2)
            assert under_cursor() != picked

    asyncio.run(scenario())


# ── Status bar ─────────────────────────────────────────────────────────────

class _SizedBar(StatusBar):
    is_mounted = property(lambda self: True)  # type: ignore[assignment]
    width_override = 80

    @property
    def size(self):  # type: ignore[override]
        from textual.geometry import Size
        return Size(self.width_override, 1)

    def update(self, content="", **kw) -> None:  # type: ignore[override]
        self.captured = str(content)


def _plain_len(markup: str) -> int:
    from rich.text import Text
    return Text.from_markup(markup).cell_len


def test_clock_is_right_aligned_to_the_bar_width() -> None:
    bar = _SizedBar()
    bar._render_snapshot({"active": 0})
    # width minus the 1+1 padding is the drawable line
    assert _plain_len(bar.captured) == 80 - 2

    bar.width_override = 120
    bar.on_resize(None)  # re-renders from the last snapshot
    assert _plain_len(bar.captured) == 120 - 2


def test_detached_bar_falls_back_to_a_fixed_gap() -> None:
    bar = _SizedBar()
    bar.width_override = 0   # not laid out yet
    bar._render_snapshot({"active": 0})
    assert "│" in bar.captured and "idle" in bar.captured
