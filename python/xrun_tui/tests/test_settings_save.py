"""Settings save: only changed fields are written, a save never cancels the
prefill, and two saves never overlap their `xrun config` writes.
"""
from __future__ import annotations

import asyncio
from pathlib import Path

import pytest


@pytest.fixture()
def bare_app(tmp_path: Path, monkeypatch):
    """XrunApp without its boot sequence, on a throwaway config dir."""
    monkeypatch.setenv("XRUN_CONFIG_DIR", str(tmp_path))
    monkeypatch.setenv("XRUN_DATA_DIR", str(tmp_path / "data"))
    monkeypatch.setenv("XRUN_TUI_NO_RESUME", "1")

    async def _noop(self) -> None:
        return None

    from xrun_tui.app import XrunApp
    monkeypatch.setattr(XrunApp, "on_mount", _noop)
    monkeypatch.setattr(XrunApp, "on_unmount", _noop)
    return XrunApp


_SHOWN = {
    "poller": {"interval_active_secs": 30, "interval_idle_secs": 120},
    "defaults": {"vendor": "vast", "exp_dir": "exp/"},
}


def _fake_services(monkeypatch, show_gate: asyncio.Event | None = None,
                   set_delay: float = 0.0):
    from xrun_tui import services

    calls: list[tuple[str, ...]] = []
    in_flight = {"now": 0, "max": 0}

    async def config_show(secrets: bool = False):
        if show_gate is not None:
            await show_gate.wait()
        return True, _SHOWN, ""

    async def _write(*call: str):
        in_flight["now"] += 1
        in_flight["max"] = max(in_flight["max"], in_flight["now"])
        calls.append(call)
        await asyncio.sleep(set_delay)
        in_flight["now"] -= 1
        return True, ""

    async def config_set(key, value, *, secret=False):
        return await _write("set", key, value)

    async def config_unset(key):
        return await _write("unset", key)

    monkeypatch.setattr(services, "config_show", config_show)
    monkeypatch.setattr(services, "config_set", config_set)
    monkeypatch.setattr(services, "config_unset", config_unset)
    return calls, in_flight


def _field(screen, key: str):
    from textual.widgets import Input

    from xrun_tui.screens.settings import _sanitize
    return screen.query_one(f"#input-xrun-{_sanitize(key)}", Input)


def test_untouched_save_writes_nothing_and_cleared_field_unsets(
    bare_app, monkeypatch
) -> None:
    calls, _ = _fake_services(monkeypatch)

    async def scenario() -> None:
        from xrun_tui.screens.settings import SettingsScreen

        app = bare_app()
        async with app.run_test(size=(120, 40)) as pilot:
            screen = SettingsScreen()
            await app.push_screen(screen)
            await pilot.pause()
            assert _field(screen, "defaults.exp_dir").value == "exp/"
            await screen._save()
            assert calls == []
            _field(screen, "defaults.exp_dir").value = ""
            _field(screen, "poller.interval_idle_secs").value = "60"
            await screen._save()
            assert sorted(calls) == [
                ("set", "poller.interval_idle_secs", "60"),
                ("unset", "defaults.exp_dir"),
            ]

    asyncio.run(scenario())


def test_save_does_not_cancel_a_pending_prefill(bare_app, monkeypatch) -> None:
    async def scenario() -> None:
        from textual.widgets import Button

        from xrun_tui.screens.settings import SettingsScreen

        gate = asyncio.Event()
        calls, _ = _fake_services(monkeypatch, show_gate=gate)
        app = bare_app()
        async with app.run_test(size=(120, 40)) as pilot:
            screen = SettingsScreen()
            await app.push_screen(screen)
            await pilot.pause()
            # Save while `config show` is still out: nothing typed, nothing
            # loaded → nothing written, and the prefill must still land.
            screen.on_button_pressed(
                Button.Pressed(screen.query_one("#btn-save", Button)))
            await pilot.pause()
            gate.set()
            await pilot.pause()
            await pilot.pause()
            assert calls == []
            assert _field(screen, "defaults.exp_dir").value == "exp/"

    asyncio.run(scenario())


def test_overlapping_saves_never_run_writes_concurrently(
    bare_app, monkeypatch
) -> None:
    calls, in_flight = _fake_services(monkeypatch, set_delay=0.05)

    async def scenario() -> None:
        from textual.widgets import Button

        from xrun_tui.screens.settings import SettingsScreen

        app = bare_app()
        async with app.run_test(size=(120, 40)) as pilot:
            screen = SettingsScreen()
            await app.push_screen(screen)
            await pilot.pause()
            _field(screen, "defaults.exp_dir").value = "runs/"
            _field(screen, "poller.interval_idle_secs").value = "60"
            btn = screen.query_one("#btn-save", Button)
            screen.on_button_pressed(Button.Pressed(btn))
            await pilot.pause()
            screen.on_button_pressed(Button.Pressed(btn))
            await screen.workers.wait_for_complete()
            await pilot.pause()
            assert in_flight["max"] == 1
            # Every changed field written exactly once.
            assert sorted(calls) == [
                ("set", "defaults.exp_dir", "runs/"),
                ("set", "poller.interval_idle_secs", "60"),
            ]

    asyncio.run(scenario())
