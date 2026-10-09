from __future__ import annotations

from rich.markup import escape
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
from xrun_tui.widgets.form import FormGuard, single_save
from xrun_tui.widgets.status_bar import StatusBar
from xrun_tui.widgets.title_bar import TitleBar

from xrun_tui import config

_THEMES = [
    ("tokyo-night",      "Tokyo Night (default)"),
    ("catppuccin-mocha", "Catppuccin Mocha"),
    ("gruvbox-dark",     "Gruvbox Dark"),
    ("opencode-dark",    "OpenCode Dark"),
]

# (key, label, default) — written into TUI JSON
_TUI_FIELDS: list[tuple[str, str, str]] = [
    ("history_limit",          "Run history limit (count)",    "300"),
]

# Every setting has one editor. Credentials, sinks and notification channels
# are owned by their own screens (Vendors, Sinks, Notifications), so they are
# deliberately absent here.
#
# Field kinds:
#   text   — free-form string, prefilled from current value
#   int    — integer, plain text input with numeric validation on save
#   float  — float, plain text input with numeric validation on save
#   bool   — accepts true/false/1/0/yes/no (CLI does the actual coercion)
#   choice — Select over `_CHOICES[key]`; blank until the prefill lands, and
#            a blank one is never written
# (key, label, placeholder, kind)
_POLLER_FIELDS: list[tuple[str, str, str, str]] = [
    ("poller.interval_active_secs", "Poller interval (active)", "30",  "int"),
    ("poller.interval_idle_secs",   "Poller interval (idle)",   "120", "int"),
]

_DEFAULTS_FIELDS: list[tuple[str, str, str, str]] = [
    ("defaults.vendor",  "Default vendor",  "local / vast / kaggle / ssh / lightning / colab", "text"),
    ("defaults.exp_dir", "Default exp dir", "exp/",                        "text"),
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

_UPDATE_FIELDS: list[tuple[str, str, str, str]] = [
    ("update.auto", "Background update check", "loading…", "choice"),
]

# (label, value) per `choice` key, in the order shown.
_CHOICES: dict[str, list[tuple[str, str]]] = {
    "update.auto": [
        ("Notify — push once per new release", "notify"),
        ("Off — no check, no network call",    "off"),
    ],
}

# Single source of truth for prefill / save iteration over xrun-core fields.
_ALL_XRUN_FIELDS: list[tuple[str, str, str, str]] = (
    _POLLER_FIELDS
    + _DEFAULTS_FIELDS
    + _BUDGET_FIELDS
    + _UPDATE_FIELDS
)


class SettingsScreen(FormGuard, Screen):
    TITLE = "xrun — settings"
    # The "keep finished runs" box feeds Clean Up, not Save.
    _FORM_IGNORE = frozenset({"input-cleanup-days"})
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

                        yield Static(
                            "[#565f89]Edited elsewhere:[/]  "
                            "[#7dcfff]g v[/] [#565f89]vendor keys, regions[/]  "
                            "[#7dcfff]g m[/] [#565f89]MLflow / WandB sinks[/]  "
                            "[#7dcfff]g n[/] [#565f89]notifications[/]",
                            classes="form-hint",
                        )

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

            # ── Launch defaults ──────────────────────────────────────────
            with TabPane("Defaults", id="tab-defaults"):
                with VerticalScroll():
                    with Vertical(classes="settings-form"):
                        yield Static(
                            "[#565f89]Defaults applied to every launch when "
                            "the manifest doesn't override them.[/]",
                            classes="form-hint",
                        )
                        for row in _DEFAULTS_FIELDS:
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

            # ── Updates ──────────────────────────────────────────────────
            with TabPane("Updates", id="tab-updates"):
                with VerticalScroll():
                    with Vertical(classes="settings-form"):
                        yield Static(
                            "[#565f89]xrun watchdog (scheduler, and this TUI "
                            "every 60 s) looks up the latest release once a "
                            "day and pushes it through the[/] [#7dcfff]g n[/] "
                            "[#565f89]channels. Nothing is installed: run[/] "
                            "[#7dcfff]xrun update[/][#565f89].[/]",
                            classes="form-hint",
                        )
                        for row in _UPDATE_FIELDS:
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
                                "30",
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
                "[#565f89]Save writes only changed fields via[/] "
                "[#7dcfff]xrun config set[/][#565f89]. "
                "A cleared field resets to default.[/]",
                id="settings-note",
                classes="form-hint",
            )
            with Horizontal(classes="form-actions"):
                yield Button("Save  [Ctrl+S]", id="btn-save", variant="primary")
                yield Button("Cancel  \\[Esc]",  id="btn-cancel")
            yield Static("", id="settings-result", classes="form-hint")

        yield StatusBar()
        yield Footer()

    def on_mount(self) -> None:
        self._loaded: dict[str, str] = {}
        # Baseline for the discard prompt; the prefill refreshes its part.
        # Backing out before the prefill lands, with nothing typed, is clean:
        # the not-yet-filled Inputs are blank in the baseline too.
        self.call_after_refresh(self.snapshot_form)
        # Own groups: `exclusive` cancels workers of the same group, and in
        # the shared default group a Save cancelled the prefill (fields stay
        # blank) and a Cleanup cancelled a Save halfway through its writes.
        self.run_worker(self._prefill_xrun_fields(), exclusive=True,
                        group="prefill")
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
                f"[#565f89]DB info unavailable: {escape(str(exc))}[/]"
            )

    async def _prefill_xrun_fields(self) -> None:
        from xrun_tui import services
        ps = self.query_one("#prefill-status", Static)
        ps.update("[#565f89]Loading xrun config…[/]")
        ok, data, err = await services.config_show()
        if not self.is_attached:
            return
        if not ok:
            ps.update(f"[#565f89]prefill unavailable: {escape(err[:80])}[/]")
            return

        filled: list[str] = []
        for key, _, _, kind in _ALL_XRUN_FIELDS:
            try:
                widget = self.query_one(f"#input-xrun-{_sanitize(key)}")
            except Exception:
                continue
            val = _nested_get(data, key)
            if val is None:
                continue
            # A value outside a choice list stays blank, and a blank choice
            # is never written.
            if kind == "choice" and str(val) not in {v for _, v in _CHOICES[key]}:
                continue
            widget.value = str(val)  # type: ignore[attr-defined]  # Input | Select
            # Remembered so Save can tell a changed field from an untouched
            # one, and a cleared field from one that was never set.
            self._loaded[key] = str(val)
            filled.append(key)

        self.snapshot_form(only={f"input-xrun-{_sanitize(k)}" for k in filled})
        if filled:
            ps.update("[#565f89]Loaded current values from xrun config[/]")
        else:
            ps.update("[#565f89]no matching config keys found[/]")

    def on_button_pressed(self, event: Button.Pressed) -> None:
        if event.button.id == "btn-save":
            # Not exclusive: a second click must not cancel a save that is
            # halfway through its `xrun config` writes (`_save` ignores it).
            self.run_worker(self._save(), group="save")
        elif event.button.id == "btn-cancel":
            self.action_go_back()
        elif event.button.id == "btn-cleanup":
            self._confirm_cleanup()

    def _confirm_cleanup(self) -> None:
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
        from xrun_tui.screens.confirm import ConfirmScreen

        def _do(confirmed: bool | None) -> None:
            if confirmed:
                self.run_worker(self._cleanup_db(days), exclusive=True,
                                group="cleanup")

        what = (
            "ALL finished runs" if days == 0
            else f"finished runs older than {days} day(s)"
        )
        self.app.push_screen(
            ConfirmScreen(f"Delete {what} from the database?\n"
                          "This cannot be undone.", default_no=True),
            _do,
        )

    async def _cleanup_db(self, days: int) -> None:
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

    async def action_save(self) -> None:
        await self._save()

    @single_save
    async def _save(self) -> None:
        # TUI settings → JSON
        tui_settings: dict = {}
        for key, _, _ in _TUI_FIELDS:
            val = self.query_one(f"#input-tui-{key}", Input).value.strip()
            if not val:
                continue
            if not val.isdigit() or int(val) < 1:
                self._set_result(
                    f"[bold #f7768e]✗[/] {key}: expected a whole number ≥ 1, "
                    f"got '{val}'  [#565f89]nothing saved[/]"
                )
                return
            tui_settings[key] = int(val)

        # Theme
        try:
            theme_sel = self.query_one("#input-tui-theme", Select)
            theme = _field_value(theme_sel)
            if theme:
                tui_settings["theme"] = theme
        except Exception:
            pass

        # xrun core fields (across all tabs): validate everything before the
        # first write, so a typo in one field never leaves a half-saved form.
        # Only changed fields are written; a field the user emptied goes back
        # to its default (`None` in `pending`).
        pending: list[tuple[str, str | None]] = []
        for key, _, _, kind in _ALL_XRUN_FIELDS:
            val = _field_value(
                self.query_one(f"#input-xrun-{_sanitize(key)}")
            )
            if val == self._loaded.get(key, ""):
                continue
            if kind == "choice" and not val:
                continue  # nothing picked yet: a Select cannot be "cleared"
            if not val:
                pending.append((key, None))
                continue

            # Light client-side validation; the CLI does the authoritative
            # coercion via the schema-driven setter.
            if val and kind in ("int", "float"):
                try:
                    (int if kind == "int" else float)(val)
                except ValueError:
                    self._set_result(
                        f"[bold #f7768e]✗[/] {key}: expected {kind}, got '{val}'"
                        "  [#565f89]nothing saved[/]"
                    )
                    return
            if val and kind == "bool":
                if val.lower() not in ("true", "false", "1", "0",
                                       "yes", "no", "on", "off"):
                    self._set_result(
                        f"[bold #f7768e]✗[/] {key}: expected boolean, "
                        f"got '{val}'  [#565f89]nothing saved[/]"
                    )
                    return
            pending.append((key, val))

        config.write_tui_settings(tui_settings)

        new_theme = tui_settings.get("theme")
        if new_theme and new_theme != getattr(self.app, "theme_name", None):
            try:
                target = config.config_dir() / "tui-theme"
                _apply_theme_to_app(self.app, new_theme, target)
                self.notify(
                    f"Theme set to {new_theme}",
                    severity="information",
                )
            except Exception as exc:
                self.notify(f"Theme apply failed: {exc}", severity="warning")

        # The CLI can still reject a key; keep going and report both lists so
        # the user knows exactly what was stored.
        applied: list[str] = []
        failed: list[str] = []
        from xrun_tui import services
        for key, val in pending:
            if val is None:
                ok, err = await services.config_unset(key)
            else:
                ok, err = await services.config_set(key, val)
            if ok:
                self._loaded[key] = val or ""
                applied.append(key)
            else:
                failed.append(f"{key}: {err[:80]}")

        if not self.is_attached:
            return  # the user left while the CLI writes were running
        if failed:
            msg = f"[bold #f7768e]✗ not saved:[/] {'; '.join(failed)}"
            if applied:
                msg += (f"  [#565f89]saved:[/] "
                        f"[#c0caf5]{', '.join(applied)}[/]")
            self._set_result(msg)
            self.notify(f"{len(failed)} setting(s) not saved",
                        severity="error", timeout=8)
            return

        msg = "[bold #9ece6a]✓ saved[/]  "
        if applied:
            msg += f"[#565f89]xrun keys:[/] [#c0caf5]{', '.join(applied)}[/]"
        else:
            msg += "[#565f89]TUI settings only[/]"
        self._set_result(msg)
        self.snapshot_form()
        self.notify("Settings saved", severity="information")

    def _set_result(self, text: str) -> None:
        try:
            self.query_one("#settings-result", Static).update(text)
        except Exception:
            pass


def _xrun_row(row: tuple[str, str, str, str]) -> Horizontal:
    """Build a form row widget for an xrun-core config key."""
    key, label, placeholder, kind = row
    field: Input | Select
    if kind == "choice":
        field = Select(
            options=_CHOICES[key],
            prompt=placeholder,
            allow_blank=True,
            id=f"input-xrun-{_sanitize(key)}",
            classes="form-input",
        )
    else:
        field = Input(
            placeholder=placeholder,
            id=f"input-xrun-{_sanitize(key)}",
            classes="form-input",
        )
    return Horizontal(
        Label(f"{label}:", classes="form-label"),
        field,
        classes="form-row",
    )


def _field_value(widget) -> str:
    """Current text of an xrun field: stripped Input text, or the picked
    Select value ("" while nothing is picked)."""
    if isinstance(widget, Select):
        # Every option value here is a string; anything else is the blank
        # sentinel, which is Select.BLANK in older Textual and Select.NULL
        # (truthy) in newer — so no comparison against either.
        value = widget.value
        return value if isinstance(value, str) else ""
    return widget.value.strip()


def _sanitize(key: str) -> str:
    return key.replace(".", "-")


def _apply_theme_to_app(app, theme: str, target_dir) -> None:
    """Render and apply a theme immediately, keeping the selection persistent."""
    from xrun_tui.themes import write_theme_for_app

    rendered = write_theme_for_app(theme, target_dir)
    app.CSS_PATH = str(rendered)
    app.theme_name = theme
    app.refresh_css(animate=False)

def _nested_get(data: dict, dotted_key: str):
    """Traverse nested dict with a dot-separated key path."""
    parts = dotted_key.split(".")
    cur = data
    for p in parts:
        if not isinstance(cur, dict):
            return None
        cur = cur.get(p)
    return cur
