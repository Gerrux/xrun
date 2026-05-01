from __future__ import annotations

import datetime
from typing import TYPE_CHECKING

from rich.text import Text
from textual.app import ComposeResult
from textual.binding import Binding
from textual.containers import Grid, Horizontal, Vertical
from textual.screen import Screen
from textual.widgets import DataTable, Footer, Static
from xrun_tui.widgets.status_bar import StatusBar
from xrun_tui.widgets.title_bar import TitleBar

from xrun_tui.utils import (
    cost,
    rel_time,
    status_label_for,
)

if TYPE_CHECKING:
    from xrun_tui.app import XrunApp


# ── ASCII bar chart helper ────────────────────────────────────────────────────

_BAR_CHARS = "▏▎▍▌▋▊▉█"


def _bar(fraction: float, width: int = 18) -> str:
    """Return a bar of *width* chars filled proportionally to *fraction* [0,1]."""
    if fraction <= 0:
        return "░" * width
    full_blocks = int(fraction * width)
    remainder = fraction * width - full_blocks
    partial_idx = int(remainder * len(_BAR_CHARS))
    # Clamp so we never exceed width
    bar = _BAR_CHARS[-1] * min(full_blocks, width)
    if full_blocks < width and partial_idx > 0:
        bar += _BAR_CHARS[partial_idx - 1]
    bar = bar.ljust(width, "░")
    return bar[:width]


def _kpi(label: str, value: str, value_style: str) -> str:
    return f"[#565f89]{label}[/]\n[{value_style}]{value}[/]"


# ── Screen ────────────────────────────────────────────────────────────────────

class BudgetScreen(Screen):
    """Budget and spend analytics: KPI cards, daily bar chart, top runs by cost."""

    TITLE = "xrun — budget"

    BINDINGS = [
        Binding("escape,q",  "go_back",     "Back"),
        Binding("enter",     "open_detail", "Detail"),
        Binding("j,down",    "cursor_down", "Down",    show=False),
        Binding("k,up",      "cursor_up",   "Up",      show=False),
        Binding("ctrl+r,f5", "refresh",     "Refresh"),
    ]

    def __init__(self) -> None:
        super().__init__()
        self._run_ids: list[str] = []

    def compose(self) -> ComposeResult:
        yield TitleBar("budget")
        yield Static("Budget & Spend", classes="screen-title")
        with Vertical(id="budget-root"):
            with Grid(id="budget-kpi"):
                yield Static(_kpi("Today",    "—", "#e0af68"), id="bkpi-today",   classes="kpi-card")
                yield Static(_kpi("7 days",   "—", "#e0af68"), id="bkpi-7d",      classes="kpi-card")
                yield Static(_kpi("30 days",  "—", "#e0af68"), id="bkpi-30d",     classes="kpi-card")
                yield Static(_kpi("Balance",  "—", "#7aa2f7"), id="bkpi-balance", classes="kpi-card")
            with Horizontal(id="budget-cols"):
                with Vertical(id="budget-chart-col"):
                    yield Static("Daily spend  (14 days)", classes="dash-section")
                    yield Static("", id="budget-chart", classes="budget-chart")
                with Vertical(id="budget-table-col"):
                    yield Static("Top runs by cost", classes="dash-section")
                    yield DataTable(
                        id="budget-table", cursor_type="row", zebra_stripes=True
                    )
        yield StatusBar()
        yield Footer()

    DEFAULT_CSS = """
    #budget-root {
        height: 1fr;
        padding: 0;
    }

    #budget-kpi {
        grid-size: 4 1;
        grid-gutter: 1 1;
        height: 5;
        padding: 1 2 0 2;
    }

    #budget-cols {
        height: 1fr;
        padding: 1 2;
    }

    #budget-chart-col {
        width: 40;
        margin-right: 1;
        height: 1fr;
    }

    #budget-table-col {
        width: 1fr;
        height: 1fr;
    }

    .budget-chart {
        background: #1a1b26;
        padding: 1 1;
        height: 1fr;
        color: #c0caf5;
    }

    #budget-table {
        height: 1fr;
    }
    """

    def on_mount(self) -> None:
        table = self.query_one("#budget-table", DataTable)
        table.add_columns(
            Text("Cost",    style="#565f89"),
            Text("Name",    style="#565f89"),
            Text("Vendor",  style="#565f89"),
            Text("Status",  style="#565f89"),
            Text("When",    style="#565f89"),
        )
        table.focus()
        self.call_after_refresh(self._refresh)

    async def _refresh(self) -> None:
        if not self.is_mounted:
            return

        app: XrunApp = self.app  # type: ignore[assignment]

        # ── Fetch runs ────────────────────────────────────────────────────────
        try:
            all_runs = await app.db.runs(status=None, limit=500)
        except Exception as exc:
            self.notify(f"DB error: {exc}", severity="error", timeout=8)
            return

        # ── Fetch daily spend ─────────────────────────────────────────────────
        try:
            daily = await app.db.spend_by_day(14)
        except Exception:
            daily = []

        if not self.is_mounted:
            return

        # ── KPI calculations ──────────────────────────────────────────────────
        today_str = datetime.date.today().isoformat()
        cutoff_7d = (datetime.date.today() - datetime.timedelta(days=7)).isoformat()
        cutoff_30d = (datetime.date.today() - datetime.timedelta(days=30)).isoformat()

        def _run_cost_float(r: dict) -> float:
            return float(
                r.get("cost_usd") or r.get("cost_usd_estimate") or 0.0
            )

        spend_today = sum(
            _run_cost_float(r)
            for r in all_runs
            if (r.get("created_at") or "")[:10] >= today_str
        )
        spend_7d = sum(
            _run_cost_float(r)
            for r in all_runs
            if (r.get("created_at") or "")[:10] >= cutoff_7d
        )
        spend_30d = sum(
            _run_cost_float(r)
            for r in all_runs
            if (r.get("created_at") or "")[:10] >= cutoff_30d
        )

        balance = app._vast_status_cache.get("credit")  # type: ignore[attr-defined]
        balance_str = f"${float(balance):.2f}" if balance is not None else "—"

        self.query_one("#bkpi-today",   Static).update(
            _kpi("Today",   f"${spend_today:.2f}", "#e0af68" if spend_today else "#414868")
        )
        self.query_one("#bkpi-7d",      Static).update(
            _kpi("7 days",  f"${spend_7d:.2f}",    "#e0af68" if spend_7d   else "#414868")
        )
        self.query_one("#bkpi-30d",     Static).update(
            _kpi("30 days", f"${spend_30d:.2f}",   "#e0af68" if spend_30d  else "#414868")
        )
        self.query_one("#bkpi-balance", Static).update(
            _kpi("Balance", balance_str, "#7aa2f7" if balance is not None else "#414868")
        )

        # ── ASCII bar chart ───────────────────────────────────────────────────
        max_spend = max((d["spend"] for d in daily), default=0.0)
        lines: list[str] = []
        for entry in daily:
            day_str = entry["day"]
            spend   = entry["spend"]
            is_today = day_str == today_str
            fraction = (spend / max_spend) if max_spend > 1e-12 else 0.0
            bar = _bar(fraction, 18)
            mm_dd = day_str[5:]  # MM-DD
            amount = f"${spend:.2f}"
            today_marker = "  [#565f89]← today[/]" if is_today else ""
            bar_color = "#7aa2f7" if is_today else "#e0af68"
            lines.append(
                f"[#565f89]{mm_dd}[/] [{bar_color}]{bar}[/] [#e0af68]{amount:>6}[/]{today_marker}"
            )
        chart_text = "\n".join(lines) if lines else "[#414868]No spend data[/]"
        self.query_one("#budget-chart", Static).update(chart_text)

        # ── Top runs table ────────────────────────────────────────────────────
        sorted_runs = sorted(all_runs, key=_run_cost_float, reverse=True)[:20]

        table = self.query_one("#budget-table", DataTable)
        selected_id: str | None = None
        if self._run_ids and table.cursor_row < len(self._run_ids):
            selected_id = self._run_ids[table.cursor_row]

        self._run_ids = []
        table.clear()

        for r in sorted_runs:
            self._run_ids.append(r["id"])
            table.add_row(
                Text(cost(r),                      style="#e0af68"),
                Text(r.get("name") or "",          overflow="ellipsis"),
                Text(r.get("vendor") or "",        style="#7dcfff"),
                status_label_for(r),
                Text(rel_time(r.get("created_at")), style="#565f89"),
                key=r["id"],
            )

        if selected_id and selected_id in self._run_ids:
            table.move_cursor(row=self._run_ids.index(selected_id))

    # ── Actions ───────────────────────────────────────────────────────────────

    def action_cursor_down(self) -> None:
        self.query_one(DataTable).action_cursor_down()

    def action_cursor_up(self) -> None:
        self.query_one(DataTable).action_cursor_up()

    async def action_open_detail(self) -> None:
        run_id = self._selected_run_id()
        if run_id:
            from xrun_tui.screens.run_detail import RunDetailScreen
            await self.app.push_screen(RunDetailScreen(run_id))

    async def action_refresh(self) -> None:
        await self._refresh()

    def action_go_back(self) -> None:
        self.app.pop_screen()

    def _selected_run_id(self) -> str | None:
        table = self.query_one(DataTable)
        row = table.cursor_row
        return self._run_ids[row] if row < len(self._run_ids) else None
