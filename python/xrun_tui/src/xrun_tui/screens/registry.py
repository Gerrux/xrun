"""The one list of navigable screens.

The `g` chords, the command palette's "Go:" entries, `run_target` and the help
screen's navigation section are all derived from SCREENS, so adding a screen
is one entry here. Classes are imported lazily inside the loaders: importing
this module stays cheap and cannot form an import cycle with the screens
(many of which import `palette.run_target`).
"""
from __future__ import annotations

from dataclasses import dataclass
from typing import Callable, Iterator

from textual.screen import Screen


@dataclass(frozen=True)
class ScreenEntry:
    slug: str                      # target is "go:<slug>"
    label: str                     # human name, used by help
    chord: str | None              # key after `g`, None = no chord
    description: str               # palette line
    load: Callable[[], type[Screen]]

    def factory(self) -> Screen:
        """A new instance of the screen."""
        return self.load()()

    @property
    def target(self) -> str:
        return f"go:{self.slug}"


def _dashboard() -> type[Screen]:
    from xrun_tui.screens.dashboard import DashboardScreen
    return DashboardScreen


def _runs() -> type[Screen]:
    from xrun_tui.screens.runs import RunsScreen
    return RunsScreen


def _watch() -> type[Screen]:
    from xrun_tui.screens.watch import WatchScreen
    return WatchScreen


def _budget() -> type[Screen]:
    from xrun_tui.screens.budget import BudgetScreen
    return BudgetScreen


def _sweep() -> type[Screen]:
    from xrun_tui.screens.sweep import SweepScreen
    return SweepScreen


def _instances() -> type[Screen]:
    from xrun_tui.screens.instances import InstancesScreen
    return InstancesScreen


def _vendors() -> type[Screen]:
    from xrun_tui.screens.vendors import VendorsScreen
    return VendorsScreen


def _sinks() -> type[Screen]:
    from xrun_tui.screens.sinks import SinksScreen
    return SinksScreen


def _doctor() -> type[Screen]:
    from xrun_tui.screens.doctor import DoctorScreen
    return DoctorScreen


def _launch() -> type[Screen]:
    from xrun_tui.screens.launch import LaunchScreen
    return LaunchScreen


def _settings() -> type[Screen]:
    from xrun_tui.screens.settings import SettingsScreen
    return SettingsScreen


def _notifications() -> type[Screen]:
    from xrun_tui.screens.notifications import NotificationsScreen
    return NotificationsScreen


def _notify() -> type[Screen]:
    from xrun_tui.screens.notify_setup import NotifySetupScreen
    return NotifySetupScreen


def _help() -> type[Screen]:
    from xrun_tui.screens.help import HelpScreen
    return HelpScreen


# Display order = palette order = help order.
SCREENS: tuple[ScreenEntry, ...] = (
    ScreenEntry("dashboard",     "Dashboard",           "d", "Dashboard", _dashboard),
    ScreenEntry("runs",          "Runs",                "r", "Runs", _runs),
    ScreenEntry("watch",         "Watch",               "w", "Watch  (live active runs)", _watch),
    ScreenEntry("budget",        "Budget",              "b", "Budget & Spend", _budget),
    ScreenEntry("sweep",         "Sweep",               "x", "Sweep results", _sweep),
    ScreenEntry("instances",     "Instances",           "i", "Instances", _instances),
    ScreenEntry("vendors",       "Vendors",             "v", "Vendors", _vendors),
    ScreenEntry("sinks",         "Sinks",               "m", "Sinks  (metrics & logs)", _sinks),
    ScreenEntry("doctor",        "Doctor",              "h", "Doctor (system health)", _doctor),
    ScreenEntry("launch",        "Launch",              "l", "Launch manifest", _launch),
    ScreenEntry("settings",      "Settings",            "s", "Settings", _settings),
    # Bare `n` and `?` open these; they have no `g` chord.
    ScreenEntry("notifications", "Notifications",       None, "Notifications history", _notifications),
    ScreenEntry("notify",        "Notifications setup", "n", "Notifications setup (push)", _notify),
    ScreenEntry("help",          "Help",                None, "Keyboard help", _help),
)

_BY_SLUG = {e.slug: e for e in SCREENS}
_BY_CHORD = {e.chord: e for e in SCREENS if e.chord}


def iter_screens() -> Iterator[ScreenEntry]:
    return iter(SCREENS)


def by_slug(slug: str) -> ScreenEntry | None:
    return _BY_SLUG.get(slug)


def by_chord(key: str) -> ScreenEntry | None:
    return _BY_CHORD.get(key)
