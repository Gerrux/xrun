"""Persistent global status bar.

Displays vendor health, balance, active runs and burn rate. Refreshed
periodically from the application database + cached vast user info.
"""
from __future__ import annotations

from datetime import datetime, timezone
from typing import Any

from textual.widgets import Static


class StatusBar(Static):
    """One-line status footer that any screen can mount."""

    DEFAULT_CSS = """
    StatusBar {
        height: 1;
        background: #1e2030;
        color: #565f89;
        padding: 0 1;
        border-top: solid #2d3149;
    }
    """

    def __init__(self) -> None:
        super().__init__("[#414868]…[/]")

    def on_mount(self) -> None:
        self._timer = self.set_interval(5.0, self._refresh_async)
        self.run_worker(self._refresh_async(), exclusive=True)

    def on_unmount(self) -> None:
        try:
            self._timer.stop()
        except Exception:
            pass

    async def _refresh_async(self) -> None:
        app = self.app
        snapshot: dict[str, Any] = {}

        # Active runs from the local DB (cheap; no subprocess)
        try:
            runs = await app.db.runs(status="active")
            snapshot["active"] = len(runs)
            burn = 0.0
            for r in runs:
                state = r.get("state_json") or ""
                # We don't track per-run dph here; leave for instance summary below
                _ = state
            snapshot["burn"] = burn
        except Exception:
            snapshot["active"] = None

        # Cached vendor info (set by VendorsScreen / DashboardScreen)
        cache = getattr(app, "_vast_status_cache", None)
        if isinstance(cache, dict):
            snapshot.update(cache)
        kaggle_cache = getattr(app, "_kaggle_status_cache", None)
        if isinstance(kaggle_cache, dict):
            snapshot.update(kaggle_cache)

        # Theme name (for awareness)
        snapshot["theme"] = getattr(app, "theme_name", None)

        self._render(snapshot)

    def _render(self, snap: dict[str, Any]) -> None:
        parts: list[str] = []
        active = snap.get("active")
        if active is None:
            parts.append("[#414868]db ?[/]")
        elif active:
            parts.append(f"[bold #9ece6a]● {active} active[/]")
        else:
            parts.append("[#414868]· idle[/]")

        if "vast_user" in snap:
            user = snap["vast_user"]
            credit = snap.get("vast_credit")
            if user and credit is not None:
                parts.append(
                    f"[#7dcfff]vast[/] [#c0caf5]{user}[/] "
                    f"[#e0af68]${credit:.2f}[/]"
                )
            elif user:
                parts.append(f"[#7dcfff]vast[/] [#c0caf5]{user}[/]")

        if snap.get("kaggle_connected") and "kaggle_user" in snap:
            parts.append(
                f"[#bb9af7]kaggle[/] [#c0caf5]{snap['kaggle_user']}[/] [#565f89]free[/]"
            )

        burn = snap.get("vast_burn_dph")
        if burn:
            parts.append(f"[#e0af68]${burn:.3f}/h[/]")

        instances = snap.get("vast_instances")
        if instances:
            parts.append(f"[#7aa2f7]{instances} inst[/]")

        theme = snap.get("theme")
        if theme:
            parts.append(f"[#414868]theme:{theme}[/]")

        # Right-aligned clock — separator handled by spaces
        now = datetime.now(timezone.utc).astimezone().strftime("%H:%M:%S")
        right = f"[#414868]{now}[/]"
        left = "  ".join(parts) if parts else "[#414868]…[/]"
        # Best effort: pad with spaces. Static supports markup; widget width is
        # fluid so we just join with a separator.
        if not self.is_mounted:
            return
        self.update(f"{left}   [#2d3149]│[/]   {right}")
