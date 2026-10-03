"""Key bindings must reach a handler. Two ways they silently did not:
an action named "action_x" (Textual prepends the prefix itself), and a screen
key shadowed by a priority binding on the app.
"""
from __future__ import annotations

import asyncio
import importlib
import inspect
import pkgutil

from textual.app import App
from textual.dom import DOMNode
from textual.screen import ModalScreen
from textual.widgets import Input

import xrun_tui.screens
import xrun_tui.widgets
from xrun_tui.app import XrunApp
from xrun_tui.screens.confirm import ConfirmScreen


def _classes_with_bindings() -> list[type]:
    found: list[type] = []
    for pkg in (xrun_tui.screens, xrun_tui.widgets):
        for info in pkgutil.walk_packages(pkg.__path__, pkg.__name__ + "."):
            module = importlib.import_module(info.name)
            for _, cls in inspect.getmembers(module, inspect.isclass):
                if (cls.__module__ == module.__name__
                        and issubclass(cls, DOMNode)
                        and "BINDINGS" in cls.__dict__):
                    found.append(cls)
    return found


def test_every_binding_resolves_to_an_action_method() -> None:
    missing: list[str] = []
    for cls in _classes_with_bindings() + [XrunApp]:
        for binding in cls.__dict__["BINDINGS"]:
            action = binding.action if hasattr(binding, "action") else binding[1]
            name = action.split("(")[0]
            if "." in name:  # namespaced (app.x / screen.x): resolved elsewhere
                continue
            if not hasattr(cls, f"action_{name}"):
                missing.append(f"{cls.__name__}: {binding.key} → {action}")
    assert not missing, missing


def test_screen_keys_do_not_collide_with_app_priority_keys() -> None:
    taken = {
        key
        for b in XrunApp.BINDINGS if b.priority
        for key in b.key.split(",")
    }
    shadowed: list[str] = []
    for cls in _classes_with_bindings():
        # Modals answer `n` themselves: XrunApp.check_action steps aside for
        # them (covered by the test below).
        own = {"n"} if issubclass(cls, ModalScreen) else set()
        for binding in cls.__dict__["BINDINGS"]:
            key = binding.key if hasattr(binding, "key") else binding[0]
            for k in key.split(","):
                if k in taken - own:
                    shadowed.append(f"{cls.__name__}: {k}")
    assert not shadowed, shadowed


class _Host(App):
    """XrunApp's bindings and action guard without its splash / DB start-up."""

    BINDINGS = [b for b in XrunApp.BINDINGS if b.key == "n"]
    check_action = XrunApp.check_action

    def __init__(self) -> None:
        super().__init__()
        self.opened_notifications = 0

    def compose(self):
        yield Input()

    async def action_open_notifications(self) -> None:
        self.opened_notifications += 1


def test_enter_does_not_confirm_a_default_no_dialog() -> None:
    async def scenario() -> None:
        app = _Host()
        async with app.run_test() as pilot:
            answers: list[bool | None] = []
            await app.push_screen(ConfirmScreen("launch?", default_no=True),
                                  answers.append)
            await pilot.pause()
            await pilot.press("enter")
            await pilot.pause()
            assert answers == [False]
            await app.push_screen(ConfirmScreen("launch?", default_no=True),
                                  answers.append)
            await pilot.pause()
            await pilot.press("y")
            await pilot.pause()
            assert answers == [False, True]

    asyncio.run(scenario())


class _FakeInput:
    def __init__(self, value: str) -> None:
        self.value = value


class _FakeApp:
    def __init__(self) -> None:
        self.pushed: list[object] = []

    def push_screen(self, screen, callback=None) -> None:
        self.pushed.append(screen)


class _FakeScreen:
    """Just enough of a Screen for the confirm helpers (no app needed)."""

    def __init__(self, value: str) -> None:
        self._input = _FakeInput(value)
        self.app = _FakeApp()

    def query_one(self, *_args):
        return self._input

    def notify(self, *_args, **_kwargs) -> None:
        pass


def test_launch_and_cleanup_confirms_default_to_no() -> None:
    from xrun_tui.screens.launch import LaunchScreen
    from xrun_tui.screens.settings import SettingsScreen

    for helper, value in ((LaunchScreen._confirm_launch, "exp/a.yaml"),
                          (SettingsScreen._confirm_cleanup, "0")):
        fake = _FakeScreen(value)
        helper(fake)
        (dialog,) = fake.app.pushed
        assert isinstance(dialog, ConfirmScreen)
        assert dialog.AUTO_FOCUS == "#btn-no", helper.__qualname__


def test_n_answers_a_confirm_dialog_instead_of_opening_notifications() -> None:
    async def scenario() -> None:
        app = _Host()
        async with app.run_test() as pilot:
            answers: list[bool | None] = []
            await app.push_screen(ConfirmScreen("sure?"), answers.append)
            await pilot.pause()
            await pilot.press("n")
            await pilot.pause()
            assert answers == [False]
            assert app.opened_notifications == 0

    asyncio.run(scenario())
