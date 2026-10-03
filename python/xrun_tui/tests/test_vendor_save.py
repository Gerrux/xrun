"""VendorEditScreen save: the Kaggle token-vs-legacy switch, blank = keep,
and saves that never overlap their credential writes.
"""
from __future__ import annotations

import asyncio
from pathlib import Path

import pytest


@pytest.fixture()
def bare_app(tmp_path: Path, monkeypatch):
    monkeypatch.setenv("XRUN_CONFIG_DIR", str(tmp_path))
    monkeypatch.setenv("XRUN_DATA_DIR", str(tmp_path / "data"))
    monkeypatch.setenv("XRUN_TUI_NO_RESUME", "1")

    async def _noop(self) -> None:
        return None

    from xrun_tui.app import XrunApp
    monkeypatch.setattr(XrunApp, "on_mount", _noop)
    monkeypatch.setattr(XrunApp, "on_unmount", _noop)

    def make(creds_toml: str):
        (tmp_path / "credentials.toml").write_text(creds_toml, encoding="utf-8")
        return XrunApp()

    return make


def _record(monkeypatch, delay: float = 0.0):
    from xrun_tui import services

    ops: list[tuple[str, ...]] = []
    in_flight = {"now": 0, "max": 0}

    async def _op(*op: str):
        in_flight["now"] += 1
        in_flight["max"] = max(in_flight["max"], in_flight["now"])
        ops.append(op)
        await asyncio.sleep(delay)
        in_flight["now"] -= 1
        return True, ""

    async def config_set(key, value, *, secret=False):
        return await _op("set", key, value, "secret" if secret else "plain")

    async def config_unset(key):
        return await _op("unset", key)

    monkeypatch.setattr(services, "config_set", config_set)
    monkeypatch.setattr(services, "config_unset", config_unset)
    return ops, in_flight


_LEGACY = '[kaggle]\nusername = "alice"\nkey = "test-key-legacy123"\n'
_TOKEN = '[kaggle]\ntoken = "test-token-abc999"\nusername = "alice"\n'


async def _open_kaggle(app, pilot):
    from xrun_tui.screens.vendors import VendorEditScreen

    screen = VendorEditScreen("kaggle", "Kaggle")
    await app.push_screen(screen)
    await pilot.pause()
    return screen


def _inp(screen, sel: str):
    from textual.widgets import Input
    return screen.query_one(sel, Input)


def test_secret_inputs_start_blank_and_blank_save_writes_nothing(
    bare_app, monkeypatch
) -> None:
    ops, _ = _record(monkeypatch)

    async def scenario() -> None:
        app = bare_app(_LEGACY)
        async with app.run_test(size=(120, 50)) as pilot:
            screen = await _open_kaggle(app, pilot)
            key = _inp(screen, "#input-kaggle-key")
            assert key.value == ""
            assert "acy123" in key.placeholder and "test-key" not in key.placeholder
            await screen.action_save()
            assert ops == []

    asyncio.run(scenario())


def test_new_token_replaces_legacy_set_before_unset(bare_app, monkeypatch) -> None:
    ops, _ = _record(monkeypatch)

    async def scenario() -> None:
        app = bare_app(_LEGACY)
        async with app.run_test(size=(120, 50)) as pilot:
            screen = await _open_kaggle(app, pilot)
            _inp(screen, "#input-kaggle-token").value = "test-token-new"
            await screen.action_save()
            # The new credential lands first: a failure later never leaves
            # the user with no Kaggle auth at all.
            assert ops == [
                ("set", "kaggle.token", "test-token-new", "secret"),
                ("unset", "kaggle.username"),
                ("unset", "kaggle.key"),
            ]

    asyncio.run(scenario())


def test_legacy_key_replaces_token_with_stored_username(
    bare_app, monkeypatch
) -> None:
    ops, _ = _record(monkeypatch)

    async def scenario() -> None:
        app = bare_app(_TOKEN)
        async with app.run_test(size=(120, 50)) as pilot:
            screen = await _open_kaggle(app, pilot)
            _inp(screen, "#input-kaggle-username").value = ""
            _inp(screen, "#input-kaggle-key").value = "test-key-new"
            await screen.action_save()
            assert ops == [
                ("set", "kaggle.username", "alice", "plain"),
                ("set", "kaggle.key", "test-key-new", "secret"),
                ("unset", "kaggle.token"),
            ]

    asyncio.run(scenario())


def test_overlapping_saves_do_not_interleave(bare_app, monkeypatch) -> None:
    ops, in_flight = _record(monkeypatch, delay=0.05)

    async def scenario() -> None:
        from textual.widgets import Button

        app = bare_app(_LEGACY)
        async with app.run_test(size=(120, 50)) as pilot:
            screen = await _open_kaggle(app, pilot)
            _inp(screen, "#input-kaggle-token").value = "test-token-new"
            btn = screen.query_one("#btn-save", Button)
            screen.on_button_pressed(Button.Pressed(btn))
            await pilot.pause()
            screen.on_button_pressed(Button.Pressed(btn))
            await screen.workers.wait_for_complete()
            await pilot.pause()
            assert in_flight["max"] == 1
            assert [o[:2] for o in ops] == [
                ("set", "kaggle.token"),
                ("unset", "kaggle.username"),
                ("unset", "kaggle.key"),
            ]

    asyncio.run(scenario())
