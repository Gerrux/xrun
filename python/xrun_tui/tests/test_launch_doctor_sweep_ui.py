"""Launch / Doctor / Sweep layout: placeholder, empty state, fitted columns."""
from __future__ import annotations

import asyncio
from pathlib import Path

import pytest
from textual.app import App
from textual.widgets import DataTable, RichLog, Static

from xrun_tui import services, themes
from xrun_tui.screens.doctor import DoctorScreen, fit_doctor_widths
from xrun_tui.screens.launch import LaunchScreen
from xrun_tui.screens.sweep import SweepScreen, fit_sweep_widths


class _FakeDB:
    def __init__(self, runs=None) -> None:
        self._runs = runs or []

    async def runs(self, status=None, limit=300):
        return self._runs

    async def latest_metrics_for_runs(self, ids):
        return {}

    async def metric_extremes_for_runs(self, ids):
        return {}

    async def instances(self):
        return []


class _Host(App):
    # The real sheet: the doctor footer's height comes from it.
    CSS_PATH = str(Path(themes.__file__).parent / "tokyo_night.tcss")

    def __init__(self, db=None) -> None:
        super().__init__()
        self.db = db or _FakeDB()


def _log_text(log: RichLog) -> str:
    return "\n".join(s.text for s in log.lines)


async def _settle(pilot) -> None:
    await pilot.pause()
    await asyncio.sleep(0.3)
    await pilot.pause()


# ── Launch ──────────────────────────────────────────────────────────────────

def test_launch_placeholder_has_no_literal_markup_and_empty_state_shows(
    monkeypatch,
) -> None:
    monkeypatch.setattr(services, "discover_manifests", lambda *a, **k: [])

    async def scenario() -> None:
        app = _Host()
        async with app.run_test(size=(120, 36)) as pilot:
            await app.push_screen(LaunchScreen())
            await _settle(pilot)
            s = app.screen
            text = _log_text(s.query_one("#launch-preview", RichLog))
            assert "select a manifest to preview" in text
            assert "[#" not in text
            assert s.query_one("#launch-table", DataTable).display is False
            empty = s.query_one("#launch-empty", Static)
            assert empty.display is True
            msg = str(empty.render())
            assert "exp/" in msg and "Manifest path" in msg

    asyncio.run(scenario())


def test_launch_without_manifests_notifies_and_does_not_crash(
    monkeypatch,
) -> None:
    monkeypatch.setattr(services, "discover_manifests", lambda *a, **k: [])

    async def scenario() -> None:
        app = _Host()
        async with app.run_test(size=(120, 36)) as pilot:
            await app.push_screen(LaunchScreen())
            await _settle(pilot)
            seen: list[str] = []
            app.screen.notify = lambda m, **k: seen.append(m)  # type: ignore[method-assign]
            await pilot.press("enter")
            await pilot.pause()
            assert seen and "No manifests found" in seen[0]

    asyncio.run(scenario())


def test_launch_table_returns_when_manifests_exist(
    monkeypatch, tmp_path: Path
) -> None:
    m = tmp_path / "a.yaml"
    m.write_text("name: a  # [x]\n", encoding="utf-8")
    found: list[Path] = []
    monkeypatch.setattr(services, "discover_manifests", lambda *a, **k: found)

    async def scenario() -> None:
        app = _Host()
        async with app.run_test(size=(120, 36)) as pilot:
            await app.push_screen(LaunchScreen())
            await _settle(pilot)
            s = app.screen
            assert s.query_one("#launch-empty").display is True
            found.append(m)
            s.action_refresh()
            await _settle(pilot)
            assert s.query_one("#launch-table").display is True
            assert s.query_one("#launch-empty").display is False
            assert s.query_one("#launch-table", DataTable).row_count == 1

    asyncio.run(scenario())


# ── Doctor ──────────────────────────────────────────────────────────────────

LONG = "C:\\" + "very\\long\\path\\" * 12 + "file.md"


def _doctor(monkeypatch) -> None:
    async def fake():
        return True, {"checks": [
            {"name": "config_dir", "status": "ok", "detail": "short"},
            {"name": "claude_md", "status": "warn", "detail": f"not found at {LONG}"},
        ]}, ""

    monkeypatch.setattr(services, "doctor", fake)


@pytest.mark.parametrize("size", [(140, 42), (120, 36)])
def test_doctor_detail_fits_width_and_footer_shows_full_detail(
    monkeypatch, size
) -> None:
    _doctor(monkeypatch)

    async def scenario() -> None:
        app = _Host()
        async with app.run_test(size=size) as pilot:
            await app.push_screen(DoctorScreen())
            await _settle(pilot)
            s = app.screen
            t = s.query_one("#doctor-table", DataTable)
            assert t.virtual_size.width <= t.scrollable_content_region.width
            assert t.max_scroll_x == 0
            t.move_cursor(row=1)
            await _settle(pilot)
            footer = s.query_one("#doctor-footer", Static)
            assert footer.size.height >= 3
            flat = str(footer.render()).replace("\n", "")
            assert LONG in flat  # nothing lost, wrapped in the footer
            # virtual_size includes the padding, so compare with the region
            assert footer.virtual_size.width <= footer.region.width
            assert footer.virtual_size.height <= footer.region.height

    asyncio.run(scenario())


def test_fit_doctor_widths_fills_available_width() -> None:
    for total in (80, 116, 138):
        w = fit_doctor_widths(total, ["config_dir", "claude_md"])
        assert sum(w.values()) + 2 * 4 == total
    assert fit_doctor_widths(30, ["a_very_long_check_name_x"])["detail"] >= 4


# ── Sweep ───────────────────────────────────────────────────────────────────

@pytest.mark.parametrize("total", [60, 80, 116, 138])
def test_fit_sweep_widths_never_exceed_total(total: int) -> None:
    w = fit_sweep_widths(total)
    assert sum(w.values()) + 2 * len(w) <= max(total, 80)
    if total >= 116:
        assert sum(w.values()) + 2 * len(w) == total


@pytest.mark.parametrize("size", [(140, 42), (120, 36)])
def test_sweep_columns_fit_without_horizontal_scroll(size) -> None:
    runs = [
        {"id": f"run{i}0000000", "name": "a-very-long-run-name-" * 4 + str(i),
         "manifest_path": "exp/sw/m.yaml", "cost_usd": 1.5}
        for i in range(3)
    ]

    async def scenario() -> None:
        app = _Host(_FakeDB(runs))
        async with app.run_test(size=size) as pilot:
            await app.push_screen(SweepScreen())
            await _settle(pilot)
            t = app.screen.query_one("#sweep-table", DataTable)
            assert t.row_count == 4
            assert t.virtual_size.width <= t.scrollable_content_region.width
            assert t.max_scroll_x == 0

    asyncio.run(scenario())
