from __future__ import annotations

import asyncio
import json
import urllib.request

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


# ═══════════════════════════════════════════════════════════════════════════════
#  Overview screen
# ═══════════════════════════════════════════════════════════════════════════════

class VendorsScreen(Screen):
    TITLE = "xrun — vendors"
    BINDINGS = [
        Binding("escape,q", "go_back", "Back"),
        Binding("e",        "edit",    "Edit credentials"),
        Binding("t",        "test",    "Test connection"),
        Binding("j,down",   "next",    "Down", show=False),
        Binding("k,up",     "prev",    "Up",   show=False),
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
                configured = bool(self._creds.get(vid, {}).get("api_key"))
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
                "[#565f89]Enter / e[/] [#c0caf5]Edit credentials[/]   "
                "[#565f89]t[/] [#c0caf5]Test connection[/]   "
                "[#565f89]j/k[/] [#c0caf5]Navigate[/]",
                classes="vendor-hint",
            )
        yield StatusBar()
        yield Footer()

    def on_mount(self) -> None:
        self._highlight(self._cursor)
        self.call_after_refresh(self._check_vast)

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
        else:
            self.notify(f"Test not implemented for {vid}", severity="warning")

    def action_go_back(self) -> None:
        self.app.pop_screen()

    # Refresh overview after returning from edit screen
    def on_screen_resume(self) -> None:
        self._creds = config.read_credentials()
        for i, (vid, _, _) in enumerate(_VENDORS):
            configured = bool(self._creds.get(vid, {}).get("api_key"))
            dot_style  = "#9ece6a" if configured else "#414868"
            self.query_one(f"#vdot-{i}",    Static).update(
                f"[{dot_style}]{'●' if configured else '○'}[/]"
            )
            self.query_one(f"#vstatus-{i}", Static).update(
                "[#9ece6a]configured[/]" if configured else "[#414868]not configured[/]"
            )
            self.query_one(f"#vinfo-{i}",   Static).update("")
        if self._creds.get("vast", {}).get("api_key"):
            self.call_after_refresh(self._check_vast)


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
        api_key = self._creds.get(self._vid, {}).get("api_key") or ""
        yield Header(show_clock=True)
        yield Static(f"Edit credentials — {self._vname}", classes="screen-title")
        with Vertical(id="vendor-form"):
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
            yield Static("", id="test-result", classes="form-hint")
            yield Static("", classes="form-spacer")
            with Horizontal(classes="form-actions"):
                yield Button("Save  [Ctrl+S]", id="btn-save", variant="primary")
                yield Button("Test  [Ctrl+T]", id="btn-test")
                yield Button("Back  [Esc]",    id="btn-back")
            if self._vid == "vast":
                yield Static(
                    "[#565f89]Native fallback:[/] [#7aa2f7]~/.config/vastai/vast_api_key[/]",
                    classes="form-footer-hint",
                )
        yield StatusBar()
        yield Footer()

    def on_input_changed(self, event: Input.Changed) -> None:
        if event.input.id == "input-api-key":
            self.query_one("#key-hint",    Static).update(_masked(event.value))
            self.query_one("#test-result", Static).update("")

    def on_button_pressed(self, event: Button.Pressed) -> None:
        match event.button.id:
            case "btn-save": self.action_save()
            case "btn-test": self.run_worker(self._do_test(), exclusive=True)
            case "btn-back": self.action_go_back()

    def action_save(self) -> None:
        api_key = self.query_one("#input-api-key", Input).value.strip()
        creds = dict(self._creds)
        creds.setdefault(self._vid, {})["api_key"] = api_key
        config.write_credentials(creds)
        self._creds = creds
        self.notify("Credentials saved", severity="information")

    async def action_test(self) -> None:
        await self._do_test()

    async def _do_test(self) -> None:
        api_key = self.query_one("#input-api-key", Input).value.strip()
        if not api_key:
            self.notify("Enter API key first", severity="warning")
            return
        result = self.query_one("#test-result", Static)
        result.update("[#e0af68]Testing…[/]")
        try:
            if self._vid == "vast":
                info   = await _fetch_user(api_key)
                name   = info.get("username") or info.get("email") or "unknown"
                credit = float(info.get("credit", 0))
                result.update(
                    f"[bold #9ece6a]✓ Connected[/]  "
                    f"[#565f89]user:[/] [#c0caf5]{name}[/]  "
                    f"[#565f89]balance:[/] [#e0af68]${credit:.2f}[/]"
                )
            else:
                result.update("[#565f89]Test not implemented for this vendor[/]")
        except Exception as exc:
            result.update(f"[bold #f7768e]✗ {exc}[/]")

    def action_go_back(self) -> None:
        self.app.pop_screen()


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


def _masked(key: str) -> str:
    key = key.strip()
    if not key:
        return "[#565f89]not configured[/]"
    if len(key) > 8:
        return f"[#565f89]{'*' * (len(key) - 6)}{key[-6:]}[/]"
    return "[#565f89]key set[/]"
