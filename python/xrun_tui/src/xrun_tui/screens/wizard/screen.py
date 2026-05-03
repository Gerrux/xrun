"""First-run wizard screen — state, event wiring, transitions, persistence.

Four steps:

1. **Local capabilities** — show OS / GPU detected via `xrun init --probe-local`.
   Spinner while the probe runs so the screen never looks stuck.
2. **Vendors** — Checkbox per vendor. Vast/Kaggle reveal a password Input when
   checked; SSH reveals an alias/host/user form. Press `o` to open the API-key
   page of the focused card.
3. **Logging mode** — radio: off / polling / mirror. If mirror chosen, sinks
   appear; MLflow reveals a tracking-URL + auth form. Mirror is auto-suggested
   when Kaggle is selected (Kaggle has no live-log API otherwise).
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
from textual.screen import Screen
from textual.widgets import Button, Checkbox, Footer, Input, RadioSet, Static

from xrun_tui import config as _config
from xrun_tui.screens.wizard import steps as _steps
from xrun_tui.screens.wizard.catalog import (
    KAGGLE_FIELDS,
    MLFLOW_FIELDS,
    SINK_BY_ID,
    SSH_FIELDS,
    VENDOR_BY_ID,
    focus_url,
)
from xrun_tui.screens.wizard.modals import ConfirmSkip
from xrun_tui.services import _run as _xrun
from xrun_tui.widgets.status_bar import StatusBar
from xrun_tui.widgets.title_bar import TitleBar


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
        self._mlflow_fields: dict[str, str] = {}
        self._doctor_loaded = False
        self._probe_results: list[dict] = []
        # Vendors / sinks that already have credentials on disk. Rendered as
        # "● configured" badges so re-running `xrun init` shows the user what
        # is already set up — without echoing secrets back into the form.
        self._existing_vendors: set[str] = set()
        self._existing_sinks: set[str] = set()
        self._existing_ssh_alias: str | None = None
        self._wizard_was_completed = False
        self._load_existing_state()

    def _load_existing_state(self) -> None:
        """Pre-populate wizard state from already-saved config + credentials.

        Non-secret fields (SSH host/user/port/key path, MLflow URL, Kaggle
        legacy username) are filled into inputs so the user can see / edit
        them. Secrets (API keys, tokens, passwords) stay blank — the existing
        value is preserved on save unless the user types a new one. We only
        record which vendors/sinks are configured, for badge rendering.
        """
        creds = _config.read_credentials()
        cfg = _config.read_global_config()
        self._wizard_was_completed = bool(cfg.get("ui", {}).get("wizard_completed", False))

        if creds.get("vast", {}).get("api_key"):
            self._existing_vendors.add("vast")
            self._selected_vendors.add("vast")

        kag = creds.get("kaggle", {}) or {}
        if kag.get("token") or (kag.get("username") and kag.get("key")):
            self._existing_vendors.add("kaggle")
            self._selected_vendors.add("kaggle")
            if usr := kag.get("username"):
                self._kaggle_fields["username"] = usr

        ssh_section = creds.get("ssh", {}) or {}
        # First [ssh.<alias>] block wins — wizard handles a single host.
        for alias, data in ssh_section.items():
            if isinstance(data, dict) and data.get("host") and data.get("user"):
                self._existing_vendors.add("ssh")
                self._existing_ssh_alias = alias
                self._selected_vendors.add("ssh")
                self._ssh_fields["alias"] = alias
                for field in ("host", "user", "port", "key"):
                    if v := data.get(field):
                        self._ssh_fields[field] = str(v)
                break

        if mlflow_url := cfg.get("mlflow", {}).get("url"):
            self._mlflow_fields["url"] = mlflow_url
            self._existing_sinks.add("mlflow")

        sinks = cfg.get("metrics", {}).get("sinks") or []
        if sinks:
            self._selected_sinks = set(sinks)
            self._log_mode = "mirror"
        elif self._wizard_was_completed:
            # Saved config explicitly chose no sinks → polling.
            self._selected_sinks = set()

    # ── Composition ───────────────────────────────────────────────────────────

    def compose(self) -> ComposeResult:
        yield TitleBar("first-run wizard")
        yield Static("xrun — Setup", classes="screen-title")
        yield Static(_steps.stepper_markup(self._step, self._n_steps),
                     id="wizard-stepper", classes="wizard-stepper")
        with VerticalScroll(id="wizard-body"):
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
        self.query_one("#wizard-stepper", Static).update(
            _steps.stepper_markup(self._step, self._n_steps),
        )
        body = self.query_one("#wizard-body", VerticalScroll)
        await body.remove_children()

        if self._step == 0:
            await _steps.render_local(self, body)
        elif self._step == 1:
            await _steps.render_vendors(self, body)
        elif self._step == 2:
            await _steps.render_logging(self, body)
        elif self._step == 3:
            await _steps.render_recap(self, body)

        next_btn = self.query_one("#btn-next", Button)
        back_btn = self.query_one("#btn-back", Button)
        back_btn.disabled = self._step == 0
        next_btn.label = ("Finish  [Ctrl+N]"
                          if self._step == self._n_steps - 1
                          else "Next  [Ctrl+N]")

    # ── Buttons & key bindings ────────────────────────────────────────────────

    def on_button_pressed(self, event: Button.Pressed) -> None:
        bid = event.button.id or ""
        if bid == "btn-next":
            self.run_worker(self._next(), exclusive=True)
        elif bid == "btn-back":
            self.run_worker(self._back(), exclusive=True)
        elif bid == "btn-skip":
            self.run_worker(self._skip_request(), exclusive=True)

    def on_click(self, event) -> None:  # type: ignore[override]
        # Static-based "Get token" / "Docs" link buttons. Walk up to find
        # the wiz-open-* id. Vendors and sinks both use the same prefix.
        widget = event.widget
        for _ in range(4):
            if widget is None:
                return
            wid = getattr(widget, "id", "") or ""
            if wid.startswith("wiz-open-"):
                key = wid[len("wiz-open-"):]
                url = self._link_url(key)
                if url:
                    self._open_external(url)
                return
            widget = widget.parent

    @staticmethod
    def _link_url(key: str) -> str | None:
        if key in VENDOR_BY_ID:
            return VENDOR_BY_ID[key][3]
        if key in SINK_BY_ID:
            return SINK_BY_ID[key][3]
        return None

    def _open_external(self, url: str) -> None:
        try:
            opened = webbrowser.open(url)
        except Exception:
            opened = False
        if not opened:
            self.notify(f"Open this URL in your browser:\n{url}",
                        title="Open URL", timeout=15)

    async def action_next(self) -> None:
        await self._next()

    async def action_back(self) -> None:
        await self._back()

    async def action_skip_request(self) -> None:
        await self._skip_request()

    async def action_open_url(self) -> None:
        url = focus_url(getattr(self.focused, "id", None))
        if url is None:
            self.notify(
                "Tab to a vendor or sink card first, then press o.",
                severity="warning",
            )
            return
        self._open_external(url)

    # ── Live state mutations (no rebuild) ────────────────────────────────────

    def on_checkbox_changed(self, event: Checkbox.Changed) -> None:
        wid = event.checkbox.id or ""
        if wid.startswith("wiz-vendor-cb-"):
            self._toggle_vendor(wid[len("wiz-vendor-cb-"):], event.value)
        elif wid.startswith("wiz-sink-cb-"):
            self._toggle_sink(wid[len("wiz-sink-cb-"):], event.value)

    def _toggle_vendor(self, vid: str, on: bool) -> None:
        if on:
            self._selected_vendors.add(vid)
        else:
            self._selected_vendors.discard(vid)
        # Reveal/hide the per-vendor key form.
        try:
            vform = self.query_one(f"#wiz-vendor-form-{vid}", Vertical)
            vform.display = on
            if not on:
                try:
                    inp = self.query_one(f"#wiz-vendor-input-{vid}", Input)
                    inp.value = ""
                except Exception:
                    pass
                self._pasted_keys.pop(vid, None)
        except Exception:
            pass
        if vid == "ssh":
            self._reveal_form("#wiz-ssh-form", on,
                              [f"#wiz-ssh-{f}" for f, *_ in SSH_FIELDS],
                              self._ssh_fields)
        if vid == "kaggle":
            self._reveal_form("#wiz-kaggle-form", on,
                              [f"#wiz-kaggle-{f}" for f, *_ in KAGGLE_FIELDS],
                              self._kaggle_fields)

    def _toggle_sink(self, sid: str, on: bool) -> None:
        if on:
            self._selected_sinks.add(sid)
        else:
            self._selected_sinks.discard(sid)
        if sid == "mlflow":
            self._reveal_form("#wiz-mlflow-form", on,
                              [f"#wiz-mlflow-{f}" for f, *_ in MLFLOW_FIELDS],
                              self._mlflow_fields)

    def _reveal_form(
        self,
        form_selector: str,
        on: bool,
        input_selectors: list[str],
        state: dict[str, str],
    ) -> None:
        """Show/hide a sub-form. When hiding, also blank inputs and clear state."""
        try:
            form = self.query_one(form_selector, Vertical)
        except Exception:
            return
        form.display = on
        if on:
            return
        for sel in input_selectors:
            try:
                self.query_one(sel, Input).value = ""
            except Exception:
                pass
        state.clear()

    def on_radio_set_changed(self, event: RadioSet.Changed) -> None:
        if event.radio_set.id != "wiz-mode-radio":
            return
        idx = event.radio_set.pressed_index
        new_mode = ["off", "polling", "mirror"][idx]
        if new_mode == self._log_mode:
            return
        self._log_mode = new_mode
        try:
            box = self.query_one("#wiz-sinks-box", Vertical)
        except Exception:
            return
        if new_mode == "mirror":
            self.run_worker(_steps.mount_sinks(self, box),
                            exclusive=True, group="sinks")
        else:
            self.run_worker(box.remove_children(),
                            exclusive=True, group="sinks")

    def on_input_changed(self, event: Input.Changed) -> None:
        wid = event.input.id or ""
        v = event.value.strip()
        if wid.startswith("wiz-vendor-input-"):
            vid = wid[len("wiz-vendor-input-"):]
            self._pasted_keys[vid] = v
        elif wid.startswith("wiz-ssh-"):
            self._set_or_pop(self._ssh_fields, wid[len("wiz-ssh-"):], v)
        elif wid.startswith("wiz-kaggle-"):
            self._set_or_pop(self._kaggle_fields, wid[len("wiz-kaggle-"):], v)
        elif wid.startswith("wiz-mlflow-"):
            self._set_or_pop(self._mlflow_fields, wid[len("wiz-mlflow-"):], v)

    @staticmethod
    def _set_or_pop(d: dict[str, str], k: str, v: str) -> None:
        if v:
            d[k] = v
        else:
            d.pop(k, None)

    # ── Step transitions ──────────────────────────────────────────────────────

    async def _back(self) -> None:
        if self._step == 0:
            return
        self._step -= 1
        await self._render_step()

    async def _next(self) -> None:
        if self._step == 1 and not self._validate_vendors():
            return
        if self._step == 2 and not self._validate_logging():
            return

        if self._step < self._n_steps - 1:
            self._step += 1
            await self._render_step()
        else:
            await self._finish()

    def _validate_vendors(self) -> bool:
        if "kaggle" in self._selected_vendors:
            tok = self._kaggle_fields.get("token", "")
            usr = self._kaggle_fields.get("username", "")
            kk  = self._kaggle_fields.get("key", "")
            if (usr and not kk) or (kk and not usr):
                self.notify(
                    "Kaggle legacy auth needs both username AND key. "
                    "Or use the JWT token field. Or leave all blank to use ~/.kaggle/kaggle.json.",
                    severity="warning", timeout=10,
                )
                return False
            if tok and (usr or kk):
                self.notify(
                    "Kaggle: pick ONE — JWT token OR username+key, not both.",
                    severity="warning", timeout=8,
                )
                return False

        if "ssh" in self._selected_vendors:
            missing = [
                lbl for f, lbl, _ph, req in SSH_FIELDS
                if req and not self._ssh_fields.get(f)
            ]
            if missing:
                self.notify(
                    f"SSH needs: {', '.join(missing)}. "
                    "Untick SSH if you want to skip.",
                    severity="warning", timeout=8,
                )
                return False
            alias = self._ssh_fields.get("alias", "")
            if not all(c.isalnum() or c in "-_" for c in alias):
                self.notify(
                    "SSH alias must be alphanumeric (plus - or _).",
                    severity="warning",
                )
                return False
        return True

    def _validate_logging(self) -> bool:
        if self._log_mode != "mirror" or "mlflow" not in self._selected_sinks:
            return True
        url = self._mlflow_fields.get("url", "")
        if not url:
            self.notify(
                "MLflow tracking URL is required when 'Mirror' is on. "
                "Untick MLflow or switch back to 'Polling' to skip.",
                severity="warning", timeout=10,
            )
            return False
        if not (url.startswith("http://") or url.startswith("https://")):
            self.notify(
                "MLflow URL must start with http:// or https://.",
                severity="warning", timeout=8,
            )
            return False
        tok = self._mlflow_fields.get("token", "")
        usr = self._mlflow_fields.get("username", "")
        pwd = self._mlflow_fields.get("password", "")
        if tok and (usr or pwd):
            self.notify(
                "MLflow: pick ONE — Bearer token OR username+password, not both.",
                severity="warning", timeout=8,
            )
            return False
        if (usr and not pwd) or (pwd and not usr):
            self.notify(
                "MLflow Basic auth needs BOTH username and password.",
                severity="warning", timeout=8,
            )
            return False
        return True

    async def _skip_request(self) -> None:
        confirmed = await self.app.push_screen_wait(ConfirmSkip())
        if not confirmed:
            return
        await _xrun("init", "--non-interactive", "--mark-completed")
        await self._exit_to_dashboard("Wizard skipped — re-run any time with `xrun init`.")

    # ── Persistence ───────────────────────────────────────────────────────────

    async def _finish(self) -> None:
        # Vendor API keys.
        for vid, key in self._pasted_keys.items():
            if not key:
                continue
            if vid == "vast":
                await _xrun("config", "set", "vast.api_key", key)

        # Kaggle: token OR legacy user+key.
        if "kaggle" in self._selected_vendors:
            tok = self._kaggle_fields.get("token", "")
            usr = self._kaggle_fields.get("username", "")
            kk  = self._kaggle_fields.get("key", "")
            if tok:
                await _xrun("config", "set", "kaggle.token", tok)
            elif usr and kk:
                await _xrun("config", "set", "kaggle.username", usr)
                await _xrun("config", "set", "kaggle.key", kk)

        # SSH host fields → ssh.<alias>.<field>.
        if "ssh" in self._selected_vendors and self._ssh_fields.get("alias"):
            alias = self._ssh_fields["alias"]
            for field in ("host", "user", "port", "key"):
                val = self._ssh_fields.get(field)
                if val:
                    await _xrun("config", "set", f"ssh.{alias}.{field}", val)

        # MLflow URL + auth (only if mirror is actually on).
        sinks = ([s for s in self._selected_sinks
                  if SINK_BY_ID.get(s, (None,) * 3)[2]]
                 if self._log_mode == "mirror" else [])
        if "mlflow" in sinks:
            await self._persist_mlflow()

        args = ["init", "--non-interactive", "--mark-completed"]
        for s in sinks:
            args += ["--sink", s]
        code, _, err = await _xrun(*args)
        if code != 0:
            self.notify(f"Failed to save config: {err}",
                        severity="error", timeout=10)
            return
        await self._exit_to_dashboard("Setup complete.")

    async def _persist_mlflow(self) -> None:
        url = self._mlflow_fields.get("url", "")
        if url:
            await _xrun("config", "set", "mlflow.url", url)
        tok = self._mlflow_fields.get("token", "")
        usr = self._mlflow_fields.get("username", "")
        pwd = self._mlflow_fields.get("password", "")
        if tok:
            await _xrun("config", "set", "mlflow.token", tok)
        elif usr and pwd:
            await _xrun("config", "set", "mlflow.username", usr)
            await _xrun("config", "set", "mlflow.password", pwd)

    async def _exit_to_dashboard(self, msg: str) -> None:
        from xrun_tui.screens.dashboard import DashboardScreen
        self.notify(msg, severity="information", timeout=6)
        await self.app.switch_screen(DashboardScreen())
