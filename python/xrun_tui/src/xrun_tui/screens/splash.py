from __future__ import annotations

import asyncio
import time
from typing import Awaitable, Callable

from rich.text import Text
from textual.app import ComposeResult
from textual.containers import Center, Middle, Vertical
from textual.screen import Screen
from textual.widgets import Static

# The name is lowercase and one word, never caps (docs/brand.md).
_NAME = "[bold #c0caf5]xrun[/]"
_TAGLINE = "[#565f89]Run GPU experiments anywhere[/]"

# Mark sizes in pixels, largest first; half blocks give two pixels per row,
# so 32 px is 32 columns × 16 rows. The larger the mark, the smoother the
# curve. Below 16 px the curve runs into the dot; 24 is the smallest that
# reads cleanly.
_MARK_SIZES = (32, 24)
# Rows the splash needs besides the mark: name, tagline, checklist, version
# and a margin.
_SPLASH_ROWS = 13
# The boot animation's length. It is cut to the finished mark as soon as the
# init steps are done: the splash never waits for it.
_MARK_ANIM_S = 0.6


def _mark_px(height: int) -> int | None:
    """The largest mark that fits above the checklist in a terminal `height`
    rows tall; None when even the smallest does not — then the splash shows
    the checklist alone."""
    for px in _MARK_SIZES:
        if height >= px // 2 + _SPLASH_ROWS:
            return px
    return None


def _theme_bg(theme: str) -> tuple[int, int, int]:
    """The splash background under `theme`, for the mark to blend over.

    The theme filter remaps only exact Tokyo Night colours: edge cells
    blended over Tokyo's background would keep a Tokyo-tinted fringe.
    """
    from xrun_tui.themes import PALETTES, TOKYO_NIGHT

    h = PALETTES.get(theme, TOKYO_NIGHT).get("#1a1b26", "#1a1b26")
    return int(h[1:3], 16), int(h[3:5], 16), int(h[5:7], 16)

_SPINNER = "⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏"
_STEP_W = 14
# Longest the splash waits for the vast.ai balance before moving on.
_VAST_WAIT_S = 1.0


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
    # Same rule as the Vendors card: at least one host with host and user.
    from xrun_tui.screens.ssh_hosts import usable_hosts

    if usable_hosts(creds):
        out.append("ssh")
    return out


def _ssh_label(creds: dict) -> str:
    """`ssh×N`, N = hosts with both host and user — the same hosts that make
    "ssh" count as configured; a half-filled host is not counted."""
    from xrun_tui.screens.ssh_hosts import usable_hosts

    n = len(usable_hosts(creds))
    return f"ssh×{n}" if n else "ssh"


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
    #splash-mark {
        /* height: set from the mark size picked in `_fit_mark` */
        content-align: center middle;
    }
    #splash-name {
        content-align: center middle;
        height: 2;
        padding-top: 1;
    }
    #splash-tag {
        content-align: center middle;
        height: 1;
    }
    #splash-steps {
        height: auto;
        padding-top: 1;
        padding-left: 16;
    }
    .splash-step    { color: #565f89; height: 1; }
    .splash-step-ok      { color: #9ece6a; }
    .splash-step-warn    { color: #e0af68; }
    .splash-step-fail    { color: #f7768e; }
    .splash-step-pending { color: #565f89; }
    #splash-version {
        content-align: center middle;
        height: 2;
        color: #565f89;
        padding-top: 1;
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
        self._brand = None  # xrun_tui.brand once loaded; Pillow is slow to import
        self._mark_final: Text | None = None  # finished mark at `_px`
        self._px: int | None = None  # mark size picked for the terminal height
        self._mark_ok = True  # False once the renderer failed to load
        self._mark_timer = None
        self._mark_t0 = 0.0
        self._mark_bg = _theme_bg("")
        self._booted = False

    def compose(self) -> ComposeResult:
        with Middle():
            with Center():
                with Vertical(id="splash-wrap"):
                    yield Static("", id="splash-mark")
                    yield Static(_NAME, id="splash-name")
                    yield Static(_TAGLINE, id="splash-tag")
                    with Vertical(id="splash-steps"):
                        for sid, label in self._STEPS:
                            yield Static(
                                self._format_line("·", "#565f89", label, "waiting"),
                                id=f"step-{sid}",
                                classes="splash-step splash-step-pending",
                            )
                    yield Static(
                        f"[#565f89]v{self._version}[/]",
                        id="splash-version",
                    )

    def on_mount(self) -> None:
        self._fit_mark()
        self._spin_timer = self.set_interval(0.08, self._tick_spinner)
        self.run_worker(self._init_sequence(), exclusive=True)
        # Own group: the init worker is exclusive and would cancel this one.
        self.run_worker(self._load_mark(), group="mark")

    def on_resize(self) -> None:
        self._fit_mark()

    def _fit_mark(self) -> None:
        """Size the mark to the terminal height, or hide it if none fits.

        The rows are held from the first paint, before the mark is drawn: if
        they appeared with it, the centred checklist would jump.
        """
        try:
            w = self.query_one("#splash-mark", Static)
        except Exception:
            return
        px = _mark_px(self.size.height) if self._mark_ok else None
        w.display = px is not None
        if px is None or px == self._px:
            return
        self._px = px
        w.styles.height = px // 2
        if self._brand is not None:
            # A running animation draws its next frame at the new size by
            # itself; a finished one is redrawn here.
            final = self._mark_final = self._brand.cells(px, bg=self._mark_bg)
            if self._mark_timer is None:
                w.update(final)

    async def _load_mark(self) -> None:
        """Import the renderer off the event loop, then start the animation.

        Pillow takes ~0.1 s to import; on the loop that would hold the
        splash's first paint. Without Pillow the splash goes on markless.
        """
        bg = self._mark_bg = _theme_bg(getattr(self.app, "theme_name", ""))
        px = self._px or _MARK_SIZES[-1]

        def _load():
            from xrun_tui import brand

            return brand, brand.cells(px, bg=bg)

        try:
            self._brand, self._mark_final = await asyncio.to_thread(_load)
        except Exception:
            self._mark_ok = False
            self._fit_mark()
            return
        if not self.is_mounted:
            return
        if self._px is not None and self._px != px:  # resized during the load
            self._mark_final = self._brand.cells(self._px, bg=bg)
        if self._booted or self.app.animation_level != "full":
            self._finish_mark()
            return
        self._mark_t0 = time.monotonic()
        self._tick_mark()
        self._mark_timer = self.set_interval(1 / 30, self._tick_mark)

    def _tick_mark(self) -> None:
        t = (time.monotonic() - self._mark_t0) / _MARK_ANIM_S
        if t >= 1 or self._brand is None:
            self._finish_mark()
            return
        frame = self._brand.cells(
            self._px or _MARK_SIZES[-1], *self._brand.frame_at(t), bg=self._mark_bg
        )
        try:
            self.query_one("#splash-mark", Static).update(frame)
        except Exception:
            pass

    def _finish_mark(self) -> None:
        """Stop the animation wherever it is and show the finished mark."""
        if self._mark_timer is not None:
            self._mark_timer.stop()
            self._mark_timer = None
        if self._mark_final is None:
            return
        try:
            self.query_one("#splash-mark", Static).update(self._mark_final)
        except Exception:
            pass

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

    async def _show_version(self) -> None:
        # Refresh the version label from the actual binary so it stays in sync
        # with the installed `xrun` rather than a hardcoded constant.
        from xrun_tui import services
        try:
            v = await services.xrun_version()
            if v and self.is_mounted:
                self._version = v
                self.query_one("#splash-version", Static).update(
                    f"[#565f89]v{v}[/]"
                )
        except Exception:
            pass

    async def _probe_vast(self, api_key: str) -> str:
        """Balance + user from the vast.ai API; fills the status-bar cache."""
        from xrun_tui.screens.vendors import _fetch_user

        app = self.app
        try:
            info = await asyncio.wait_for(_fetch_user(api_key), timeout=4)
            user = info.get("username") or info.get("email") or "?"
            credit = float(info.get("credit", 0))
            app._vast_status_cache = {  # type: ignore[attr-defined]
                "vast_user": user,
                "vast_credit": credit,
                # Same values under the names Budget reads.
                "username": user,
                "credit": credit,
            }
            return f"vast ${credit:.2f}"
        except Exception:
            return "vast ?"

    async def _scan_manifests(self) -> None:
        from xrun_tui import services
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

    async def _init_sequence(self) -> None:
        from xrun_tui import config

        # The steps below are independent (a subprocess, an HTTPS call, a
        # directory walk), so they run side by side: the splash lasts as long
        # as the slowest one instead of their sum.
        version_task = asyncio.create_task(self._show_version())
        await self._set("scan", "running", detail="scanning…")
        scan_task = asyncio.create_task(self._scan_manifests())

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
        try:
            creds = config.read_credentials()
        except Exception:
            creds = {}  # step 2 already reported it
        configured = _configured_vendors(creds)
        if not configured:
            await self._set("vendors", "warn", detail="nothing to probe")
        else:
            results: list[str] = []
            # vast: live API call (balance + user) only if api_key is set.
            # The dashboard does not need the answer, so a slow API holds
            # the splash for at most `_VAST_WAIT_S`; the probe then finishes
            # in the background and the status bar picks the balance up
            # from the cache on its next tick.
            api_key = config.get_vast_api_key()
            if api_key:
                probe = asyncio.create_task(self._probe_vast(api_key))
                self.app._splash_probe = probe  # type: ignore[attr-defined]
                try:
                    results.append(
                        await asyncio.wait_for(asyncio.shield(probe), _VAST_WAIT_S)
                    )
                except asyncio.TimeoutError:
                    results.append("vast …")
            if "kaggle" in configured:
                results.append("kaggle")
            if "ssh" in configured:
                results.append(_ssh_label(creds))
            if "mlflow" in configured:
                results.append("mlflow")
            state = "ok" if results else "warn"
            await self._set("vendors", state, detail="  ".join(results) or "none")

        # 4) Manifest scan — started at the top, collected here.
        await scan_task
        await version_task

        # 5) Done. The mark jumps to its last frame rather than holding the
        # dashboard back, then one short beat so the finished checklist is
        # readable.
        await self._set("ready", "ok", detail="ready")
        self._running_sid = None
        self._booted = True
        self._finish_mark()
        await asyncio.sleep(0.12)
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
