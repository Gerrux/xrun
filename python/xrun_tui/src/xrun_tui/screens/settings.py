from __future__ import annotations

import asyncio
import subprocess
import sys

from textual.app import ComposeResult
from textual.binding import Binding
from textual.containers import Horizontal, Vertical, VerticalScroll
from textual.screen import Screen
from textual.widgets import Button, Footer, Header, Input, Label, Select, Static
from xrun_tui.widgets.status_bar import StatusBar

from xrun_tui import config

_THEMES = [
    ("tokyo-night",     "Tokyo Night (default)"),
    ("catppuccin-mocha","Catppuccin Mocha"),
    ("gruvbox-dark",    "Gruvbox Dark"),
]

_TUI_FIELDS: list[tuple[str, str, str]] = [
    ("runs_refresh_secs",      "Runs auto-refresh (sec)",      "5"),
    ("instances_refresh_secs", "Instances auto-refresh (sec)", "15"),
    ("default_vendor",         "Default vendor",               "vast"),
    ("history_limit",          "Run history limit (count)",    "300"),
]

_XRUN_FIELDS: list[tuple[str, str, str, str]] = [
    ("mlflow.url",                  "MLflow URL",                  "http://…",        "text"),
    ("mlflow.experiment_default",   "MLflow default experiment",   "experiment-name", "text"),
    ("mlflow.token",                "MLflow token",                "***",             "secret"),
    ("poller.interval_active_secs", "Poller interval (active)",    "30",              "int"),
    ("poller.interval_idle_secs",   "Poller interval (idle)",      "120",             "int"),
    ("defaults.vendor",             "Default vendor (xrun core)",  "vast",            "text"),
    ("search.exclude_countries",    "Excluded countries",          "CN, RU, IR",      "list"),
]


class SettingsScreen(Screen):
    TITLE = "xrun — settings"
    BINDINGS = [
        Binding("escape,q", "go_back", "Back"),
        Binding("ctrl+s",   "save",    "Save"),
    ]

    def compose(self) -> ComposeResult:
        settings = config.get_settings()
        current_theme = settings.get("theme", "tokyo-night")
        yield Header(show_clock=True)
        yield Static("Settings", classes="screen-title")
        with VerticalScroll(id="settings-scroll"):
            with Vertical(id="settings-form"):
                yield Static("[bold #bb9af7]TUI[/]", classes="form-section")
                for key, label, default in _TUI_FIELDS:
                    with Horizontal(classes="form-row"):
                        yield Label(f"{label}:", classes="form-label")
                        yield Input(
                            str(settings.get(key, default)),
                            id=f"input-tui-{key}",
                            classes="form-input",
                        )
                with Horizontal(classes="form-row"):
                    yield Label("Theme:", classes="form-label")
                    yield Select(
                        options=[(name, tid) for tid, name in _THEMES],
                        value=current_theme,
                        id="input-tui-theme",
                        classes="form-input",
                    )

                yield Static("[bold #bb9af7]xrun core (shared)[/]",
                             classes="form-section")
                yield Static(
                    "[#565f89]Forwarded to[/] [#7dcfff]xrun config set[/]   "
                    "[#565f89]— blank entries are skipped[/]",
                    classes="form-hint",
                )
                yield Static("", id="prefill-status", classes="form-hint")
                for key, label, placeholder, kind in _XRUN_FIELDS:
                    with Horizontal(classes="form-row"):
                        yield Label(f"{label}:", classes="form-label")
                        yield Input(
                            placeholder=placeholder,
                            password=(kind == "secret"),
                            id=f"input-xrun-{_sanitize(key)}",
                            classes="form-input",
                        )
                        if key == "search.exclude_countries":
                            yield Button(
                                "Pick…",
                                id="btn-pick-countries",
                                classes="form-input",
                            )

                yield Static("[bold #bb9af7]Database[/]", classes="form-section")
                yield Static("", id="db-info", classes="form-hint")
                with Horizontal(classes="form-row"):
                    yield Label("Keep finished runs (days):", classes="form-label")
                    yield Input("0", placeholder="0 = delete all", id="input-cleanup-days", classes="form-input")
                    yield Button("Clean Up", id="btn-cleanup", classes="form-input")

                yield Static("", classes="form-spacer")
                with Horizontal(classes="form-actions"):
                    yield Button("Save  [Ctrl+S]", id="btn-save", variant="primary")
                    yield Button("Cancel  [Esc]",  id="btn-cancel")
                yield Static("", id="settings-result", classes="form-hint")
        yield StatusBar()
        yield Footer()

    def on_mount(self) -> None:
        self.run_worker(self._prefill_xrun_fields(), exclusive=True)
        self.run_worker(self._load_db_info(), exclusive=False)

    async def _load_db_info(self) -> None:
        db = self.app.db  # type: ignore[attr-defined]
        try:
            size = await db.db_size_bytes()
            finished = await db.count_finished_runs()
            size_mb = size / (1024 * 1024)
            self.query_one("#db-info", Static).update(
                f"[#565f89]Path:[/] [#7dcfff]{db.path}[/]   "
                f"[#565f89]Size:[/] [#c0caf5]{size_mb:.1f} MB[/]   "
                f"[#565f89]Finished runs:[/] [#c0caf5]{finished}[/]"
            )
        except Exception as exc:
            self.query_one("#db-info", Static).update(f"[#414868]DB info unavailable: {exc}[/]")

    async def _prefill_xrun_fields(self) -> None:
        from xrun_tui import services
        ps = self.query_one("#prefill-status", Static)
        ps.update("[#565f89]Loading xrun config…[/]")
        ok, data, err = await services.config_show()
        if not ok:
            ps.update(f"[#414868]prefill unavailable: {err[:80]}[/]")
            return

        filled: list[str] = []
        for key, _, _, _ in _XRUN_FIELDS:
            val = _nested_get(data, key)
            if val is None:
                continue
            try:
                inp = self.query_one(f"#input-xrun-{_sanitize(key)}", Input)
                inp.value = ", ".join(str(v) for v in val) if isinstance(val, list) else str(val)
                filled.append(key)
            except Exception:
                pass
        if filled:
            ps.update(f"[#565f89]prefilled:[/] [#7aa2f7]{', '.join(filled)}[/]")
        else:
            ps.update("[#414868]no matching config keys found[/]")

    def on_button_pressed(self, event: Button.Pressed) -> None:
        if event.button.id == "btn-save":
            self.run_worker(self._save(), exclusive=True)
        elif event.button.id == "btn-cancel":
            self.action_go_back()
        elif event.button.id == "btn-pick-countries":
            self._open_country_picker()
        elif event.button.id == "btn-cleanup":
            self.run_worker(self._cleanup_db(), exclusive=True)

    async def _cleanup_db(self) -> None:
        raw = self.query_one("#input-cleanup-days", Input).value.strip()
        try:
            days = int(raw)
            if days < 0:
                raise ValueError
        except ValueError:
            self._set_result("[bold #f7768e]✗[/] 'Keep days' must be 0 or a positive integer (0 = all)")
            return

        btn = self.query_one("#btn-cleanup", Button)
        btn.disabled = True
        try:
            db = self.app.db  # type: ignore[attr-defined]
            deleted = await db.cleanup_runs(keep_days=days)
            if deleted:
                await db.vacuum()
            self._set_result(
                f"[bold #9ece6a]✓[/] Deleted [#c0caf5]{deleted}[/] finished run(s)"
                f" older than [#c0caf5]{days}[/] day(s)"
            )
            self.notify(f"Cleaned up {deleted} run(s)", severity="information")
            # refresh DB info
            await self._load_db_info()
        except Exception as exc:
            self._set_result(f"[bold #f7768e]✗[/] Cleanup failed: {exc}")
        finally:
            btn.disabled = False

    def _open_country_picker(self) -> None:
        from xrun_tui.screens.country_exclude import CountryExcludeScreen
        inp = self.query_one(
            f"#input-xrun-{_sanitize('search.exclude_countries')}", Input
        )
        current = [
            c.strip() for c in inp.value.split(",") if c.strip()
        ]

        def _done(result: list[str] | None) -> None:
            if result is None:
                return
            inp.value = ", ".join(result)

        self.app.push_screen(CountryExcludeScreen(current), _done)

    async def action_save(self) -> None:
        await self._save()

    async def _save(self) -> None:
        # TUI settings → JSON
        tui_settings: dict = {}
        for key, _, _ in _TUI_FIELDS:
            val = self.query_one(f"#input-tui-{key}", Input).value.strip()
            if not val:
                continue
            if key.endswith("_secs") or key == "history_limit":
                try:
                    tui_settings[key] = int(val)
                except ValueError:
                    self._set_result(f"[bold #f7768e]✗[/] '{val}' is not a number for {key}")
                    return
            else:
                tui_settings[key] = val

        # Theme
        try:
            theme_sel = self.query_one("#input-tui-theme", Select)
            if theme_sel.value and theme_sel.value is not Select.BLANK:
                tui_settings["theme"] = str(theme_sel.value)
        except Exception:
            pass

        config.write_tui_settings(tui_settings)

        # Reload theme CSS if changed
        new_theme = tui_settings.get("theme")
        if new_theme and new_theme != getattr(self.app, "theme_name", None):
            try:
                from xrun_tui.themes import write_theme_for_app
                target = config.config_dir() / "tui-theme"
                write_theme_for_app(new_theme, target)
                self.app.theme_name = new_theme  # type: ignore[attr-defined]
                self.notify(
                    f"Theme set to {new_theme} — restart for full effect",
                    severity="information",
                )
            except Exception as exc:
                self.notify(f"Theme apply failed: {exc}", severity="warning")

        # xrun core fields
        applied: list[str] = []
        for key, _, _, _ in _XRUN_FIELDS:
            val = self.query_one(f"#input-xrun-{_sanitize(key)}", Input).value.strip()
            # `search.exclude_countries` is a list field — accept an empty
            # value to let users clear it. Other fields skip on blank.
            if not val and key != "search.exclude_countries":
                continue
            ok, err = await _xrun_config_set(key, val)
            if ok:
                applied.append(key)
            else:
                self._set_result(f"[bold #f7768e]✗[/] {key}: {err[:120]}")
                return

        msg = "[bold #9ece6a]✓ saved[/]  "
        if applied:
            msg += f"[#565f89]xrun keys:[/] [#c0caf5]{', '.join(applied)}[/]"
        else:
            msg += "[#565f89]TUI settings only[/]"
        self._set_result(msg)
        self.notify("Settings saved", severity="information")

    def _set_result(self, text: str) -> None:
        try:
            self.query_one("#settings-result", Static).update(text)
        except Exception:
            pass

    def action_go_back(self) -> None:
        self.app.pop_screen()


def _sanitize(key: str) -> str:
    return key.replace(".", "-")


def _nested_get(data: dict, dotted_key: str):
    """Traverse nested dict with a dot-separated key path."""
    parts = dotted_key.split(".")
    cur = data
    for p in parts:
        if not isinstance(cur, dict):
            return None
        cur = cur.get(p)
    return cur


async def _xrun_config_set(key: str, value: str) -> tuple[bool, str]:
    kwargs: dict = {}
    if sys.platform == "win32":
        kwargs["creationflags"] = subprocess.CREATE_NO_WINDOW
    try:
        proc = await asyncio.create_subprocess_exec(
            "xrun", "config", "set", key, value,
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.PIPE,
            **kwargs,
        )
        out, err = await asyncio.wait_for(proc.communicate(), timeout=15)
    except asyncio.TimeoutError:
        return False, "timeout"
    except FileNotFoundError:
        return False, "xrun not found in PATH"
    if proc.returncode == 0:
        return True, ""
    return False, (err.decode(errors="replace") or out.decode(errors="replace")).strip()
