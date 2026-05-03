from __future__ import annotations

import asyncio
from typing import Awaitable, Callable

from textual.app import ComposeResult
from textual.containers import Center, Middle, Vertical
from textual.screen import Screen
from textual.widgets import Static

_LOGO = r"""[bold #7aa2f7]██╗  ██╗[/][bold #bb9af7]██████╗ [/][bold #9ece6a]██╗   ██╗[/][bold #e0af68]███╗   ██╗[/]
[bold #7aa2f7]╚██╗██╔╝[/][bold #bb9af7]██╔══██╗[/][bold #9ece6a]██║   ██║[/][bold #e0af68]████╗  ██║[/]
[bold #7aa2f7] ╚███╔╝ [/][bold #bb9af7]██████╔╝[/][bold #9ece6a]██║   ██║[/][bold #e0af68]██╔██╗ ██║[/]
[bold #7aa2f7] ██╔██╗ [/][bold #bb9af7]██╔══██╗[/][bold #9ece6a]██║   ██║[/][bold #e0af68]██║╚██╗██║[/]
[bold #7aa2f7]██╔╝ ██╗[/][bold #bb9af7]██║  ██║[/][bold #9ece6a]╚██████╔╝[/][bold #e0af68]██║ ╚████║[/]
[bold #7aa2f7]╚═╝  ╚═╝[/][bold #bb9af7]╚═╝  ╚═╝[/][bold #9ece6a] ╚═════╝ [/][bold #e0af68]╚═╝  ╚═══╝[/]"""

_TAGLINE = "[#565f89]Run GPU experiments anywhere[/]"

_SPINNER = "⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏"
_STEP_W = 14


def _configured_vendors(creds: dict) -> list[str]:
    """Return the list of vendor names that have *any* credential configured.

    Splash uses this both to decide what to probe and to decide whether the
    `config` step is informative. Mirrors the logic used by the dynamic
    `xrun doctor` so the two stay consistent.
    """
    out: list[str] = []
    vast = creds.get("vast")
    if isinstance(vast, dict) and vast.get("api_key"):
        out.append("vast")
    kaggle = creds.get("kaggle")
    if isinstance(kaggle, dict) and (
        kaggle.get("token") or (kaggle.get("username") and kaggle.get("key"))
    ):
        out.append("kaggle")
    mlflow = creds.get("mlflow")
    if isinstance(mlflow, dict) and (
        mlflow.get("token")
        or (mlflow.get("username") and mlflow.get("password"))
    ):
        out.append("mlflow")
    ssh = creds.get("ssh")
    if isinstance(ssh, dict) and ssh:
        out.append("ssh")
    return out


class SplashScreen(Screen):
    """Boot screen showing real init progress."""

    DEFAULT_CSS = """
    SplashScreen {
        background: #1a1b26;
    }
    SplashScreen Middle {
        background: transparent;
    }
    SplashScreen Center {
        background: transparent;
        height: auto;
    }
    #splash-wrap {
        width: 56;
        height: auto;
    }
    #splash-logo {
        content-align: center middle;
        height: 6;
    }
    #splash-tag {
        content-align: center middle;
        height: 2;
        padding-top: 1;
    }
    #splash-steps {
        height: auto;
        padding-top: 2;
        padding-left: 16;
    }
    .splash-step    { color: #565f89; height: 1; }
    .splash-step-ok      { color: #9ece6a; }
    .splash-step-warn    { color: #e0af68; }
    .splash-step-fail    { color: #f7768e; }
    .splash-step-pending { color: #565f89; }
    #splash-version {
        content-align: center middle;
        height: 1;
        color: #414868;
        padding-top: 2;
    }
    """

    _STEPS: list[tuple[str, str]] = [
        ("db", "Database"),
        ("config", "Credentials"),
        ("vendors", "Vendors"),
        ("scan", "Manifests"),
        ("ready", "Workspace"),
    ]

    def __init__(
        self,
        on_done: Callable[[], Awaitable[None]],
        version: str = "0.2",
    ) -> None:
        super().__init__()
        self._on_done = on_done
        self._version = version
        self._spin_frame = 0
        self._running_sid: str | None = None
        self._spin_timer = None
        self._current_detail = "…"

    def compose(self) -> ComposeResult:
        with Middle():
            with Center():
                with Vertical(id="splash-wrap"):
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
        self._spin_timer = self.set_interval(0.08, self._tick_spinner)
        self.run_worker(self._init_sequence(), exclusive=True)

    def _tick_spinner(self) -> None:
        if self._running_sid is None:
            return
        self._spin_frame = (self._spin_frame + 1) % len(_SPINNER)
        try:
            w = self.query_one(f"#step-{self._running_sid}", Static)
        except Exception:
            return
        label = next(
            (l for s, l in self._STEPS if s == self._running_sid), self._running_sid
        )
        sym = _SPINNER[self._spin_frame]
        w.update(self._format_line(sym, "#e0af68", label, self._current_detail))

    async def _init_sequence(self) -> None:
        from xrun_tui import config, services

        # Refresh the version label from the actual binary so it stays in sync
        # with the installed `xrun` rather than a hardcoded constant.
        try:
            v = await services.xrun_version()
            if v:
                self._version = v
                self.query_one("#splash-version", Static).update(
                    f"[#414868]xrun[/] [#565f89]v{v}[/]"
                )
        except Exception:
            pass

        # 1) DB
        await self._set("db", "running", detail="opening…")
        try:
            assert self.app.db._conn is not None  # type: ignore[attr-defined]
            await self._set("db", "ok", detail="ready")
        except Exception as exc:
            await self._set("db", "fail", detail=str(exc)[:32])

        # 2) Config / creds — count every kind of credential, not just api_key.
        await self._set("config", "running", detail="reading…")
        try:
            creds = config.read_credentials()
            configured = _configured_vendors(creds)
            if not configured:
                await self._set("config", "warn", detail="none configured")
            else:
                await self._set(
                    "config", "ok", detail=", ".join(configured)
                )
        except Exception as exc:
            await self._set("config", "warn", detail=str(exc)[:32])

        # 3) Vendors probe — only probe what is actually configured.
        await self._set("vendors", "running", detail="probing…")
        configured = _configured_vendors(config.read_credentials())
        if not configured:
            await self._set("vendors", "warn", detail="nothing to probe")
        else:
            results: list[str] = []
            # vast: live API call (balance + user) only if api_key is set.
            api_key = config.get_vast_api_key()
            if api_key:
                try:
                    from xrun_tui.screens.vendors import _fetch_user

                    info = await asyncio.wait_for(_fetch_user(api_key), timeout=4)
                    user = info.get("username") or info.get("email") or "?"
                    credit = float(info.get("credit", 0))
                    self.app._vast_status_cache = {  # type: ignore[attr-defined]
                        "vast_user": user,
                        "vast_credit": credit,
                    }
                    results.append(f"vast ${credit:.2f}")
                except Exception:
                    results.append("vast ?")
            if "kaggle" in configured:
                results.append("kaggle")
            if "ssh" in configured:
                ssh_count = len(creds.get("ssh", {})) if isinstance(creds.get("ssh"), dict) else 0
                results.append(f"ssh×{ssh_count}" if ssh_count else "ssh")
            if "mlflow" in configured:
                results.append("mlflow")
            state = "ok" if results else "warn"
            await self._set("vendors", state, detail="  ".join(results) or "none")

        # 4) Manifest scan
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

        self._running_sid = None
        await asyncio.sleep(0.35)
        self.app.call_later(self._on_done)

    @staticmethod
    def _format_line(sym: str, sym_colour: str, label: str, detail: str) -> str:
        pad = max(1, _STEP_W - len(label))
        return (
            f"[{sym_colour}]{sym}[/]  "
            f"[#c0caf5]{label}[/]{' ' * pad}"
            f"[#565f89]{detail}[/]"
        )

    async def _set(self, sid: str, state: str, detail: str = "") -> None:
        try:
            w = self.query_one(f"#step-{sid}", Static)
        except Exception:
            return
        label = next((l for s, l in self._STEPS if s == sid), sid)
        marks = {
            "running": ("·", "#e0af68", "splash-step-pending"),
            "ok": ("✓", "#9ece6a", "splash-step-ok"),
            "warn": ("!", "#e0af68", "splash-step-warn"),
            "fail": ("✗", "#f7768e", "splash-step-fail"),
        }
        sym, colour, cls = marks.get(state, ("·", "#565f89", "splash-step-pending"))
        w.remove_class(
            "splash-step-pending",
            "splash-step-ok",
            "splash-step-warn",
            "splash-step-fail",
        )
        w.add_class(cls)
        w.update(self._format_line(sym, colour, label, detail or "…"))
        if state == "running":
            self._running_sid = sid
            self._current_detail = detail or "…"
        elif self._running_sid == sid:
            self._running_sid = None
