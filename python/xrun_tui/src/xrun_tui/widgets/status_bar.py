"""Persistent global status bar.

Displays active runs per vendor plus cached vast/kaggle account info. Refreshed
periodically from the application database + cached vast user info.
"""
from __future__ import annotations

from datetime import datetime, timezone
from typing import Any

from rich.text import Text
from textual.widgets import Static


class StatusBar(Static):
    """One-line status footer that any screen can mount."""

    DEFAULT_CSS = """
    StatusBar {
        height: 1;
        background: #1e2030;
        color: #565f89;
        padding: 0 1;
    }
    """

    def __init__(self) -> None:
        super().__init__("[#565f89]…[/]")
        self._last_snapshot: dict[str, Any] | None = None

    def on_mount(self) -> None:
        self._timer = self.set_interval(5.0, self._refresh_async)
        self.run_worker(self._refresh_async(), exclusive=True)

    def on_unmount(self) -> None:
        try:
            self._timer.stop()
        except Exception:
            pass

    async def _refresh_async(self) -> None:
        # Every mounted screen carries its own bar; only the visible one
        # needs the query.
        if not self.screen.is_current:
            return
        app = self.app
        snapshot: dict[str, Any] = {}

        # Active runs from the local DB (cheap; no subprocess)
        try:
            runs = await app.db.runs(status="active")
            snapshot["active"] = len(runs)
            by_vendor: dict[str, int] = {}
            for r in runs:
                v = r.get("vendor") or "?"
                by_vendor[v] = by_vendor.get(v, 0) + 1
            snapshot["by_vendor"] = by_vendor
        except Exception:
            snapshot["active"] = None

        # Cached vendor info (set by VendorsScreen / DashboardScreen)
        cache = getattr(app, "_vast_status_cache", None)
        if isinstance(cache, dict):
            snapshot.update(cache)
        kaggle_cache = getattr(app, "_kaggle_status_cache", None)
        if isinstance(kaggle_cache, dict):
            snapshot.update(kaggle_cache)

        if not self.is_mounted:
            return
        self._render_snapshot(snapshot)

    # Not `_render`: that is Widget's own paint hook, called with no arguments.
    def _render_snapshot(self, snap: dict[str, Any]) -> None:
        parts: list[str] = []
        active = snap.get("active")
        if active is None:
            parts.append("[#565f89]db ?[/]")
        elif active:
            parts.append(f"[bold #9ece6a]● {active} active[/]")
            by_vendor = snap.get("by_vendor") or {}
            if by_vendor:
                # Local/ssh users have no vast chip; this is their only
                # sign of where the runs are.
                chips = " · ".join(
                    f"{v} {n}" for v, n in sorted(by_vendor.items()) if n
                )
                if chips:
                    parts.append(f"[#7dcfff]{chips}[/]")
        else:
            parts.append("[#565f89]· idle[/]")

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

        self._last_snapshot = snap
        now = datetime.now(timezone.utc).astimezone().strftime("%H:%M:%S")
        right = f"[#565f89]{now}[/]"
        left = "  ".join(parts) if parts else "[#565f89]…[/]"
        if not self.is_mounted:
            return
        # Right-align the clock: pad between the two halves with the room the
        # widget has (its width minus the 1+1 horizontal padding). A detached
        # or not-yet-laid-out bar has no width; fall back to a fixed gap.
        inner = self.size.width - 2
        used = (
            Text.from_markup(left).cell_len + 2 + Text.from_markup(right).cell_len
        )
        gap = max(3, inner - used) if inner > 0 else 3
        self.update(f"{left}{' ' * gap}[#2d3149]│[/] {right}")

    def on_resize(self, event: Any) -> None:
        # Width decides where the clock sits; redraw from the last data.
        if self._last_snapshot is not None:
            self._render_snapshot(self._last_snapshot)
