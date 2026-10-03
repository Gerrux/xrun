"""Wizard: storing one auth mode drops the other, the same rule as the
Vendors edit form. A leftover Kaggle / MLflow token outranks a legacy pair,
so without this the new credentials were silently ignored.
"""
from __future__ import annotations

import asyncio

from xrun_tui import services
from xrun_tui.screens.wizard.screen import WizardScreen


def _record(monkeypatch, fail_key: str | None = None) -> list[tuple[str, ...]]:
    calls: list[tuple[str, ...]] = []

    async def config_set(key: str, value: str, *, secret: bool = False):
        calls.append(("set", key))
        return (key != fail_key), "boom"

    async def config_unset(key: str):
        calls.append(("unset", key))
        return True, ""

    monkeypatch.setattr(services, "config_set", config_set)
    monkeypatch.setattr(services, "config_unset", config_unset)
    return calls


class _Wizard:
    """The persistence helpers only read these fields off the screen."""

    _set = staticmethod(WizardScreen._set)
    _replace_auth = staticmethod(WizardScreen._replace_auth)

    def __init__(self, **mlflow: str) -> None:
        self._mlflow_fields = mlflow


def test_mlflow_basic_auth_drops_a_stored_token(monkeypatch) -> None:
    calls = _record(monkeypatch)
    failed: list[str] = []
    wizard = _Wizard(username="admin", password="test-pass-abc")
    asyncio.run(WizardScreen._persist_mlflow(wizard, failed))
    assert not failed
    assert calls == [
        ("set", "mlflow.username"),
        ("set", "mlflow.password"),
        ("unset", "mlflow.token"),
    ]


def test_mlflow_token_drops_stored_basic_auth(monkeypatch) -> None:
    calls = _record(monkeypatch)
    failed: list[str] = []
    asyncio.run(WizardScreen._persist_mlflow(
        _Wizard(token="test-token-abc"), failed))
    assert calls == [
        ("set", "mlflow.token"),
        ("unset", "mlflow.username"),
        ("unset", "mlflow.password"),
    ]


def test_old_auth_is_kept_when_the_new_one_was_not_stored(monkeypatch) -> None:
    calls = _record(monkeypatch, fail_key="mlflow.password")
    failed: list[str] = []
    wizard = _Wizard(username="admin", password="test-pass-abc")
    asyncio.run(WizardScreen._persist_mlflow(wizard, failed))
    assert failed
    assert ("unset", "mlflow.token") not in calls
