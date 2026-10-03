"""UI polish: settings prefill status, instance id ellipsis, run-detail action row."""
from __future__ import annotations

import asyncio

from textual.app import App
from textual.widgets import DataTable, Input

from xrun_tui import config
from xrun_tui.screens.instances import InstancesScreen, _ellipsize

_BAD = "#414868"


class _FakeDB:
    def __init__(self, instances=None) -> None:
        self._instances = instances or []

    async def runs(self, status=None, limit=300):
        return []

    async def instances(self):
        return self._instances


class _Host(App):
    def __init__(self, db) -> None:
        super().__init__()
        self.db = db


def test_ellipsize_marks_the_cut() -> None:
    assert _ellipsize("short", 10) == "short"
    out = _ellipsize("kaggle:fakefentus/treetop3d-v10-full", 20)
    assert len(out) == 20 and out.endswith("…")


def _id_cells(width: int, ids: list[str], monkeypatch) -> tuple[list[str], DataTable]:
    monkeypatch.setattr(config, "get_vast_api_key", lambda: None)
    inst = [{"id": i, "vendor": "kaggle", "created_at": None,
             "destroyed_at": None} for i in ids]
    out: list[str] = []

    async def scenario() -> None:
        app = _Host(_FakeDB(inst))
        async with app.run_test(size=(width, 40)) as pilot:
            await app.push_screen(InstancesScreen())
            await pilot.pause()
            await asyncio.sleep(0.3)
            t = app.screen.query_one("#local-table", DataTable)
            for row in t.ordered_rows:
                out.append(str(t.get_row(row.key)[1]))
            # no horizontal scroll: columns fit the table
            total = sum(c.get_render_width(t) for c in t.columns.values())
            assert total <= t.size.width, (total, t.size.width)

    asyncio.run(scenario())
    return out, None  # type: ignore[return-value]


def test_instance_ids_fit_at_140_and_ellipsize_when_too_long(monkeypatch) -> None:
    ids = ["kaggle:fakefentus/treetop3d-v10-ds", "x" * 80]
    cells, _ = _id_cells(140, ids, monkeypatch)
    assert cells[0] == ids[0]
    assert cells[1].endswith("…") and len(cells[1]) < 80


def test_instance_ids_ellipsize_at_120(monkeypatch) -> None:
    long_id = "kaggle:fakefentus/treetop3d-v10-full-experiment"
    cells, _ = _id_cells(120, [long_id], monkeypatch)
    assert cells[0].endswith("…") and cells[0] != long_id


def test_prefill_status_has_no_raw_key_list(tmp_path, monkeypatch) -> None:
    monkeypatch.setenv("XRUN_CONFIG_DIR", str(tmp_path))
    monkeypatch.setenv("XRUN_DATA_DIR", str(tmp_path / "data"))
    monkeypatch.setenv("XRUN_TUI_NO_RESUME", "1")
    from xrun_tui import services
    from xrun_tui.app import XrunApp

    async def _noop(self) -> None:
        return None

    monkeypatch.setattr(XrunApp, "on_mount", _noop)
    monkeypatch.setattr(XrunApp, "on_unmount", _noop)

    async def config_show(secrets: bool = False):
        return True, {"poller": {"interval_active_secs": 30}}, ""

    monkeypatch.setattr(services, "config_show", config_show)
    seen: dict[str, str] = {}

    async def scenario() -> None:
        from textual.widgets import Static

        from xrun_tui.screens.settings import SettingsScreen
        app = XrunApp()
        async with app.run_test(size=(100, 32)) as pilot:
            await app.push_screen(SettingsScreen())
            await pilot.pause()
            await asyncio.sleep(0.3)
            s = app.screen
            seen["status"] = str(s.query_one("#prefill-status", Static).render())
            note = s.query_one("#settings-note", Static)
            btn = s.query_one("#btn-save")
            first_input = s.query_one("#tab-general").query(Input).first()
            seen["note_x"] = str(note.content_region.x)
            seen["btn_x"] = str(btn.region.x)
            seen["input_x"] = str(first_input.region.x)
            seen["note_bottom"] = str(note.region.bottom)
            seen["btn_top"] = str(btn.region.y)

    asyncio.run(scenario())
    assert "poller." not in seen["status"] and "budget." not in seen["status"]
    assert "Loaded" in seen["status"]
    # the note wraps above the buttons instead of running under them
    assert int(seen["note_bottom"]) <= int(seen["btn_top"])
    # note and buttons stay in the form's input column, not flush left
    assert seen["note_x"] == seen["btn_x"]
    assert int(seen["note_x"]) >= int(seen["input_x"])
