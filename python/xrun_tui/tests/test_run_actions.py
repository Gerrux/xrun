"""Shared confirm-and-run flows for a run: stop asks first (default No), the
wording is one string for every entry point, failures surface the CLI message
and still refresh, bulk stop summarises mixed results.
"""
from __future__ import annotations

import asyncio

from textual.app import App
from textual.screen import Screen
from textual.widgets import Static

from xrun_tui import run_actions, services

RUN = {"id": "01HXYZ12ABCDEFGH", "name": "my_experiment", "status": "running"}


class _Host(App):
    def __init__(self) -> None:
        super().__init__()
        self.notes: list[tuple[str, str, float | None]] = []

    def notify(self, message, *, title="", severity="information", timeout=None, **kw):
        self.notes.append((str(message), severity, timeout))


class _Page(Screen):
    def compose(self):
        yield Static("page")


def _stub(monkeypatch, name: str, result=(True, "")):
    calls: list[tuple] = []

    async def fake(*args, **kwargs):
        calls.append((args, kwargs))
        r = result(args[0]) if callable(result) else result
        return r

    monkeypatch.setattr(services, name, fake)
    return calls


def _drive(body):
    async def scenario() -> None:
        app = _Host()
        async with app.run_test() as pilot:
            await app.push_screen(_Page())
            await pilot.pause()
            await body(app, app.screen, pilot)

    asyncio.run(scenario())


def test_stop_asks_first_enter_does_not_confirm_y_does(monkeypatch) -> None:
    calls = _stub(monkeypatch, "stop_run")
    refreshed: list[bool] = []

    async def body(app, page, pilot) -> None:
        await run_actions.stop(page, RUN, after=refreshed.append)
        await pilot.pause()
        msg = str(app.screen.query_one(".confirm-message", Static).render())
        assert msg == "Stop my_experiment (01HXYZ12)?"
        assert calls == []
        await pilot.press("enter")  # focus is on No
        await pilot.pause()
        assert calls == [] and refreshed == []

        await run_actions.stop(page, RUN, after=refreshed.append)
        await pilot.pause()
        await pilot.press("y")
        await pilot.pause()
        await pilot.pause()
        assert calls == [((RUN["id"],), {})]
        assert refreshed == [True]
        assert app.notes == [("Stopped my_experiment (01HXYZ12)", "information", None)]

    _drive(body)


def test_failure_shows_cli_message_and_still_refreshes(monkeypatch) -> None:
    _stub(monkeypatch, "rerun_run", (False, "vendor said no"))
    refreshed: list[bool] = []

    async def body(app, page, pilot) -> None:
        await run_actions.rerun(page, RUN, after=refreshed.append)
        await pilot.pause()
        await pilot.press("y")
        await pilot.pause()
        await pilot.pause()
        assert app.notes == [("Rerun failed: vendor said no", "error", 8)]
        assert refreshed == [False]

    _drive(body)


def test_after_is_skipped_when_screen_is_gone(monkeypatch) -> None:
    _stub(monkeypatch, "stop_run")
    refreshed: list[bool] = []

    async def body(app, page, pilot) -> None:
        await run_actions.stop(page, RUN, after=refreshed.append)
        await pilot.pause()
        await app.pop_screen()  # drops the confirm
        await app.pop_screen()  # drops the page; the answer never comes
        await pilot.pause()
        # Direct path: work finishes after the screen detached.
        await run_actions.run_and_report(
            page, lambda: services.stop_run("x"),
            ok_msg="done", fail_prefix="f", after=refreshed.append,
        )
        assert refreshed == []
        assert app.notes[-1][0] == "done"

    _drive(body)


def test_unnamed_run_uses_short_id_only() -> None:
    assert run_actions.run_label({"id": "01HXYZ12ABCDEFGH"}) == "01HXYZ12"
    assert run_actions.run_label(RUN) == "my_experiment (01HXYZ12)"


def test_runs_list_and_detail_produce_the_same_prompt(monkeypatch) -> None:
    from xrun_tui.screens.run_detail import RunDetailScreen
    from xrun_tui.screens.runs import RunsScreen

    prompts: list[tuple[str, bool]] = []

    async def fake_confirm(screen, prompt, work, *, default_no=False, **kw) -> None:
        prompts.append((prompt, default_no))

    monkeypatch.setattr(run_actions, "confirm_and_run", fake_confirm)

    class _ListFake:
        _runs = [RUN]
        _run_ids = [RUN["id"]]
        _filter_text = ""
        _selected_run_id = lambda self: RUN["id"]  # noqa: E731
        notify = lambda self, *a, **k: None  # noqa: E731
        _after_action = None

    class _DetailFake:
        _run = RUN
        _run_id = RUN["id"]
        _after_action = None

    async def go() -> None:
        await RunsScreen.action_stop_run(_ListFake())
        await RunDetailScreen.action_stop_run(_DetailFake())
        await RunsScreen.action_rerun(_ListFake())
        await RunDetailScreen.action_rerun(_DetailFake())
        await RunsScreen.action_pull_run(_ListFake())
        await RunDetailScreen.action_pull(_DetailFake())

    # RunsScreen.action_rerun/pull use _selected_run, a plain method.
    _ListFake._selected_run = RunsScreen._selected_run
    asyncio.run(go())
    assert prompts[0] == prompts[1] == ("Stop my_experiment (01HXYZ12)?", True)
    assert prompts[2] == prompts[3] == ("Rerun my_experiment (01HXYZ12)?", True)
    assert prompts[4] == prompts[5] == (
        "Pull latest checkpoint of my_experiment (01HXYZ12)?", False)


def test_grouped_runs_list_stops_the_highlighted_run(monkeypatch) -> None:
    """Grouped mode puts header rows in the table that `_run_ids` lacks; the
    cursor row used to index `_run_ids` (and stop re-indexed the ungrouped
    list), so `s` on one run prompted for and stopped another."""
    from textual.widgets import DataTable

    from xrun_tui.screens.runs import RunsScreen

    prompts: list[str] = []

    async def fake_confirm(screen, prompt, work, **kw) -> None:
        prompts.append(prompt)

    monkeypatch.setattr(run_actions, "confirm_and_run", fake_confirm)
    a = {"id": "AAAA00000000", "name": "alpha", "status": "running"}
    b = {"id": "BBBB00000000", "name": "beta", "status": "running"}

    class _Grouped(Screen):
        _selected_run_id = RunsScreen._selected_run_id
        _selected_run = RunsScreen._selected_run
        _after_action = None

        def compose(self):
            yield DataTable()

        def on_mount(self) -> None:
            t = self.query_one(DataTable)
            t.add_columns("m", "name")
            t.add_row("", "── group ──")
            t.add_row("", "alpha", key=a["id"])
            t.add_row("", "beta", key=b["id"])
            self._run_ids = [a["id"], b["id"]]
            self._runs = [a, b]

    async def scenario() -> None:
        app = _Host()
        async with app.run_test() as pilot:
            screen = _Grouped()
            await app.push_screen(screen)
            await pilot.pause()
            table = screen.query_one(DataTable)
            table.move_cursor(row=0)
            assert screen._selected_run_id() is None  # the group header
            table.move_cursor(row=1)
            assert screen._selected_run_id() == a["id"]
            await RunsScreen.action_stop_run(screen)
            table.move_cursor(row=2)
            await RunsScreen.action_stop_run(screen)
            assert prompts == ["Stop alpha (AAAA0000)?", "Stop beta (BBBB0000)?"]

    asyncio.run(scenario())


def test_bulk_prompt_names_at_most_five_runs() -> None:
    runs = [{"id": f"RUN{i}0000000", "name": f"r{i}"} for i in range(7)]
    lines = run_actions._bulk_prompt("Stop 7 runs?", runs).splitlines()
    assert lines[0] == "Stop 7 runs?"
    assert lines[1:6] == [f"r{i} (RUN{i}0000)" for i in range(5)]
    assert lines[6] == "… and 2 more"


def test_rerun_cannot_be_started_twice_while_the_first_is_running(monkeypatch) -> None:
    gate = asyncio.Event()
    calls: list[str] = []

    async def slow_rerun(run_id: str):
        calls.append(run_id)
        await gate.wait()
        return True, ""

    monkeypatch.setattr(services, "rerun_run", slow_rerun)

    async def body(app, page, pilot) -> None:
        await run_actions.rerun(page, RUN)
        await pilot.pause()
        await pilot.press("y")
        await pilot.pause()
        assert calls == [RUN["id"]]

        # Second press while the CLI call is still out: no prompt, no call.
        await run_actions.rerun(page, RUN)
        await pilot.pause()
        assert app.screen is page
        assert calls == [RUN["id"]]
        assert app.notes[-1] == (
            "Rerun already in progress for this run", "warning", None)

        gate.set()
        for _ in range(3):
            await pilot.pause()
        # Finished: the action is available again.
        await run_actions.rerun(page, RUN)
        await pilot.pause()
        assert app.screen is not page

    _drive(body)
    run_actions._busy.clear()


def test_bulk_stop_summarises_mixed_results(monkeypatch) -> None:
    runs = [{"id": f"RUN{i}0000000", "name": f"r{i}"} for i in range(3)]
    calls = _stub(
        monkeypatch, "stop_run",
        lambda rid: (False, "boom") if rid.startswith("RUN1") else (True, ""),
    )
    refreshed: list[bool] = []

    async def body(app, page, pilot) -> None:
        await run_actions.bulk_stop(page, runs, after=refreshed.append)
        await pilot.pause()
        msg = str(app.screen.query_one(".confirm-message", Static).render())
        assert msg.splitlines() == [
            "Stop 3 runs?", "r0 (RUN00000)", "r1 (RUN10000)", "r2 (RUN20000)",
        ]
        await pilot.press("enter")  # default No
        await pilot.pause()
        assert calls == []

        await run_actions.bulk_stop(page, runs, after=refreshed.append)
        await pilot.pause()
        await pilot.press("y")
        for _ in range(3):
            await pilot.pause()
        assert [c[0][0] for c in calls] == [r["id"] for r in runs]
        assert app.notes == [
            ("Stop: 2 ok, 1 failed. First error: r1 (RUN10000): boom", "error", 8)
        ]
        assert refreshed == [False]

    _drive(body)
