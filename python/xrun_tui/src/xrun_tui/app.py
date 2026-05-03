from __future__ import annotations

import time
from collections import deque
from pathlib import Path
from typing import Any

from textual.app import App
from textual.binding import Binding
from textual.notifications import Notification

from xrun_tui import config
from xrun_tui.db import Database, find_db_path
from xrun_tui.themes import write_theme_for_app

XRUN_VERSION = "0.4.0"

# Map: chord-leader → {key → action_name (without the "action_" prefix)}
_CHORDS: dict[str, dict[str, str]] = {
    "g": {
        "d": "goto_dashboard",
        "r": "goto_runs",
        "i": "goto_instances",
        "v": "goto_vendors",
        "h": "goto_doctor",
        "l": "goto_launch",
        "s": "goto_settings",
        "w": "goto_watch",
        "b": "goto_budget",
        "x": "goto_sweep",
    }
}

_CHORD_TIMEOUT_S = 1.5


def _wizard_pending() -> bool:
    try:
        return config.wizard_pending()
    except Exception:
        return False


def _resolved_css_path() -> str:
    """Render the user-selected theme into the config dir and return its path."""
    theme = (config.get_settings() or {}).get("theme") or "tokyo-night"
    target_dir = config.config_dir() / "tui-theme"
    try:
        rendered = write_theme_for_app(theme, target_dir)
        return str(rendered)
    except Exception:
        # Fall back to the bundled Tokyo Night sheet if config dir unavailable
        return str(Path(__file__).parent / "app.tcss")


class XrunApp(App):
    CSS_PATH = _resolved_css_path()
    TITLE = "xrun"
    SUB_TITLE = "GPU job runner"
    ENABLE_COMMAND_PALETTE = False  # We ship our own (Ctrl+P)

    BINDINGS = [
        Binding("question_mark", "open_help",          "Help",     priority=True),
        Binding("ctrl+p",        "open_palette",       "Palette",  priority=True),
        Binding("ctrl+o",        "open_jump",          "Jump",     priority=True),
        Binding("n",             "open_notifications", "Notifs",   priority=True),
        Binding("g",             "chord_g",            "Go…",      priority=True),
    ]

    def __init__(self, start_in_wizard: bool = False) -> None:
        super().__init__()
        self._start_in_wizard = start_in_wizard
        db_path = find_db_path()
        self.db = Database(db_path)
        # Cross-screen state
        self._vast_status_cache: dict[str, Any] = {}
        self._kaggle_status_cache: dict[str, Any] = {}
        # Resolved at splash time from `xrun config show --json` (defaults.exp_dir).
        self._exp_dir: str | None = None
        self._notif_history: deque[dict[str, Any]] = deque(maxlen=200)
        self._chord_leader: str | None = None
        self._chord_expires: float = 0.0
        self._compare_selection: list[str] = []
        self.theme_name: str = (
            (config.get_settings() or {}).get("theme") or "tokyo-night"
        )

    async def on_mount(self) -> None:
        try:
            await self.db.connect()
        except Exception as exc:
            self.notify(
                f"Cannot open database: {exc}\nPath: {self.db.path}",
                severity="error",
                timeout=15,
            )

        from xrun_tui.screens.splash import SplashScreen

        async def _after_splash() -> None:
            if self._start_in_wizard or _wizard_pending():
                from xrun_tui.screens.wizard import WizardScreen
                await self.switch_screen(WizardScreen())
            else:
                from xrun_tui.screens.dashboard import DashboardScreen
                await self.switch_screen(DashboardScreen())

        # Skip splash entirely when launched directly into wizard mode (xrun init).
        if self._start_in_wizard:
            from xrun_tui.screens.wizard import WizardScreen
            await self.push_screen(WizardScreen())
        else:
            await self.push_screen(SplashScreen(_after_splash, version=XRUN_VERSION))

    async def on_unmount(self) -> None:
        await self.db.close()

    # ── Notification history ─────────────────────────────────────────────────

    def notify(self, *args: Any, **kwargs: Any):  # type: ignore[override]
        # Capture before delegating
        message = args[0] if args else kwargs.get("message", "")
        severity = kwargs.get("severity", "information")
        title    = kwargs.get("title", "")
        self._notif_history.append({
            "ts":       time.time(),
            "message":  str(message),
            "severity": str(severity),
            "title":    str(title),
        })
        return super().notify(*args, **kwargs)

    # ── Header icon → open command palette ──────────────────────────────────

    async def action_command_palette(self) -> None:
        await self.action_open_palette()

    # ── Global actions ───────────────────────────────────────────────────────

    async def action_open_help(self) -> None:
        from xrun_tui.screens.help import HelpScreen
        if isinstance(self.screen, HelpScreen):
            return
        await self.push_screen(HelpScreen())

    async def action_open_jump(self) -> None:
        from xrun_tui.widgets.jump_overlay import JumpOverlay
        from xrun_tui.screens.palette import run_target

        if isinstance(self.screen, JumpOverlay):
            return

        async def _on_pick(target: str | None) -> None:
            if target:
                await run_target(self, target)

        await self.push_screen(JumpOverlay(), _on_pick)

    async def action_open_palette(self) -> None:
        from xrun_tui.screens.palette import CommandPalette, run_target

        async def _on_pick(target: str | None) -> None:
            if target:
                await run_target(self, target)

        await self.push_screen(CommandPalette(), _on_pick)

    async def action_open_notifications(self) -> None:
        from xrun_tui.screens.notifications import NotificationsScreen
        if isinstance(self.screen, NotificationsScreen):
            return
        await self.push_screen(NotificationsScreen())

    # ── Chord support: g → d/r/i/v/h/l/s ─────────────────────────────────────

    async def action_chord_g(self) -> None:
        # Only enter chord mode if no input/textbox owns the keyboard
        from textual.widgets import Input
        if isinstance(self.focused, Input):
            return
        self._chord_leader = "g"
        self._chord_expires = time.time() + _CHORD_TIMEOUT_S
        try:
            self.notify("g …", timeout=1, severity="information")
        except Exception:
            pass

    async def on_key(self, event: Any) -> None:
        # Only intercept while a chord leader is active
        if self._chord_leader and time.time() < self._chord_expires:
            mapping = _CHORDS.get(self._chord_leader, {})
            target = mapping.get(event.key)
            self._chord_leader = None
            if target:
                event.prevent_default()
                event.stop()
                await self._dispatch_chord(target)
                return
        elif self._chord_leader:
            self._chord_leader = None  # expired

    async def _dispatch_chord(self, target: str) -> None:
        from xrun_tui.screens.palette import run_target
        mapping = {
            "goto_dashboard":  "go:dashboard",
            "goto_runs":       "go:runs",
            "goto_instances":  "go:instances",
            "goto_vendors":    "go:vendors",
            "goto_doctor":     "go:doctor",
            "goto_launch":     "go:launch",
            "goto_settings":   "go:settings",
            "goto_watch":      "go:watch",
            "goto_budget":     "go:budget",
            "goto_sweep":      "go:sweep",
        }
        slug = mapping.get(target)
        if slug:
            await run_target(self, slug)
