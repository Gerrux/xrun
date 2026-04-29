from __future__ import annotations

import asyncio
from typing import Awaitable, Callable

from textual.app import ComposeResult
from textual.containers import Center, Middle, Vertical
from textual.screen import Screen
from textual.widgets import Static

# ANSI Shadow style — clean, modern, monospace-safe.
_LOGO = r"""[bold #7aa2f7]██╗  ██╗[/][bold #bb9af7]██████╗ [/][bold #9ece6a]██╗   ██╗[/][bold #e0af68]███╗   ██╗[/]
[bold #7aa2f7]╚██╗██╔╝[/][bold #bb9af7]██╔══██╗[/][bold #9ece6a]██║   ██║[/][bold #e0af68]████╗  ██║[/]
[bold #7aa2f7] ╚███╔╝ [/][bold #bb9af7]██████╔╝[/][bold #9ece6a]██║   ██║[/][bold #e0af68]██╔██╗ ██║[/]
[bold #7aa2f7] ██╔██╗ [/][bold #bb9af7]██╔══██╗[/][bold #9ece6a]██║   ██║[/][bold #e0af68]██║╚██╗██║[/]
[bold #7aa2f7]██╔╝ ██╗[/][bold #bb9af7]██║  ██║[/][bold #9ece6a]╚██████╔╝[/][bold #e0af68]██║ ╚████║[/]
[bold #7aa2f7]╚═╝  ╚═╝[/][bold #bb9af7]╚═╝  ╚═╝[/][bold #9ece6a] ╚═════╝ [/][bold #e0af68]╚═╝  ╚═══╝[/]"""

_TAGLINE = "[#565f89]Run GPU experiments anywhere[/]"

# Step id → (short label, fixed width for alignment)
_STEP_W = 14


class SplashScreen(Screen):
    """Boot screen showing real init progress."""

    DEFAULT_CSS = """
    SplashScreen { background: #1a1b26; align: center middle; }
    #splash-logo    { content-align: center middle; height: auto; }
    #splash-tag     { content-align: center middle; height: 1; padding-top: 1; }
    #splash-steps   { width: 56; height: auto; padding-top: 2; }
    .splash-step    { color: #565f89; height: 1; }
    .splash-step-ok      { color: #9ece6a; }
    .splash-step-warn    { color: #e0af68; }
    .splash-step-fail    { color: #f7768e; }
    .splash-step-pending { color: #565f89; }
    #splash-version { content-align: center middle; height: 1;
                      color: #414868; padding-top: 2; }
    """

    # (id, short label shown on left, default detail shown on right when pending)
    _STEPS: list[tuple[str, str]] = [
        ("db",      "Database"),
        ("config",  "Credentials"),
        ("vast",    "Vast.ai"),
        ("scan",    "Manifests"),
        ("ready",   "Workspace"),
    ]

    def __init__(
        self,
        on_done: Callable[[], Awaitable[None]],
        version: str = "0.2",
    ) -> None:
        super().__init__()
        self._on_done = on_done
        self._version = version

    def compose(self) -> ComposeResult:
        with Middle():
            with Center():
                with Vertical():
                    yield Static(_LOGO, id="splash-logo")
                    yield Static(_TAGLINE, id="splash-tag")
                    with Vertical(id="splash-steps"):
                        for sid, label in self._STEPS:
                            yield Static(
                                self._format_line("·", "#414868", label, "waiting"),
                                id=f"step-{sid}",
                                classes="splash-step splash-step-pending",
                            )
                    yield Static(
                        f"[#414868]xrun[/] [#565f89]v{self._version}[/]",
                        id="splash-version",
                    )

    def on_mount(self) -> None:
        self.run_worker(self._init_sequence(), exclusive=True)

    async def _init_sequence(self) -> None:
        from xrun_tui import config, services

        # 1) DB
        await self._set("db", "running", detail="opening…")
        try:
            assert self.app.db._conn is not None  # type: ignore[attr-defined]
            await self._set("db", "ok", detail="ready")
        except Exception as exc:
            await self._set("db", "fail", detail=str(exc)[:32])

        # 2) Config / creds
        await self._set("config", "running", detail="reading…")
        try:
            creds = config.read_credentials()
            keys = sum(1 for v in creds.values()
                       if isinstance(v, dict) and v.get("api_key"))
            if keys == 0:
                await self._set("config", "warn", detail="none configured")
            else:
                noun = "key" if keys == 1 else "keys"
                await self._set("config", "ok", detail=f"{keys} {noun}")
        except Exception as exc:
            await self._set("config", "warn", detail=str(exc)[:32])

        # 3) Vast probe (best-effort, async, short timeout)
        await self._set("vast", "running", detail="probing…")
        api_key = config.get_vast_api_key()
        if not api_key:
            await self._set("vast", "warn", detail="no API key")
        else:
            try:
                from xrun_tui.screens.vendors import _fetch_user
                info = await asyncio.wait_for(_fetch_user(api_key), timeout=4)
                user = info.get("username") or info.get("email") or "?"
                credit = float(info.get("credit", 0))
                self.app._vast_status_cache = {  # type: ignore[attr-defined]
                    "vast_user":   user,
                    "vast_credit": credit,
                }
                await self._set(
                    "vast", "ok",
                    detail=f"{user}  [#e0af68]${credit:.2f}[/]",
                )
            except Exception as exc:
                await self._set("vast", "warn", detail=str(exc)[:32])

        # 4) Manifest scan — strict scope: `defaults.exp_dir` (or conventional roots).
        await self._set("scan", "running", detail="scanning…")
        try:
            exp_dir: str | None = None
            ok, cfg, _ = await services.config_show()
            if ok:
                exp_dir = (cfg.get("defaults") or {}).get("exp_dir") or None
            self.app._exp_dir = exp_dir  # type: ignore[attr-defined]
            ms = await asyncio.to_thread(services.discover_manifests, exp_dir)
            n = len(ms)
            noun = "manifest" if n == 1 else "manifests"
            await self._set("scan", "ok", detail=f"{n} {noun}")
        except Exception as exc:
            await self._set("scan", "warn", detail=str(exc)[:32])

        # 5) Warm
        await self._set("ready", "running", detail="warming…")
        await asyncio.sleep(0.15)
        await self._set("ready", "ok", detail="ready")

        await asyncio.sleep(0.3)
        self.app.call_later(self._on_done)

    @staticmethod
    def _format_line(sym: str, sym_colour: str, label: str, detail: str) -> str:
        # ✓  Database          ready
        pad = max(1, _STEP_W - len(label))
        return (
            f"[{sym_colour}]{sym}[/]  "
            f"[#c0caf5]{label}[/]{' ' * pad}"
            f"[#565f89]{detail}[/]"
        )

    async def _set(
        self,
        sid: str,
        state: str,
        detail: str = "",
    ) -> None:
        try:
            w = self.query_one(f"#step-{sid}", Static)
        except Exception:
            return
        label = next((l for s, l in self._STEPS if s == sid), sid)
        marks = {
            "running": ("◌", "#e0af68", "splash-step-pending"),
            "ok":      ("✓", "#9ece6a", "splash-step-ok"),
            "warn":    ("!", "#e0af68", "splash-step-warn"),
            "fail":    ("✗", "#f7768e", "splash-step-fail"),
        }
        sym, colour, cls = marks.get(state, ("·", "#565f89", "splash-step-pending"))
        w.remove_class(
            "splash-step-pending", "splash-step-ok",
            "splash-step-warn",    "splash-step-fail",
        )
        w.add_class(cls)
        w.update(self._format_line(sym, colour, label, detail or "…"))
