"""Sinks screen — parallel to Vendors, but for metric/log mirrors.

Each card represents one tracking-server sink: MLflow, WandB, Comet. A sink
contributes to a run when both (a) it's listed in `[metrics] sinks = […]`
in `~/.config/xrun/config.toml`, and (b) its credentials are set in
`credentials.toml`. The screen shows both signals as separate state on the
card so the gap is visible — a "key set but not in sinks list" sink is the
common configuration mistake we're trying to make obvious.

Comet is rendered as a disabled `[v0.8]` placeholder until the sink crate
ships. We keep it visible so users see the roadmap without us having to
write a docs page.
"""
from __future__ import annotations

from textual.app import ComposeResult
from textual.binding import Binding
from textual.containers import Horizontal, Vertical
from textual.screen import Screen
from textual.widgets import Button, Footer, Input, Label, Rule, Static

from xrun_tui import config, services
from xrun_tui.screens.confirm import ConfirmScreen
from xrun_tui.widgets.cards import CardCursor, pill
from xrun_tui.widgets.form import FormGuard, single_save
from xrun_tui.widgets.status_bar import StatusBar
from xrun_tui.widgets.title_bar import TitleBar


# (sink_id, display_name, description, enabled_in_v07)
_SINKS: list[tuple[str, str, str, bool]] = [
    ("mlflow", "MLflow", "Self-hosted tracking server",  True),
    ("wandb",  "WandB",  "Weights & Biases dashboard",   True),
    ("comet",  "Comet ML", "comet.com (arrives in v0.8)", False),
]
_LOGOS = {"mlflow": "✦", "wandb": "▲", "comet": "◆"}
# Brand accents — kept here rather than in CSS so adding comet later is
# one line in this file rather than a CSS edit.
_BRAND = {
    "mlflow": "#0174c4",
    "wandb":  "#ffbe0b",
    "comet":  "#2bd4f6",
}


def _read_state() -> tuple[dict, list[str]]:
    """Return (credentials, metrics.sinks list).

    `metrics.sinks` is the ordered list of sink names that should be
    activated on the next launch. A sink is "default" iff it's in this
    list AND its credentials are set.
    """
    creds = config.read_credentials()
    glob  = config.read_global_config()
    sinks = glob.get("metrics", {}).get("sinks", [])
    if not isinstance(sinks, list):
        sinks = []
    return creds, list(sinks)


def _sink_configured(creds: dict, sid: str) -> bool:
    """True when this sink has the *minimum* creds to authenticate."""
    if sid == "mlflow":
        # MLflow needs either token *or* user+password to be auth-ready,
        # plus the URL — which lives in global config, not creds.
        m = creds.get("mlflow", {})
        if not (m.get("token") or (m.get("username") and m.get("password"))):
            return False
        glob = config.read_global_config()
        return bool(glob.get("mlflow", {}).get("url"))
    if sid == "wandb":
        return bool(creds.get("wandb", {}).get("api_key"))
    return False


# Credential keys each sink owns — what Revoke clears.
_REVOKE_KEYS = {
    "mlflow": ("mlflow.token", "mlflow.username", "mlflow.password"),
    "wandb":  ("wandb.api_key",),
}


async def _set_metrics_sinks(sinks: list[str]) -> tuple[bool, str]:
    """Persist the `metrics.sinks` list via `xrun config set metrics.sinks`.

    The CLI is the only config writer; it also coerces the comma-separated
    input into the array shape Rust expects. Returns (ok, error text).
    """
    return await services.config_set("metrics.sinks", ",".join(sinks))


# ════════════════════════════════════════════════════════════════════════════
#  Overview screen
# ════════════════════════════════════════════════════════════════════════════

class SinksScreen(CardCursor, Screen):
    """List of metric/log sinks. Mirrors Vendors but for tracking servers."""

    TITLE = "xrun — sinks"
    _CARD_PREFIX = "srow"
    _CARD_COUNT = len(_SINKS)
    # Same class as Vendors (CardCursor's default): it is the one both
    # stylesheets style; the old "selected" had no rule, so no cursor showed.
    BINDINGS = [
        Binding("escape,q",   "go_back",  "Back"),
        Binding("enter,e",    "edit",     "Edit"),
        Binding("t",          "test",     "Test"),
        Binding("space,d",    "toggle_default", "Toggle default"),
        Binding("r",          "revoke",   "Revoke"),
        Binding("j,down",     "next",     "Down", show=False),
        Binding("k,up",       "prev",     "Up",   show=False),
    ]

    def __init__(self) -> None:
        super().__init__()
        self._cursor = 0
        self._creds, self._sinks_list = _read_state()

    # ── compose ──────────────────────────────────────────────────────────
    def compose(self) -> ComposeResult:
        yield TitleBar("sinks")
        yield Static("Metric & Log Sinks", classes="screen-title")
        with Vertical(id="vendor-overview"):
            for i, (sid, name, desc, enabled) in enumerate(_SINKS):
                state = self._compute_state(sid, enabled)
                brand = _BRAND[sid]
                with Vertical(
                    classes=f"vendor-card vendor-card-{sid}",
                    id=f"srow-{i}",
                ):
                    with Horizontal(classes="vendor-card-head"):
                        yield Static(f"[{brand}]{_LOGOS[sid]}[/]",
                                     classes="vendor-logo", id=f"slogo-{i}")
                        yield Static(
                            f"[bold #c0caf5]{name}[/]  [#565f89]{desc}[/]",
                            classes="vendor-card-title",
                        )
                        yield Static(pill(state),
                                     id=f"sstatus-{i}", classes="vendor-card-pill")
                    with Horizontal(classes="vendor-card-foot"):
                        yield Static(
                            self._foot_text(sid, state, enabled),
                            id=f"sinfo-{i}", classes="vendor-card-info",
                        )
            yield Rule()
            yield Static(
                "[#565f89]Enter/e[/] [#c0caf5]Edit[/]   "
                "[#565f89]t[/] [#c0caf5]Test[/]   "
                "[#565f89]Space/d[/] [#c0caf5]Toggle default[/]   "
                "[#565f89]r[/] [#c0caf5]Revoke[/]   "
                "[#565f89]j/k[/] [#c0caf5]Navigate[/]",
                classes="vendor-hint",
            )
        yield StatusBar()
        yield Footer()

    # ── helpers ──────────────────────────────────────────────────────────
    def _compute_state(self, sid: str, enabled: bool) -> str:
        if not enabled:
            return "disabled"
        if not _sink_configured(self._creds, sid):
            return "empty"
        if sid not in self._sinks_list:
            return "paused"
        return "ok"

    def _foot_text(self, sid: str, state: str, enabled: bool) -> str:
        if not enabled:
            return "[#565f89]Coming in v0.8[/]"
        if state == "empty":
            return ("[#565f89]Press[/] [#c0caf5]Enter[/] "
                    "[#565f89]to add credentials[/]")
        if state == "paused":
            return ("[#e0af68]configured but inactive — "
                    "[/][#c0caf5]Space[/][#e0af68] to add to default[/]")
        # ok
        if sid == "mlflow":
            url = config.read_global_config().get("mlflow", {}).get("url", "")
            return f"[#9ece6a]✓ active[/]  [#565f89]url:[/] [#7aa2f7]{url}[/]"
        if sid == "wandb":
            return "[#9ece6a]✓ active[/]  [#565f89]entity probed on first launch[/]"
        return ""

    def on_mount(self) -> None:
        self._highlight(self._cursor)

    # ── navigation (j/k: CardCursor) ─────────────────────────────────────
    def action_go_back(self) -> None:
        self.app.pop_screen()

    # ── edit / test / revoke ─────────────────────────────────────────────
    async def action_edit(self) -> None:
        sid, name, _, enabled = _SINKS[self._cursor]
        if not enabled:
            self.notify(f"{name} arrives in v0.8 — not editable yet",
                        severity="warning")
            return
        # `push_screen` returns once the form is mounted, not when it closes:
        # the cards are re-read in `on_screen_resume`.
        await self.app.push_screen(SinkEditScreen(sid, name))

    def on_screen_resume(self) -> None:
        self._refresh_cards()

    async def action_test(self) -> None:
        sid, name, _, enabled = _SINKS[self._cursor]
        if not enabled:
            return
        if not _sink_configured(self._creds, sid):
            self.notify(f"{name}: configure credentials first",
                        severity="warning")
            return
        idx = self._cursor
        status_w = self.query_one(f"#sstatus-{idx}", Static)
        info_w   = self.query_one(f"#sinfo-{idx}",   Static)
        status_w.update(pill("checking"))
        info_w.update("[#e0af68]probing…[/]")
        ok, detail = await services.probe_sink(
            sid, self._creds, config.read_global_config())
        if not self.is_attached:
            return
        status_w.update(pill("ok" if ok else "error"))
        info_w.update(
            f"[#9ece6a]✓ {detail}[/]" if ok
            else f"[#f7768e]✗ {detail}[/]"
        )

    async def action_toggle_default(self) -> None:
        sid, name, _, enabled = _SINKS[self._cursor]
        if not enabled:
            return
        if sid in self._sinks_list:
            new = [s for s in self._sinks_list if s != sid]
            msg = f"{name} removed from default sinks"
        else:
            new = [*self._sinks_list, sid]
            msg = f"{name} added to default sinks"
        ok, err = await _set_metrics_sinks(new)
        # On failure _refresh_cards re-reads the real state, so the card
        # does not show a change that never reached config.toml.
        self._refresh_cards()
        if ok:
            self.notify(msg, severity="information")
        else:
            self.notify(f"metrics.sinks: {err or 'failed'}", severity="error")

    async def action_revoke(self) -> None:
        sid, name, _, enabled = _SINKS[self._cursor]
        if not enabled:
            return
        if not _sink_configured(self._creds, sid):
            return

        async def _after(yes: bool | None) -> None:
            if not yes:
                return
            failed = []
            for k in _REVOKE_KEYS[sid]:
                ok, err = await services.config_unset(k)
                if not ok:
                    failed.append(f"{k}: {err or 'failed'}")
            self._refresh_cards()
            if failed:
                self.notify("; ".join(failed), severity="error")
            else:
                self.notify(f"{name} credentials revoked", severity="warning")

        self.app.push_screen(
            ConfirmScreen(f"Revoke {name} credentials?", default_no=True), _after)

    # ── refresh after a state change ─────────────────────────────────────
    def _refresh_cards(self) -> None:
        if not self.is_attached:
            return  # the user left while the CLI write was still running
        self._creds, self._sinks_list = _read_state()
        for i, (sid, _, _, enabled) in enumerate(_SINKS):
            state = self._compute_state(sid, enabled)
            self.query_one(f"#sstatus-{i}", Static).update(pill(state))
            self.query_one(f"#sinfo-{i}",   Static).update(
                self._foot_text(sid, state, enabled)
            )


# ════════════════════════════════════════════════════════════════════════════
#  Edit screen
# ════════════════════════════════════════════════════════════════════════════

class SinkEditScreen(FormGuard, Screen):
    """Per-sink credential editor. MLflow has a longer form (url + auth);
    WandB is a single api_key field."""

    TITLE = "xrun — edit sink"
    BINDINGS = [
        Binding("escape", "go_back", "Back"),
        Binding("ctrl+s", "save",    "Save"),
    ]

    def __init__(self, sid: str, name: str) -> None:
        super().__init__()
        self._sid   = sid
        self._sname = name
        self._creds = config.read_credentials()
        self._global = config.read_global_config()

    def compose(self) -> ComposeResult:
        yield TitleBar("edit sink")
        yield Static(f"Edit sink — {self._sname}", classes="screen-title")
        with Vertical(id="vendor-form"):
            if self._sid == "mlflow":
                m   = self._creds.get("mlflow", {})
                url = self._global.get("mlflow", {}).get("url", "") or ""
                yield Static("[bold #bb9af7]Server URL[/]",
                             classes="form-section")
                with Horizontal(classes="form-row"):
                    yield Label("URL:", classes="form-label")
                    yield Input(url, id="input-mlflow-url",
                                placeholder="https://mlflow.your-host:5000",
                                classes="form-input")
                yield Static("[bold #bb9af7]Auth[/] "
                             "[#565f89](Bearer token wins over user+pass)[/]",
                             classes="form-section")
                with Horizontal(classes="form-row"):
                    yield Label("Token:", classes="form-label")
                    yield Input(id="input-mlflow-token", password=True,
                                placeholder=services.secret_placeholder(
                                    m.get("token"), "(optional Bearer token)"),
                                classes="form-input")
                with Horizontal(classes="form-row"):
                    yield Label("Username:", classes="form-label")
                    yield Input(m.get("username") or "",
                                id="input-mlflow-user",
                                placeholder="(or HTTP Basic username)",
                                classes="form-input")
                with Horizontal(classes="form-row"):
                    yield Label("Password:", classes="form-label")
                    yield Input(id="input-mlflow-pass", password=True,
                                placeholder=services.secret_placeholder(
                                    m.get("password"), "…paired with username"),
                                classes="form-input")
                with Horizontal(classes="form-row"):
                    yield Label("Default experiment:", classes="form-label")
                    yield Input(
                        self._global.get("mlflow", {}).get(
                            "experiment_default", "") or "",
                        id="input-mlflow-exp",
                        placeholder="(experiment name when a run sets none)",
                        classes="form-input")
            elif self._sid == "wandb":
                w = self._creds.get("wandb", {})
                yield Static("[bold #bb9af7]API key[/] "
                             "[#565f89]from wandb.ai/authorize[/]",
                             classes="form-section")
                with Horizontal(classes="form-row"):
                    yield Label("Key:", classes="form-label")
                    yield Input(id="input-wandb-key", password=True,
                                placeholder=services.secret_placeholder(
                                    w.get("api_key"), "wandb_v1_…"),
                                classes="form-input")
                yield Static(
                    "[#565f89]Tip:[/] entity is probed automatically on first "
                    "launch; pin via [#7aa2f7]xrun config set …[/] later.",
                    classes="form-footer-hint",
                )
            yield Static("", classes="form-spacer")
            with Horizontal(classes="form-actions"):
                yield Button("Save  [Ctrl+S]", id="btn-save", variant="primary")
                yield Button("Back  \\[Esc]",    id="btn-back")

        yield StatusBar()
        yield Footer()

    def on_mount(self) -> None:
        self.call_after_refresh(self.snapshot_form)

    async def on_button_pressed(self, event: Button.Pressed) -> None:
        if event.button.id == "btn-save":
            await self.action_save()
        elif event.button.id == "btn-back":
            self.action_go_back()

    @single_save
    async def action_save(self) -> None:
        # (key, value, secret, unset) — blank secret means "keep", so it is
        # simply not in the list.
        writes: list[tuple[str, str, bool, bool]] = []
        if self._sid == "mlflow":
            url   = self.query_one("#input-mlflow-url",   Input).value.strip()
            token = self.query_one("#input-mlflow-token", Input).value.strip()
            user  = self.query_one("#input-mlflow-user",  Input).value.strip()
            pwd   = self.query_one("#input-mlflow-pass",  Input).value.strip()
            exp   = self.query_one("#input-mlflow-exp",   Input).value.strip()
            if url and not url.lower().startswith(("http://", "https://")):
                self.notify("MLflow URL must start with http:// or https://",
                            severity="error")
                return
            old_url = self._global.get("mlflow", {}).get("url", "") or ""
            if url and url != old_url:
                writes.append(("mlflow.url", url, False, False))
            elif not url and old_url:
                writes.append(("mlflow.url", "", False, True))
            if token:
                writes.append(("mlflow.token", token, True, False))
            old_user = self._creds.get("mlflow", {}).get("username") or ""
            if user and user != old_user:
                writes.append(("mlflow.username", user, False, False))
            elif not user and old_user:
                writes.append(("mlflow.username", "", False, True))
            if pwd:
                writes.append(("mlflow.password", pwd, True, False))
            old_exp = self._global.get("mlflow", {}).get(
                "experiment_default", "") or ""
            if exp != old_exp:
                if exp:
                    writes.append(("mlflow.experiment_default", exp, False, False))
                else:
                    writes.append(("mlflow.experiment_default", "", False, True))
        elif self._sid == "wandb":
            key = self.query_one("#input-wandb-key", Input).value.strip()
            if key:
                writes.append(("wandb.api_key", key, True, False))

        saved = 0
        for k, v, secret, unset in writes:
            if unset:
                ok, err = await services.config_unset(k)
            else:
                ok, err = await services.config_set(k, v, secret=secret)
            if not ok:
                self.notify(f"{k}: {err or 'failed'}"
                            + (f" (saved {saved} before it)" if saved else ""),
                            severity="error")
                self._creds = config.read_credentials()
                self._global = config.read_global_config()
                return
            saved += 1
        self._creds = config.read_credentials()
        self._global = config.read_global_config()
        if not self.is_attached:
            return  # the user left while the CLI writes were running
        self._reset_secret_inputs()
        self.snapshot_form()
        self.notify(f"{self._sname}: saved {saved}" if saved
                    else f"{self._sname}: nothing to change",
                    severity="information")

    def _reset_secret_inputs(self) -> None:
        """After a save: clear the secret fields and show the new stored tail
        in their placeholders."""
        m = self._creds.get("mlflow") or {}
        fields = {
            "#input-mlflow-token": (m.get("token"), "(optional Bearer token)"),
            "#input-mlflow-pass": (m.get("password"), "…paired with username"),
            "#input-wandb-key": ((self._creds.get("wandb") or {}).get("api_key"),
                                 "wandb_v1_…"),
        }
        for sel, (stored, empty) in fields.items():
            try:
                inp = self.query_one(sel, Input)
            except Exception:
                continue  # the other sink's form
            inp.value = ""
            inp.placeholder = services.secret_placeholder(stored, empty)
