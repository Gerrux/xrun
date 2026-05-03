from __future__ import annotations

import asyncio
import subprocess
import sys

from textual.app import ComposeResult
from textual.binding import Binding
from textual.containers import Horizontal, Vertical, VerticalScroll
from textual.screen import Screen
from textual.widgets import (
    Button,
    Footer,
    Input,
    Label,
    Select,
    Static,
    TabbedContent,
    TabPane,
)
from xrun_tui.widgets.status_bar import StatusBar
from xrun_tui.widgets.title_bar import TitleBar

from xrun_tui import config

_THEMES = [
    ("tokyo-night",     "Tokyo Night (default)"),
    ("catppuccin-mocha","Catppuccin Mocha"),
    ("gruvbox-dark",    "Gruvbox Dark"),
]

# (key, label, default) — written into TUI JSON
_TUI_FIELDS: list[tuple[str, str, str]] = [
    ("runs_refresh_secs",      "Runs auto-refresh (sec)",      "5"),
    ("instances_refresh_secs", "Instances auto-refresh (sec)", "15"),
    ("default_vendor",         "Default vendor",               "vast"),
    ("history_limit",          "Run history limit (count)",    "300"),
]

# Field kinds:
#   text   — free-form string, prefilled from current value
#   secret — password-masked, blank-on-save means "leave unchanged"; the
#            placeholder is filled with `…tail6` when the credential is set
#   int    — integer, plain text input with numeric validation on save
#   float  — float, plain text input with numeric validation on save
#   bool   — accepts true/false/1/0/yes/no (CLI does the actual coercion)
#   list   — comma-separated values
# (key, label, placeholder, kind)
_MLFLOW_CONFIG_FIELDS: list[tuple[str, str, str, str]] = [
    ("mlflow.url",                "MLflow URL",                "http://…",        "text"),
    ("mlflow.experiment_default", "MLflow default experiment", "experiment-name", "text"),
]
_MLFLOW_CRED_FIELDS: list[tuple[str, str, str, str]] = [
    ("mlflow.username", "MLflow username (Basic)", "admin", "text"),
    ("mlflow.password", "MLflow password (Basic)", "***",   "secret"),
    ("mlflow.token",    "MLflow token (Bearer)",   "***",   "secret"),
]

_POLLER_FIELDS: list[tuple[str, str, str, str]] = [
    ("poller.interval_active_secs", "Poller interval (active)", "30",  "int"),
    ("poller.interval_idle_secs",   "Poller interval (idle)",   "120", "int"),
]

_VENDOR_FIELDS: list[tuple[str, str, str, str]] = [
    ("defaults.vendor",          "Default vendor (xrun core)", "vast",         "text"),
    ("defaults.exp_dir",         "Default exp dir",            "exp/",         "text"),
    ("search.exclude_countries", "Excluded countries",         "CN, RU, IR",   "list"),
    ("metrics.sinks",            "Metrics sinks",              "mlflow",       "list"),
]

_BUDGET_FIELDS: list[tuple[str, str, str, str]] = [
    ("budget.max_lifetime_hours",
        "Max lifetime per instance (hours)",       "8",   "float"),
    ("budget.max_cost_per_instance_usd",
        "Max cost per instance (USD)",             "10",  "float"),
    ("budget.idle_timeout_min",
        "Idle timeout (min, 0 = off)",             "30",  "float"),
    ("budget.daily_budget_usd",
        "Daily budget alert (USD)",                "",    "float"),
    ("budget.daily_budget_hard",
        "Daily budget hard-stop",                  "true / false", "bool"),
    ("budget.monthly_budget_usd",
        "Monthly budget alert (USD)",              "",    "float"),
    ("budget.require_confirm_above_hourly",
        "Confirm prompt above (USD/h)",            "0.5", "float"),
    ("budget.require_typed_confirm_above_hourly",
        "Typed confirm above (USD/h)",             "2.0", "float"),
]

_CREDENTIAL_FIELDS: list[tuple[str, str, str, str]] = [
    ("vast.api_key",     "vast.api_key",     "***", "secret"),
    ("kaggle.token",     "kaggle.token",     "***", "secret"),
    ("kaggle.username",  "kaggle.username",  "your-handle", "text"),
    ("kaggle.key",       "kaggle.key",       "***", "secret"),
]

# Single source of truth for prefill / save iteration over xrun-core fields.
_ALL_XRUN_FIELDS: list[tuple[str, str, str, str]] = (
    _MLFLOW_CONFIG_FIELDS
    + _MLFLOW_CRED_FIELDS
    + _POLLER_FIELDS
    + _VENDOR_FIELDS
    + _BUDGET_FIELDS
    + _CREDENTIAL_FIELDS
)


class SettingsScreen(Screen):
    TITLE = "xrun — settings"
    BINDINGS = [
        Binding("escape,q", "go_back", "Back"),
        Binding("ctrl+s",   "save",    "Save"),
    ]

    def compose(self) -> ComposeResult:
        settings = config.get_settings()
        current_theme = settings.get("theme", "tokyo-night")
        yield TitleBar("settings")
        yield Static("Settings", classes="screen-title")

        with TabbedContent(id="settings-tabs"):
            # ── General (TUI) ────────────────────────────────────────────
            with TabPane("General", id="tab-general"):
                with VerticalScroll():
                    with Vertical(classes="settings-form"):
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

            # ── MLflow (experiment logging) ──────────────────────────────
            with TabPane("MLflow", id="tab-mlflow"):
                with VerticalScroll():
                    with Vertical(classes="settings-form"):
                        yield Static(
                            "[#565f89]Experiment logging — metrics, params, "
                            "artifacts, live training logs via [/]"
                            "[#7dcfff]xrun_hook[/][#565f89].[/]",
                            classes="form-hint",
                        )
                        for row in _MLFLOW_CONFIG_FIELDS:
                            yield _xrun_row(row)
                        yield Static(
                            "[#565f89]Auth — Bearer token takes precedence "
                            "over Basic.[/]",
                            classes="form-hint",
                        )
                        for row in _MLFLOW_CRED_FIELDS:
                            yield _xrun_row(row)

            # ── Poller (events/metrics collection) ───────────────────────
            with TabPane("Poller", id="tab-poller"):
                with VerticalScroll():
                    with Vertical(classes="settings-form"):
                        yield Static(
                            "[#565f89]Background daemon polling vendor APIs "
                            "for events & metrics. Active = run alive, "
                            "idle = run finished but artifacts pending.[/]",
                            classes="form-hint",
                        )
                        for row in _POLLER_FIELDS:
                            yield _xrun_row(row)

            # ── Vendors ──────────────────────────────────────────────────
            with TabPane("Vendors", id="tab-vendors"):
                with VerticalScroll():
                    with Vertical(classes="settings-form"):
                        yield Static(
                            "[#565f89]Defaults applied to every launch when "
                            "the manifest doesn't override them.[/]",
                            classes="form-hint",
                        )
                        for row in _VENDOR_FIELDS:
                            yield _xrun_row(row)

            # ── Budget ───────────────────────────────────────────────────
            with TabPane("Budget", id="tab-budget"):
                with VerticalScroll():
                    with Vertical(classes="settings-form"):
                        yield Static(
                            "[#565f89]Auto-destroy guards. Per-instance caps "
                            "trigger immediately; daily/monthly are alerts "
                            "unless hard-stop is on.[/]",
                            classes="form-hint",
                        )
                        for row in _BUDGET_FIELDS:
                            yield _xrun_row(row)

            # ── Credentials (vendor) ─────────────────────────────────────
            with TabPane("Credentials", id="tab-credentials"):
                with VerticalScroll():
                    with Vertical(classes="settings-form"):
                        yield Static(
                            "[#565f89]Vendor API keys. Blank input on save "
                            "means [/][#7dcfff]leave unchanged[/][#565f89] "
                            "— use the CLI ([/][#7dcfff]xrun config set[/]"
                            "[#565f89] with empty string) to clear. "
                            "Placeholder shows the last 6 chars of the "
                            "stored key when set.[/]",
                            classes="form-hint",
                        )
                        for row in _CREDENTIAL_FIELDS:
                            yield _xrun_row(row)

            # ── Storage (local DB) ───────────────────────────────────────
            with TabPane("Storage", id="tab-storage"):
                with VerticalScroll():
                    with Vertical(classes="settings-form"):
                        yield Static("", id="db-info", classes="form-hint")
                        with Horizontal(classes="form-row"):
                            yield Label("Keep finished runs (days):",
                                        classes="form-label")
                            yield Input(
                                "0",
                                placeholder="0 = delete all",
                                id="input-cleanup-days",
                                classes="form-input",
                            )
                            yield Button("Clean Up", id="btn-cleanup",
                                         classes="form-input")

        # ── Footer: prefill status + actions (shared) ────────────────────
        with Vertical(id="settings-footer"):
            yield Static("", id="prefill-status", classes="form-hint")
            yield Static(
                "[#565f89]Save writes TUI keys to JSON and forwards xrun "
                "keys to[/] [#7dcfff]xrun config set[/][#565f89]. Blank "
                "secrets are kept unchanged; blank non-secret fields are "
                "skipped.[/]",
                classes="form-hint",
            )
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
            self.query_one("#db-info", Static).update(
                f"[#414868]DB info unavailable: {exc}[/]"
            )

    async def _prefill_xrun_fields(self) -> None:
        from xrun_tui import services
        ps = self.query_one("#prefill-status", Static)
        ps.update("[#565f89]Loading xrun config…[/]")
        # Pass secrets=True so the response includes `_credentials_tail` —
        # we use it to render a `…XXXXXX` placeholder for set secrets, never
        # to populate the input value itself.
        ok, data, err = await services.config_show(secrets=True)
        if not ok:
            ps.update(f"[#414868]prefill unavailable: {err[:80]}[/]")
            return

        tail_map: dict = data.get("_credentials_tail") or {}
        set_map: dict = data.get("_credentials_set") or {}

        filled: list[str] = []
        for key, _, _, kind in _ALL_XRUN_FIELDS:
            try:
                inp = self.query_one(f"#input-xrun-{_sanitize(key)}", Input)
            except Exception:
                continue
            if kind == "secret":
                # Never write secret values into the Input — they would be
                # readable to anyone screen-scraping or via .value.
                tail = tail_map.get(key)
                if tail:
                    inp.placeholder = f"…{tail}  (leave blank to keep)"
                    filled.append(key)
                elif set_map.get(key):
                    inp.placeholder = "<set>  (leave blank to keep)"
                    filled.append(key)
                continue
            val = _nested_get(data, key)
            if val is None:
                continue
            inp.value = (
                ", ".join(str(v) for v in val)
                if isinstance(val, list) else str(val)
            )
            filled.append(key)

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
            self._set_result(
                "[bold #f7768e]✗[/] 'Keep days' must be 0 or a positive "
                "integer (0 = all)"
            )
            return

        btn = self.query_one("#btn-cleanup", Button)
        btn.disabled = True
        try:
            db = self.app.db  # type: ignore[attr-defined]
            deleted = await db.cleanup_runs(keep_days=days)
            if deleted:
                await db.vacuum()
            self._set_result(
                f"[bold #9ece6a]✓[/] Deleted [#c0caf5]{deleted}[/] finished "
                f"run(s) older than [#c0caf5]{days}[/] day(s)"
            )
            self.notify(f"Cleaned up {deleted} run(s)", severity="information")
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
        current = [c.strip() for c in inp.value.split(",") if c.strip()]

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
                    self._set_result(
                        f"[bold #f7768e]✗[/] '{val}' is not a number for {key}"
                    )
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

        # xrun core fields (across all tabs)
        applied: list[str] = []
        for key, _, _, kind in _ALL_XRUN_FIELDS:
            val = self.query_one(
                f"#input-xrun-{_sanitize(key)}", Input
            ).value.strip()
            # Skip rules:
            #   secret blank → leave the stored credential alone (do not clear)
            #   list   blank → allowed (means "set to empty list")
            #   other  blank → skip
            if not val:
                if kind == "secret":
                    continue
                if kind != "list":
                    continue

            # Light client-side validation; the CLI does the authoritative
            # coercion via the schema-driven setter.
            if val and kind in ("int", "float"):
                try:
                    (int if kind == "int" else float)(val)
                except ValueError:
                    self._set_result(
                        f"[bold #f7768e]✗[/] {key}: expected {kind}, got '{val}'"
                    )
                    return
            if val and kind == "bool":
                if val.lower() not in ("true", "false", "1", "0",
                                       "yes", "no", "on", "off"):
                    self._set_result(
                        f"[bold #f7768e]✗[/] {key}: expected boolean, "
                        f"got '{val}'"
                    )
                    return

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


def _xrun_row(row: tuple[str, str, str, str]) -> Horizontal:
    """Build a form row widget for an xrun-core config key."""
    key, label, placeholder, kind = row
    children: list = [
        Label(f"{label}:", classes="form-label"),
        Input(
            placeholder=placeholder,
            password=(kind == "secret"),
            id=f"input-xrun-{_sanitize(key)}",
            classes="form-input",
        ),
    ]
    if key == "search.exclude_countries":
        children.append(
            Button("Pick…", id="btn-pick-countries", classes="form-input")
        )
    return Horizontal(*children, classes="form-row")


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
