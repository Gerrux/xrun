from __future__ import annotations

import asyncio
import base64
import json
import os
import urllib.request
from pathlib import Path

from textual.app import ComposeResult
from textual.binding import Binding
from textual.containers import Horizontal, Vertical
from textual.screen import Screen
from textual.widgets import Button, Footer, Header, Input, Label, Rule, Static
from xrun_tui.widgets.status_bar import StatusBar

from xrun_tui import config

# Vendors supported by xrun
_VENDORS = [
    ("vast",   "vast.ai",  "GPU cloud (primary)"),
    ("kaggle", "Kaggle",   "Notebook platform"),
]


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
        Binding("j,down",     "next",    "Down",   show=False),
        Binding("k,up",       "prev",    "Up",     show=False),
    ]

    def __init__(self) -> None:
        super().__init__()
        self._cursor = 0
        self._creds  = config.read_credentials()

    def compose(self) -> ComposeResult:
        yield Header(show_clock=True)
        yield Static("Vendors & Credentials", classes="screen-title")
        with Vertical(id="vendor-overview"):
            for i, (vid, vname, vdesc) in enumerate(_VENDORS):
                configured = _vendor_configured(self._creds, vid)
                dot_style  = "#9ece6a" if configured else "#414868"
                dot_sym    = "●" if configured else "○"
                with Horizontal(classes="vendor-row", id=f"vrow-{i}"):
                    yield Static(f"[{dot_style}]{dot_sym}[/]", classes="vendor-dot",
                                 id=f"vdot-{i}")
                    yield Static(f"[bold #c0caf5]{vname}[/]  [#565f89]{vdesc}[/]",
                                 classes="vendor-name-col")
                    yield Static(
                        "[#9ece6a]configured[/]" if configured else "[#414868]not configured[/]",
                        classes="vendor-status-col", id=f"vstatus-{i}",
                    )
                    yield Static("", id=f"vinfo-{i}", classes="vendor-info-col")
            yield Rule()
            yield Static(
                "[#565f89]Enter/e[/] [#c0caf5]Edit[/]   "
                "[#565f89]i[/] [#c0caf5]Import native[/]   "
                "[#565f89]t[/] [#c0caf5]Test[/]   "
                "[#565f89]r[/] [#c0caf5]Revoke[/]   "
                "[#565f89]j/k[/] [#c0caf5]Navigate[/]",
                classes="vendor-hint",
            )
        yield StatusBar()
        yield Footer()

    def on_mount(self) -> None:
        self._highlight(self._cursor)
        self.call_after_refresh(self._check_vast)
        self.call_after_refresh(self._check_kaggle)

    async def _check_vast(self) -> None:
        api_key = config.get_vast_api_key()
        if not api_key:
            return
        status_widget = self.query_one("#vstatus-0", Static)
        info_widget   = self.query_one("#vinfo-0",   Static)
        status_widget.update("[#e0af68]checking…[/]")
        try:
            info = await _fetch_user(api_key)
            name   = info.get("username") or info.get("email") or "?"
            credit = float(info.get("credit", 0))
            status_widget.update("[bold #9ece6a]✓ connected[/]")
            info_widget.update(
                f"[#565f89]user:[/] [#c0caf5]{name}[/]  "
                f"[#565f89]balance:[/] [#e0af68]${credit:.2f}[/]"
            )
            self.query_one("#vdot-0", Static).update("[#9ece6a]●[/]")
        except Exception as exc:
            status_widget.update(f"[#f7768e]✗ {exc}[/]")
            self.query_one("#vdot-0", Static).update("[#f7768e]●[/]")

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
        status_widget.update("[#e0af68]checking…[/]")
        info_widget.update("")
        try:
            label, info = await _test_kaggle_api(username, key, token)
            status_widget.update("[bold #9ece6a]✓ connected[/]")
            info_widget.update(f"[#565f89]user:[/] [#c0caf5]{label}[/]  {info}")
            self.query_one(f"#vdot-{idx}", Static).update("[#9ece6a]●[/]")
            cache = getattr(self.app, "_kaggle_status_cache", None)
            if isinstance(cache, dict):
                cache.update({"kaggle_user": label, "kaggle_connected": True})
        except Exception as exc:
            status_widget.update(f"[#f7768e]✗ {exc}[/]")
            self.query_one(f"#vdot-{idx}", Static).update("[#f7768e]●[/]")
            cache = getattr(self.app, "_kaggle_status_cache", None)
            if isinstance(cache, dict):
                cache.clear()

    # ── Navigation ────────────────────────────────────────────────────────────

    def _highlight(self, idx: int) -> None:
        for i in range(len(_VENDORS)):
            row = self.query_one(f"#vrow-{i}", Horizontal)
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
            creds = dict(self._creds)
            creds.setdefault("vast", {})["api_key"] = key
            config.write_credentials(creds)
            self._creds = creds
            self._refresh_row(0)
            self.notify("vast.ai key imported from native config", severity="information")
            self.call_after_refresh(self._check_vast)

        elif vid == "kaggle":
            # 1. KAGGLE_API_TOKEN env var
            env_token = os.environ.get("KAGGLE_API_TOKEN", "").strip()
            # 2. New-style access_token file
            access_token_path = Path.home() / ".kaggle" / "access_token"
            # 3. Legacy kaggle.json
            legacy_path = Path.home() / ".kaggle" / "kaggle.json"

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
            elif legacy_path.exists():
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
            else:
                self.notify(
                    "No Kaggle credentials found. Checked: KAGGLE_API_TOKEN, "
                    "~/.kaggle/access_token, ~/.kaggle/kaggle.json",
                    severity="warning",
                )

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
        dot_style  = "#9ece6a" if configured else "#414868"
        self.query_one(f"#vdot-{idx}",    Static).update(
            f"[{dot_style}]{'●' if configured else '○'}[/]"
        )
        self.query_one(f"#vstatus-{idx}", Static).update(
            "[#9ece6a]configured[/]" if configured else "[#414868]not configured[/]"
        )
        self.query_one(f"#vinfo-{idx}",   Static).update("")

    def action_go_back(self) -> None:
        self.app.pop_screen()

    def on_screen_resume(self) -> None:
        self._creds = config.read_credentials()
        for i in range(len(_VENDORS)):
            self._refresh_row(i)
        if self._creds.get("vast", {}).get("api_key"):
            self.call_after_refresh(self._check_vast)
        if _vendor_configured(self._creds, "kaggle"):
            self.call_after_refresh(self._check_kaggle)


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
        yield Header(show_clock=True)
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
            elif event.input.id == "input-kaggle-token":
                self.query_one("#token-hint", Static).update(_masked(event.value))
                self.query_one("#test-result", Static).update("")
            elif event.input.id == "input-kaggle-key":
                self.query_one("#key-hint", Static).update(_masked(event.value))
                self.query_one("#test-result", Static).update("")
        except Exception:
            pass

    def on_button_pressed(self, event: Button.Pressed) -> None:
        match event.button.id:
            case "btn-save": self.action_save()
            case "btn-test": self.run_worker(self._do_test(), exclusive=True)
            case "btn-back": self.action_go_back()

    def action_save(self) -> None:
        creds = dict(self._creds)
        if self._vid == "kaggle":
            token    = self.query_one("#input-kaggle-token",    Input).value.strip()
            username = self.query_one("#input-kaggle-username", Input).value.strip()
            key      = self.query_one("#input-kaggle-key",      Input).value.strip()
            entry: dict = {}
            if token:
                entry["token"] = token
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
    """Test Kaggle credentials. Returns (label, info_markup) on success."""
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
            data = json.loads(r.read())
        count = len(data) if isinstance(data, list) else "?"
        return label, f"[#565f89]competitions visible:[/] [#c0caf5]{count}[/]"
    return await asyncio.to_thread(_do)


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
