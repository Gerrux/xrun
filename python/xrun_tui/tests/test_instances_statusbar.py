"""Status bar per-vendor counts and the vendor-neutral Instances screen.

Both used to assume vast.ai: the bar showed only "active/idle" and the
Instances screen led with a vast tab that told local/ssh users to add a key.
"""
from __future__ import annotations

import asyncio

from textual.app import App
from textual.widgets import DataTable, TabbedContent

from xrun_tui import config
from xrun_tui.screens.instances import TAB_ALL, InstancesScreen
from xrun_tui.widgets.status_bar import StatusBar


class _FakeDB:
    def __init__(self, runs=None, instances=None) -> None:
        self._runs = runs or []
        self._instances = instances or []

    async def runs(self, status=None, limit=300):
        return self._runs

    async def instances(self):
        return self._instances


class _Host(App):
    def __init__(self, db: _FakeDB) -> None:
        super().__init__()
        self.db = db


class _DetachedBar(StatusBar):
    """Renders a snapshot without an app: records what update() gets."""

    is_mounted = property(lambda self: True)  # type: ignore[assignment]

    def update(self, content="", **kw) -> None:  # type: ignore[override]
        self.captured = str(content)


def _plain(snap: dict) -> str:
    bar = _DetachedBar()
    bar._render(snap)
    return bar.captured


def test_status_bar_shows_per_vendor_counts() -> None:
    out = _plain({"active": 4, "by_vendor": {"vast": 1, "local": 1, "ssh": 2}})
    assert "4 active" in out
    assert "local 1 · ssh 2 · vast 1" in out


def test_status_bar_idle_has_no_vendor_chip_and_no_theme() -> None:
    out = _plain({"active": 0, "by_vendor": {}, "theme": "tokyo"})
    assert "idle" in out
    assert "theme:" not in out
    assert " 1 ·" not in out


def test_status_bar_refresh_groups_runs_by_vendor() -> None:
    runs = [{"vendor": "ssh"}, {"vendor": "ssh"}, {"vendor": "local"}]

    async def scenario() -> None:
        app = _Host(_FakeDB(runs=runs))
        async with app.run_test() as pilot:
            bar = StatusBar()
            await app.mount(bar)
            await pilot.pause()
            await asyncio.sleep(0.1)
            assert "local 1 · ssh 2" in str(bar.render())

    asyncio.run(scenario())


def test_instances_default_tab_is_all_vendors_with_vendor_column(monkeypatch) -> None:
    monkeypatch.setattr(config, "get_vast_api_key", lambda: None)
    inst = [{"id": "i-1", "vendor": "ssh", "run_id": "abcdef012345",
             "created_at": None, "destroyed_at": None}]

    async def scenario() -> None:
        app = _Host(_FakeDB(instances=inst))
        async with app.run_test(size=(140, 40)) as pilot:
            await app.push_screen(InstancesScreen())
            await pilot.pause()
            await asyncio.sleep(0.2)
            screen = app.screen
            assert screen.query_one(TabbedContent).active == TAB_ALL
            table = screen.query_one("#local-table", DataTable)
            labels = [str(c.label) for c in table.columns.values()]
            assert "Vendor" in labels
            assert table.row_count == 1

    asyncio.run(scenario())


def test_instances_empty_states_are_vendor_neutral(monkeypatch) -> None:
    monkeypatch.setattr(config, "get_vast_api_key", lambda: None)

    async def scenario() -> None:
        app = _Host(_FakeDB())
        async with app.run_test(size=(140, 40)) as pilot:
            await app.push_screen(InstancesScreen())
            await pilot.pause()
            await asyncio.sleep(0.2)
            screen = app.screen

            def cells(table_id: str) -> str:
                t = screen.query_one(table_id, DataTable)
                return " ".join(str(c) for row in t.ordered_rows
                                for c in t.get_row(row.key))

            live = cells("#remote-table")
            assert "No API key" not in live and "go to Vendors" not in live
            assert "not configured" in live and "All vendors" in live
            assert "xrun launch" in cells("#local-table")

    asyncio.run(scenario())


def test_summary_follows_active_tab(monkeypatch) -> None:
    from textual.widgets import Static

    monkeypatch.setattr(config, "get_vast_api_key", lambda: None)
    inst = [
        {"id": "i-1", "vendor": "ssh", "destroyed_at": None},
        {"id": "i-2", "vendor": "local", "destroyed_at": None},
        {"id": "i-3", "vendor": "ssh", "destroyed_at": "2026-01-01"},
    ]

    async def scenario() -> None:
        app = _Host(_FakeDB(instances=inst))
        async with app.run_test(size=(140, 40)) as pilot:
            await app.push_screen(InstancesScreen())
            await pilot.pause()
            await asyncio.sleep(0.3)
            screen = app.screen

            def summary() -> str:
                return str(screen.query_one("#inst-summary", Static).render())

            # a local/ssh user must not read the vast notice as page summary
            assert "not configured" not in summary()
            assert "2 active" in summary() and "local 1 · ssh 1" in summary()
            assert "1 destroyed" in summary()

            screen.query_one(TabbedContent).active = "tab-vast"
            await pilot.pause()
            await asyncio.sleep(0.3)
            assert "not configured" in summary()

            screen.query_one(TabbedContent).active = TAB_ALL
            await pilot.pause()
            await asyncio.sleep(0.3)
            assert "2 active" in summary()

    asyncio.run(scenario())


def test_vast_timer_stops_without_a_key_and_destroy_defaults_to_no(monkeypatch) -> None:
    monkeypatch.setattr(config, "get_vast_api_key", lambda: None)

    async def scenario() -> None:
        app = _Host(_FakeDB())
        async with app.run_test(size=(140, 40)) as pilot:
            await app.push_screen(InstancesScreen())
            await pilot.pause()
            await asyncio.sleep(0.2)
            screen = app.screen
            assert screen._has_vast_key is False
            calls: list[int] = []

            async def fake_refresh() -> None:
                calls.append(1)

            screen._refresh_remote = fake_refresh  # type: ignore[method-assign]
            await screen._tick_remote()
            assert calls == []
            screen._has_vast_key = True
            await screen._tick_remote()
            assert calls == [1]

            # destroy confirmation focuses "No"
            screen.query_one(TabbedContent).active = "tab-vast"
            screen._remote_instances = [{"id": 7, "gpu_name": "RTX"}]
            screen.query_one("#remote-table", DataTable).add_row("x")
            await pilot.pause()
            await screen.action_destroy()
            await pilot.pause()
            assert app.screen.AUTO_FOCUS == "#btn-no"

    asyncio.run(scenario())


def test_remote_refresh_keeps_the_cursor_on_the_same_instance(monkeypatch) -> None:
    """The 20 s tick rebuilt the table with the cursor back on row 0 (and the
    API order can change), so `x` after a tick targeted another instance."""
    from xrun_tui.screens import vendors

    monkeypatch.setattr(config, "get_vast_api_key", lambda: "test-key-abc")
    listing = {"now": [{"id": i, "gpu_name": f"G{i}"} for i in (1, 2, 3)]}

    async def fetch(api_key):
        return list(listing["now"])

    monkeypatch.setattr(vendors, "fetch_vast_instances", fetch)

    async def scenario() -> None:
        app = _Host(_FakeDB())
        async with app.run_test(size=(140, 40)) as pilot:
            await app.push_screen(InstancesScreen())
            await pilot.pause()
            screen = app.screen
            screen.query_one(TabbedContent).active = "tab-vast"
            await pilot.pause()
            await asyncio.sleep(0.1)
            screen.query_one("#remote-table", DataTable).move_cursor(row=2)
            assert screen._selected_remote_instance()["id"] == 3
            # A new instance shows up first in the next listing.
            listing["now"] = [{"id": 9, "gpu_name": "G9"}, *listing["now"]]
            await screen._refresh_remote()
            assert screen._selected_remote_instance()["id"] == 3
            # The instance under the cursor is gone: no crash, some row.
            listing["now"] = [{"id": 9, "gpu_name": "G9"}]
            await screen._refresh_remote()
            assert screen._selected_remote_instance()["id"] == 9

    asyncio.run(scenario())


def test_destroy_on_all_vendors_tab_explains_instead_of_acting(monkeypatch) -> None:
    monkeypatch.setattr(config, "get_vast_api_key", lambda: None)

    async def scenario() -> None:
        app = _Host(_FakeDB())
        async with app.run_test(size=(140, 40)) as pilot:
            await app.push_screen(InstancesScreen())
            await pilot.pause()
            await asyncio.sleep(0.2)
            seen: list[str] = []
            screen = app.screen
            screen.notify = lambda msg, **kw: seen.append(msg)  # type: ignore[method-assign]
            await screen.action_destroy()
            assert seen and "Runs screen" in seen[0]

    asyncio.run(scenario())
