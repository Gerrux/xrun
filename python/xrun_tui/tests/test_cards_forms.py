"""Shared card + form pieces: the one `pill()`, the discard-changes prompt on
Esc, one-save-at-a-time, the shared revoke confirm, and the SSH / MLflow rules
that went in with them."""
from __future__ import annotations

import asyncio
from pathlib import Path

import pytest

from xrun_tui.widgets.cards import pill


# ── pill() ───────────────────────────────────────────────────────────────────

# The exact markup each screen rendered before the three copies were merged.
_VENDOR_PILLS = {
    "empty":    "[#c0caf5 on #414868] EMPTY [/]",
    "checking": "[#1a1b26 on #e0af68] CHECK [/]",
    "ok":       "[#1a1b26 on #9ece6a] READY [/]",
    "error":    "[#c0caf5 on #f7768e] ERROR [/]",
}
_SINK_PILLS = {
    **_VENDOR_PILLS,
    "paused":   "[#1a1b26 on #7aa2f7] PAUSED [/]",
    "disabled": "[#c0caf5 on #414868] v0.8 [/]",
}
# Notifications: `checking` / `ok` meant TEST / ✓ SENT there; they are
# `testing` / `sent` now.
_NOTIFY_PILLS = {
    "on":      "[#1a1b26 on #9ece6a] ON [/]",
    "off":     "[#1a1b26 on #7aa2f7] OFF [/]",
    "empty":   "[#c0caf5 on #414868] EMPTY [/]",
    "testing": "[#1a1b26 on #e0af68] TEST [/]",
    "sent":    "[#1a1b26 on #9ece6a] ✓ SENT [/]",
    "error":   "[#c0caf5 on #f7768e] ERROR [/]",
    "info":    "[#c0caf5 on #414868] · [/]",
}


@pytest.mark.parametrize("table", [_VENDOR_PILLS, _SINK_PILLS, _NOTIFY_PILLS],
                         ids=["vendors", "sinks", "notify"])
def test_pill_matches_what_each_screen_rendered(table) -> None:
    for state, markup in table.items():
        assert pill(state) == markup, state


def test_pill_unknown_state_is_empty() -> None:
    assert pill("nonsense") == _VENDOR_PILLS["empty"]


# ── fixtures ─────────────────────────────────────────────────────────────────

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

    def make(creds: str = "", config: str = ""):
        (tmp_path / "credentials.toml").write_text(creds, encoding="utf-8")
        (tmp_path / "config.toml").write_text(config, encoding="utf-8")
        return XrunApp()

    return make


@pytest.fixture()
def writes(monkeypatch):
    """Stub the config writers; returns (ops, in_flight, delay)."""
    from xrun_tui import services

    ops: list[tuple[str, ...]] = []
    in_flight = {"now": 0, "max": 0}
    delay = {"s": 0.0}

    async def _op(*op: str):
        in_flight["now"] += 1
        in_flight["max"] = max(in_flight["max"], in_flight["now"])
        ops.append(op)
        await asyncio.sleep(delay["s"])
        in_flight["now"] -= 1
        return True, ""

    async def config_set(key, value, *, secret=False):
        return await _op("set", key, value)

    async def config_unset(key):
        return await _op("unset", key)

    async def config_show(secrets: bool = False):
        return True, {}, ""

    async def probe(vendor, *, env=None, extra_args=None, timeout=25):
        return {"vendor": vendor, "ok": True, "detail": "stub"}

    monkeypatch.setattr(services, "config_set", config_set)
    monkeypatch.setattr(services, "config_unset", config_unset)
    monkeypatch.setattr(services, "config_show", config_show)
    monkeypatch.setattr(services, "probe", probe)
    return ops, in_flight, delay


def _inp(screen, sel: str):
    from textual.widgets import Input
    return screen.query_one(sel, Input)


# ── Esc on a form: discard prompt ────────────────────────────────────────────

def _forms():
    """(factory, selector of an Input to edit) for each of the six forms."""
    from xrun_tui.screens.notify_setup import ChannelEditScreen, RulesEditScreen
    from xrun_tui.screens.settings import SettingsScreen
    from xrun_tui.screens.sinks import SinkEditScreen
    from xrun_tui.screens.ssh_hosts import SshHostEditScreen
    from xrun_tui.screens.vendors import VendorEditScreen

    return {
        "vendor":   (lambda: VendorEditScreen("vast", "vast.ai"), "#input-api-key"),
        "sink":     (lambda: SinkEditScreen("wandb", "WandB"), "#input-wandb-key"),
        "channel":  (lambda: ChannelEditScreen("webhook", "Webhook"), "#in-wh-url"),
        "rules":    (lambda: RulesEditScreen(), "#in-pct"),
        "ssh":      (lambda: SshHostEditScreen(None), "#input-ssh-host"),
        "settings": (lambda: SettingsScreen(), "#input-xrun-defaults-exp_dir"),
    }


@pytest.mark.parametrize("name", ["vendor", "sink", "channel", "rules", "ssh", "settings"])
def test_esc_on_untouched_form_leaves_without_prompt(bare_app, writes, monkeypatch, name) -> None:
    _stub_vast(monkeypatch)
    make, _ = _forms()[name]

    async def scenario() -> None:
        app = bare_app()
        async with app.run_test(size=(120, 50)) as pilot:
            base = app.screen
            screen = make()
            await app.push_screen(screen)
            await pilot.pause()
            await pilot.pause()
            await pilot.press("escape")
            await pilot.pause()
            assert app.screen is base

    asyncio.run(scenario())


@pytest.mark.parametrize("name", ["vendor", "sink", "channel", "rules", "ssh", "settings"])
def test_esc_after_typing_asks_enter_stays_y_leaves(bare_app, writes, monkeypatch, name) -> None:
    from xrun_tui.screens.confirm import ConfirmScreen

    _stub_vast(monkeypatch)
    make, sel = _forms()[name]

    async def scenario() -> None:
        app = bare_app()
        async with app.run_test(size=(120, 50)) as pilot:
            base = app.screen
            screen = make()
            await app.push_screen(screen)
            await pilot.pause()
            await pilot.pause()
            _inp(screen, sel).value = "something-new"
            await pilot.press("escape")
            await pilot.pause()
            assert isinstance(app.screen, ConfirmScreen)
            await pilot.press("enter")  # focus is on No
            await pilot.pause()
            assert app.screen is screen
            await pilot.press("escape")
            await pilot.pause()
            assert isinstance(app.screen, ConfirmScreen)
            await pilot.press("y")
            await pilot.pause()
            assert app.screen is base

    asyncio.run(scenario())


def test_esc_after_successful_save_leaves_without_prompt(bare_app, writes) -> None:
    from xrun_tui.screens.sinks import SinkEditScreen

    ops, _, _ = writes

    async def scenario() -> None:
        app = bare_app()
        async with app.run_test(size=(120, 50)) as pilot:
            base = app.screen
            screen = SinkEditScreen("wandb", "WandB")
            await app.push_screen(screen)
            await pilot.pause()
            _inp(screen, "#input-wandb-key").value = "test-key-abc"
            await screen.action_save()
            assert ops == [("set", "wandb.api_key", "test-key-abc")]
            # The typed secret does not linger in the field.
            assert _inp(screen, "#input-wandb-key").value == ""
            await pilot.press("escape")
            await pilot.pause()
            assert app.screen is base

    asyncio.run(scenario())


def test_esc_during_save_is_refused_and_result_reaches_parent(bare_app, writes) -> None:
    """Esc while a save runs used to open the discard prompt; the save then
    finished and its `dismiss` popped the prompt instead of the form, leaving
    the form open with its result already delivered."""
    from xrun_tui.screens.confirm import ConfirmScreen
    from xrun_tui.screens.notify_setup import ChannelEditScreen

    ops, _, delay = writes
    delay["s"] = 0.2
    results: list = []

    async def scenario() -> None:
        app = bare_app()
        async with app.run_test(size=(120, 50)) as pilot:
            base = app.screen
            screen = ChannelEditScreen("webhook", "Webhook")
            await app.push_screen(screen, results.append)
            await pilot.pause()
            await pilot.pause()
            _inp(screen, "#in-wh-url").value = "https://hooks.example/test"
            save = asyncio.ensure_future(screen.action_save())
            await asyncio.sleep(0.05)
            assert screen._saving
            await pilot.press("escape")
            await pilot.pause()
            assert not isinstance(app.screen, ConfirmScreen)
            assert app.screen is screen
            await save
            await pilot.pause()
            await pilot.pause()
            assert results == [{"test_channel": "webhook"}]
            assert app.screen is base
            assert ("set", "webhook.url", "https://hooks.example/test") in ops

    asyncio.run(scenario())


def test_settings_esc_before_prefill_with_nothing_typed_is_clean(
    bare_app, monkeypatch
) -> None:
    from xrun_tui import services
    from xrun_tui.screens.settings import SettingsScreen

    gate = asyncio.Event()

    async def config_show(secrets: bool = False):
        await gate.wait()
        return True, {"defaults": {"exp_dir": "exp/"}}, ""

    monkeypatch.setattr(services, "config_show", config_show)

    async def scenario() -> None:
        app = bare_app()
        async with app.run_test(size=(120, 50)) as pilot:
            base = app.screen
            screen = SettingsScreen()
            await app.push_screen(screen)
            await pilot.pause()
            await pilot.press("escape")
            await pilot.pause()
            assert app.screen is base
            gate.set()

    asyncio.run(scenario())


def test_settings_prefill_is_part_of_the_baseline(bare_app, monkeypatch) -> None:
    """The prefilled value is "unchanged"; editing it afterwards is not."""
    from xrun_tui import services
    from xrun_tui.screens.settings import SettingsScreen

    async def config_show(secrets: bool = False):
        return True, {"defaults": {"exp_dir": "exp/"}}, ""

    monkeypatch.setattr(services, "config_show", config_show)

    async def scenario() -> None:
        app = bare_app()
        async with app.run_test(size=(120, 50)) as pilot:
            screen = SettingsScreen()
            await app.push_screen(screen)
            await pilot.pause()
            await pilot.pause()
            assert _inp(screen, "#input-xrun-defaults-exp_dir").value == "exp/"
            assert not screen.form_dirty()
            _inp(screen, "#input-xrun-defaults-exp_dir").value = "other/"
            assert screen.form_dirty()

    asyncio.run(scenario())


# ── one save at a time ───────────────────────────────────────────────────────

def test_overlapping_sink_saves_do_not_interleave(bare_app, writes) -> None:
    from xrun_tui.screens.sinks import SinkEditScreen

    ops, in_flight, delay = writes
    delay["s"] = 0.05

    async def scenario() -> None:
        app = bare_app()
        async with app.run_test(size=(120, 50)) as pilot:
            screen = SinkEditScreen("mlflow", "MLflow")
            await app.push_screen(screen)
            await pilot.pause()
            _inp(screen, "#input-mlflow-url").value = "https://mlflow.example"
            _inp(screen, "#input-mlflow-token").value = "test-token-abc"
            await asyncio.gather(screen.action_save(), screen.action_save())
            assert in_flight["max"] == 1
            # The second press was refused, not queued: one sequence only.
            assert ops == [
                ("set", "mlflow.url", "https://mlflow.example"),
                ("set", "mlflow.token", "test-token-abc"),
            ]

    asyncio.run(scenario())


def test_cleared_mlflow_url_is_unset(bare_app, writes) -> None:
    from xrun_tui.screens.sinks import SinkEditScreen

    ops, _, _ = writes

    async def scenario() -> None:
        app = bare_app(config='[mlflow]\nurl = "https://mlflow.example"\n')
        async with app.run_test(size=(120, 50)) as pilot:
            screen = SinkEditScreen("mlflow", "MLflow")
            await app.push_screen(screen)
            await pilot.pause()
            _inp(screen, "#input-mlflow-url").value = ""
            await screen.action_save()
            assert ops == [("unset", "mlflow.url")]

    asyncio.run(scenario())


# ── cards ────────────────────────────────────────────────────────────────────

def _stub_vast(monkeypatch) -> None:
    """No network: the vast edit form and the Vendors screen query the API."""
    from xrun_tui.screens import vendors

    async def fetch_user(api_key):
        return {"username": "tester", "credit": 1.0}

    async def list_keys(api_key):
        return []

    monkeypatch.setattr(vendors, "_fetch_user", fetch_user)
    monkeypatch.setattr(vendors, "_list_vast_ssh_keys", list_keys)


def test_vendors_revoke_uses_shared_confirm_default_no(bare_app, writes, monkeypatch) -> None:
    from xrun_tui.screens.confirm import ConfirmScreen
    from xrun_tui.screens.vendors import VendorsScreen, _row_index

    _stub_vast(monkeypatch)
    ops, _, _ = writes

    async def scenario() -> None:
        app = bare_app(creds='[vast]\napi_key = "test-key-abc"\n')
        async with app.run_test(size=(120, 50)) as pilot:
            screen = VendorsScreen()
            await app.push_screen(screen)
            await pilot.pause()
            screen._cursor = _row_index("vast")
            await screen.action_revoke()
            await pilot.pause()
            assert isinstance(app.screen, ConfirmScreen)
            assert app.screen.focused.id == "btn-no"
            await pilot.press("enter")  # No
            await pilot.pause()
            assert ops == []
            await screen.action_revoke()
            await pilot.pause()
            await pilot.press("y")
            await pilot.pause()
            await pilot.pause()
            assert ops == [("unset", "vast.api_key")]

    asyncio.run(scenario())


def test_ssh_host_without_user_is_not_configured() -> None:
    from xrun_tui.screens.splash import _configured_vendors
    from xrun_tui.screens.vendors import _vendor_configured

    half = {"ssh": {"nas": {"host": "10.0.0.5"}}}
    assert _vendor_configured(half, "ssh") is False
    assert "ssh" not in _configured_vendors(half)
    whole = {"ssh": {"nas": {"host": "10.0.0.5", "user": "me"}}}
    assert _vendor_configured(whole, "ssh") is True
    assert "ssh" in _configured_vendors(whole)


def test_splash_ssh_count_skips_incomplete_hosts() -> None:
    from xrun_tui.screens.splash import _ssh_label

    mixed = {"ssh": {"nas": {"host": "10.0.0.5"},
                     "vps": {"host": "h", "user": "u"},
                     "lab": {"host": "h2", "user": "u2"}}}
    assert _ssh_label(mixed) == "ssh×2"  # used to say ssh×3
    assert _ssh_label({"ssh": "garbage"}) == "ssh"


def test_incomplete_ssh_host_is_listed_and_marked(bare_app, writes) -> None:
    from textual.widgets import Static

    from xrun_tui.screens.ssh_hosts import SshHostsScreen

    async def scenario() -> None:
        app = bare_app(creds='[ssh.nas]\nhost = "10.0.0.5"\n'
                             '[ssh.ok]\nhost = "h"\nuser = "u"\n')
        async with app.run_test(size=(120, 50)) as pilot:
            screen = SshHostsScreen()
            await app.push_screen(screen)
            await pilot.pause()
            text = str(screen.query_one("#ssh-hosts-body", Static).render())
            assert "nas" in text and "ok" in text
            assert text.count("INCOMPLETE") == 1

    asyncio.run(scenario())


def test_card_cursor_wraps_and_marks_active_card(bare_app, writes) -> None:
    from xrun_tui.screens.sinks import SinksScreen

    async def scenario() -> None:
        app = bare_app()
        async with app.run_test(size=(120, 50)) as pilot:
            screen = SinksScreen()
            await app.push_screen(screen)
            await pilot.pause()
            active = "vendor-row-active"
            assert screen.query_one("#srow-0").has_class(active)
            await pilot.press("k")
            assert screen._cursor == 2
            assert screen.query_one("#srow-2").has_class(active)
            assert not screen.query_one("#srow-0").has_class(active)
            await pilot.press("j")
            assert screen._cursor == 0

    asyncio.run(scenario())


def test_sinks_cards_reread_when_the_edit_form_closes(bare_app, writes, tmp_path) -> None:
    """The refresh used to run right after `push_screen` (form mounted, not
    closed), so a key saved in the form left the card on EMPTY."""
    from textual.widgets import Static

    from xrun_tui.screens.sinks import SinkEditScreen, SinksScreen

    async def scenario() -> None:
        app = bare_app()
        async with app.run_test(size=(120, 50)) as pilot:
            screen = SinksScreen()
            await app.push_screen(screen)
            await pilot.pause()
            screen._cursor = 1  # wandb
            await screen.action_edit()
            await pilot.pause()
            assert isinstance(app.screen, SinkEditScreen)
            # What a successful save leaves behind (the CLI is stubbed).
            (tmp_path / "credentials.toml").write_text(
                '[wandb]\napi_key = "test-key-abc"\n', encoding="utf-8")
            await pilot.press("escape")
            await pilot.pause()
            assert app.screen is screen
            assert "PAUSED" in str(screen.query_one("#sstatus-1", Static).render())

    asyncio.run(scenario())


@pytest.mark.parametrize("sheet", ["app.tcss", "themes/tokyo_night.tcss"])
def test_card_active_class_is_styled_by_both_stylesheets(sheet) -> None:
    """Every CardCursor screen marks its cursor with a class the stylesheets
    actually select on; an unstyled class leaves the cursor invisible."""
    import xrun_tui
    from xrun_tui.screens.notify_setup import NotifySetupScreen
    from xrun_tui.screens.sinks import SinksScreen
    from xrun_tui.screens.vendors import VendorsScreen

    css = (Path(xrun_tui.__file__).parent / sheet).read_text(encoding="utf-8")
    for cls in (VendorsScreen, SinksScreen, NotifySetupScreen):
        assert f".{cls._ACTIVE_CLASS}" in css, (cls.__name__, cls._ACTIVE_CLASS)


def test_probe_sink_keeps_secrets_out_of_argv(monkeypatch) -> None:
    from xrun_tui import services

    seen: dict = {}

    async def probe(vendor, *, env=None, extra_args=None, timeout=25):
        seen.update(vendor=vendor, env=env, extra=extra_args)
        return {"ok": True, "detail": "fine"}

    monkeypatch.setattr(services, "probe", probe)
    creds = {"mlflow": {"token": "test-token-abc"}}
    ok, detail = asyncio.run(
        services.probe_sink("mlflow", creds, {"mlflow": {"url": "https://m"}}))
    assert (ok, detail) == (True, "fine")
    assert seen["env"] == {"XRUN_PROBE_MLFLOW_TOKEN": "test-token-abc"}
    assert seen["extra"] == ["--mlflow-url", "https://m"]
    assert "test-token-abc" not in " ".join(seen["extra"])
    assert asyncio.run(services.probe_sink("comet", {}, {})) == (False, "unsupported sink")
