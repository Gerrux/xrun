"""Lightning AI and Google Colab: Vendors cards, the Lightning edit form,
probe wiring, wizard catalog/form tables."""
from __future__ import annotations

import asyncio
from pathlib import Path

import pytest


@pytest.fixture()
def home(tmp_path: Path, monkeypatch) -> Path:
    """Fake home: native login files are looked up here, never on the real one."""
    h = tmp_path / "home"
    h.mkdir()
    monkeypatch.setattr(Path, "home", classmethod(lambda cls: h))
    return h


@pytest.fixture()
def bare_app(tmp_path: Path, monkeypatch, home):
    monkeypatch.setenv("XRUN_CONFIG_DIR", str(tmp_path))
    monkeypatch.setenv("XRUN_DATA_DIR", str(tmp_path / "data"))
    monkeypatch.setenv("XRUN_TUI_NO_RESUME", "1")

    async def _noop(self) -> None:
        return None

    from xrun_tui.app import XrunApp
    monkeypatch.setattr(XrunApp, "on_mount", _noop)
    monkeypatch.setattr(XrunApp, "on_unmount", _noop)

    def make(creds_toml: str = ""):
        (tmp_path / "credentials.toml").write_text(creds_toml, encoding="utf-8")
        return XrunApp()

    return make


def _stub_services(monkeypatch):
    from xrun_tui import services

    ops: list[tuple] = []
    probes: list[tuple] = []

    async def config_set(key, value, *, secret=False):
        ops.append(("set", key, value, "secret" if secret else "plain"))
        return True, ""

    async def config_unset(key):
        ops.append(("unset", key))
        return True, ""

    async def probe(vendor, *, env=None, extra_args=None, timeout=25):
        probes.append((vendor, env))
        return {"vendor": vendor, "ok": True, "detail": "stub ok"}

    monkeypatch.setattr(services, "config_set", config_set)
    monkeypatch.setattr(services, "config_unset", config_unset)
    monkeypatch.setattr(services, "probe", probe)
    return ops, probes


# ── configured / card info ────────────────────────────────────────────────────

def test_lightning_configured_needs_key_and_user_id_or_native_file(home) -> None:
    from xrun_tui.screens.vendors import _vendor_configured

    assert not _vendor_configured({}, "lightning")
    assert not _vendor_configured({"lightning": {"api_key": "k"}}, "lightning")
    assert not _vendor_configured({"lightning": {"user_id": "u"}}, "lightning")
    assert _vendor_configured(
        {"lightning": {"api_key": "k", "user_id": "u"}}, "lightning")
    native = home / ".lightning"
    native.mkdir()
    (native / "credentials.json").write_text("{}", encoding="utf-8")
    assert _vendor_configured({}, "lightning")


def test_colab_configured_is_token_file_existence(home) -> None:
    from xrun_tui.screens.vendors import _card_info, _vendor_configured

    assert not _vendor_configured({}, "colab")
    assert "xrun config login colab" in _card_info({}, "colab")
    tok = home / ".config" / "colab-cli"
    tok.mkdir(parents=True)
    (tok / "token.json").write_text("{}", encoding="utf-8")
    assert _vendor_configured({}, "colab")
    assert "Logged in" in _card_info({}, "colab")


def test_lightning_card_info_shows_user_and_teamspace(home) -> None:
    from xrun_tui.screens.vendors import _card_info

    creds = {"lightning": {"api_key": "k", "user_id": "u-1", "teamspace": "me/ts"}}
    assert "u-1 · me/ts" in _card_info(creds, "lightning")
    creds = {"lightning": {"api_key": "k", "user_id": "u-1"}}
    info = _card_info(creds, "lightning")
    assert "u-1" in info and "·" not in info


def test_vendor_tables_are_complete() -> None:
    from xrun_tui.screens.vendors import _BRAND, _LOGOS, _NOT_SECRET, _VENDORS

    ids = [v for v, _, _ in _VENDORS]
    assert ids[-2:] == ["lightning", "colab"]
    for vid in ("lightning", "colab"):
        assert vid in _LOGOS and vid in _BRAND
    assert _LOGOS["lightning"] == "ϟ" and _BRAND["lightning"] == "#7c3aed"
    assert _LOGOS["colab"] == "◉" and _BRAND["colab"] == "#f9ab00"
    assert {"lightning.user_id", "lightning.teamspace"} <= _NOT_SECRET
    assert "lightning.api_key" not in _NOT_SECRET


def test_probe_env_and_save_ops_helpers() -> None:
    from xrun_tui.screens.vendors import _lightning_probe_env, _lightning_save_ops

    assert _lightning_probe_env({"api_key": "k", "user_id": "u", "teamspace": " "}) == {
        "XRUN_PROBE_LIGHTNING_API_KEY": "k",
        "XRUN_PROBE_LIGHTNING_USER_ID": "u",
    }
    assert _lightning_probe_env({}) == {}

    ops, warn = _lightning_save_ops({}, {"user_id": "u", "api_key": "k", "teamspace": "a/b"})
    assert not warn and ops == [
        ("set", "lightning.user_id", "u"),
        ("set", "lightning.api_key", "k"),
        ("set", "lightning.teamspace", "a/b"),
    ]
    _, warn = _lightning_save_ops({}, {"user_id": "", "api_key": "k", "teamspace": ""})
    assert warn
    # blank teamspace clears a stored one; nothing else changes
    ops, _ = _lightning_save_ops(
        {"user_id": "u", "api_key": "k", "teamspace": "a/b"},
        {"user_id": "u", "api_key": "", "teamspace": ""})
    assert ops == [("unset", "lightning.teamspace")]


def test_splash_counts_lightning_and_colab(home) -> None:
    from xrun_tui.screens.splash import _configured_vendors

    assert _configured_vendors({}) == []
    got = _configured_vendors({"lightning": {"api_key": "k", "user_id": "u"}})
    assert "lightning" in got and "colab" not in got


# ── Screens ───────────────────────────────────────────────────────────────────

def test_lightning_form_saves_through_config_ops(bare_app, monkeypatch) -> None:
    ops, _ = _stub_services(monkeypatch)

    async def scenario() -> None:
        from textual.widgets import Input
        from xrun_tui.screens.vendors import VendorEditScreen

        app = bare_app()
        async with app.run_test(size=(120, 50)) as pilot:
            screen = VendorEditScreen("lightning", "Lightning AI")
            await app.push_screen(screen)
            await pilot.pause()
            key = screen.query_one("#input-lightning-api-key", Input)
            assert key.password
            assert not screen.query_one("#input-lightning-user-id", Input).password
            await screen.action_save()
            assert ops == []  # blank form: nothing to save
            screen.query_one("#input-lightning-user-id", Input).value = "u-1"
            key.value = "test-key-abc"
            screen.query_one("#input-lightning-teamspace", Input).value = "me/ts"
            await screen.action_save()
            assert ops == [
                ("set", "lightning.user_id", "u-1", "plain"),
                ("set", "lightning.api_key", "test-key-abc", "secret"),
                ("set", "lightning.teamspace", "me/ts", "plain"),
            ]

    asyncio.run(scenario())


def test_lightning_form_test_button_passes_probe_env(bare_app, monkeypatch) -> None:
    _, probes = _stub_services(monkeypatch)

    async def scenario() -> None:
        from textual.widgets import Input
        from xrun_tui.screens.vendors import VendorEditScreen

        app = bare_app('[lightning]\napi_key = "test-key-abc"\nuser_id = "u-1"\n')
        async with app.run_test(size=(120, 50)) as pilot:
            screen = VendorEditScreen("lightning", "Lightning AI")
            await app.push_screen(screen)
            await pilot.pause()
            screen.query_one("#input-lightning-teamspace", Input).value = "me/ts"
            await screen.action_test()
            assert probes == [("lightning", {
                "XRUN_PROBE_LIGHTNING_API_KEY": "test-key-abc",
                "XRUN_PROBE_LIGHTNING_USER_ID": "u-1",
                "XRUN_PROBE_LIGHTNING_TEAMSPACE": "me/ts",
            })]

    asyncio.run(scenario())


def test_probe_failure_is_an_error_card_not_a_crash(bare_app, monkeypatch) -> None:
    from xrun_tui import services

    async def boom(vendor, *, env=None, extra_args=None, timeout=25):
        raise RuntimeError("xrun binary too old")

    monkeypatch.setattr(services, "probe", boom)

    async def scenario() -> None:
        from textual.widgets import Static
        from xrun_tui.screens.vendors import VendorsScreen, _row_index

        app = bare_app('[lightning]\napi_key = "test-key-abc"\nuser_id = "u-1"\n')
        async with app.run_test(size=(120, 60)) as pilot:
            screen = VendorsScreen()
            await app.push_screen(screen)
            await pilot.pause()
            for _ in range(_row_index("lightning")):
                screen.action_next()
            await screen.action_test()
            info = str(screen.query_one(f"#vinfo-{_row_index('lightning')}", Static).render())
            assert "too old" in info

    asyncio.run(scenario())


def test_colab_card_test_revoke_and_edit(bare_app, monkeypatch, home) -> None:
    ops, probes = _stub_services(monkeypatch)
    tok = home / ".config" / "colab-cli"
    tok.mkdir(parents=True)
    (tok / "token.json").write_text("{}", encoding="utf-8")

    async def scenario() -> None:
        from xrun_tui.screens.vendors import VendorsScreen, _row_index

        app = bare_app()
        async with app.run_test(size=(120, 60)) as pilot:
            screen = VendorsScreen()
            await app.push_screen(screen)
            await pilot.pause()
            for _ in range(_row_index("colab")):
                screen.action_next()
            await screen.action_edit()  # hint only, no form pushed
            await pilot.pause()
            assert app.screen is screen
            await screen.action_test()
            assert probes == [("colab", None)]
            await screen.action_revoke()  # no-op with a hint, no confirm dialog
            await pilot.pause()
            assert app.screen is screen and ops == []
            assert (tok / "token.json").exists()  # never deleted

    asyncio.run(scenario())


def test_lightning_revoke_unsets_three_keys(bare_app, monkeypatch) -> None:
    ops, _ = _stub_services(monkeypatch)

    async def scenario() -> None:
        from textual.widgets import Button
        from xrun_tui.screens.vendors import VendorsScreen, _row_index

        app = bare_app('[lightning]\napi_key = "test-key-abc"\nuser_id = "u-1"\n')
        async with app.run_test(size=(120, 60)) as pilot:
            screen = VendorsScreen()
            await app.push_screen(screen)
            await pilot.pause()
            for _ in range(_row_index("lightning")):
                screen.action_next()
            await screen.action_revoke()
            await pilot.pause()
            app.screen.query_one("#btn-yes", Button).press()
            await pilot.pause()
            await pilot.pause()
            assert ops == [
                ("unset", "lightning.api_key"),
                ("unset", "lightning.user_id"),
                ("unset", "lightning.teamspace"),
            ]

    asyncio.run(scenario())


# ── Wizard tables ─────────────────────────────────────────────────────────────

def test_wizard_catalog_lightning_and_colab() -> None:
    from xrun_tui.screens.wizard.catalog import (
        LIGHTNING_FIELDS,
        VENDOR_BY_ID,
        VENDOR_CARDS,
        focus_url,
    )

    ids = [c[0] for c in VENDOR_CARDS]
    assert len(ids) == len(set(ids))
    for vid in ("lightning", "colab"):
        _id, _label, _desc, url, available, takes_key = VENDOR_BY_ID[vid]
        assert available is True and takes_key is False and url.startswith("https://")
    assert "xrun config login colab" in VENDOR_BY_ID["colab"][2]
    # runpod / lambda stay as they were
    assert VENDOR_BY_ID["runpod"][4] is False and VENDOR_BY_ID["lambda"][4] is False

    assert [f for f, *_ in LIGHTNING_FIELDS] == ["user_id", "api_key", "teamspace"]
    by_field = {f: (pw, req) for f, _ph, pw, req in LIGHTNING_FIELDS}
    assert by_field["api_key"] == (True, True)
    assert by_field["user_id"] == (False, True)
    assert by_field["teamspace"] == (False, False)
    assert focus_url("wiz-lightning-api_key") == "https://lightning.ai/me/settings"
    assert focus_url("wiz-vendor-cb-colab") == VENDOR_BY_ID["colab"][3]


def test_wizard_probe_targets_and_validation(bare_app, monkeypatch, home) -> None:
    bare_app()  # config dir + empty credentials
    from xrun_tui.screens.wizard.screen import WizardScreen
    from xrun_tui.screens.wizard.steps import _probe_targets

    notes: list[str] = []
    w = WizardScreen()  # not mounted: no live probe, no app needed
    monkeypatch.setattr(w, "notify", lambda msg, **kw: notes.append(msg))
    w._selected_vendors = {"lightning", "colab"}
    w._lightning_fields = {"user_id": "u-1", "api_key": "test-key-abc"}
    targets = {t["vendor"]: t for t in _probe_targets(w)}
    assert targets["lightning"]["env"] == {
        "XRUN_PROBE_LIGHTNING_USER_ID": "u-1",
        "XRUN_PROBE_LIGHTNING_API_KEY": "test-key-abc",
    }
    assert targets["colab"]["env"] is None
    w._lightning_fields = {"user_id": "u-1"}
    assert w._validate_vendors() is False and notes  # key missing
    w._lightning_fields = {}
    assert w._validate_vendors() is True  # blank = keep what is on disk


def test_wizard_rerun_keeps_stored_lightning_key(bare_app, monkeypatch, home) -> None:
    """Re-running the wizard with Lightning already in credentials.toml: the
    user ID is prefilled, the key field is blank (= keep). That must pass
    validation, probe with the stored key, and write only edited fields."""
    ops, _ = _stub_services(monkeypatch)
    bare_app('[lightning]\napi_key = "test-key-abc"\nuser_id = "u-1"\n')
    from xrun_tui.screens.wizard import screen as wiz_mod
    from xrun_tui.screens.wizard.steps import _probe_targets

    notes: list[str] = []
    w = wiz_mod.WizardScreen()
    monkeypatch.setattr(w, "notify", lambda msg, **kw: notes.append(msg))
    assert w._lightning_fields == {"user_id": "u-1"}
    assert "lightning" in w._selected_vendors
    assert w._validate_vendors() is True, notes

    targets = {t["vendor"]: t for t in _probe_targets(w)}
    assert targets["lightning"]["env"] == {
        "XRUN_PROBE_LIGHTNING_USER_ID": "u-1",
        "XRUN_PROBE_LIGHTNING_API_KEY": "test-key-abc",
    }

    async def fake_xrun(*args, **kwargs):
        return 0, "", ""

    async def no_exit(msg: str) -> None:
        return None

    monkeypatch.setattr(wiz_mod, "_xrun", fake_xrun)
    monkeypatch.setattr(w, "_exit_to_dashboard", no_exit)
    w._lightning_fields["teamspace"] = "me/ts"
    asyncio.run(w._finish())
    assert [o for o in ops if o[1].startswith("lightning.")] == [
        ("set", "lightning.teamspace", "me/ts", "plain"),
    ]

    # A typed key without any user ID is still rejected.
    w._lightning_fields = {"api_key": "test-key-abc"}
    notes.clear()
    assert w._validate_vendors() is False and notes


def test_wizard_finish_saves_lightning_fields(bare_app, monkeypatch) -> None:
    ops, _ = _stub_services(monkeypatch)
    bare_app()
    from xrun_tui.screens.wizard import screen as wiz_mod

    async def fake_xrun(*args, **kwargs):
        return 0, "", ""

    monkeypatch.setattr(wiz_mod, "_xrun", fake_xrun)
    w = wiz_mod.WizardScreen()
    w._selected_vendors = {"lightning"}
    w._lightning_fields = {"user_id": "u-1", "api_key": "test-key-abc",
                           "teamspace": "me/ts"}

    async def no_exit(msg: str) -> None:
        return None

    monkeypatch.setattr(w, "_exit_to_dashboard", no_exit)
    asyncio.run(w._finish())
    assert [o for o in ops if o[1].startswith("lightning.")] == [
        ("set", "lightning.user_id", "u-1", "plain"),
        ("set", "lightning.api_key", "test-key-abc", "secret"),
        ("set", "lightning.teamspace", "me/ts", "plain"),
    ]
