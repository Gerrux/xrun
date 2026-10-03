"""SSH hosts: the list of your own servers (alias → host/user/port/key) and the
add / edit form. Everything is written through `xrun config` (the only writer);
none of these fields is a secret — `key` is a path to the identity file."""
from __future__ import annotations

from rich.markup import escape
from textual.app import ComposeResult
from textual.binding import Binding
from textual.containers import Horizontal, Vertical
from textual.screen import Screen
from textual.widgets import Button, Footer, Input, Label, Static
from xrun_tui.screens.confirm import ConfirmScreen
from xrun_tui.widgets.form import FormGuard, single_save
from xrun_tui.widgets.status_bar import StatusBar
from xrun_tui.widgets.title_bar import TitleBar

from xrun_tui import config, services

# Optional fields: clearing one in the form unsets it. host and user are required.
_OPTIONAL = ("key", "default_workdir")
_FIELDS = ("host", "user", "port", *_OPTIONAL)


def ssh_probe_args(host: dict) -> list[str]:
    """`xrun config probe --vendor ssh` flags for one stored/typed host.
    Port and key are left out when empty."""
    args = ["--ssh-host", str(host.get("host") or ""),
            "--ssh-user", str(host.get("user") or "")]
    if host.get("port"):
        args += ["--ssh-port", str(host["port"])]
    if host.get("key"):
        args += ["--ssh-key", str(host["key"])]
    return args


def alias_ok(alias: str) -> bool:
    """The CLI's alias rule: non-empty, letters/digits plus - and _. Anything
    else (a dot above all) would make `ssh.<alias>.<field>` address another
    key — `ssh.lab.port` is the port of host `lab`, not a host `lab.port`."""
    return bool(alias) and all(c.isalnum() or c in "-_" for c in alias)


def validate_host_form(
    values: dict[str, str],
    *,
    editing: bool,
    existing: frozenset[str] | set[str] = frozenset(),
) -> tuple[str, str] | None:
    """(field, message) for the first invalid value, None when all are fine.
    The alias rule mirrors the CLI's: letters/digits plus - and _. A new
    host may not reuse an alias in `existing` (that would merge into it)."""
    alias = values.get("alias", "").strip()
    if not editing:
        if not alias:
            return "alias", "required"
        if not alias_ok(alias):
            return "alias", "letters, digits, - and _ only"
        if alias in existing:
            return "alias", f"{alias} already exists — edit it instead"
    if not values.get("host", "").strip():
        return "host", "required"
    if not values.get("user", "").strip():
        return "user", "required"
    port = values.get("port", "").strip()
    if port:
        if not port.isascii() or not port.isdigit() or not 1 <= int(port) <= 65535:
            return "port", "must be an integer 1..65535"
    return None


def host_complete(host: dict) -> bool:
    """A host xrun can connect to needs both `host` and `user`."""
    return bool(str(host.get("host") or "").strip()
                and str(host.get("user") or "").strip())


def hosts_in(creds: dict) -> dict[str, dict]:
    """alias → fields of every `[ssh.<alias>]` table (malformed entries
    skipped), complete or not."""
    raw = creds.get("ssh")
    if not isinstance(raw, dict):
        return {}
    return {str(a): h for a, h in raw.items() if isinstance(h, dict)}


def usable_hosts(creds: dict) -> dict[str, dict]:
    """Only the hosts that have both `host` and `user`: the ones that count as
    a configured SSH vendor and can be probed."""
    return {a: h for a, h in hosts_in(creds).items() if host_complete(h)}


def _hosts() -> dict[str, dict]:
    return hosts_in(config.read_credentials())


# ═══════════════════════════════════════════════════════════════════════════════
#  Host list
# ═══════════════════════════════════════════════════════════════════════════════

class SshHostsScreen(Screen):
    TITLE = "xrun — SSH hosts"
    BINDINGS = [
        Binding("escape,q", "go_back", "Back"),
        Binding("a",        "add",     "Add"),
        Binding("enter,e",  "edit",    "Edit"),
        Binding("t",        "test",    "Test"),
        Binding("r",        "remove",  "Remove"),
        Binding("j,down",   "next",    "Down", show=False),
        Binding("k,up",     "prev",    "Up",   show=False),
    ]

    def __init__(self) -> None:
        super().__init__()
        self._hosts = _hosts()
        self._cursor = 0
        # alias → probe result line (markup); gone when the host list reloads
        self._status: dict[str, str] = {}

    def compose(self) -> ComposeResult:
        yield TitleBar("ssh hosts")
        yield Static("SSH hosts", classes="screen-title")
        with Vertical(id="vendor-overview"):
            yield Static("", id="ssh-hosts-body")
        yield StatusBar()
        yield Footer()

    def on_mount(self) -> None:
        self._render_list()

    def on_screen_resume(self) -> None:
        # Back from the edit form: pick up what it wrote.
        self._hosts = _hosts()
        self._status = {a: s for a, s in self._status.items() if a in self._hosts}
        self._cursor = min(self._cursor, max(len(self._hosts) - 1, 0))
        self._render_list()

    def _render_list(self) -> None:
        if not self.is_attached:
            return
        body = self.query_one("#ssh-hosts-body", Static)
        if not self._hosts:
            body.update(
                "[bold #c0caf5]No SSH hosts yet[/]\n\n"
                "[#565f89]An SSH host is your own machine — a server, NAS or VPS — "
                "that xrun can run experiments on over SSH, with no cloud account.\n"
                "Press[/] [#c0caf5]a[/] [#565f89]to add one, then use its alias "
                "in a manifest.[/]"
            )
            return
        lines: list[str] = []
        for i, (alias, h) in enumerate(self._hosts.items()):
            mark = "[#bb9af7]▶[/]" if i == self._cursor else " "
            port = h.get("port") or 22
            target = f"{h.get('user') or '?'}@{h.get('host') or '?'}:{port}"
            # An unusable host stays visible so it can be edited or removed.
            incomplete = ("" if host_complete(h)
                          else "  [#1a1b26 on #e0af68] INCOMPLETE [/]")
            lines.append(
                f"{mark} [bold #c0caf5]{escape(alias)}[/]  [#7aa2f7]{escape(target)}[/]"
                f"{incomplete}"
            )
            extra = []
            if h.get("key"):
                extra.append(f"key {escape(str(h['key']))}")
            if h.get("default_workdir"):
                extra.append(f"workdir {escape(str(h['default_workdir']))}")
            status = self._status.get(alias)
            if extra or status:
                lines.append("    [#565f89]" + "  ".join(extra) + "[/]"
                             + (f"  {status}" if status else ""))
        body.update("\n".join(lines))

    def _selected(self) -> str | None:
        aliases = list(self._hosts)
        return aliases[self._cursor] if 0 <= self._cursor < len(aliases) else None

    def action_next(self) -> None:
        if self._hosts:
            self._cursor = (self._cursor + 1) % len(self._hosts)
            self._render_list()

    def action_prev(self) -> None:
        if self._hosts:
            self._cursor = (self._cursor - 1) % len(self._hosts)
            self._render_list()

    async def action_add(self) -> None:
        await self.app.push_screen(SshHostEditScreen(None))

    async def action_edit(self) -> None:
        alias = self._selected()
        if alias is None:
            await self.action_add()
            return
        if not self._writable(alias):
            return
        await self.app.push_screen(SshHostEditScreen(alias))

    def _writable(self, alias: str) -> bool:
        """A hand-written alias outside the CLI rule cannot be addressed as
        `ssh.<alias>`: `lab.port` would hit the port of host `lab`."""
        if alias_ok(alias):
            return True
        self.notify(
            f"Alias {alias!r} is not valid for `xrun config` (letters, digits, "
            "- and _ only) — rename it in credentials.toml by hand",
            severity="error", timeout=10,
        )
        return False

    async def action_test(self) -> None:
        alias = self._selected()
        if alias is None:
            self.notify("No host selected — press a to add one", severity="warning")
            return
        if not host_complete(self._hosts[alias]):
            self.notify(f"{alias} has no host or user — press Enter to fix it",
                        severity="warning")
            return
        self._status[alias] = "[#e0af68]testing…[/]"
        self._render_list()
        res = await services.probe("ssh", extra_args=ssh_probe_args(self._hosts[alias]))
        if not self.is_attached:
            return
        detail = escape(str(res.get("detail") or ""))
        if res.get("ok"):
            self._status[alias] = f"[#9ece6a]✓ {detail or 'ok'}[/]"
        else:
            self._status[alias] = f"[#f7768e]✗ {detail or 'failed'}[/]"
        self._render_list()

    async def action_remove(self) -> None:
        alias = self._selected()
        if alias is None or not self._writable(alias):
            return

        async def _do(confirmed: bool | None) -> None:
            if not confirmed:
                return
            ok, err = await services.config_unset(f"ssh.{alias}")
            if not self.is_attached:
                return
            if not ok:
                self.notify(f"Remove failed: {err}", severity="error", timeout=10)
                return
            self._hosts = _hosts()
            self._status.pop(alias, None)
            self._cursor = min(self._cursor, max(len(self._hosts) - 1, 0))
            self._render_list()
            self.notify(f"SSH host {alias} removed", severity="information")

        await self.app.push_screen(
            ConfirmScreen(f"Remove SSH host {alias}?", default_no=True), _do
        )

    def action_go_back(self) -> None:
        self.app.pop_screen()


# ═══════════════════════════════════════════════════════════════════════════════
#  Add / edit form
# ═══════════════════════════════════════════════════════════════════════════════

class SshHostEditScreen(FormGuard, Screen):
    BINDINGS = [
        Binding("escape", "go_back", "Back"),
        Binding("ctrl+s", "save",    "Save"),
        Binding("ctrl+t", "test",    "Test"),
    ]

    def __init__(self, alias: str | None) -> None:
        super().__init__()
        self._alias = alias
        self._stored: dict = _hosts().get(alias, {}) if alias else {}

    def compose(self) -> ComposeResult:
        s = self._stored
        title = f"Edit SSH host — {self._alias}" if self._alias else "Add SSH host"
        yield TitleBar("ssh host")
        yield Static(title, classes="screen-title")
        with Vertical(id="vendor-form"):
            rows = [
                ("alias", "Alias:", self._alias or "",
                 "myhost (used in manifests)"),
                ("host", "Host:", str(s.get("host") or ""),
                 "192.168.1.10 or vps.example.com"),
                ("user", "User:", str(s.get("user") or ""), "root"),
                ("port", "Port:", str(s.get("port") or 22), "22"),
                ("key", "Key path:", str(s.get("key") or ""),
                 "~/.ssh/id_ed25519 (optional)"),
                ("default_workdir", "Workdir:", str(s.get("default_workdir") or ""),
                 "/home/me/xrun (optional)"),
            ]
            for field, label, value, placeholder in rows:
                with Horizontal(classes="form-row"):
                    yield Label(label, classes="form-label")
                    yield Input(
                        value,
                        id=f"input-ssh-{field}",
                        placeholder=placeholder,
                        classes="form-input",
                        disabled=(field == "alias" and self._alias is not None),
                    )
            yield Static(
                "[#565f89]The key is a path to your identity file, not the key itself. "
                "Clear an optional field and save to remove it.[/]",
                classes="form-hint",
            )
            yield Static("", id="test-result", classes="form-hint")
            with Horizontal(classes="form-actions"):
                yield Button("Save  [Ctrl+S]", id="btn-save", variant="primary")
                yield Button("Test  [Ctrl+T]", id="btn-test")
                yield Button("Back  \\[Esc]",    id="btn-back")
        yield StatusBar()
        yield Footer()

    def on_mount(self) -> None:
        self.call_after_refresh(self.snapshot_form)

    def _values(self) -> dict[str, str]:
        out = {"alias": self._alias or
               self.query_one("#input-ssh-alias", Input).value.strip()}
        for f in _FIELDS:
            out[f] = self.query_one(f"#input-ssh-{f}", Input).value.strip()
        return out

    def _show(self, markup: str) -> None:
        self.query_one("#test-result", Static).update(markup)

    def on_button_pressed(self, event: Button.Pressed) -> None:
        bid = event.button.id
        if bid == "btn-save":
            self.run_worker(self.action_save(), exclusive=False, group="save")
        elif bid == "btn-test":
            self.run_worker(self.action_test(), exclusive=False, group="test")
        elif bid == "btn-back":
            self.action_go_back()

    @single_save
    async def action_save(self) -> None:
        values = self._values()
        # Re-read the stored aliases now, not at form open: a host added
        # meanwhile (another TUI, the CLI) must not be silently merged into.
        bad = validate_host_form(values, editing=self._alias is not None,
                                 existing=set(_hosts()))
        if bad:
            self._show(f"[bold #f7768e]{bad[0]}: {bad[1]}[/]")
            return
        alias = values["alias"]
        ops: list[tuple[str, str, str]] = []
        for f in _FIELDS:
            new = values[f]
            old = str(self._stored.get(f) or "")
            if f == "port" and not old:
                old = "22"  # absent port means the default, as the form shows it
            if new == old or (f == "port" and not new and not self._stored.get("port")):
                continue
            if new:
                ops.append(("set", f, new))
            else:
                ops.append(("unset", f, ""))  # only optional fields get here
        if not ops:
            self.notify("Nothing to save", severity="information")
            return
        for op, f, val in ops:
            if op == "set":
                ok, err = await services.config_set(f"ssh.{alias}.{f}", val)
            else:
                ok, err = await services.config_unset(f"ssh.{alias}.{f}")
            if not self.is_attached:
                return
            if not ok:
                self._show(f"[bold #f7768e]{f}: {escape(err or 'failed')}[/]")
                return
        self.snapshot_form()
        self.notify(f"SSH host {alias} saved", severity="information")
        self.app.pop_screen()

    async def action_test(self) -> None:
        values = self._values()
        bad = validate_host_form(values, editing=self._alias is not None)
        if bad:
            self._show(f"[bold #f7768e]{bad[0]}: {bad[1]}[/]")
            return
        self._show("[#e0af68]Testing…[/]")
        res = await services.probe("ssh", extra_args=ssh_probe_args(values))
        if not self.is_attached:
            return
        detail = escape(str(res.get("detail") or ""))
        if res.get("ok"):
            self._show(f"[bold #9ece6a]✓ Connected[/]  [#565f89]{detail}[/]")
        else:
            self._show(f"[bold #f7768e]✗ {detail or 'failed'}[/]")
