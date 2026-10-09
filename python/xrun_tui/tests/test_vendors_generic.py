"""Vendors screen with local + ssh as first-class cards, and the SSH hosts
screens (validation, minimal writes, remove with confirmation)."""
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


def _stub_services(monkeypatch):
    from xrun_tui import services

    ops: list[tuple[str, ...]] = []

    async def config_set(key, value, *, secret=False):
        ops.append(("set", key, value, "secret" if secret else "plain"))
        return True, ""

    async def config_unset(key):
        ops.append(("unset", key))
        return True, ""

    async def probe(vendor, *, env=None, extra_args=None, timeout=25):
        return {"vendor": vendor, "ok": True, "detail": "stub"}

    monkeypatch.setattr(services, "config_set", config_set)
    monkeypatch.setattr(services, "config_unset", config_unset)
    monkeypatch.setattr(services, "probe", probe)
    return ops


_NAS = (
    '[ssh.nas]\nhost = "10.0.0.5"\nuser = "me"\nport = 2222\n'
    'key = "~/.ssh/test_key"\ndefault_workdir = "/data"\n'
)


def _inp(screen, field: str):
    from textual.widgets import Input
    return screen.query_one(f"#input-ssh-{field}", Input)


def test_ssh_configured_detection() -> None:
    from xrun_tui.screens.vendors import _vendor_configured

    assert _vendor_configured({}, "local") is True
    assert _vendor_configured({}, "ssh") is False
    assert _vendor_configured({"ssh": {}}, "ssh") is False
    assert _vendor_configured({"ssh": {"nas": {"host": "h", "user": "u"}}}, "ssh")


def test_splash_counts_ssh_hosts() -> None:
    from xrun_tui.screens.splash import _configured_vendors

    assert "ssh" in _configured_vendors({"ssh": {"nas": {"host": "h", "user": "u"}}})
    # A host without a user is not usable, so it does not count.
    assert "ssh" not in _configured_vendors({"ssh": {"nas": {"host": "h"}}})
    assert "ssh" not in _configured_vendors({"ssh": {}})


def test_vendors_screen_has_six_cards_in_order(bare_app) -> None:
    async def scenario() -> None:
        from textual.widgets import Static
        from xrun_tui.screens.vendors import VendorsScreen

        app = bare_app(_NAS)
        async with app.run_test(size=(120, 50)) as pilot:
            screen = VendorsScreen()
            await app.push_screen(screen)
            await pilot.pause()
            rows = [w.id for w in screen.query(".vendor-card")]
            assert rows == [f"vrow-{i}" for i in range(6)]
            info = str(screen.query_one("#vinfo-1", Static).render())
            assert "nas" in info and "1" in info
            # Actions that do not apply to local / ssh only notify.
            for _ in range(2):
                await screen.action_revoke()
                await screen.action_import_native()
                screen.action_open_quota()
                screen.action_next()
            await pilot.pause()
            assert app.screen is screen

    asyncio.run(scenario())


def test_vendors_screen_fits_six_cards_in_30_rows(bare_app) -> None:
    async def scenario() -> None:
        from xrun_tui.screens.vendors import VendorsScreen

        app = bare_app(_NAS)
        async with app.run_test(size=(100, 30)) as pilot:
            screen = VendorsScreen()
            await app.push_screen(screen)
            await pilot.pause()
            box = screen.query_one("#vendor-overview")
            assert box.max_scroll_y == 0
            for i in range(6):
                row = screen.query_one(f"#vrow-{i}").region
                assert box.region.contains_region(row), (i, row, box.region)
            headers = [str(w.render()) for w in screen.query(".vendor-group")]
            assert headers == ["Your hardware", "Cloud"]
            # The count also sees native login files in the real home dir.
            assert "of 6 configured" in str(screen.query_one("#vtitle").render())
            # Two columns: j/k move a row, h/l a card.
            assert screen._cols == 2
            await pilot.press("j")
            assert screen._cursor == 2
            await pilot.press("l")
            assert screen._cursor == 3
            await pilot.press("k")
            assert screen._cursor == 1
            await pilot.press("h")
            assert screen._cursor == 0

    asyncio.run(scenario())


def test_groups_follow_vendor_order() -> None:
    """The cursor walks `_VENDORS`, the grids draw `_GROUPS`: same order, or
    j/k would jump across groups."""
    from xrun_tui.screens.vendors import _GROUPS, _VENDORS

    assert [v for _, vids in _GROUPS for v in vids] == [v for v, _, _ in _VENDORS]


def test_vast_logo_is_white_v_on_black() -> None:
    from xrun_tui.screens.vendors import _BRAND, _ink, _logo

    assert _BRAND["vast"] == "#000000"
    assert _logo("vast") == "[bold #ffffff on #000000] V [/]"
    assert _ink("vast") == "#ffffff"  # black dots would vanish on the card
    assert _ink("kaggle") == _BRAND["kaggle"]


def test_vendors_cursor_scrolls_last_card_into_view(bare_app) -> None:
    async def scenario() -> None:
        from xrun_tui.screens.vendors import VendorsScreen

        app = bare_app(_NAS)
        async with app.run_test(size=(80, 24)) as pilot:
            screen = VendorsScreen()
            await app.push_screen(screen)
            await pilot.pause()
            box = screen.query_one("#vendor-overview")
            assert box.max_scroll_y > 0  # 24 rows cannot hold six cards
            for _ in range(5):
                await pilot.press("j")
            await pilot.pause()
            last = screen.query_one("#vrow-5")
            assert last.has_class("vendor-row-active")
            assert box.region.contains_region(last.region)

    asyncio.run(scenario())


def test_local_card_test_uses_probe_detail(bare_app, monkeypatch) -> None:
    _stub_services(monkeypatch)

    async def scenario() -> None:
        from textual.widgets import Static
        from xrun_tui.screens.vendors import VendorsScreen

        app = bare_app("")
        async with app.run_test(size=(120, 50)) as pilot:
            screen = VendorsScreen()
            await app.push_screen(screen)
            await pilot.pause()
            await screen.action_test()  # cursor 0 = local
            assert "stub" in str(screen.query_one("#vinfo-0", Static).render())

    asyncio.run(scenario())


def test_bad_alias_or_port_rejected_before_any_write(bare_app, monkeypatch) -> None:
    ops = _stub_services(monkeypatch)

    async def scenario() -> None:
        from xrun_tui.screens.ssh_hosts import SshHostEditScreen

        app = bare_app("")
        async with app.run_test(size=(120, 50)) as pilot:
            screen = SshHostEditScreen(None)
            await app.push_screen(screen)
            await pilot.pause()
            _inp(screen, "host").value = "10.0.0.9"
            _inp(screen, "user").value = "root"
            _inp(screen, "alias").value = "bad alias!"
            await screen.action_save()
            _inp(screen, "alias").value = "good_1"
            _inp(screen, "port").value = "70000"
            await screen.action_save()
            _inp(screen, "port").value = "abc"
            await screen.action_save()
            assert ops == []

    asyncio.run(scenario())


def test_edit_writes_only_changed_field(bare_app, monkeypatch) -> None:
    ops = _stub_services(monkeypatch)

    async def scenario() -> None:
        from xrun_tui.screens.ssh_hosts import SshHostEditScreen

        app = bare_app(_NAS)
        async with app.run_test(size=(120, 50)) as pilot:
            screen = SshHostEditScreen("nas")
            await app.push_screen(screen)
            await pilot.pause()
            assert _inp(screen, "alias").disabled
            _inp(screen, "host").value = "10.0.0.6"
            await screen.action_save()
            assert ops == [("set", "ssh.nas.host", "10.0.0.6", "plain")]

    asyncio.run(scenario())


def test_clearing_optional_field_unsets_it(bare_app, monkeypatch) -> None:
    ops = _stub_services(monkeypatch)

    async def scenario() -> None:
        from xrun_tui.screens.ssh_hosts import SshHostEditScreen

        app = bare_app(_NAS)
        async with app.run_test(size=(120, 50)) as pilot:
            screen = SshHostEditScreen("nas")
            await app.push_screen(screen)
            await pilot.pause()
            _inp(screen, "default_workdir").value = ""
            await screen.action_save()
            assert ops == [("unset", "ssh.nas.default_workdir")]

    asyncio.run(scenario())


def test_failed_write_stays_on_form(bare_app, monkeypatch) -> None:
    from xrun_tui import services

    async def config_set(key, value, *, secret=False):
        return False, "boom"

    monkeypatch.setattr(services, "config_set", config_set)

    async def scenario() -> None:
        from textual.widgets import Static
        from xrun_tui.screens.ssh_hosts import SshHostEditScreen

        app = bare_app(_NAS)
        async with app.run_test(size=(120, 50)) as pilot:
            screen = SshHostEditScreen("nas")
            await app.push_screen(screen)
            await pilot.pause()
            _inp(screen, "user").value = "other"
            await screen.action_save()
            await pilot.pause()
            assert app.screen is screen
            assert "user: boom" in str(screen.query_one("#test-result", Static).render())

    asyncio.run(scenario())


def test_new_host_writes_host_and_user_but_not_default_port(bare_app, monkeypatch) -> None:
    ops = _stub_services(monkeypatch)

    async def scenario() -> None:
        from xrun_tui.screens.ssh_hosts import SshHostEditScreen

        app = bare_app("")
        async with app.run_test(size=(120, 50)) as pilot:
            screen = SshHostEditScreen(None)
            await app.push_screen(screen)
            await pilot.pause()
            assert _inp(screen, "port").value == "22"
            _inp(screen, "alias").value = "box"
            _inp(screen, "host").value = "10.0.0.9"
            _inp(screen, "user").value = "root"
            await screen.action_save()
            assert ops == [
                ("set", "ssh.box.host", "10.0.0.9", "plain"),
                ("set", "ssh.box.user", "root", "plain"),
            ]

    asyncio.run(scenario())


def test_new_host_with_existing_alias_is_rejected(bare_app, monkeypatch) -> None:
    """Adding `nas` again used to merge into the stored `nas`: host/user
    overwritten, its key and workdir silently kept from the old host."""
    ops = _stub_services(monkeypatch)

    async def scenario() -> None:
        from textual.widgets import Static
        from xrun_tui.screens.ssh_hosts import SshHostEditScreen

        app = bare_app(_NAS)
        async with app.run_test(size=(120, 50)) as pilot:
            screen = SshHostEditScreen(None)
            await app.push_screen(screen)
            await pilot.pause()
            _inp(screen, "alias").value = "nas"
            _inp(screen, "host").value = "10.9.9.9"
            _inp(screen, "user").value = "other"
            await screen.action_save()
            await pilot.pause()
            assert ops == []
            assert app.screen is screen
            msg = str(screen.query_one("#test-result", Static).render())
            assert "already exists" in msg

    asyncio.run(scenario())


def test_dotted_alias_is_never_addressed(bare_app, monkeypatch) -> None:
    """A hand-written `[ssh."lab.port"]` must not become `unset ssh.lab.port`
    (the port of host `lab`)."""
    ops = _stub_services(monkeypatch)

    async def scenario() -> None:
        from xrun_tui.screens.ssh_hosts import SshHostEditScreen, SshHostsScreen

        app = bare_app(
            '[ssh.lab]\nhost = "h"\nuser = "u"\nport = 2200\n'
            '[ssh."lab.port"]\nhost = "x"\nuser = "y"\n'
        )
        async with app.run_test(size=(120, 50)) as pilot:
            screen = SshHostsScreen()
            await app.push_screen(screen)
            await pilot.pause()
            seen: list[str] = []
            screen.notify = lambda msg, **kw: seen.append(msg)  # type: ignore[method-assign]
            screen.action_next()
            assert screen._selected() == "lab.port"
            await screen.action_remove()
            await screen.action_edit()
            await pilot.pause()
            assert app.screen is screen  # no confirm dialog, no edit form
            assert not isinstance(app.screen, SshHostEditScreen)
            assert ops == []
            assert len(seen) == 2 and "not valid" in seen[0]

    asyncio.run(scenario())


def test_probe_detail_with_brackets_does_not_break_cards(bare_app, monkeypatch) -> None:
    """Probe text is CLI output, not markup: `[/x]` used to raise MarkupError."""
    from xrun_tui import services

    async def probe(vendor, *, env=None, extra_args=None, timeout=25):
        return {"vendor": vendor, "ok": False, "detail": "bad [/section] value"}

    monkeypatch.setattr(services, "probe", probe)

    async def scenario() -> None:
        from textual.widgets import Static
        from xrun_tui.screens.vendors import VendorsScreen, _row_index

        app = bare_app(_NAS)
        async with app.run_test(size=(120, 50)) as pilot:
            screen = VendorsScreen()
            await app.push_screen(screen)
            await pilot.pause()
            await screen.action_test()  # local
            screen.action_next()
            await screen.action_test()  # ssh
            await pilot.pause()
            local = str(screen.query_one(f"#vinfo-{_row_index('local')}", Static).render())
            ssh = str(screen.query_one(f"#vinfo-{_row_index('ssh')}", Static).render())
            assert "bad [/section] value" in local
            assert "nas:" in ssh and "[/section]" in ssh

    asyncio.run(scenario())


def test_vast_and_kaggle_rows_follow_their_ids_not_old_indices(bare_app, monkeypatch) -> None:
    """After the reorder vast is row 2 and kaggle row 3; revoke on the vast
    card must unset vast keys and repaint row 2, not row 0 (local)."""
    ops = _stub_services(monkeypatch)
    from xrun_tui.screens import vendors as vendors_mod

    async def fake_fetch_user(api_key):  # no network
        return {"username": "tester", "credit": 1.0}

    monkeypatch.setattr(vendors_mod, "_fetch_user", fake_fetch_user)

    async def scenario() -> None:
        from textual.widgets import Button, Static
        from xrun_tui.screens.vendors import _VENDORS, VendorsScreen, _row_index

        assert [v for v, _, _ in _VENDORS] == ["local", "ssh", "vast", "kaggle", "lightning", "colab"]
        app = bare_app('[vast]\napi_key = "test-key-abc"\n')
        async with app.run_test(size=(120, 50)) as pilot:
            screen = VendorsScreen()
            await app.push_screen(screen)
            await pilot.pause()
            for _ in range(_row_index("vast")):
                screen.action_next()
            await screen.action_revoke()
            await pilot.pause()
            assert app.screen is not screen  # the confirm dialog
            app.screen.query_one("#btn-yes", Button).press()
            await pilot.pause()
            await pilot.pause()
            assert ops == [("unset", "vast.api_key")]
            # local row still shows its own resting text
            local = str(screen.query_one(f"#vinfo-{_row_index('local')}", Static).render())
            assert "Always available" in local

    asyncio.run(scenario())


def test_remove_asks_confirmation_then_unsets(bare_app, monkeypatch) -> None:
    ops = _stub_services(monkeypatch)

    async def scenario() -> None:
        from xrun_tui.screens.confirm import ConfirmScreen
        from xrun_tui.screens.ssh_hosts import SshHostsScreen

        app = bare_app(_NAS)
        async with app.run_test(size=(120, 50)) as pilot:
            screen = SshHostsScreen()
            await app.push_screen(screen)
            await pilot.pause()
            await screen.action_remove()
            await pilot.pause()
            assert isinstance(app.screen, ConfirmScreen)
            assert ops == []  # nothing before the answer
            app.screen.action_cancel()
            await pilot.pause()
            assert ops == []
            await screen.action_remove()
            await pilot.pause()
            app.screen.action_confirm()
            await pilot.pause()
            assert ops == [("unset", "ssh.nas")]

    asyncio.run(scenario())
