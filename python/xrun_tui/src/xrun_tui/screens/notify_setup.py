"""Notifications setup — the "make it ping my phone" screen.

Parallel to Sinks: one card per delivery channel (ntfy / Telegram / webhook /
desktop), plus two "rule" cards (what to send, and the watchdog scheduler).
Every card is fully driven from the keyboard:

    Enter/e  edit the channel (form with hints and a Generate/Detect button)
    Space/d  enable / disable the channel in [notify].channels
    t        send a real test notification through that channel
    r        forget the channel's credentials

Config writes go through `xrun config set …` (schema-driven coercion, same
as the CLI) and `config.write_credentials` for secrets — the TUI never
invents its own file format.
"""
from __future__ import annotations

import asyncio
import json
import secrets
from typing import Any

from textual.app import ComposeResult
from textual.binding import Binding
from textual.containers import Horizontal, Vertical, VerticalScroll
from textual.screen import Screen
from textual.widgets import Button, Footer, Input, Label, RadioButton, RadioSet, Rule, Static

from xrun_tui import config
from xrun_tui.services import _run as _xrun
from xrun_tui.widgets.status_bar import StatusBar
from xrun_tui.widgets.title_bar import TitleBar

# (id, name, description)
CHANNELS: list[tuple[str, str, str]] = [
    ("ntfy",     "ntfy",     "Phone push via ntfy.sh — free, no account, 1 minute to set up"),
    ("telegram", "Telegram", "Message from your own bot"),
    ("webhook",  "Webhook",  "Slack / Discord incoming webhook, or your own JSON endpoint"),
    ("desktop",  "Desktop",  "OS toast on this machine (only useful while you're at it)"),
]
_LOGOS = {"ntfy": "📱", "telegram": "✈", "webhook": "⇥", "desktop": "🖥"}
_BRAND = {"ntfy": "#3e9e4a", "telegram": "#2aabee", "webhook": "#e0af68", "desktop": "#bb9af7"}

# Extra rows after the channels.
_RULES_ROW = len(CHANNELS)
_WATCHDOG_ROW = len(CHANNELS) + 1
_N_ROWS = len(CHANNELS) + 2

# Event presets for the Rules form. `custom` keeps whatever is in config.
PRESETS: list[tuple[str, str, list[str]]] = [
    ("all",      "Everything — done, failed, budget, anomalies, watchdog", ["*"]),
    ("problems", "Problems only — failed, idle, budget, anomalies, poller dead, orphans",
                 ["run.failed", "run.idle", "budget.*", "instance.*", "metric.anomaly", "poller.dead"]),
    ("money",    "Money only — budget thresholds, auto-destroy, cleanup failed, orphans, poller dead",
                 ["budget.*", "instance.*", "poller.dead"]),
]


def _pill(state: str) -> str:
    return {
        "on":       "[#1a1b26 on #9ece6a] ON [/]",
        "off":      "[#1a1b26 on #7aa2f7] OFF [/]",
        "empty":    "[#c0caf5 on #414868] EMPTY [/]",
        "checking": "[#1a1b26 on #e0af68] TEST [/]",
        "ok":       "[#1a1b26 on #9ece6a] ✓ SENT [/]",
        "error":    "[#c0caf5 on #f7768e] ERROR [/]",
        "info":     "[#c0caf5 on #414868] · [/]",
    }.get(state, "[#c0caf5 on #414868] · [/]")


def _read_state() -> tuple[dict, dict]:
    """(credentials, [notify] section of config.toml)."""
    creds = config.read_credentials()
    notify = config.read_global_config().get("notify", {}) or {}
    return creds, notify


def channel_configured(creds: dict, cid: str) -> bool:
    if cid == "ntfy":
        return bool((creds.get("ntfy") or {}).get("topic"))
    if cid == "telegram":
        t = creds.get("telegram") or {}
        return bool(t.get("bot_token") and t.get("chat_id"))
    if cid == "webhook":
        return bool((creds.get("webhook") or {}).get("url"))
    if cid == "desktop":
        return True
    return False


def preset_for(events: list[str]) -> str:
    ev = sorted(events or [])
    for pid, _, pats in PRESETS:
        if sorted(pats) == ev:
            return pid
    return "custom"


def generate_topic() -> str:
    """Random, unguessable ntfy topic. The topic *is* the secret."""
    return f"xrun-{secrets.token_hex(6)}"


async def _config_set(key: str, value: str) -> tuple[bool, str]:
    code, out, err = await _xrun("config", "set", key, value, timeout=15)
    return code == 0, (err or out).strip()


async def set_channels(channels: list[str]) -> tuple[bool, str]:
    # An empty list is a valid "off" — `xrun config set` treats "" as [].
    return await _config_set("notify.channels", ",".join(channels))


async def test_channel(cid: str) -> tuple[bool, str]:
    """`xrun notify test --channel <cid> --json` → (ok, detail)."""
    _code, out, err = await _xrun("notify", "test", "--channel", cid, "--json", timeout=30)
    try:
        data = json.loads(out)
    except Exception:
        return False, (err.strip().splitlines() or [out.strip() or "no output"])[-1][:160]
    outcome = data.get("outcome")
    warnings = data.get("warnings") or []
    if isinstance(outcome, list):
        for d in outcome:
            if d.get("channel") == cid:
                return bool(d.get("ok")), ("delivered" if d.get("ok") else str(d.get("error") or "failed"))
        return False, "channel did not run"
    if warnings:
        return False, str(warnings[0])
    return False, str(outcome or "skipped")


async def detect_telegram_chat_id(bot_token: str) -> tuple[str | None, str]:
    """Ask the Bot API for recent updates and pick the first chat id.

    The user must have sent the bot any message first — that's the only
    way Telegram lets a bot learn a chat id. Runs in a thread so the UI
    stays responsive; 10 s timeout.
    """
    import urllib.error
    import urllib.request

    url = f"https://api.telegram.org/bot{bot_token}/getUpdates"

    def _fetch() -> tuple[str | None, str]:
        try:
            with urllib.request.urlopen(url, timeout=10) as resp:  # noqa: S310
                data = json.loads(resp.read().decode("utf-8"))
        except urllib.error.HTTPError as e:
            if e.code == 401:
                return None, "bot token rejected (401) — check it with @BotFather"
            return None, f"HTTP {e.code}"
        except Exception as e:  # network, timeout, JSON
            return None, str(e)[:120]
        for upd in reversed(data.get("result") or []):
            msg = upd.get("message") or upd.get("channel_post") or upd.get("edited_message") or {}
            chat = msg.get("chat") or {}
            if chat.get("id") is not None:
                who = chat.get("username") or chat.get("title") or chat.get("first_name") or ""
                return str(chat["id"]), f"chat {chat['id']} ({who})" if who else f"chat {chat['id']}"
        return None, "no messages yet — open the bot in Telegram, press Start, then Detect again"

    return await asyncio.to_thread(_fetch)


class NotifySetupScreen(Screen):
    """Channel cards + rules + watchdog scheduler."""

    TITLE = "xrun — notifications"
    BINDINGS = [
        Binding("escape,q",  "go_back",  "Back"),
        Binding("enter,e",   "edit",     "Edit"),
        Binding("space,d",   "toggle_channel", "On/Off"),
        Binding("t",         "test",     "Test"),
        Binding("r",         "revoke",   "Revoke"),
        Binding("h",         "history",  "History"),
        Binding("j,down",    "next",     "Down", show=False),
        Binding("k,up",      "prev",     "Up",   show=False),
    ]

    def __init__(self) -> None:
        super().__init__()
        self._cursor = 0
        self._creds, self._notify = _read_state()
        self._sched: dict[str, Any] | None = None

    # ── derived state ────────────────────────────────────────────────────
    def _enabled(self) -> list[str]:
        ch = self._notify.get("channels") or []
        return [str(c) for c in ch]

    def _state(self, cid: str) -> str:
        if not channel_configured(self._creds, cid):
            return "empty"
        return "on" if cid in self._enabled() else "off"

    def _foot(self, cid: str) -> str:
        st = self._state(cid)
        if cid == "ntfy":
            n = self._creds.get("ntfy") or {}
            if st == "empty":
                return ("[#565f89]Enter → topic is generated for you; install the ntfy app and subscribe.[/]")
            server = (n.get("url") or "https://ntfy.sh").rstrip("/")
            return f"[#c0caf5]{server}/{n.get('topic')}[/]  [#565f89]t = send a test push[/]"
        if cid == "telegram":
            t = self._creds.get("telegram") or {}
            if st == "empty":
                if t.get("bot_token"):
                    return "[#e0af68]bot token set, chat id missing — Enter → Detect[/]"
                return "[#565f89]Enter → paste a token from @BotFather; chat id is detected for you.[/]"
            return f"[#c0caf5]chat {t.get('chat_id')}[/]  [#565f89]t = send a test message[/]"
        if cid == "webhook":
            w = self._creds.get("webhook") or {}
            if st == "empty":
                return "[#565f89]Enter → paste a Slack/Discord incoming-webhook URL.[/]"
            # Slack/Discord webhook URLs embed the secret in the path — show
            # the host only.
            from urllib.parse import urlsplit
            host = urlsplit(str(w.get("url"))).netloc or "configured"
            return f"[#c0caf5]{host}/…[/]  [#565f89]t = send a test[/]"
        if cid == "desktop":
            return ("[#565f89]No setup needed. Space to enable; t shows a toast.[/]"
                    if st != "on" else "[#c0caf5]enabled[/]  [#565f89]t = show a toast[/]")
        return ""

    def _rules_text(self) -> tuple[str, str]:
        events = [str(e) for e in (self._notify.get("events") or ["*"])]
        pid = preset_for(events)
        label = {p[0]: p[1].split(" — ")[0] for p in PRESETS}.get(pid, "Custom")
        pct = self._notify.get("cost_warn_pct") or [50, 80]
        pct_s = ", ".join(str(p) for p in pct) + " %"
        head = f"[bold #c0caf5]Rules[/]  [#565f89]what to send[/]"
        foot = (f"[#c0caf5]{label}[/]  [#565f89]· budget warn at[/] [#c0caf5]{pct_s}[/] "
                f"[#565f89]of --max-cost · Enter to change[/]")
        return head, foot

    def _watchdog_text(self) -> tuple[str, str]:
        head = "[bold #c0caf5]Watchdog[/]  [#565f89]catches a dead poller while the instance keeps billing[/]"
        if self._sched is None:
            foot = "[#e0af68]checking scheduler…[/]"
        elif self._sched.get("installed"):
            foot = (f"[#9ece6a]✓ scheduled[/] [#565f89]({self._sched.get('backend')}) · "
                    f"Enter to remove[/]")
        else:
            foot = ("[#e0af68]not scheduled[/] [#565f89]— only runs while the TUI is open. "
                    "Enter → register every 5 min[/]")
        return head, foot

    # ── compose ──────────────────────────────────────────────────────────
    def compose(self) -> ComposeResult:
        yield TitleBar("notifications")
        yield Static("Notifications", classes="screen-title")
        with VerticalScroll(id="vendor-overview"):
            for i, (cid, name, desc) in enumerate(CHANNELS):
                st = self._state(cid)
                with Vertical(classes="vendor-card", id=f"nrow-{i}"):
                    with Horizontal(classes="vendor-card-head"):
                        yield Static(f"[{_BRAND[cid]}]{_LOGOS[cid]}[/]", classes="vendor-logo")
                        yield Static(f"[bold #c0caf5]{name}[/]  [#565f89]{desc}[/]",
                                     classes="vendor-card-title")
                        yield Static(_pill(st), id=f"nstatus-{i}", classes="vendor-card-pill")
                    with Horizontal(classes="vendor-card-foot"):
                        yield Static(self._foot(cid), id=f"ninfo-{i}", classes="vendor-card-info")
            rh, rf = self._rules_text()
            with Vertical(classes="vendor-card", id=f"nrow-{_RULES_ROW}"):
                with Horizontal(classes="vendor-card-head"):
                    yield Static("[#7aa2f7]⚙[/]", classes="vendor-logo")
                    yield Static(rh, classes="vendor-card-title")
                    yield Static(_pill("info"), id=f"nstatus-{_RULES_ROW}", classes="vendor-card-pill")
                with Horizontal(classes="vendor-card-foot"):
                    yield Static(rf, id=f"ninfo-{_RULES_ROW}", classes="vendor-card-info")
            wh, wf = self._watchdog_text()
            with Vertical(classes="vendor-card", id=f"nrow-{_WATCHDOG_ROW}"):
                with Horizontal(classes="vendor-card-head"):
                    yield Static("[#f7768e]♥[/]", classes="vendor-logo")
                    yield Static(wh, classes="vendor-card-title")
                    yield Static(_pill("info"), id=f"nstatus-{_WATCHDOG_ROW}", classes="vendor-card-pill")
                with Horizontal(classes="vendor-card-foot"):
                    yield Static(wf, id=f"ninfo-{_WATCHDOG_ROW}", classes="vendor-card-info")
            yield Rule()
            yield Static(
                "[#565f89]Enter/e[/] [#c0caf5]Edit[/]   "
                "[#565f89]Space/d[/] [#c0caf5]On/Off[/]   "
                "[#565f89]t[/] [#c0caf5]Test[/]   "
                "[#565f89]r[/] [#c0caf5]Revoke[/]   "
                "[#565f89]h[/] [#c0caf5]History[/]   "
                "[#565f89]j/k[/] [#c0caf5]Navigate[/]",
                classes="vendor-hint",
            )
        yield StatusBar()
        yield Footer()

    def on_mount(self) -> None:
        self._highlight(0)
        self.run_worker(self._load_schedule(), exclusive=True, group="sched")

    async def _load_schedule(self) -> None:
        code, out, _ = await _xrun("watchdog", "schedule", "--json", timeout=15)
        try:
            self._sched = json.loads(out) if code == 0 else {"installed": False}
        except Exception:
            self._sched = {"installed": False}
        self._refresh_row(_WATCHDOG_ROW)

    # ── cursor ───────────────────────────────────────────────────────────
    def _highlight(self, idx: int) -> None:
        for i in range(_N_ROWS):
            try:
                self.query_one(f"#nrow-{i}", Vertical).set_class(i == idx, "vendor-row-active")
            except Exception:
                pass

    def action_next(self) -> None:
        self._cursor = (self._cursor + 1) % _N_ROWS
        self._highlight(self._cursor)

    def action_prev(self) -> None:
        self._cursor = (self._cursor - 1) % _N_ROWS
        self._highlight(self._cursor)

    def action_go_back(self) -> None:
        self.app.pop_screen()

    async def action_history(self) -> None:
        from xrun_tui.screens.notifications import NotificationsScreen
        await self.app.push_screen(NotificationsScreen())

    # ── refresh ──────────────────────────────────────────────────────────
    def _refresh_all(self) -> None:
        self._creds, self._notify = _read_state()
        for i in range(_N_ROWS):
            self._refresh_row(i)

    def _refresh_row(self, i: int) -> None:
        try:
            status_w = self.query_one(f"#nstatus-{i}", Static)
            info_w = self.query_one(f"#ninfo-{i}", Static)
        except Exception:
            return
        if i < len(CHANNELS):
            cid = CHANNELS[i][0]
            status_w.update(_pill(self._state(cid)))
            info_w.update(self._foot(cid))
        elif i == _RULES_ROW:
            info_w.update(self._rules_text()[1])
        else:
            info_w.update(self._watchdog_text()[1])

    # ── actions ──────────────────────────────────────────────────────────
    async def action_edit(self) -> None:
        i = self._cursor
        if i < len(CHANNELS):
            cid, name, _ = CHANNELS[i]
            if cid == "desktop":
                await self.action_toggle_channel()
                return
            await self.app.push_screen(ChannelEditScreen(cid, name), self._after_edit)
        elif i == _RULES_ROW:
            await self.app.push_screen(RulesEditScreen(), self._after_edit)
        else:
            await self._toggle_schedule()

    def _after_edit(self, result: Any) -> None:
        self._refresh_all()
        # Saving a channel enables it and offers a test in one go.
        if isinstance(result, dict) and result.get("test_channel"):
            self._cursor = next(
                (i for i, c in enumerate(CHANNELS) if c[0] == result["test_channel"]),
                self._cursor,
            )
            self._highlight(self._cursor)
            self.run_worker(self.action_test(), exclusive=False)

    async def action_toggle_channel(self) -> None:
        i = self._cursor
        if i >= len(CHANNELS):
            if i == _WATCHDOG_ROW:
                await self._toggle_schedule()
            return
        cid, name, _ = CHANNELS[i]
        if not channel_configured(self._creds, cid):
            self.notify(f"{name}: press Enter to set it up first", severity="warning")
            return
        enabled = self._enabled()
        if cid in enabled:
            enabled = [c for c in enabled if c != cid]
            msg = f"{name} off"
        else:
            enabled.append(cid)
            msg = f"{name} on — running runs pick it up within ~5 s"
        ok, err = await set_channels(enabled)
        if not ok:
            self.notify(f"could not save: {err}", severity="error", timeout=8)
            return
        self._refresh_all()
        self.notify(msg, severity="information")

    async def action_test(self) -> None:
        i = self._cursor
        if i >= len(CHANNELS):
            return
        cid, name, _ = CHANNELS[i]
        if not channel_configured(self._creds, cid):
            self.notify(f"{name}: set it up first (Enter)", severity="warning")
            return
        if cid not in self._enabled():
            ok, err = await set_channels(self._enabled() + [cid])
            if not ok:
                self.notify(f"could not enable {name}: {err}", severity="error")
                return
            self._creds, self._notify = _read_state()
        status_w = self.query_one(f"#nstatus-{i}", Static)
        info_w = self.query_one(f"#ninfo-{i}", Static)
        status_w.update(_pill("checking"))
        info_w.update("[#e0af68]sending test…[/]")
        ok, detail = await test_channel(cid)
        status_w.update(_pill("ok" if ok else "error"))
        info_w.update(f"[#9ece6a]✓ {detail} — check your device[/]" if ok
                      else f"[#f7768e]✗ {detail}[/]")
        if ok:
            self.notify(f"{name}: test delivered", severity="information")

    async def action_revoke(self) -> None:
        i = self._cursor
        if i >= len(CHANNELS):
            return
        cid, name, _ = CHANNELS[i]
        if cid == "desktop" or not channel_configured(self._creds, cid):
            return
        creds = dict(self._creds)
        creds.pop(cid, None)
        config.write_credentials(creds)
        enabled = [c for c in self._enabled() if c != cid]
        await set_channels(enabled)
        self._refresh_all()
        self.notify(f"{name} credentials removed", severity="warning")

    async def _toggle_schedule(self) -> None:
        installed = bool(self._sched and self._sched.get("installed"))
        flag = "--remove" if installed else "--install"
        info_w = self.query_one(f"#ninfo-{_WATCHDOG_ROW}", Static)
        info_w.update("[#e0af68]updating scheduler…[/]")
        code, out, err = await _xrun("watchdog", "schedule", flag, "--json", timeout=20)
        if code != 0:
            self._refresh_row(_WATCHDOG_ROW)
            self.notify(f"scheduler: {(err or out).strip()[:160]}", severity="error", timeout=10)
            return
        try:
            self._sched = json.loads(out)
        except Exception:
            self._sched = {"installed": not installed}
        self._refresh_row(_WATCHDOG_ROW)
        self.notify("watchdog removed from scheduler" if installed
                    else "watchdog scheduled every 5 min", severity="information")


# ════════════════════════════════════════════════════════════════════════════
#  Channel edit form
# ════════════════════════════════════════════════════════════════════════════

class ChannelEditScreen(Screen[dict | None]):
    """One form per channel. Saving writes credentials, enables the channel
    and hands `{"test_channel": cid}` back so the parent fires a test."""

    TITLE = "xrun — edit channel"
    BINDINGS = [
        Binding("escape", "go_back", "Back"),
        Binding("ctrl+s", "save",    "Save & test"),
    ]

    def __init__(self, cid: str, name: str) -> None:
        super().__init__()
        self._cid = cid
        self._name = name
        self._creds = config.read_credentials()

    def compose(self) -> ComposeResult:
        yield TitleBar("edit channel")
        yield Static(f"Notifications — {self._name}", classes="screen-title")
        with Vertical(id="vendor-form"):
            if self._cid == "ntfy":
                n = self._creds.get("ntfy") or {}
                yield Static(
                    "[bold #bb9af7]How it works[/]  [#565f89]1) install the ntfy app "
                    "(Android / iOS / web at ntfy.sh)  2) subscribe to the topic below  "
                    "3) Save & test. The topic is the secret — keep it random.[/]",
                    classes="form-section")
                with Horizontal(classes="form-row"):
                    yield Label("Topic:", classes="form-label")
                    yield Input(n.get("topic") or generate_topic(), id="in-ntfy-topic",
                                placeholder="xrun-…", classes="form-input")
                    yield Button("Generate", id="btn-gen-topic")
                with Horizontal(classes="form-row"):
                    yield Label("Server:", classes="form-label")
                    yield Input(n.get("url") or "", id="in-ntfy-url",
                                placeholder="https://ntfy.sh (default) or your own server",
                                classes="form-input")
                with Horizontal(classes="form-row"):
                    yield Label("Token:", classes="form-label")
                    yield Input(n.get("token") or "", id="in-ntfy-token", password=True,
                                placeholder="(optional) tk_… for protected topics",
                                classes="form-input")
            elif self._cid == "telegram":
                t = self._creds.get("telegram") or {}
                yield Static(
                    "[bold #bb9af7]How it works[/]  [#565f89]1) in Telegram open @BotFather → "
                    "/newbot → copy the token  2) open your new bot and press Start  "
                    "3) paste the token here and press Detect.[/]",
                    classes="form-section")
                with Horizontal(classes="form-row"):
                    yield Label("Bot token:", classes="form-label")
                    yield Input(t.get("bot_token") or "", id="in-tg-token", password=True,
                                placeholder="123456789:AAH…", classes="form-input")
                with Horizontal(classes="form-row"):
                    yield Label("Chat id:", classes="form-label")
                    yield Input(str(t.get("chat_id") or ""), id="in-tg-chat",
                                placeholder="(press Detect after messaging the bot)",
                                classes="form-input")
                    yield Button("Detect", id="btn-detect-chat")
                yield Static("", id="tg-detect-result", classes="form-hint")
            elif self._cid == "webhook":
                w = self._creds.get("webhook") or {}
                yield Static(
                    "[bold #bb9af7]How it works[/]  [#565f89]Slack: app → Incoming Webhooks → "
                    "Add to workspace. Discord: channel settings → Integrations → Webhooks. "
                    "Any URL receiving JSON works (fields: kind, title, body, text, content).[/]",
                    classes="form-section")
                with Horizontal(classes="form-row"):
                    yield Label("URL:", classes="form-label")
                    yield Input(w.get("url") or "", id="in-wh-url", password=True,
                                placeholder="https://hooks.slack.com/services/…",
                                classes="form-input")
            yield Static("", classes="form-spacer")
            with Horizontal(classes="form-actions"):
                yield Button("Save & test  [Ctrl+S]", id="btn-save", variant="primary")
                yield Button("Back  [Esc]", id="btn-back")
        yield StatusBar()
        yield Footer()

    async def on_button_pressed(self, event: Button.Pressed) -> None:
        bid = event.button.id
        if bid == "btn-save":
            await self.action_save()
        elif bid == "btn-back":
            self.action_go_back()
        elif bid == "btn-gen-topic":
            self.query_one("#in-ntfy-topic", Input).value = generate_topic()
        elif bid == "btn-detect-chat":
            await self._detect_chat()

    async def _detect_chat(self) -> None:
        token = self.query_one("#in-tg-token", Input).value.strip()
        out = self.query_one("#tg-detect-result", Static)
        if not token:
            out.update("[#f7768e]paste the bot token first[/]")
            return
        out.update("[#e0af68]asking Telegram…[/]")
        chat_id, detail = await detect_telegram_chat_id(token)
        if chat_id:
            self.query_one("#in-tg-chat", Input).value = chat_id
            out.update(f"[#9ece6a]✓ {detail}[/]")
        else:
            out.update(f"[#f7768e]✗ {detail}[/]")

    async def action_save(self) -> None:
        creds = dict(self._creds)
        entry: dict[str, Any] = {}
        if self._cid == "ntfy":
            topic = self.query_one("#in-ntfy-topic", Input).value.strip()
            if not topic:
                self.notify("topic is required", severity="error")
                return
            entry["topic"] = topic
            if url := self.query_one("#in-ntfy-url", Input).value.strip():
                entry["url"] = url
            if tok := self.query_one("#in-ntfy-token", Input).value.strip():
                entry["token"] = tok
        elif self._cid == "telegram":
            tok = self.query_one("#in-tg-token", Input).value.strip()
            chat = self.query_one("#in-tg-chat", Input).value.strip()
            if not tok or not chat:
                self.notify("bot token and chat id are both required (use Detect)",
                            severity="error")
                return
            entry = {"bot_token": tok, "chat_id": chat}
        elif self._cid == "webhook":
            url = self.query_one("#in-wh-url", Input).value.strip()
            if not url.startswith("http"):
                self.notify("URL must start with http(s)://", severity="error")
                return
            entry = {"url": url}
        creds[self._cid] = entry
        config.write_credentials(creds)
        notify = config.read_global_config().get("notify", {}) or {}
        enabled = [str(c) for c in (notify.get("channels") or [])]
        if self._cid not in enabled:
            ok, err = await set_channels(enabled + [self._cid])
            if not ok:
                self.notify(f"saved credentials, but could not enable: {err}",
                            severity="error", timeout=8)
                self.dismiss({})
                return
        self.notify(f"{self._name} saved and enabled — running runs pick it up within ~5 s",
                    severity="information")
        self.dismiss({"test_channel": self._cid})

    def action_go_back(self) -> None:
        self.dismiss(None)


# ════════════════════════════════════════════════════════════════════════════
#  Rules form
# ════════════════════════════════════════════════════════════════════════════

class RulesEditScreen(Screen[dict | None]):
    TITLE = "xrun — notification rules"
    BINDINGS = [
        Binding("escape", "go_back", "Back"),
        Binding("ctrl+s", "save",    "Save"),
    ]

    def __init__(self) -> None:
        super().__init__()
        self._notify = config.read_global_config().get("notify", {}) or {}

    def compose(self) -> ComposeResult:
        events = [str(e) for e in (self._notify.get("events") or ["*"])]
        current = preset_for(events)
        pct = self._notify.get("cost_warn_pct") or [50, 80]
        yield TitleBar("notification rules")
        yield Static("Notifications — rules", classes="screen-title")
        with Vertical(id="vendor-form"):
            yield Static("[bold #bb9af7]What to send[/]", classes="form-section")
            yield RadioSet(
                *[RadioButton(label, value=(pid == current), id=f"preset-{pid}")
                  for pid, label, _ in PRESETS],
                RadioButton(f"Custom — keep current list: {', '.join(events)}",
                            value=(current == "custom"), id="preset-custom"),
                id="preset-radio",
            )
            yield Static("[bold #bb9af7]Budget warnings[/]  [#565f89]percent of --max-cost, "
                         "each fires once per instance[/]", classes="form-section")
            with Horizontal(classes="form-row"):
                yield Label("Warn at %:", classes="form-label")
                yield Input(", ".join(str(p) for p in pct), id="in-pct",
                            placeholder="50, 80", classes="form-input")
            with Horizontal(classes="form-row"):
                yield Label("Poller stale after (min):", classes="form-label")
                yield Input(str(self._notify.get("heartbeat_stale_min") or 5), id="in-stale",
                            placeholder="5", classes="form-input")
            yield Static("", classes="form-spacer")
            with Horizontal(classes="form-actions"):
                yield Button("Save  [Ctrl+S]", id="btn-save", variant="primary")
                yield Button("Back  [Esc]", id="btn-back")
        yield StatusBar()
        yield Footer()

    async def on_button_pressed(self, event: Button.Pressed) -> None:
        if event.button.id == "btn-save":
            await self.action_save()
        elif event.button.id == "btn-back":
            self.action_go_back()

    async def action_save(self) -> None:
        rs = self.query_one("#preset-radio", RadioSet)
        pressed = (rs.pressed_button.id if rs.pressed_button else None) or "preset-custom"
        pid = pressed[len("preset-"):]
        errors: list[str] = []
        if pid != "custom":
            pats = next(p[2] for p in PRESETS if p[0] == pid)
            ok, err = await _config_set("notify.events", ",".join(pats))
            if not ok:
                errors.append(f"events: {err}")
        raw_pct = self.query_one("#in-pct", Input).value
        pct = [p.strip() for p in raw_pct.split(",") if p.strip()]
        if not all(p.isdigit() and 0 < int(p) < 100 for p in pct):
            self.notify("warn % must be whole numbers between 1 and 99", severity="error")
            return
        ok, err = await _config_set("notify.cost_warn_pct", ",".join(pct))
        if not ok:
            errors.append(f"cost_warn_pct: {err}")
        stale = self.query_one("#in-stale", Input).value.strip()
        if not stale.isdigit() or int(stale) < 1:
            self.notify("stale minutes must be a whole number ≥ 1", severity="error")
            return
        ok, err = await _config_set("notify.heartbeat_stale_min", stale)
        if not ok:
            errors.append(f"heartbeat_stale_min: {err}")
        if errors:
            self.notify("; ".join(errors)[:200], severity="error", timeout=10)
            return
        self.notify("rules saved", severity="information")
        self.dismiss({})

    def action_go_back(self) -> None:
        self.dismiss(None)
