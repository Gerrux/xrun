"""Config writes go through `xrun config`, never through the TUI's own TOML
serialiser, and a secret never travels in argv.
"""
from __future__ import annotations

import asyncio
from pathlib import Path

from xrun_tui import config, services

_SRC = Path(services.__file__).parent


def _record_run(monkeypatch) -> list[tuple[tuple[str, ...], str | None]]:
    calls: list[tuple[tuple[str, ...], str | None]] = []

    async def fake_run(*args: str, timeout: int = 30, env=None, stdin=None):
        calls.append((args, stdin))
        return 0, "", ""

    monkeypatch.setattr(services, "_run", fake_run)
    return calls


def test_secret_value_goes_through_stdin_not_argv(monkeypatch) -> None:
    calls = _record_run(monkeypatch)
    ok, _ = asyncio.run(
        services.config_set("vast.api_key", "test-key-abc", secret=True)
    )
    assert ok
    args, stdin = calls[0]
    assert args == ("config", "set", "vast.api_key", "--stdin")
    assert stdin == "test-key-abc"


def test_plain_value_and_unset_use_argv(monkeypatch) -> None:
    calls = _record_run(monkeypatch)
    asyncio.run(services.config_set("defaults.exp_dir", "exp/"))
    asyncio.run(services.config_unset("budget.daily_budget_usd"))
    assert calls == [
        (("config", "set", "defaults.exp_dir", "--", "exp/"), None),
        (("config", "unset", "budget.daily_budget_usd"), None),
    ]


def test_failed_write_reports_the_cli_error(monkeypatch) -> None:
    async def fake_run(*args: str, **kwargs):
        return 1, "", "unknown config key: `nope`\n"

    monkeypatch.setattr(services, "_run", fake_run)
    assert asyncio.run(services.config_set("nope", "1")) == (
        False, "unknown config key: `nope`")


def test_secret_placeholder_shows_only_the_tail() -> None:
    text = services.secret_placeholder("test-key-abcdef123456", "paste key…")
    assert "123456" in text and "test-key" not in text
    assert services.secret_placeholder("abc", "paste key…").startswith("…***")
    assert services.secret_placeholder(None, "paste key…") == "paste key…"


def test_tui_has_no_credentials_writer_of_its_own() -> None:
    assert not hasattr(config, "write_credentials")
    offenders = [
        str(path.relative_to(_SRC))
        for path in _SRC.rglob("*.py")
        if "write_credentials" in path.read_text(encoding="utf-8")
    ]
    assert not offenders, offenders
