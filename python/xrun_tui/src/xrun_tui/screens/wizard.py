"""First-run wizard.

Four steps:

1. **Local capabilities** — show OS / GPU detected via `xrun init --probe-local`.
   Spinner while the probe runs so the screen never looks stuck.
2. **Vendors** — Checkbox per vendor (focus + Space-toggle out of the box).
   Vast/Kaggle reveal a password Input when checked. Press `o` to open the
   API-key page of the *focused* card (works before you've selected anything).
3. **Logging mode** — radio: off / polling / mirror. If mirror chosen, sinks
   appear as Checkboxes. Mirror is auto-suggested when Kaggle is selected
   (Kaggle has no live-log API otherwise).
4. **Recap** — runs `xrun doctor --json` live and prints status per check, plus
   the config path so the user knows what was written. Finishing flips
   `[ui] wizard_completed = true` via `xrun init --mark-completed`.

Open URLs explicitly with `o` (no auto-open — predictable on headless / SSH).
Esc requests a skip and asks for confirmation (single keystroke must not lose
all input).
"""
from __future__ import annotations

import json
import webbrowser

from textual.app import ComposeResult
from textual.binding import Binding
from textual.containers import Horizontal, Vertical, VerticalScroll
from textual.screen import ModalScreen, Screen
from textual.widgets import (
    Button,
    Checkbox,
    Footer,
    Input,
    Label,
    LoadingIndicator,
    RadioButton,
    RadioSet,
    Static,
)

from xrun_tui.services import _run as _xrun
from xrun_tui.services import doctor as _doctor
from xrun_tui.widgets.status_bar import StatusBar
from xrun_tui.widgets.title_bar import TitleBar

# ── Catalogue ────────────────────────────────────────────────────────────────

# (id, label, description, api_key_url, available_now, takes_paste_key)
_VENDOR_CARDS = [
    ("vast",   "vast.ai",
        "GPU spot marketplace — primary cloud vendor",
        "https://cloud.vast.ai/account/", True, True),
    ("kaggle", "Kaggle",
        "Free notebooks; live logs via MLflow mirror",
        "https://www.kaggle.com/settings/account", True, True),
    ("ssh",    "SSH machine",
        "Your own server / NAS / VPS over SSH (configure host in Vendors screen)",
        "https://www.openssh.com/manual.html", True, False),
    ("runpod", "RunPod",
        "REST + SSH cloud (v0.7)",
        "https://www.runpod.io/console/user/settings", False, False),
    ("lambda", "Lambda Labs",
        "Stable-priced GPU cloud (v0.7)",
        "https://cloud.lambdalabs.com/api-keys", False, False),
    ("lightning", "Lightning AI",
        "80 free GPU-h/mo (v0.7)",
        "https://lightning.ai/me/settings", False, False),
]

# (id, label, available_now, url)
_SINKS = [
    ("mlflow", "MLflow",  True,  "https://mlflow.org/docs/latest/tracking.html"),
    ("wandb",  "WandB",   False, "https://wandb.ai/authorize"),
    ("comet",  "Comet ML", False, "https://www.comet.com/account-settings/apiKeys"),
]

_VENDOR_BY_ID = {c[0]: c for c in _VENDOR_CARDS}
_SINK_BY_ID = {s[0]: s for s in _SINKS}

# SSH form fields shown when the SSH vendor card is checked.
# (field, label, placeholder, required)
_SSH_FIELDS = [
    ("alias", "Alias",         "myhost (used in manifests as ssh.host_alias)", True),
    ("host",  "Host",          "192.168.1.10 or vps.example.com",              True),
    ("user",  "User",          "root",                                          True),
    ("port",  "Port",          "22 (optional)",                                 False),
    ("key",   "Identity file", "~/.ssh/id_ed25519 (optional)",                  False),
]

# Kaggle form fields. Auth modes:
#   - JWT token (one field)            → kaggle.token
#   - Legacy username + key (two)      → kaggle.username + kaggle.key
# Both blank is OK — adapter auto-imports ~/.kaggle/kaggle.json at run time.
# (field, placeholder, password)
_KAGGLE_FIELDS = [
    ("token",    "JWT access token (Account → Tokens → Create new) — preferred", True),
    ("username", "or legacy username (from kaggle.json)",                         False),
    ("key",      "or legacy API key (paired with username)",                      True),
]

# Map widget-id prefix -> URL lookup, used to drive `o` from focused widget.
_FOCUS_URL_PREFIXES = {
    "wiz-vendor-cb-": lambda vid: _VENDOR_BY_ID.get(vid, (None,)*4)[3],
    "wiz-vendor-input-": lambda vid: _VENDOR_BY_ID.get(vid, (None,)*4)[3],
    "wiz-sink-cb-": lambda sid: _SINK_BY_ID.get(sid, (None,)*4)[3],
    "wiz-ssh-": lambda _f: _VENDOR_BY_ID["ssh"][3],
    "wiz-kaggle-": lambda _f: _VENDOR_BY_ID["kaggle"][3],
}


def _focus_url(widget_id: str | None) -> str | None:
    if not widget_id:
        return None
    for prefix, lookup in _FOCUS_URL_PREFIXES.items():
        if widget_id.startswith(prefix):
            return lookup(widget_id[len(prefix):])
    return None


# ── Confirm-skip modal ────────────────────────────────────────────────────────


class _ConfirmSkip(ModalScreen[bool]):
    """Asks the user to confirm wizard skip. Returns True if confirmed."""

    BINDINGS = [
        Binding("escape", "dismiss(False)", "Cancel"),
        Binding("y",      "dismiss(True)",  "Yes"),
        Binding("n",      "dismiss(False)", "No"),
    ]

    DEFAULT_CSS = """
    _ConfirmSkip {
        align: center middle;
    }
    _ConfirmSkip > Vertical {
        width: 56;
        height: auto;
        padding: 1 2;
        background: #24283b;
        border: round #7aa2f7;
    }
    _ConfirmSkip Horizontal {
        height: 3;
        align: center middle;
    }
    _ConfirmSkip Button {
        margin: 0 1;
    }
    """

    def compose(self) -> ComposeResult:
        with Vertical():
            yield Label(
                "Skip the setup wizard?\n\n"
                "Your selections so far will be discarded.\n"
                "You can re-run it any time with [b]xrun init[/].",
                markup=True,
            )
            with Horizontal():
                yield Button("Yes, skip [Y]", id="confirm-yes", variant="warning")
                yield Button("No, keep editing [N]", id="confirm-no", variant="primary")

    def on_button_pressed(self, event: Button.Pressed) -> None:
        self.dismiss(event.button.id == "confirm-yes")


# ── Wizard screen ─────────────────────────────────────────────────────────────


class WizardScreen(Screen):
    TITLE = "xrun — first-run wizard"
    BINDINGS = [
        Binding("escape", "skip_request", "Skip"),
        Binding("ctrl+n", "next",         "Next"),
        Binding("ctrl+b", "back",         "Back"),
        Binding("o",      "open_url",     "Open URL"),
    ]

    def __init__(self) -> None:
        super().__init__()
        self._step = 0
        self._n_steps = 4
        self._probe: dict = {}
        self._probe_done = False
        self._selected_vendors: set[str] = set()
        self._pasted_keys: dict[str, str] = {}
        self._log_mode = "polling"
        self._selected_sinks: set[str] = {"mlflow"}
        self._ssh_fields: dict[str, str] = {}
        self._kaggle_fields: dict[str, str] = {}
        self._doctor_loaded = False

    # ── Composition ───────────────────────────────────────────────────────────

    def compose(self) -> ComposeResult:
        yield TitleBar("first-run wizard")
        yield Static("xrun — Setup", classes="screen-title")
        yield Static(self._stepper_markup(), id="wizard-stepper",
                     classes="wizard-stepper")
        with Vertical(id="wizard-body"):
            pass
        with Horizontal(id="wizard-actions", classes="form-actions"):
            yield Button("Back  [Ctrl+B]", id="btn-back")
            yield Button("Skip  [Esc]",    id="btn-skip")
            yield Button("Next  [Ctrl+N]", id="btn-next", variant="primary")
        yield StatusBar()
        yield Footer()

    async def on_mount(self) -> None:
        await self._render_step()
        self.run_worker(self._do_probe(), exclusive=True, group="probe")

    def _stepper_markup(self) -> str:
        labels = ["Local", "Vendors", "Logging", "Done"]
        cells = []
        for i, lbl in enumerate(labels):
            if i < self._step:
                cells.append(f"[#9ece6a]✓ {lbl}[/]")
            elif i == self._step:
                cells.append(f"[bold #7aa2f7]● {i+1}/{self._n_steps} {lbl}[/]")
            else:
                cells.append(f"[#414868]○ {lbl}[/]")
        return "  →  ".join(cells)

    # ── Background loaders ────────────────────────────────────────────────────

    async def _do_probe(self) -> None:
        code, out, _ = await _xrun("init", "--probe-local", "--json")
        if code == 0:
            try:
                self._probe = json.loads(out)
            except Exception:
                self._probe = {}
        self._probe_done = True
        if self._step == 0:
            await self._render_step()

    # ── Step rendering ────────────────────────────────────────────────────────

    async def _render_step(self) -> None:
        self.query_one("#wizard-stepper", Static).update(self._stepper_markup())
        body = self.query_one("#wizard-body", Vertical)
        await body.remove_children()

        if self._step == 0:
            await self._render_local(body)
        elif self._step == 1:
            await self._render_vendors(body)
        elif self._step == 2:
            await self._render_logging(body)
        elif self._step == 3:
            await self._render_recap(body)

        next_btn = self.query_one("#btn-next", Button)
        back_btn = self.query_one("#btn-back", Button)
        back_btn.disabled = self._step == 0
        next_btn.label = ("Finish  [Ctrl+N]"
                          if self._step == self._n_steps - 1
                          else "Next  [Ctrl+N]")

    async def _render_local(self, body: Vertical) -> None:
        if not self._probe_done:
            await body.mount(Static(
                "[bold #c0caf5]Step 1 — Local capabilities[/]\n\n"
                "[#565f89]Detecting OS and GPU…[/]",
                classes="wizard-text",
            ))
            await body.mount(LoadingIndicator())
            return

        gpus = self._probe.get("gpus", [])
        os_str = self._probe.get("os", "?")
        arch = self._probe.get("arch", "?")
        lines = [
            "[bold #c0caf5]Step 1 — Local capabilities[/]",
            "",
            f"[#565f89]OS:[/]   [#c0caf5]{os_str}[/]  [#414868]({arch})[/]",
        ]
        if gpus:
            lines.append(f"[#565f89]GPU:[/]  [#9ece6a]{len(gpus)} detected[/]")
            for g in gpus:
                lines.append(f"       • {g}")
            lines.append("")
            lines.append("[#9ece6a]Local vendor available[/] — you can run smoke "
                         "tests and small jobs without any cloud account.")
        else:
            lines.append(f"[#565f89]GPU:[/]  [#e0af68]none detected[/] — "
                         "CPU-only, fine for smoke tests.")
            lines.append("")
            lines.append("Add a cloud vendor on the next step to actually train.")
        await body.mount(Static("\n".join(lines), classes="wizard-text"))

    async def _render_vendors(self, body: Vertical) -> None:
        await body.mount(Static(
            "[bold #c0caf5]Step 2 — Vendors[/]\n\n"
            "[#565f89]Tab to move between cards · Space to toggle · "
            "[/][bold]o[/][#565f89] opens the API-key page of the focused card.[/]",
            classes="wizard-text",
        ))
        scroll = VerticalScroll(id="wiz-vendor-scroll")
        await body.mount(scroll)
        for vid, label, desc, _url, available, takes_key in _VENDOR_CARDS:
            row = Vertical(classes="wizard-vendor-row")
            await scroll.mount(row)
            badge = "" if available else "  [#e0af68][v0.7+][/]"
            cb = Checkbox(
                f"[bold]{label}[/]{badge}  [#565f89]{desc}[/]",
                value=(vid in self._selected_vendors and available),
                id=f"wiz-vendor-cb-{vid}",
                disabled=not available,
            )
            await row.mount(cb)
            if vid == "kaggle":
                kform = Vertical(id="wiz-kaggle-form", classes="wizard-ssh-form")
                kform.display = vid in self._selected_vendors
                await row.mount(kform)
                await kform.mount(Static(
                    "[#565f89]Either fill the JWT token, or username + API key. "
                    "Leave blank if [/][bold]~/.kaggle/kaggle.json[/][#565f89] is "
                    "already on this machine — xrun imports it automatically.[/]",
                    classes="wizard-text",
                ))
                await kform.mount(Static(
                    " Get token ↗  Open Kaggle Account → Tokens ",
                    id=f"wiz-open-{vid}",
                    classes="wizard-link-btn",
                ))
                for field, placeholder, password in _KAGGLE_FIELDS:
                    finp = Input(
                        value=self._kaggle_fields.get(field, ""),
                        placeholder=placeholder,
                        password=password,
                        id=f"wiz-kaggle-{field}",
                        classes="wizard-input",
                    )
                    await kform.mount(finp)
            elif takes_key:
                vform = Vertical(id=f"wiz-vendor-form-{vid}", classes="wizard-ssh-form")
                vform.display = vid in self._selected_vendors
                await row.mount(vform)
                await vform.mount(Static(
                    f" Get key ↗  Open {label} API-key page ",
                    id=f"wiz-open-{vid}",
                    classes="wizard-link-btn",
                ))
                inp = Input(
                    value=self._pasted_keys.get(vid, ""),
                    password=True,
                    placeholder=f"Paste {label} API key (optional — can also set later)",
                    id=f"wiz-vendor-input-{vid}",
                    classes="wizard-input",
                )
                await vform.mount(inp)
            elif vid == "ssh":
                ssh_form = Vertical(id="wiz-ssh-form", classes="wizard-ssh-form")
                ssh_form.display = vid in self._selected_vendors
                await row.mount(ssh_form)
                for field, _flbl, placeholder, _req in _SSH_FIELDS:
                    finp = Input(
                        value=self._ssh_fields.get(field, ""),
                        placeholder=placeholder,
                        id=f"wiz-ssh-{field}",
                        classes="wizard-input",
                    )
                    await ssh_form.mount(finp)

    async def _render_logging(self, body: Vertical) -> None:
        kaggle_selected = "kaggle" in self._selected_vendors
        await body.mount(Static(
            "[bold #c0caf5]Step 3 — Live logging[/]\n\n"
            "[#565f89]How should xrun stream events and metrics during a run?"
            + (" [#e0af68]Kaggle is selected — pick 'mirror' for live logs.[/]"
               if kaggle_selected else "")
            + "[/]",
            classes="wizard-text",
        ))
        rs = RadioSet(
            RadioButton(
                "Off — local SQLite only (no live updates in TUI)",
                value=self._log_mode == "off",
                id="wiz-mode-off",
            ),
            RadioButton(
                "Polling — TUI tails JSONL from instance (vast / ssh / local). Default.",
                value=self._log_mode == "polling",
                id="wiz-mode-polling",
            ),
            RadioButton(
                "Polling + mirror to backend — required for Kaggle live logs",
                value=self._log_mode == "mirror",
                id="wiz-mode-mirror",
            ),
            id="wiz-mode-radio",
        )
        await body.mount(rs)

        sinks_box = Vertical(id="wiz-sinks-box")
        await body.mount(sinks_box)
        if self._log_mode == "mirror":
            await self._mount_sinks(sinks_box)

    async def _mount_sinks(self, container: Vertical) -> None:
        await container.remove_children()
        await container.mount(Static(
            "\n[bold #c0caf5]Mirror sinks[/]  "
            "[#565f89]Press [/][bold]o[/][#565f89] to open the API-key page "
            "of the focused sink.[/]",
            classes="wizard-text",
        ))
        for sid, label, available, _url in _SINKS:
            badge = "" if available else "  [#e0af68][v0.8][/]"
            cb = Checkbox(
                f"[bold]{label}[/]{badge}",
                value=(sid in self._selected_sinks and available),
                id=f"wiz-sink-cb-{sid}",
                disabled=not available,
            )
            await container.mount(cb)

    async def _render_recap(self, body: Vertical) -> None:
        sinks = sorted(self._selected_sinks) if self._log_mode == "mirror" else []
        active_vendors = [v for v in self._selected_vendors
                          if _VENDOR_BY_ID.get(v, (None,)*5)[4]]
        keys_set = [v for v, k in self._pasted_keys.items() if k.strip()]
        if "kaggle" in self._selected_vendors:
            ktok = self._kaggle_fields.get("token", "")
            kusr = self._kaggle_fields.get("username", "")
            kkey = self._kaggle_fields.get("key", "")
            if ktok:
                keys_set.append("kaggle (token)")
            elif kusr and kkey:
                keys_set.append(f"kaggle (legacy, {kusr})")
            elif "kaggle" in self._selected_vendors:
                keys_set.append("kaggle (auto-import from ~/.kaggle/)")
        ssh_line = "[#414868]not configured[/]"
        if "ssh" in self._selected_vendors and self._ssh_fields.get("alias"):
            a = self._ssh_fields["alias"]
            u = self._ssh_fields.get("user", "?")
            h = self._ssh_fields.get("host", "?")
            ssh_line = f"[#9ece6a]{a}[/] → [#c0caf5]{u}@{h}[/]"
        gpu_line = (f"[#9ece6a]{len(self._probe.get('gpus', []))} detected[/]"
                    if self._probe.get("gpus") else "[#e0af68]none[/]")
        lines = [
            "[bold #c0caf5]Step 4 — Recap[/]",
            "",
            f"[#565f89]Local GPU:[/]      {gpu_line}",
            f"[#565f89]Vendors:[/]        "
            + (", ".join(f"[#9ece6a]{v}[/]" for v in active_vendors)
               if active_vendors else "[#414868]none[/]"),
            f"[#565f89]Keys staged:[/]    "
            + (", ".join(f"[#9ece6a]{v}[/]" for v in keys_set)
               if keys_set else "[#414868]none[/]"),
            f"[#565f89]SSH host:[/]       {ssh_line}",
            f"[#565f89]Logging mode:[/]   [#c0caf5]{self._log_mode}[/]",
            f"[#565f89]Sinks:[/]          "
            + (", ".join(f"[#9ece6a]{s}[/]" for s in sinks)
               if sinks else "[#414868]none[/]"),
            "",
            "[#565f89]Pressing [/][bold]Finish[/][#565f89] writes the config and "
            "marks the wizard as done. Re-run any time with[/] [bold]xrun init[/][#565f89].[/]",
            "",
            "[bold #c0caf5]Environment check (xrun doctor)[/]",
        ]
        await body.mount(Static("\n".join(lines), classes="wizard-text"))
        await body.mount(LoadingIndicator(id="wiz-doctor-spinner"))
        self._doctor_loaded = False
        self.run_worker(self._fill_doctor(body), exclusive=True, group="doctor")

    async def _fill_doctor(self, body: Vertical) -> None:
        ok, data, err = await _doctor()
        # Replace spinner with results.
        try:
            spinner = self.query_one("#wiz-doctor-spinner", LoadingIndicator)
        except Exception:
            return  # step changed
        await spinner.remove()
        if not ok:
            await body.mount(Static(
                f"[#f7768e]doctor failed:[/] {err or 'unknown error'}",
                classes="wizard-text",
            ))
            return
        rows = []
        for entry in data if isinstance(data, list) else []:
            name = entry.get("check", "?")
            status = entry.get("status", "?")
            detail = entry.get("detail", "")
            glyph = {"OK": "[#9ece6a]✓[/]",
                     "WARN": "[#e0af68]⚠[/]",
                     "FAIL": "[#f7768e]✗[/]"}.get(status, "·")
            rows.append(f"  {glyph} [bold]{name:<22}[/] [#565f89]{detail}[/]")
        if not rows:
            rows.append("[#414868]  (no checks reported)[/]")
        await body.mount(Static("\n".join(rows), classes="wizard-text"))
        self._doctor_loaded = True

    # ── Interaction ───────────────────────────────────────────────────────────

    def on_button_pressed(self, event: Button.Pressed) -> None:
        bid = event.button.id or ""
        if bid == "btn-next":
            self.run_worker(self._next(), exclusive=True)
        elif bid == "btn-back":
            self.run_worker(self._back(), exclusive=True)
        elif bid == "btn-skip":
            self.run_worker(self._skip_request(), exclusive=True)

    def on_click(self, event) -> None:  # type: ignore[override]
        # Static-based "Get token" link buttons. Walk up to find the wiz-open-* id.
        widget = event.widget
        for _ in range(4):
            if widget is None:
                return
            wid = getattr(widget, "id", "") or ""
            if wid.startswith("wiz-open-"):
                vid = wid[len("wiz-open-"):]
                url = _VENDOR_BY_ID.get(vid, (None,)*4)[3]
                if not url:
                    return
                try:
                    opened = webbrowser.open(url)
                except Exception:
                    opened = False
                if not opened:
                    self.notify(f"Open this URL in your browser:\n{url}",
                                title="Open URL", timeout=15)
                return
            widget = widget.parent

    async def action_next(self) -> None:
        await self._next()

    async def action_back(self) -> None:
        await self._back()

    async def action_skip_request(self) -> None:
        await self._skip_request()

    async def action_open_url(self) -> None:
        focused = self.focused
        url = _focus_url(getattr(focused, "id", None))
        if url is None:
            self.notify(
                "Tab to a vendor or sink card first, then press o.",
                severity="warning",
            )
            return
        try:
            opened = webbrowser.open(url)
        except Exception:
            opened = False
        if not opened:
            self.notify(f"Open this URL in your browser:\n{url}",
                        title="Open URL", timeout=15)

    # Checkbox toggles do NOT rebuild the body — only the affected widget changes.
    def on_checkbox_changed(self, event: Checkbox.Changed) -> None:
        wid = event.checkbox.id or ""
        if wid.startswith("wiz-vendor-cb-"):
            vid = wid[len("wiz-vendor-cb-"):]
            if event.value:
                self._selected_vendors.add(vid)
            else:
                self._selected_vendors.discard(vid)
            # Reveal/hide the per-vendor form (button + input). No rebuild.
            try:
                vform = self.query_one(f"#wiz-vendor-form-{vid}", Vertical)
                vform.display = event.value
                if not event.value:
                    try:
                        inp = self.query_one(f"#wiz-vendor-input-{vid}", Input)
                        inp.value = ""
                    except Exception:
                        pass
                    self._pasted_keys.pop(vid, None)
            except Exception:
                pass
            # Same for the SSH form, when present.
            if vid == "ssh":
                try:
                    form = self.query_one("#wiz-ssh-form", Vertical)
                    form.display = event.value
                    if not event.value:
                        for field, _, _, _ in _SSH_FIELDS:
                            try:
                                fi = self.query_one(f"#wiz-ssh-{field}", Input)
                                fi.value = ""
                            except Exception:
                                pass
                        self._ssh_fields.clear()
                except Exception:
                    pass
            if vid == "kaggle":
                try:
                    form = self.query_one("#wiz-kaggle-form", Vertical)
                    form.display = event.value
                    if not event.value:
                        for field, _, _ in _KAGGLE_FIELDS:
                            try:
                                fi = self.query_one(f"#wiz-kaggle-{field}", Input)
                                fi.value = ""
                            except Exception:
                                pass
                        self._kaggle_fields.clear()
                except Exception:
                    pass
        elif wid.startswith("wiz-sink-cb-"):
            sid = wid[len("wiz-sink-cb-"):]
            if event.value:
                self._selected_sinks.add(sid)
            else:
                self._selected_sinks.discard(sid)

    def on_radio_set_changed(self, event: RadioSet.Changed) -> None:
        if event.radio_set.id != "wiz-mode-radio":
            return
        idx = event.radio_set.pressed_index
        new_mode = ["off", "polling", "mirror"][idx]
        if new_mode == self._log_mode:
            return
        self._log_mode = new_mode
        # Mount or clear the sinks box without rebuilding the whole step.
        try:
            box = self.query_one("#wiz-sinks-box", Vertical)
        except Exception:
            return
        if new_mode == "mirror":
            self.run_worker(self._mount_sinks(box), exclusive=True, group="sinks")
        else:
            self.run_worker(box.remove_children(), exclusive=True, group="sinks")

    def on_input_changed(self, event: Input.Changed) -> None:
        wid = event.input.id or ""
        if wid.startswith("wiz-vendor-input-"):
            vid = wid[len("wiz-vendor-input-"):]
            self._pasted_keys[vid] = event.value.strip()
        elif wid.startswith("wiz-ssh-"):
            field = wid[len("wiz-ssh-"):]
            v = event.value.strip()
            if v:
                self._ssh_fields[field] = v
            else:
                self._ssh_fields.pop(field, None)
        elif wid.startswith("wiz-kaggle-"):
            field = wid[len("wiz-kaggle-"):]
            v = event.value.strip()
            if v:
                self._kaggle_fields[field] = v
            else:
                self._kaggle_fields.pop(field, None)

    # ── Step transitions ──────────────────────────────────────────────────────

    async def _back(self) -> None:
        if self._step == 0:
            return
        self._step -= 1
        await self._render_step()

    async def _next(self) -> None:
        # Validate step 2 (Vendors) before leaving it.
        if self._step == 1 and "kaggle" in self._selected_vendors:
            tok = self._kaggle_fields.get("token", "")
            usr = self._kaggle_fields.get("username", "")
            kk  = self._kaggle_fields.get("key", "")
            if (usr and not kk) or (kk and not usr):
                self.notify(
                    "Kaggle legacy auth needs both username AND key. "
                    "Or use the JWT token field. Or leave all blank to use ~/.kaggle/kaggle.json.",
                    severity="warning", timeout=10,
                )
                return
            if tok and (usr or kk):
                self.notify(
                    "Kaggle: pick ONE — JWT token OR username+key, not both.",
                    severity="warning", timeout=8,
                )
                return

        if self._step == 1 and "ssh" in self._selected_vendors:
            missing = [
                lbl for f, lbl, _ph, req in _SSH_FIELDS
                if req and not self._ssh_fields.get(f)
            ]
            if missing:
                self.notify(
                    f"SSH needs: {', '.join(missing)}. "
                    "Untick SSH if you want to skip.",
                    severity="warning",
                    timeout=8,
                )
                return
            alias = self._ssh_fields.get("alias", "")
            if not all(c.isalnum() or c in "-_" for c in alias):
                self.notify(
                    "SSH alias must be alphanumeric (plus - or _).",
                    severity="warning",
                )
                return

        if self._step < self._n_steps - 1:
            self._step += 1
            await self._render_step()
        else:
            await self._finish()

    async def _skip_request(self) -> None:
        # Don't burn the user's pasted secrets if they hit Esc by accident.
        confirmed = await self.app.push_screen_wait(_ConfirmSkip())
        if not confirmed:
            return
        await _xrun("init", "--non-interactive", "--mark-completed")
        await self._exit_to_dashboard("Wizard skipped — re-run any time with `xrun init`.")

    async def _finish(self) -> None:
        # Persist pasted credentials. We use `xrun config set` per key (keys are
        # short strings; multiple sequential calls are fine and avoid the
        # one-shot stdin limit of `xrun init --vast-key=-`).
        for vid, key in self._pasted_keys.items():
            if not key:
                continue
            if vid == "vast":
                await _xrun("config", "set", "vast.api_key", key)

        # Kaggle: write whichever auth path the user picked.
        if "kaggle" in self._selected_vendors:
            tok = self._kaggle_fields.get("token", "")
            usr = self._kaggle_fields.get("username", "")
            kk  = self._kaggle_fields.get("key", "")
            if tok:
                await _xrun("config", "set", "kaggle.token", tok)
            elif usr and kk:
                await _xrun("config", "set", "kaggle.username", usr)
                await _xrun("config", "set", "kaggle.key", kk)

        # SSH host: write each non-empty field via `xrun config set ssh.<alias>.<field>`.
        if "ssh" in self._selected_vendors and self._ssh_fields.get("alias"):
            alias = self._ssh_fields["alias"]
            for field in ("host", "user", "port", "key"):
                val = self._ssh_fields.get(field)
                if val:
                    await _xrun("config", "set", f"ssh.{alias}.{field}", val)

        sinks = ([s for s in self._selected_sinks
                  if _SINK_BY_ID.get(s, (None,)*3)[2]]
                 if self._log_mode == "mirror" else [])
        args = ["init", "--non-interactive", "--mark-completed"]
        for s in sinks:
            args += ["--sink", s]
        code, _, err = await _xrun(*args)
        if code != 0:
            self.notify(f"Failed to save config: {err}", severity="error", timeout=10)
            return
        await self._exit_to_dashboard("Setup complete.")

    async def _exit_to_dashboard(self, msg: str) -> None:
        from xrun_tui.screens.dashboard import DashboardScreen
        self.notify(msg, severity="information", timeout=6)
        await self.app.switch_screen(DashboardScreen())
