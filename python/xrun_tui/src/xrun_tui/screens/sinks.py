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

import asyncio
import os
import subprocess
from typing import Any

from textual.app import ComposeResult
from textual.binding import Binding
from textual.containers import Horizontal, Vertical
from textual.screen import Screen
from textual.widgets import Button, Footer, Input, Label, Rule, Static

from xrun_tui import config
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


def _pill(state: str) -> str:
    """state ∈ {empty, paused, checking, ok, error, disabled}."""
    if state == "checking":
        return "[#1a1b26 on #e0af68] CHECK [/]"
    if state == "ok":
        return "[#1a1b26 on #9ece6a] READY [/]"
    if state == "error":
        return "[#c0caf5 on #f7768e] ERROR [/]"
    if state == "paused":
        return "[#1a1b26 on #7aa2f7] PAUSED [/]"
    if state == "disabled":
        return "[#c0caf5 on #414868] v0.8 [/]"
    return "[#c0caf5 on #414868] EMPTY [/]"


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


def _set_metrics_sinks(sinks: list[str]) -> None:
    """Persist the `metrics.sinks` list back to config.toml.

    We shell out to `xrun config set metrics.sinks "<csv>"` rather than
    editing the TOML directly: it's the one path that already knows how
    to coerce the comma-separated input into the array shape Rust expects.

    Resolves the binary via PATH. On Windows we still pass the plain name
    (`xrun`) — `_winapi.CreateProcess` honours PATHEXT, so the `.exe`
    suffix is found automatically.
    """
    csv = ",".join(sinks)
    subprocess.run(
        ["xrun", "config", "set", "metrics.sinks", csv],
        check=False,
        capture_output=True,
        text=True,
    )


# ════════════════════════════════════════════════════════════════════════════
#  Overview screen
# ════════════════════════════════════════════════════════════════════════════

class SinksScreen(Screen):
    """List of metric/log sinks. Mirrors Vendors but for tracking servers."""

    TITLE = "xrun — sinks"
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
                        yield Static(_pill(state),
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

    def _highlight(self, idx: int) -> None:
        for i in range(len(_SINKS)):
            row = self.query_one(f"#srow-{i}")
            row.set_class(i == idx, "selected")

    def on_mount(self) -> None:
        self._highlight(self._cursor)

    # ── navigation ───────────────────────────────────────────────────────
    def action_next(self) -> None:
        self._cursor = (self._cursor + 1) % len(_SINKS)
        self._highlight(self._cursor)

    def action_prev(self) -> None:
        self._cursor = (self._cursor - 1) % len(_SINKS)
        self._highlight(self._cursor)

    def action_go_back(self) -> None:
        self.app.pop_screen()

    # ── edit / test / revoke ─────────────────────────────────────────────
    async def action_edit(self) -> None:
        sid, name, _, enabled = _SINKS[self._cursor]
        if not enabled:
            self.notify(f"{name} arrives in v0.8 — not editable yet",
                        severity="warning")
            return
        await self.app.push_screen(SinkEditScreen(sid, name))
        # Re-read state after edit returns
        self._creds, self._sinks_list = _read_state()
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
        status_w.update(_pill("checking"))
        info_w.update("[#e0af68]probing…[/]")
        try:
            ok, detail = await self._probe(sid)
            status_w.update(_pill("ok" if ok else "error"))
            info_w.update(
                f"[#9ece6a]✓ {detail}[/]" if ok
                else f"[#f7768e]✗ {detail}[/]"
            )
        except Exception as exc:
            status_w.update(_pill("error"))
            info_w.update(f"[#f7768e]{exc}[/]")

    async def _probe(self, sid: str) -> tuple[bool, str]:
        """Run `xrun config probe --vendor <sid>` with the stored creds piped
        in via env vars (so the key never lands in the process argv)."""
        env = {}
        if sid == "mlflow":
            m = self._creds.get("mlflow", {})
            if m.get("token"):
                env["XRUN_PROBE_MLFLOW_TOKEN"] = m["token"]
            if m.get("username") and m.get("password"):
                env["XRUN_PROBE_MLFLOW_USERNAME"] = m["username"]
                env["XRUN_PROBE_MLFLOW_PASSWORD"] = m["password"]
            url = config.read_global_config().get("mlflow", {}).get("url", "")
            extra = ["--mlflow-url", url] if url else []
        elif sid == "wandb":
            key = self._creds.get("wandb", {}).get("api_key", "")
            if key:
                env["XRUN_PROBE_WANDB_KEY"] = key
            extra = []
        else:
            return False, "unsupported sink"

        proc = await asyncio.create_subprocess_exec(
            "xrun", "config", "probe", "--vendor", sid, *extra,
            env={**os.environ, **env},
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.PIPE,
        )
        out, err = await proc.communicate()
        try:
            import json
            obj = json.loads(out.decode("utf-8"))
            return bool(obj.get("ok")), str(obj.get("detail", ""))
        except Exception:
            return False, (err.decode("utf-8").strip()
                           or "probe returned non-JSON")

    def action_toggle_default(self) -> None:
        sid, name, _, enabled = _SINKS[self._cursor]
        if not enabled:
            return
        if sid in self._sinks_list:
            self._sinks_list = [s for s in self._sinks_list if s != sid]
            msg = f"{name} removed from default sinks"
        else:
            self._sinks_list.append(sid)
            msg = f"{name} added to default sinks"
        _set_metrics_sinks(self._sinks_list)
        self._refresh_cards()
        self.notify(msg, severity="information")

    async def action_revoke(self) -> None:
        sid, name, _, enabled = _SINKS[self._cursor]
        if not enabled:
            return
        if not _sink_configured(self._creds, sid):
            return
        # No confirm modal here — Vendors uses one but for simplicity the
        # Sinks screen relies on the user noticing the EMPTY pill after
        # revoke; we can add a confirm in slice 5 if it bites.
        if sid == "mlflow":
            self._creds.setdefault("mlflow", {})
            for k in ("token", "username", "password"):
                self._creds["mlflow"].pop(k, None)
        elif sid == "wandb":
            self._creds.setdefault("wandb", {}).pop("api_key", None)
        config.write_credentials(self._creds)
        self._refresh_cards()
        self.notify(f"{name} credentials revoked", severity="warning")

    # ── refresh after a state change ─────────────────────────────────────
    def _refresh_cards(self) -> None:
        self._creds, self._sinks_list = _read_state()
        for i, (sid, _, _, enabled) in enumerate(_SINKS):
            state = self._compute_state(sid, enabled)
            self.query_one(f"#sstatus-{i}", Static).update(_pill(state))
            self.query_one(f"#sinfo-{i}",   Static).update(
                self._foot_text(sid, state, enabled)
            )


# ════════════════════════════════════════════════════════════════════════════
#  Edit screen
# ════════════════════════════════════════════════════════════════════════════

class SinkEditScreen(Screen):
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
                    yield Input(m.get("token") or "", id="input-mlflow-token",
                                password=True, placeholder="(optional Bearer token)",
                                classes="form-input")
                with Horizontal(classes="form-row"):
                    yield Label("Username:", classes="form-label")
                    yield Input(m.get("username") or "",
                                id="input-mlflow-user",
                                placeholder="(or HTTP Basic username)",
                                classes="form-input")
                with Horizontal(classes="form-row"):
                    yield Label("Password:", classes="form-label")
                    yield Input(m.get("password") or "",
                                id="input-mlflow-pass",
                                password=True, placeholder="…paired with username",
                                classes="form-input")
            elif self._sid == "wandb":
                w = self._creds.get("wandb", {})
                yield Static("[bold #bb9af7]API key[/] "
                             "[#565f89]from wandb.ai/authorize[/]",
                             classes="form-section")
                with Horizontal(classes="form-row"):
                    yield Label("Key:", classes="form-label")
                    yield Input(w.get("api_key") or "", id="input-wandb-key",
                                password=True,
                                placeholder="wandb_v1_…",
                                classes="form-input")
                yield Static(
                    "[#565f89]Tip:[/] entity is probed automatically on first "
                    "launch; pin via [#7aa2f7]xrun config set …[/] later.",
                    classes="form-footer-hint",
                )
            yield Static("", classes="form-spacer")
            with Horizontal(classes="form-actions"):
                yield Button("Save  [Ctrl+S]", id="btn-save", variant="primary")
                yield Button("Back  [Esc]",    id="btn-back")

        yield StatusBar()
        yield Footer()

    def on_button_pressed(self, event: Button.Pressed) -> None:
        if event.button.id == "btn-save":
            self.action_save()
        elif event.button.id == "btn-back":
            self.action_go_back()

    def action_save(self) -> None:
        creds = dict(self._creds)
        if self._sid == "mlflow":
            url   = self.query_one("#input-mlflow-url",   Input).value.strip()
            token = self.query_one("#input-mlflow-token", Input).value.strip()
            user  = self.query_one("#input-mlflow-user",  Input).value.strip()
            pwd   = self.query_one("#input-mlflow-pass",  Input).value.strip()
            entry: dict[str, Any] = {}
            if token:
                entry["token"] = token
            if user:
                entry["username"] = user
            if pwd:
                entry["password"] = pwd
            creds["mlflow"] = entry
            # URL lives in global config, not credentials. Shell out to
            # `xrun config set mlflow.url …` so we get the schema-driven
            # type coercion (TOML can't store nullable strings the way
            # Python would).
            if url:
                subprocess.run(
                    ["xrun", "config", "set", "mlflow.url", url],
                    check=False, capture_output=True, text=True,
                )
        elif self._sid == "wandb":
            key = self.query_one("#input-wandb-key", Input).value.strip()
            entry = {}
            if key:
                entry["api_key"] = key
            creds["wandb"] = entry
        config.write_credentials(creds)
        self._creds = creds
        self.notify(f"{self._sname} credentials saved",
                    severity="information")

    def action_go_back(self) -> None:
        self.app.pop_screen()
