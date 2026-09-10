"""Notifications setup screen + wizard step: pure helpers and a headless
pilot pass over each screen so compose errors surface in CI, not on the
user's first `g n`.
"""
from __future__ import annotations

import asyncio
import os
import sys
import tomllib
from pathlib import Path

import pytest

from xrun_tui import config
from xrun_tui.screens.notify_setup import (
    CHANNELS,
    PRESETS,
    channel_configured,
    generate_topic,
    preset_for,
)
from xrun_tui.screens.notifications import _journal_entries


# ── pure helpers ─────────────────────────────────────────────────────────────

def test_generate_topic_is_random_and_prefixed() -> None:
    a, b = generate_topic(), generate_topic()
    assert a.startswith("xrun-") and b.startswith("xrun-")
    assert a != b
    assert len(a) == len("xrun-") + 12


def test_channel_configured_rules() -> None:
    assert not channel_configured({}, "ntfy")
    assert channel_configured({"ntfy": {"topic": "t"}}, "ntfy")
    assert not channel_configured({"telegram": {"bot_token": "x"}}, "telegram")
    assert channel_configured({"telegram": {"bot_token": "x", "chat_id": "1"}}, "telegram")
    assert channel_configured({"webhook": {"url": "https://h"}}, "webhook")
    assert channel_configured({}, "desktop")
    assert {c[0] for c in CHANNELS} == {"ntfy", "telegram", "webhook", "desktop"}


def test_preset_roundtrip() -> None:
    for pid, _, pats in PRESETS:
        assert preset_for(list(pats)) == pid
    assert preset_for(["run.done"]) == "custom"
    assert preset_for([]) == "custom"


def test_journal_entries_collapse_channels_and_flag_failures() -> None:
    rows = [
        {"ts": "2026-09-10T10:00:00Z", "dedupe_key": "run.done:a", "kind": "run.done",
         "channel": "ntfy", "ok": True, "title": "done"},
        {"ts": "2026-09-10T10:00:00Z", "dedupe_key": "run.done:a", "kind": "run.done",
         "channel": "telegram", "ok": False, "title": "done", "error": "401"},
        {"ts": "2026-09-10T11:00:00Z", "dedupe_key": "poller.dead:b", "kind": "poller.dead",
         "channel": "ntfy", "ok": True, "title": "dead"},
    ]
    entries = _journal_entries(rows)
    assert len(entries) == 2
    done = next(e for e in entries if "done" in e["message"])
    assert done["severity"] == "error"          # one channel failed
    assert "ntfy" in done["message"] and "telegram✗" in done["message"]
    assert "401" in done["message"]
    dead = next(e for e in entries if "dead" in e["message"])
    assert dead["severity"] == "error"
    assert dead["ts"] > done["ts"]


# ── credentials writer keeps nested ssh tables ───────────────────────────────

def test_write_credentials_preserves_nested_tables(tmp_path: Path, monkeypatch) -> None:
    monkeypatch.setenv("XRUN_CONFIG_DIR", str(tmp_path))
    creds = {
        "vast": {"api_key": "k"},
        "ntfy": {"topic": "xrun-abc", "url": None},
        "ssh": {"box": {"host": "10.0.0.2", "user": "root", "port": 22}},
    }
    config.write_credentials(creds)
    back = tomllib.loads((tmp_path / "credentials.toml").read_text(encoding="utf-8"))
    assert back["vast"]["api_key"] == "k"
    assert back["ntfy"] == {"topic": "xrun-abc"}
    assert back["ssh"]["box"] == {"host": "10.0.0.2", "user": "root", "port": 22}
    assert config.read_credentials() == back


# ── headless pilot over the screens ──────────────────────────────────────────

class _BareApp:
    """XrunApp without the boot sequence (DB connect, splash → dashboard /
    wizard). The screens under test only need an App to mount into; the
    splash's later `switch_screen` would otherwise replace the screen we
    just pushed. Textual dispatches `on_mount` for every class in the MRO,
    so a subclass override is not enough — the fixture patches the class.
    """

    @staticmethod
    def make():
        from xrun_tui.app import XrunApp

        return XrunApp()


def _fresh_xrun_bin() -> Path | None:
    root = Path(__file__).resolve().parents[3]
    exe = root / "target" / "debug" / ("xrun.exe" if sys.platform == "win32" else "xrun")
    return exe if exe.exists() else None


@pytest.fixture()
def isolated_env(tmp_path: Path, monkeypatch):
    """Point xrun + the TUI at a throwaway config/data dir; prefer the
    freshly built binary so `notify` / `watchdog schedule` exist."""
    cfg = tmp_path / "cfg"
    data = tmp_path / "data"
    cfg.mkdir()
    data.mkdir()
    monkeypatch.setenv("XRUN_CONFIG_DIR", str(cfg))
    monkeypatch.setenv("XRUN_DATA_DIR", str(data))
    monkeypatch.setenv("XRUN_TUI_NO_RESUME", "1")

    async def _noop(self) -> None:
        return None

    from xrun_tui.app import XrunApp
    monkeypatch.setattr(XrunApp, "on_mount", _noop)
    monkeypatch.setattr(XrunApp, "on_unmount", _noop)
    if exe := _fresh_xrun_bin():
        monkeypatch.setenv("PATH", str(exe.parent) + os.pathsep + os.environ.get("PATH", ""))
    (cfg / "config.toml").write_text(
        '[notify]\nchannels = ["desktop"]\nevents = ["*"]\ncost_warn_pct = [50, 80]\n',
        encoding="utf-8",
    )
    (cfg / "credentials.toml").write_text('[ntfy]\ntopic = "xrun-test"\n', encoding="utf-8")
    return cfg


def test_notify_setup_screen_mounts_and_navigates(isolated_env: Path) -> None:
    asyncio.run(_test_notify_setup_screen_mounts_and_navigates())


async def _test_notify_setup_screen_mounts_and_navigates() -> None:
    from textual.widgets import Static

    from xrun_tui.screens.notify_setup import NotifySetupScreen, _RULES_ROW, _WATCHDOG_ROW

    app = _BareApp.make()
    async with app.run_test(size=(120, 40)) as pilot:
        await pilot.pause()
        screen = NotifySetupScreen()
        await app.push_screen(screen)
        await pilot.pause()
        # Cards rendered with the right initial states.
        assert "ON" in str(screen.query_one("#nstatus-3", Static).render())      # desktop enabled
        assert "OFF" in str(screen.query_one("#nstatus-0", Static).render())     # ntfy configured, off
        assert "EMPTY" in str(screen.query_one("#nstatus-1", Static).render())   # telegram
        assert screen.query_one(f"#ninfo-{_RULES_ROW}", Static)
        assert screen.query_one(f"#ninfo-{_WATCHDOG_ROW}", Static)
        # Cursor moves and wraps.
        for _ in range(6):
            await pilot.press("j")
        assert screen._cursor == 0
        await pilot.press("k")
        assert screen._cursor == _WATCHDOG_ROW


def test_channel_edit_and_rules_forms_compose(isolated_env: Path) -> None:
    asyncio.run(_test_channel_edit_and_rules_forms_compose())


async def _test_channel_edit_and_rules_forms_compose() -> None:
    from textual.widgets import Input

    from xrun_tui.screens.notify_setup import ChannelEditScreen, RulesEditScreen

    app = _BareApp.make()
    async with app.run_test(size=(120, 40)) as pilot:
        await pilot.pause()
        for cid, name in (("ntfy", "ntfy"), ("telegram", "Telegram"), ("webhook", "Webhook")):
            form = ChannelEditScreen(cid, name)
            await app.push_screen(form)
            await pilot.pause()
            if cid == "ntfy":
                assert form.query_one("#in-ntfy-topic", Input).value == "xrun-test"
            await pilot.press("escape")
            await pilot.pause()
        rules = RulesEditScreen()
        await app.push_screen(rules)
        await pilot.pause()
        assert rules.query_one("#in-pct", Input).value == "50, 80"
        await pilot.press("escape")


def test_wizard_notify_step_prefills_topic(isolated_env: Path) -> None:
    asyncio.run(_test_wizard_notify_step_prefills_topic())


async def _test_wizard_notify_step_prefills_topic() -> None:
    from textual.widgets import Checkbox, Input

    from xrun_tui.screens.wizard.screen import WizardScreen

    app = _BareApp.make()
    async with app.run_test(size=(120, 45)) as pilot:
        await pilot.pause()
        wiz = WizardScreen()
        await app.push_screen(wiz)
        await pilot.pause()
        assert wiz._n_steps == 5
        # Existing topic is picked up; channel list didn't include ntfy → off.
        assert wiz._notify_topic == "xrun-test"
        assert wiz._notify_ntfy is False
        assert wiz._notify_desktop is True
        wiz._step = 3
        await wiz._render_step()
        await pilot.pause()
        cb = wiz.query_one("#wiz-notify-cb-ntfy", Checkbox)
        assert cb.value is False
        cb.value = True
        await pilot.pause()
        assert wiz._notify_ntfy is True
        assert wiz.query_one("#wiz-notify-topic", Input).value == "xrun-test"
