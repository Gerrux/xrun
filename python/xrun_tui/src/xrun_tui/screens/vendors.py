from __future__ import annotations

import asyncio
import base64
import json
import os
import sys
import urllib.error
import urllib.request
from pathlib import Path
from typing import Any

from textual import events
from textual.app import ComposeResult
from textual.binding import Binding
from textual.containers import Horizontal, Vertical
from textual.screen import Screen
from textual.widgets import Button, Footer, Input, Label, Rule, Static
from xrun_tui.widgets.status_bar import StatusBar
from xrun_tui.widgets.title_bar import TitleBar

from xrun_tui import config

# Vendors supported by xrun
_VENDORS = [
    ("vast",   "vast.ai",  "GPU cloud (primary)"),
    ("kaggle", "Kaggle",   "Notebook platform"),
]

# Brand emblems and accent colors (used in CSS via .vendor-card-{vid})
_LOGOS = {
    "vast":   "⚡",
    "kaggle": "◆",
}
_BRAND = {
    "vast":   "#ff6b35",
    "kaggle": "#20beff",
}


def _pill(state: str) -> str:
    """Render a status pill. state ∈ {empty, checking, ok, error}."""
    if state == "checking":
        return "[#1a1b26 on #e0af68] CHECK [/]"
    if state == "ok":
        return "[#1a1b26 on #9ece6a] READY [/]"
    if state == "error":
        return "[#c0caf5 on #f7768e] ERROR [/]"
    return "[#c0caf5 on #414868] EMPTY [/]"


def _vendor_configured(creds: dict, vid: str) -> bool:
    """Return True if the vendor has all required credentials set."""
    v = creds.get(vid, {})
    if vid == "kaggle":
        # Env var / access_token file takes priority (no stored creds needed)
        if os.environ.get("KAGGLE_API_TOKEN", "").strip():
            return True
        if (Path.home() / ".kaggle" / "access_token").exists():
            return True
        return bool(v.get("token")) or (bool(v.get("username")) and bool(v.get("key")))
    return bool(v.get("api_key"))


# ═══════════════════════════════════════════════════════════════════════════════
#  Overview screen
# ═══════════════════════════════════════════════════════════════════════════════

class VendorsScreen(Screen):
    TITLE = "xrun — vendors"
    BINDINGS = [
        Binding("escape,q",   "go_back", "Back"),
        Binding("enter,e",    "edit",    "Edit"),
        Binding("i",          "import_native", "Import"),
        Binding("t",          "test",    "Test"),
        Binding("r",          "revoke",  "Revoke"),
        Binding("u",          "open_quota", "Quota"),
        Binding("j,down",     "next",    "Down",   show=False),
        Binding("k,up",       "prev",    "Up",     show=False),
    ]

    def __init__(self) -> None:
        super().__init__()
        self._cursor = 0
        self._creds  = config.read_credentials()
        self._last_click: tuple[int, float] = (-1, 0.0)
        self._pulse_timers: dict[int, Any] = {}
        self._pulse_phase: dict[int, int] = {}

    def compose(self) -> ComposeResult:
        yield TitleBar("vendors")
        yield Static("Vendors & Credentials", classes="screen-title")
        with Vertical(id="vendor-overview"):
            for i, (vid, vname, vdesc) in enumerate(_VENDORS):
                configured = _vendor_configured(self._creds, vid)
                brand      = _BRAND[vid]
                state      = "ok" if configured else "empty"
                with Vertical(
                    classes=f"vendor-card vendor-card-{vid}",
                    id=f"vrow-{i}",
                ):
                    with Horizontal(classes="vendor-card-head"):
                        yield Static(
                            f"[{brand}]{_LOGOS[vid]}[/]",
                            classes="vendor-logo",
                            id=f"vlogo-{i}",
                        )
                        yield Static(
                            f"[bold #c0caf5]{vname}[/]  [#565f89]{vdesc}[/]",
                            classes="vendor-card-title",
                        )
                        yield Static(
                            _pill(state),
                            id=f"vstatus-{i}",
                            classes="vendor-card-pill",
                        )
                    with Horizontal(classes="vendor-card-foot"):
                        yield Static(
                            f"[{brand if configured else '#414868'}]"
                            f"{'●' if configured else '○'}[/]",
                            classes="vendor-card-dot",
                            id=f"vdot-{i}",
                        )
                        yield Static(
                            "[#565f89]Press[/] [#c0caf5]Enter[/] "
                            "[#565f89]or double-click to edit[/]"
                            if not configured else "",
                            id=f"vinfo-{i}",
                            classes="vendor-card-info",
                        )
            yield Rule()
            yield Static(
                "[#565f89]Enter/e[/] [#c0caf5]Edit[/]   "
                "[#565f89]i[/] [#c0caf5]Import native[/]   "
                "[#565f89]t[/] [#c0caf5]Test[/]   "
                "[#565f89]u[/] [#c0caf5]Quota in browser[/]   "
                "[#565f89]r[/] [#c0caf5]Revoke[/]   "
                "[#565f89]j/k[/] [#c0caf5]Navigate[/]",
                classes="vendor-hint",
            )
        yield StatusBar()
        yield Footer()

    def on_mount(self) -> None:
        self._highlight(self._cursor)
        self.run_worker(self._check_vast(),   exclusive=False, group="probe")
        self.run_worker(self._check_kaggle(), exclusive=False, group="probe")

    async def _check_vast(self) -> None:
        api_key = config.get_vast_api_key()
        if not api_key:
            return
        idx = 0
        status_widget = self.query_one(f"#vstatus-{idx}", Static)
        info_widget   = self.query_one(f"#vinfo-{idx}",   Static)
        status_widget.update(_pill("checking"))
        info_widget.update("")
        self._start_pulse(idx, "vast")
        try:
            info = await _fetch_user(api_key)
            if not self.is_attached:
                return
            name   = info.get("username") or info.get("email") or "?"
            credit = float(info.get("credit", 0))
            self._stop_pulse(idx, ok=True, vid="vast")
            status_widget.update(_pill("ok"))
            info_widget.update(
                f"[#565f89]user:[/] [#c0caf5]{name}[/]  "
                f"[#565f89]balance:[/] [#e0af68]${credit:.2f}[/]"
            )
            cache = getattr(self.app, "_vast_status_cache", None)
            if isinstance(cache, dict):
                cache.update({"credit": credit, "username": name})
        except Exception as exc:
            if not self.is_attached:
                return
            self._stop_pulse(idx, ok=False, vid="vast")
            status_widget.update(_pill("error"))
            info_widget.update(f"[#f7768e]{exc}[/]")

    async def _check_kaggle(self) -> None:
        v = self._creds.get("kaggle", {})
        if not _vendor_configured({"kaggle": v}, "kaggle"):
            return
        # Resolve token: env var > access_token file > stored token > legacy user+key
        token = (
            os.environ.get("KAGGLE_API_TOKEN", "").strip()
            or _read_access_token_file()
            or v.get("token", "").strip()
        )
        username = v.get("username", "").strip()
        key      = v.get("key", "").strip()
        idx = next(i for i, (vid, _, _) in enumerate(_VENDORS) if vid == "kaggle")
        status_widget = self.query_one(f"#vstatus-{idx}", Static)
        info_widget   = self.query_one(f"#vinfo-{idx}",   Static)
        status_widget.update(_pill("checking"))
        info_widget.update("")
        self._start_pulse(idx, "kaggle")
        try:
            label, info = await _test_kaggle_api(username, key, token)
            if not self.is_attached:
                return
            self._stop_pulse(idx, ok=True, vid="kaggle")
            status_widget.update(_pill("ok"))
            info_widget.update(f"[#565f89]user:[/] [#c0caf5]{label}[/]  {info}")
            cache = getattr(self.app, "_kaggle_status_cache", None)
            if isinstance(cache, dict):
                cache.update({"kaggle_user": label, "kaggle_connected": True})
        except Exception as exc:
            if not self.is_attached:
                return
            self._stop_pulse(idx, ok=False, vid="kaggle")
            status_widget.update(_pill("error"))
            info_widget.update(f"[#f7768e]{exc}[/]")
            cache = getattr(self.app, "_kaggle_status_cache", None)
            if isinstance(cache, dict):
                cache.clear()

    # ── Navigation ────────────────────────────────────────────────────────────

    def _highlight(self, idx: int) -> None:
        for i in range(len(_VENDORS)):
            row = self.query_one(f"#vrow-{i}", Vertical)
            if i == idx:
                row.add_class("vendor-row-active")
            else:
                row.remove_class("vendor-row-active")

    def action_next(self) -> None:
        self._cursor = (self._cursor + 1) % len(_VENDORS)
        self._highlight(self._cursor)

    def action_prev(self) -> None:
        self._cursor = (self._cursor - 1) % len(_VENDORS)
        self._highlight(self._cursor)

    async def action_edit(self) -> None:
        vid, vname, _ = _VENDORS[self._cursor]
        await self.app.push_screen(VendorEditScreen(vid, vname))

    async def action_test(self) -> None:
        vid = _VENDORS[self._cursor][0]
        if vid == "vast":
            await self._check_vast()
        elif vid == "kaggle":
            await self._check_kaggle()

    async def action_import_native(self) -> None:
        vid = _VENDORS[self._cursor][0]
        if vid == "vast":
            native = Path.home() / ".config" / "vastai" / "vast_api_key"
            if not native.exists():
                self.notify("~/.config/vastai/vast_api_key not found", severity="warning")
                return
            key = native.read_text(encoding="utf-8").strip()
            if not key:
                self.notify("vast_api_key file is empty", severity="warning")
                return

            async def _do_vast(confirmed: bool) -> None:
                if not confirmed:
                    return
                creds = dict(self._creds)
                creds.setdefault("vast", {})["api_key"] = key
                config.write_credentials(creds)
                self._creds = creds
                self._refresh_row(0)
                self.notify("vast.ai key imported from native config", severity="information")
                self.run_worker(self._check_vast(), exclusive=False, group="probe")

            if _vendor_configured(self._creds, "vast"):
                from xrun_tui.screens.confirm import ConfirmScreen
                await self.app.push_screen(
                    ConfirmScreen("Overwrite existing vast.ai credentials?"), _do_vast
                )
            else:
                await _do_vast(True)

        elif vid == "kaggle":
            # Determine the best available source in priority order
            env_token = os.environ.get("KAGGLE_API_TOKEN", "").strip()
            access_token_path = Path.home() / ".kaggle" / "access_token"
            legacy_path = Path.home() / ".kaggle" / "kaggle.json"

            if env_token:
                source_desc = "KAGGLE_API_TOKEN env var"
            elif access_token_path.exists():
                source_desc = "~/.kaggle/access_token"
            elif legacy_path.exists():
                source_desc = "~/.kaggle/kaggle.json"
            else:
                self.notify(
                    "No Kaggle credentials found. Checked: KAGGLE_API_TOKEN, "
                    "~/.kaggle/access_token, ~/.kaggle/kaggle.json",
                    severity="warning",
                )
                return

            async def _do_kaggle(confirmed: bool) -> None:
                if not confirmed:
                    return
                creds = dict(self._creds)
                if env_token:
                    creds.setdefault("kaggle", {})["token"] = env_token
                    config.write_credentials(creds)
                    self._creds = creds
                    self._refresh_row(1)
                    self.notify("Kaggle token imported from KAGGLE_API_TOKEN env var", severity="information")
                elif access_token_path.exists():
                    token = access_token_path.read_text(encoding="utf-8").strip()
                    if not token:
                        self.notify("~/.kaggle/access_token is empty", severity="warning")
                        return
                    creds.setdefault("kaggle", {})["token"] = token
                    config.write_credentials(creds)
                    self._creds = creds
                    self._refresh_row(1)
                    self.notify("Kaggle token imported from ~/.kaggle/access_token", severity="information")
                else:
                    try:
                        data = json.loads(legacy_path.read_text(encoding="utf-8"))
                        username = data.get("username", "").strip()
                        key = data.get("key", "").strip()
                        if not username or not key:
                            self.notify("kaggle.json missing username or key", severity="warning")
                            return
                    except Exception as exc:
                        self.notify(f"Failed to parse kaggle.json: {exc}", severity="error")
                        return
                    creds.setdefault("kaggle", {}).update({"username": username, "key": key})
                    config.write_credentials(creds)
                    self._creds = creds
                    self._refresh_row(1)
                    self.notify(f"Kaggle credentials imported ({username})", severity="information")
                self.run_worker(self._check_kaggle(), exclusive=False, group="probe")

            if _vendor_configured({"kaggle": self._creds.get("kaggle", {})}, "kaggle"):
                from xrun_tui.screens.confirm import ConfirmScreen
                await self.app.push_screen(
                    ConfirmScreen(f"Overwrite existing Kaggle credentials from {source_desc}?"),
                    _do_kaggle,
                )
            else:
                await _do_kaggle(True)

    def action_open_quota(self) -> None:
        import webbrowser
        vid = _VENDORS[self._cursor][0]
        urls = {
            "vast":   "https://cloud.vast.ai/billing/",
            "kaggle": "https://www.kaggle.com/settings",
        }
        url = urls.get(vid)
        if not url:
            self.notify(f"No quota page for {vid}", severity="warning")
            return
        try:
            webbrowser.open(url)
            self.notify(f"Opened {url}", severity="information")
        except Exception as exc:
            self.notify(f"Failed to open browser: {exc}", severity="error")

    async def action_revoke(self) -> None:
        vid, vname, _ = _VENDORS[self._cursor]
        idx = self._cursor

        async def _do_revoke(confirmed: bool) -> None:
            if not confirmed:
                return
            creds = dict(self._creds)
            creds.pop(vid, None)
            config.write_credentials(creds)
            self._creds = creds
            self._refresh_row(idx)
            self.notify(f"{vname} credentials revoked", severity="information")

        from textual.widgets import Button
        self.app.push_screen(
            _ConfirmRevoke(vname),
            _do_revoke,
        )

    def _refresh_row(self, idx: int) -> None:
        vid = _VENDORS[idx][0]
        configured = _vendor_configured(self._creds, vid)
        brand = _BRAND[vid]
        self.query_one(f"#vdot-{idx}", Static).update(
            f"[{brand if configured else '#414868'}]"
            f"{'●' if configured else '○'}[/]"
        )
        self.query_one(f"#vstatus-{idx}", Static).update(
            _pill("ok" if configured else "empty")
        )
        self.query_one(f"#vinfo-{idx}", Static).update(
            "" if configured else
            "[#565f89]Press[/] [#c0caf5]Enter[/] "
            "[#565f89]or double-click to edit[/]"
        )

    # ── Pulse animation on status dot during 'checking' state ────────────────

    def _start_pulse(self, idx: int, vid: str) -> None:
        self._stop_pulse(idx, ok=False, vid=vid, _restore=False)
        self._pulse_phase[idx] = 0
        frames = ["◐", "◓", "◑", "◒"]
        brand  = _BRAND[vid]

        def _tick() -> None:
            try:
                w = self.query_one(f"#vdot-{idx}", Static)
            except Exception:
                return
            ph = self._pulse_phase.get(idx, 0)
            self._pulse_phase[idx] = (ph + 1) % len(frames)
            w.update(f"[{brand}]{frames[ph]}[/]")

        self._pulse_timers[idx] = self.set_interval(0.15, _tick)

    def _stop_pulse(self, idx: int, *, ok: bool, vid: str,
                    _restore: bool = True) -> None:
        timer = self._pulse_timers.pop(idx, None)
        if timer is not None:
            try:
                timer.stop()
            except Exception:
                pass
        self._pulse_phase.pop(idx, None)
        if not _restore:
            return
        try:
            w = self.query_one(f"#vdot-{idx}", Static)
        except Exception:
            return
        color = _BRAND[vid] if ok else "#f7768e"
        w.update(f"[{color}]●[/]")

    def on_click(self, event: events.Click) -> None:
        widget = event.widget
        for _ in range(10):
            if widget is None or widget is self:
                break
            wid = getattr(widget, "id", None) or ""
            if wid.startswith("vrow-"):
                try:
                    idx = int(wid[5:])
                    self._cursor = idx
                    self._highlight(idx)
                    is_double = getattr(event, "chain", 1) >= 2
                    if not is_double:
                        import time as _time
                        now = _time.monotonic()
                        last_idx, last_t = self._last_click
                        if last_idx == idx and (now - last_t) < 0.5:
                            is_double = True
                            self._last_click = (-1, 0.0)
                        else:
                            self._last_click = (idx, now)
                    if is_double:
                        asyncio.create_task(self.action_edit())
                except (ValueError, IndexError):
                    pass
                return
            widget = widget.parent

    def action_go_back(self) -> None:
        self.app.pop_screen()

    def on_screen_resume(self) -> None:
        self._creds = config.read_credentials()
        for i in range(len(_VENDORS)):
            self._refresh_row(i)
        if self._creds.get("vast", {}).get("api_key"):
            self.run_worker(self._check_vast(),   exclusive=False, group="probe")
        if _vendor_configured(self._creds, "kaggle"):
            self.run_worker(self._check_kaggle(), exclusive=False, group="probe")


# ═══════════════════════════════════════════════════════════════════════════════
#  Edit credentials screen
# ═══════════════════════════════════════════════════════════════════════════════

class VendorEditScreen(Screen):
    BINDINGS = [
        Binding("escape",  "go_back", "Back"),
        Binding("ctrl+s",  "save",    "Save"),
        Binding("ctrl+t",  "test",    "Test"),
    ]

    def __init__(self, vendor_id: str, vendor_name: str) -> None:
        super().__init__()
        self._vid   = vendor_id
        self._vname = vendor_name
        self._creds = config.read_credentials()

    def compose(self) -> ComposeResult:
        v = self._creds.get(self._vid, {})
        yield TitleBar("edit credentials")
        yield Static(f"Edit credentials — {self._vname}", classes="screen-title")
        with Vertical(id="vendor-form"):
            if self._vid == "kaggle":
                token    = v.get("token") or ""
                username = v.get("username") or ""
                key      = v.get("key") or ""
                # New-style token (Bearer)
                yield Static(
                    "[bold #bb9af7]API Token[/] [#565f89](recommended — kaggle CLI ≥ 1.8.0)[/]",
                    classes="form-section",
                )
                with Horizontal(classes="form-row"):
                    yield Label("Token:", classes="form-label")
                    yield Input(
                        token,
                        id="input-kaggle-token",
                        password=True,
                        placeholder="Paste API token from kaggle.com/settings…",
                        classes="form-input",
                    )
                yield Static(_masked(token), id="token-hint", classes="form-hint")
                # Legacy credentials
                yield Static(
                    "[bold #bb9af7]Legacy credentials[/] [#565f89](kaggle.json / username+key)[/]",
                    classes="form-section",
                )
                with Horizontal(classes="form-row"):
                    yield Label("Username:", classes="form-label")
                    yield Input(
                        username,
                        id="input-kaggle-username",
                        placeholder="Kaggle username…",
                        classes="form-input",
                    )
                with Horizontal(classes="form-row"):
                    yield Label("Key:", classes="form-label")
                    yield Input(
                        key,
                        id="input-kaggle-key",
                        password=True,
                        placeholder="Kaggle API key (from kaggle.json)…",
                        classes="form-input",
                    )
                yield Static(_masked(key), id="key-hint", classes="form-hint")
                yield Static(
                    "[#565f89]Native fallback:[/] [#7aa2f7]~/.kaggle/kaggle.json[/]",
                    classes="form-footer-hint",
                )
            else:
                # vast and others: single api_key field
                api_key = v.get("api_key") or ""
                with Horizontal(classes="form-row"):
                    yield Label("API Key:", classes="form-label")
                    yield Input(
                        api_key,
                        id="input-api-key",
                        password=True,
                        placeholder=f"Enter {self._vname} API key…",
                        classes="form-input",
                    )
                yield Static(_masked(api_key), id="key-hint", classes="form-hint")
                if self._vid == "vast":
                    yield Static(
                        "[#565f89]Native fallback:[/] [#7aa2f7]~/.config/vastai/vast_api_key[/]",
                        classes="form-footer-hint",
                    )
                    yield Static(
                        "[bold #bb9af7]SSH Keys[/] "
                        "[#565f89](needed for `xrun pull`, live logs, and SSH into rented instances)[/]",
                        classes="form-section",
                    )
                    yield Static(
                        "[#565f89]Loading registered keys…[/]",
                        id="vast-ssh-list",
                        classes="form-hint",
                    )
                    with Horizontal(classes="form-row"):
                        yield Label("Public key:", classes="form-label")
                        yield Input(
                            "",
                            id="input-ssh-pubkey",
                            placeholder="ssh-ed25519 AAAA…  (paste contents of *.pub)",
                            classes="form-input",
                        )
                    with Horizontal(classes="form-actions"):
                        yield Button("Add key",            id="btn-ssh-add")
                        yield Button("Load from ~/.ssh",   id="btn-ssh-load")
                        yield Button("Generate new",       id="btn-ssh-gen")
                        yield Button("Refresh",            id="btn-ssh-refresh")
                    yield Static("", id="ssh-result", classes="form-hint")

                    # Region filter — applies to offer search via xrun-core
                    # config key `search.exclude_countries`.
                    yield Static(
                        "[bold #bb9af7]Region filter[/] "
                        "[#565f89](skip offers from these countries during search)[/]",
                        classes="form-section",
                    )
                    yield Static(
                        "[#565f89]Loading current exclusions…[/]",
                        id="region-current",
                        classes="form-hint",
                    )
                    with Horizontal(classes="form-actions"):
                        yield Button("Pick countries…", id="btn-pick-countries-vast")
                        yield Button("Clear",           id="btn-clear-countries")
                    yield Static("", id="region-result", classes="form-hint")

            yield Static("", id="test-result", classes="form-hint")
            yield Static("", classes="form-spacer")
            with Horizontal(classes="form-actions"):
                yield Button("Save  [Ctrl+S]", id="btn-save", variant="primary")
                yield Button("Test  [Ctrl+T]", id="btn-test")
                yield Button("Back  [Esc]",    id="btn-back")

        yield StatusBar()
        yield Footer()

    def on_input_changed(self, event: Input.Changed) -> None:
        try:
            if event.input.id == "input-api-key":
                self.query_one("#key-hint", Static).update(_masked(event.value))
                self.query_one("#test-result", Static).update("")
                if self._vid == "vast":
                    # Re-probe SSH keys with the new credential.
                    self.run_worker(self._refresh_ssh_keys(), exclusive=True, group="ssh")
            elif event.input.id == "input-kaggle-token":
                self.query_one("#token-hint", Static).update(_masked(event.value))
                self.query_one("#test-result", Static).update("")
            elif event.input.id == "input-kaggle-key":
                self.query_one("#key-hint", Static).update(_masked(event.value))
                self.query_one("#test-result", Static).update("")
        except Exception:
            pass

    def on_mount(self) -> None:
        if self._vid == "vast":
            self.run_worker(self._refresh_ssh_keys(), exclusive=True, group="ssh")
            self.run_worker(self._load_excluded_countries(), exclusive=True, group="region")

    def on_button_pressed(self, event: Button.Pressed) -> None:
        bid = event.button.id or ""
        if bid == "btn-save":
            self.action_save()
        elif bid == "btn-test":
            self.run_worker(self._do_test(), exclusive=True)
        elif bid == "btn-back":
            self.action_go_back()
        elif bid == "btn-ssh-add":
            self.run_worker(self._do_ssh_add(), exclusive=True, group="ssh")
        elif bid == "btn-ssh-load":
            self._do_ssh_load_local()
        elif bid == "btn-ssh-gen":
            self.run_worker(self._do_ssh_generate(), exclusive=True, group="ssh")
        elif bid == "btn-ssh-refresh":
            self.run_worker(self._refresh_ssh_keys(), exclusive=True, group="ssh")
        elif bid.startswith("btn-ssh-del-"):
            key_id = bid[len("btn-ssh-del-"):]
            self.run_worker(self._do_ssh_delete(key_id), exclusive=True, group="ssh")
        elif bid == "btn-pick-countries-vast":
            self._open_country_picker()
        elif bid == "btn-clear-countries":
            self.run_worker(self._save_excluded_countries([]),
                            exclusive=True, group="region")

    # ── Region filter (vast only) ────────────────────────────────────────────

    async def _load_excluded_countries(self) -> list[str]:
        from xrun_tui import services
        ok, data, err = await services.config_show(secrets=False)
        codes: list[str] = []
        if ok:
            search_cfg = data.get("search") or {}
            raw = search_cfg.get("exclude_countries") or []
            if isinstance(raw, list):
                codes = [str(c).strip().upper() for c in raw if str(c).strip()]
            elif isinstance(raw, str):
                codes = [c.strip().upper() for c in raw.split(",") if c.strip()]
        try:
            from xrun_tui.screens.country_exclude import _flag
            w = self.query_one("#region-current", Static)
            if codes:
                pretty = " ".join(_flag(c) for c in codes)
                w.update(f"[#565f89]Excluded:[/] {pretty}")
            elif not ok:
                w.update(f"[#414868]Could not read config: {err[:80]}[/]")
            else:
                w.update("[#414868]No countries excluded.[/]")
        except Exception:
            pass
        self._excluded_countries = codes
        return codes

    def _open_country_picker(self) -> None:
        from xrun_tui.screens.country_exclude import CountryExcludeScreen
        current = list(getattr(self, "_excluded_countries", []) or [])

        def _done(result: list[str] | None) -> None:
            if result is None:
                return
            self.run_worker(
                self._save_excluded_countries(result),
                exclusive=True, group="region",
            )

        self.app.push_screen(CountryExcludeScreen(current), _done)

    async def _save_excluded_countries(self, codes: list[str]) -> None:
        from xrun_tui.screens.settings import _xrun_config_set
        value = ", ".join(codes)
        result = self.query_one("#region-result", Static)
        result.update("[#e0af68]Saving…[/]")
        ok, err = await _xrun_config_set("search.exclude_countries", value)
        if ok:
            result.update(
                f"[bold #9ece6a]✓ Saved.[/] "
                f"[#565f89]{len(codes)} countr{'y' if len(codes) == 1 else 'ies'} excluded.[/]"
            )
            await self._load_excluded_countries()
        else:
            result.update(f"[bold #f7768e]✗ {err[:120]}[/]")

    # ── SSH key management (vast only) ────────────────────────────────────────

    def _current_api_key(self) -> str:
        try:
            return self.query_one("#input-api-key", Input).value.strip()
        except Exception:
            return ""

    async def _refresh_ssh_keys(self) -> None:
        try:
            list_widget = self.query_one("#vast-ssh-list", Static)
        except Exception:
            return
        api_key = self._current_api_key()
        if not api_key:
            list_widget.update("[#e0af68]Enter API key above to manage SSH keys.[/]")
            return
        list_widget.update("[#565f89]Loading registered keys…[/]")
        try:
            keys = await _list_vast_ssh_keys(api_key)
        except Exception as exc:
            list_widget.update(f"[bold #f7768e]✗ Failed to fetch keys:[/] {exc}")
            return
        if not keys:
            list_widget.update(
                "[#e0af68]No SSH keys registered.[/] "
                "[#565f89]Paste a public key below or click[/] [bold]Load from ~/.ssh[/]."
            )
            return
        # Render a compact list. Avoid mounting per-row buttons (Static can't host
        # them) — surface ids the user can revoke via the existing Vendors screen
        # in a follow-up. Here we just show them.
        lines = ["[bold #c0caf5]Registered keys[/]"]
        for k in keys:
            kid = k.get("id", "?")
            label = _short_pubkey(k.get("ssh_key", ""))
            kid_str = "primary" if kid == "-" else f"#{kid}"
            lines.append(f"  [#9ece6a]●[/] [#414868]{kid_str:<12}[/] {label}")
        list_widget.update("\n".join(lines))

    async def _do_ssh_add(self) -> None:
        result = self.query_one("#ssh-result", Static)
        api_key = self._current_api_key()
        if not api_key:
            self.notify("Enter and save an API key first.", severity="warning")
            return
        try:
            inp = self.query_one("#input-ssh-pubkey", Input)
        except Exception:
            return
        pub = inp.value.strip()
        if not pub:
            self.notify("Paste a public key first (or click 'Load from ~/.ssh').", severity="warning")
            return
        if not pub.startswith(("ssh-", "ecdsa-", "sk-")):
            self.notify(
                "That doesn't look like a public key. Expected a single line starting "
                "with 'ssh-ed25519' / 'ssh-rsa' / 'ecdsa-…'.",
                severity="warning", timeout=10,
            )
            return
        result.update("[#e0af68]Registering key…[/]")
        try:
            await _add_vast_ssh_key(api_key, pub)
        except Exception as exc:
            result.update(f"[bold #f7768e]✗ {exc}[/]")
            return
        inp.value = ""
        result.update("[bold #9ece6a]✓ Key registered.[/]")
        await self._refresh_ssh_keys()

    async def _do_ssh_delete(self, key_id: str) -> None:
        api_key = self._current_api_key()
        if not api_key or key_id in ("", "-"):
            return
        result = self.query_one("#ssh-result", Static)
        result.update(f"[#e0af68]Deleting key #{key_id}…[/]")
        try:
            await _delete_vast_ssh_key(api_key, key_id)
        except Exception as exc:
            result.update(f"[bold #f7768e]✗ {exc}[/]")
            return
        result.update(f"[bold #9ece6a]✓ Key #{key_id} removed.[/]")
        await self._refresh_ssh_keys()

    async def _do_ssh_generate(self) -> None:
        """Generate ~/.ssh/id_ed25519 via ssh-keygen and load the .pub into the form.

        Refuses to overwrite an existing key — the user must delete it manually
        or pick a different name. We don't ask for a passphrase here (empty
        passphrase) since vast.ai's automation flow assumes unattended SSH; if
        the user wants a passphrase, they should run ssh-keygen themselves and
        click 'Load from ~/.ssh' afterwards.
        """
        result = self.query_one("#ssh-result", Static)
        ssh_dir = Path.home() / ".ssh"
        priv = ssh_dir / "id_ed25519"
        pub = ssh_dir / "id_ed25519.pub"
        if priv.exists() or pub.exists():
            # Already have a key — auto-load it so the user only needs one
            # more click (Add key) to register it on vast.ai. We never
            # silently overwrite an existing private key.
            if pub.exists():
                try:
                    pub_text = pub.read_text(encoding="utf-8").strip()
                    self.query_one("#input-ssh-pubkey", Input).value = pub_text
                    result.update(
                        f"[bold #9ece6a]✓ Existing key loaded[/] [#c0caf5]({pub.name})[/]. "
                        f"[#565f89]Press[/] [bold]Add key[/] [#565f89]to register on vast.ai. "
                        f"To make a fresh key, rename or delete[/] [bold]{priv}[/] "
                        f"[#565f89]first.[/]"
                    )
                    return
                except Exception:
                    pass
            result.update(
                f"[#e0af68]A private key exists at[/] [bold]{priv}[/] "
                f"[#565f89]but its[/] [bold]{pub.name}[/] [#565f89]is missing. "
                f"Regenerate the public part with[/] "
                f"[bold]ssh-keygen -y -f {priv} > {pub}[/][#565f89], "
                f"or rename the private key and click[/] [bold]Generate new[/] "
                f"[#565f89]again.[/]"
            )
            return
        ssh_dir.mkdir(parents=True, exist_ok=True)
        result.update("[#e0af68]Running ssh-keygen…[/]")

        async def _spawn() -> tuple[int, str]:
            kwargs: dict[str, Any] = dict(
                stdout=asyncio.subprocess.PIPE,
                stderr=asyncio.subprocess.STDOUT,
            )
            if sys.platform == "win32":
                import subprocess as _sub
                kwargs["creationflags"] = _sub.CREATE_NO_WINDOW
            proc = await asyncio.create_subprocess_exec(
                "ssh-keygen", "-t", "ed25519",
                "-f", str(priv),
                "-N", "",                # empty passphrase
                "-C", f"xrun-vast@{os.environ.get('COMPUTERNAME') or 'host'}",
                **kwargs,
            )
            out, _ = await asyncio.wait_for(proc.communicate(), timeout=30)
            return proc.returncode or 0, out.decode(errors="replace")

        try:
            code, output = await _spawn()
        except FileNotFoundError:
            result.update(
                "[bold #f7768e]✗ ssh-keygen not found in PATH.[/] "
                "[#565f89]On Windows install OpenSSH Client via "
                "Settings → Apps → Optional features.[/]"
            )
            return
        except Exception as exc:
            result.update(f"[bold #f7768e]✗ ssh-keygen failed:[/] {exc}")
            return
        if code != 0 or not pub.exists():
            tail = output.strip().splitlines()[-1] if output.strip() else "non-zero exit"
            result.update(f"[bold #f7768e]✗ ssh-keygen failed:[/] {tail}")
            return

        try:
            pub_text = pub.read_text(encoding="utf-8").strip()
        except Exception as exc:
            result.update(f"[bold #f7768e]✗ Could not read {pub}:[/] {exc}")
            return
        try:
            self.query_one("#input-ssh-pubkey", Input).value = pub_text
        except Exception:
            pass
        result.update(
            f"[bold #9ece6a]✓ Generated[/] [#c0caf5]{priv}[/] "
            f"[#565f89](no passphrase). Public key loaded — press[/] "
            f"[bold]Add key[/] [#565f89]to register on vast.ai.[/]"
        )

    def _do_ssh_load_local(self) -> None:
        result = self.query_one("#ssh-result", Static)
        found = _scan_local_pubkeys()
        if not found:
            result.update(
                "[#e0af68]No *.pub files in[/] [bold]~/.ssh[/]. "
                "[#565f89]Generate one with[/] [bold]ssh-keygen -t ed25519[/]."
            )
            return
        # Prefer ed25519 over rsa over the rest.
        priority = {"id_ed25519.pub": 0, "id_rsa.pub": 1}
        found.sort(key=lambda pc: priority.get(pc[0].name, 9))
        path, text = found[0]
        try:
            self.query_one("#input-ssh-pubkey", Input).value = text
        except Exception:
            return
        extras = (
            f"  [#414868](+{len(found) - 1} more in ~/.ssh — edit field manually if needed)[/]"
            if len(found) > 1 else ""
        )
        result.update(
            f"[#565f89]Loaded[/] [bold]{path.name}[/]. "
            f"[#565f89]Press[/] [bold]Add key[/] [#565f89]to register.[/]{extras}"
        )

    def action_save(self) -> None:
        creds = dict(self._creds)
        if self._vid == "kaggle":
            token    = self.query_one("#input-kaggle-token",    Input).value.strip()
            username = self.query_one("#input-kaggle-username", Input).value.strip()
            key      = self.query_one("#input-kaggle-key",      Input).value.strip()

            # Auth methods are mutually exclusive: a Kaggle access token
            # already encodes the account identity, so storing legacy
            # username+key alongside it is at best dead weight and at worst
            # actively wrong (stale username from a different account, the
            # very symptom that prompted this priority rule). Enforce here:
            # token wins; legacy fields are saved only when no token is set.
            entry: dict = {}
            if token:
                entry["token"] = token
                # Clear stale legacy fields from BOTH the saved creds AND
                # the form so the user sees the auth mode they actually
                # have, not a confusing token+username mix.
                try:
                    self.query_one("#input-kaggle-username", Input).value = ""
                    self.query_one("#input-kaggle-key",      Input).value = ""
                except Exception:
                    pass
                if username or key:
                    self.notify(
                        "Token set — legacy username+key cleared "
                        "(token wins over legacy auth)",
                        severity="information",
                        timeout=6,
                    )
            else:
                if username:
                    entry["username"] = username
                if key:
                    entry["key"] = key
            creds["kaggle"] = entry
        else:
            api_key = self.query_one("#input-api-key", Input).value.strip()
            creds.setdefault(self._vid, {})["api_key"] = api_key
        config.write_credentials(creds)
        self._creds = creds
        self.notify("Credentials saved", severity="information")

    async def action_test(self) -> None:
        await self._do_test()

    async def _do_test(self) -> None:
        result = self.query_one("#test-result", Static)
        result.update("[#e0af68]Testing…[/]")
        try:
            if self._vid == "vast":
                api_key = self.query_one("#input-api-key", Input).value.strip()
                if not api_key:
                    self.notify("Enter API key first", severity="warning")
                    result.update("")
                    return
                info   = await _fetch_user(api_key)
                name   = info.get("username") or info.get("email") or "unknown"
                credit = float(info.get("credit", 0))
                result.update(
                    f"[bold #9ece6a]✓ Connected[/]  "
                    f"[#565f89]user:[/] [#c0caf5]{name}[/]  "
                    f"[#565f89]balance:[/] [#e0af68]${credit:.2f}[/]"
                )
            elif self._vid == "kaggle":
                token    = self.query_one("#input-kaggle-token",    Input).value.strip()
                username = self.query_one("#input-kaggle-username", Input).value.strip()
                key      = self.query_one("#input-kaggle-key",      Input).value.strip()
                if not token and not (username and key):
                    self.notify("Enter token or username+key first", severity="warning")
                    result.update("")
                    return
                label, info = await _test_kaggle_api(username, key, token)
                result.update(
                    f"[bold #9ece6a]✓ Connected[/]  "
                    f"[#565f89]user:[/] [#c0caf5]{label}[/]  {info}"
                )
            else:
                result.update("[#565f89]Test not available for this vendor[/]")
        except Exception as exc:
            result.update(f"[bold #f7768e]✗ {exc}[/]")

    def action_go_back(self) -> None:
        self.app.pop_screen()


# ═══════════════════════════════════════════════════════════════════════════════
#  Confirm revoke modal
# ═══════════════════════════════════════════════════════════════════════════════

class _ConfirmRevoke(Screen):
    def __init__(self, vendor_name: str) -> None:
        super().__init__()
        self._vname = vendor_name

    def compose(self) -> ComposeResult:
        with Vertical(id="confirm-box"):
            yield Static(
                f"[bold #f7768e]Revoke {self._vname} credentials?[/]\n\n"
                f"[#c0caf5]This will delete saved keys from credentials.toml.[/]",
                id="confirm-msg",
            )
            with Horizontal(classes="form-actions"):
                yield Button("Yes, revoke", id="btn-yes",    variant="error")
                yield Button("Cancel",      id="btn-cancel")

    def on_button_pressed(self, event: Button.Pressed) -> None:
        self.dismiss(event.button.id == "btn-yes")


# ── API helpers ───────────────────────────────────────────────────────────────

async def _fetch_user(api_key: str) -> dict:
    def _do() -> dict:
        req = urllib.request.Request(
            "https://console.vast.ai/api/v0/users/current/",
            headers={"Authorization": f"Bearer {api_key}"},
        )
        with urllib.request.urlopen(req, timeout=10) as r:
            return json.loads(r.read())
    return await asyncio.to_thread(_do)


async def _test_kaggle_api(username: str, key: str, token: str = "") -> tuple[str, str]:
    """Test Kaggle credentials. Returns (label, info_markup) on success.

    Hits /competitions/list?pageSize=1 only as an auth probe — the response
    body is discarded. The Kaggle public API doesn't expose remaining quota;
    we surface the well-known free-tier limits instead.
    """
    def _do() -> tuple[str, str]:
        if token:
            auth_header = f"Bearer {token}"
            label = "API token"
        else:
            encoded = base64.b64encode(f"{username}:{key}".encode()).decode()
            auth_header = f"Basic {encoded}"
            label = username or "?"
        req = urllib.request.Request(
            "https://www.kaggle.com/api/v1/competitions/list?page=1&pageSize=1",
            headers={"Authorization": auth_header},
        )
        with urllib.request.urlopen(req, timeout=20) as r:
            r.read()
        info = (
            "[#565f89]free tier:[/] "
            "[#9ece6a]CPU ∞[/][#565f89], 24h/session · [/]"
            "[#9ece6a]GPU 30h[/][#565f89]/wk, 12h/session · [/]"
            "[#9ece6a]TPU 20h[/][#565f89]/wk, 9h/session[/]"
        )
        return label, info
    return await asyncio.to_thread(_do)


async def _list_vast_ssh_keys(api_key: str) -> list[dict]:
    """Return registered SSH keys for the vast.ai account.

    Each entry carries at minimum {id, ssh_key}. Older accounts may also have a
    single primary key on `users/current/.ssh_key`; we surface that as a virtual
    entry with id=`-` so the user can see it even if the dedicated endpoint is
    empty.
    """
    def _do() -> list[dict]:
        keys: list[dict] = []
        try:
            req = urllib.request.Request(
                "https://console.vast.ai/api/v0/ssh/",
                headers={"Authorization": f"Bearer {api_key}"},
            )
            with urllib.request.urlopen(req, timeout=15) as r:
                data = json.loads(r.read())
            if isinstance(data, list):
                keys = data
            elif isinstance(data, dict):
                keys = data.get("results") or data.get("ssh_keys") or []
        except urllib.error.HTTPError as e:
            if e.code != 404:
                raise
        # Legacy primary-key field on the user record.
        try:
            req = urllib.request.Request(
                "https://console.vast.ai/api/v0/users/current/",
                headers={"Authorization": f"Bearer {api_key}"},
            )
            with urllib.request.urlopen(req, timeout=10) as r:
                user = json.loads(r.read())
            primary = (user.get("ssh_key") or "").strip()
            if primary and not any(
                (k.get("ssh_key") or "").strip() == primary for k in keys
            ):
                keys.insert(0, {"id": "-", "ssh_key": primary, "name": "primary"})
        except Exception:
            pass
        return keys
    return await asyncio.to_thread(_do)


async def _add_vast_ssh_key(api_key: str, pubkey: str) -> None:
    """Register a public key with the vast.ai account.

    Tries the modern collection endpoint first; on 400/404/405 falls back to
    `PUT /users/current/.ssh_key` (legacy single-key model). Surfaces the
    server's error body on final failure so the user can see what vast.ai is
    actually complaining about.
    """
    pubkey = pubkey.strip()

    def _post(url: str, body_obj: dict, method: str = "POST") -> tuple[bool, int, str]:
        body = json.dumps(body_obj).encode()
        req = urllib.request.Request(
            url,
            data=body,
            method=method,
            headers={
                "Authorization": f"Bearer {api_key}",
                "Accept": "application/json",
                "Content-Type": "application/json",
            },
        )
        try:
            with urllib.request.urlopen(req, timeout=15) as r:
                r.read()
            return True, 200, ""
        except urllib.error.HTTPError as e:
            try:
                detail = e.read().decode("utf-8", errors="replace").strip()
            except Exception:
                detail = ""
            # Strip noisy HTML wrappers — vast.ai usually returns JSON or a
            # short plain-text reason.
            if detail.startswith("<"):
                detail = ""
            return False, e.code, detail
        except urllib.error.URLError as e:
            return False, 0, str(e.reason)

    def _do() -> None:
        ok, code, detail = _post(
            "https://console.vast.ai/api/v0/ssh/",
            {"ssh_key": pubkey},
        )
        if ok:
            return
        # Fall back to the legacy single-key endpoint for accounts that don't
        # expose the collection (or that 400 on it for unknown reasons).
        if code in (400, 404, 405):
            ok2, code2, detail2 = _post(
                "https://console.vast.ai/api/v0/users/current/",
                {"ssh_key": pubkey},
                method="PUT",
            )
            if ok2:
                return
            # Prefer the original /ssh/ error message if it's informative,
            # otherwise show the legacy endpoint's reason.
            primary = detail or f"HTTP {code}"
            secondary = detail2 or f"HTTP {code2}"
            raise RuntimeError(
                f"vast.ai rejected the key. /ssh/ → {primary}; "
                f"/users/current/ → {secondary}"
            )
        raise RuntimeError(f"HTTP {code}: {detail or 'Bad Request'}")

    await asyncio.to_thread(_do)


async def _delete_vast_ssh_key(api_key: str, key_id: str) -> None:
    def _do() -> None:
        req = urllib.request.Request(
            f"https://console.vast.ai/api/v0/ssh/{key_id}/",
            method="DELETE",
            headers={"Authorization": f"Bearer {api_key}"},
        )
        with urllib.request.urlopen(req, timeout=15) as r:
            r.read()
    await asyncio.to_thread(_do)


def _scan_local_pubkeys() -> list[tuple[Path, str]]:
    """Return [(path, contents)] for any *.pub under ~/.ssh."""
    found: list[tuple[Path, str]] = []
    ssh_dir = Path.home() / ".ssh"
    if not ssh_dir.is_dir():
        return found
    for p in sorted(ssh_dir.glob("*.pub")):
        try:
            text = p.read_text(encoding="utf-8").strip()
            if text.startswith(("ssh-", "ecdsa-", "sk-")):
                found.append((p, text))
        except Exception:
            continue
    return found


def _short_pubkey(pubkey: str) -> str:
    parts = pubkey.strip().split()
    if not parts:
        return "(empty)"
    algo = parts[0]
    body = parts[1] if len(parts) > 1 else ""
    comment = " ".join(parts[2:]) if len(parts) > 2 else ""
    tail = body[-12:] if len(body) > 12 else body
    label = f"{algo} …{tail}"
    if comment:
        label += f"  [#565f89]{comment}[/]"
    return label


async def fetch_vast_instances(api_key: str) -> list[dict]:
    def _do() -> dict:
        req = urllib.request.Request(
            "https://console.vast.ai/api/v0/instances/",
            headers={"Authorization": f"Bearer {api_key}"},
        )
        with urllib.request.urlopen(req, timeout=15) as r:
            return json.loads(r.read())
    data = await asyncio.to_thread(_do)
    return data.get("instances", [])


def _read_access_token_file() -> str:
    """Return contents of ~/.kaggle/access_token, or empty string if absent."""
    p = Path.home() / ".kaggle" / "access_token"
    try:
        return p.read_text(encoding="utf-8").strip() if p.exists() else ""
    except Exception:
        return ""


def _masked(key: str) -> str:
    key = key.strip()
    if not key:
        return "[#565f89]not configured[/]"
    if len(key) > 8:
        return f"[#565f89]{'*' * (len(key) - 6)}{key[-6:]}[/]"
    return "[#565f89]key set[/]"
