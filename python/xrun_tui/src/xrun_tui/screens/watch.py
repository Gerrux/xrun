from __future__ import annotations

from typing import TYPE_CHECKING

from rich.text import Text
from textual.app import ComposeResult
from textual.binding import Binding
from textual.screen import Screen
from textual.widgets import DataTable, Footer, Header, Static
from xrun_tui.widgets.status_bar import StatusBar

from xrun_tui.utils import (
    cost,
    duration,
    rel_time,
    status_dot_for,
)

if TYPE_CHECKING:
    from xrun_tui.app import XrunApp

# ── Sparkline helper ──────────────────────────────────────────────────────────

_SPARK_BARS = "▁▂▃▄▅▆▇█"


def _spark(values: list[float], n: int = 8) -> str:
    if not values:
        return ""
    tail = values[-n:]
    lo, hi = min(values), max(values)
    if hi - lo < 1e-12:
        return _SPARK_BARS[3] * len(tail)
    rng = hi - lo
    return "".join(
        _SPARK_BARS[int((v - lo) / rng * (len(_SPARK_BARS) - 1))] for v in tail
    )


# ── Screen ────────────────────────────────────────────────────────────────────

class WatchScreen(Screen):
    """Live view of active runs with their latest metrics. Auto-refreshes every 10 s."""

    TITLE = "xrun — watch"

    BINDINGS = [
        Binding("escape,q",  "go_back",      "Back"),
        Binding("enter",     "open_detail",  "Detail"),
        Binding("j,down",    "cursor_down",  "Down",    show=False),
        Binding("k,up",      "cursor_up",    "Up",      show=False),
        Binding("ctrl+r,f5", "refresh",      "Refresh"),
    ]

    def __init__(self) -> None:
        super().__init__()
        self._run_ids: list[str] = []

    def compose(self) -> ComposeResult:
        yield Header(show_clock=True)
        yield Static("Watch — active runs", classes="screen-title")
        yield Static("", id="watch-summary", classes="stats-bar")
        yield DataTable(id="watch-table", cursor_type="row", zebra_stripes=True)
        yield Static("", id="watch-empty", classes="empty-state")
        yield StatusBar()
        yield Footer()

    def on_mount(self) -> None:
        self.query_one("#watch-empty", Static).display = False
        table = self.query_one("#watch-table", DataTable)
        table.add_columns(
            Text(" ",          style="#565f89"),
            Text("ID",         style="#565f89"),
            Text("Name",       style="#565f89"),
            Text("Stage",      style="#565f89"),
            Text("Metric",     style="#565f89"),
            Text("Value",      style="#565f89"),
            Text("Spark",      style="#565f89"),
            Text("Duration",   style="#565f89"),
            Text("Cost",       style="#565f89"),
        )
        table.focus()
        self.set_interval(10, self._refresh)
        self.call_after_refresh(self._refresh)

    async def _refresh(self) -> None:
        if not self.is_mounted:
            return

        app: XrunApp = self.app  # type: ignore[assignment]
        table = self.query_one("#watch-table", DataTable)
        first_load = table.row_count == 0
        if first_load:
            table.loading = True

        try:
            runs = await app.db.runs(status="active", limit=50)
        except Exception as exc:
            self.notify(f"DB error: {exc}", severity="error", timeout=8)
            if first_load:
                table.loading = False
            return
        finally:
            if first_load and self.is_mounted:
                table.loading = False

        if not self.is_mounted:
            return

        run_ids = [r["id"] for r in runs]

        # Fetch latest metric per run
        try:
            latest_metrics = await app.db.latest_metrics_for_runs(run_ids)
        except Exception:
            latest_metrics = {}

        # Fetch last event (stage) for each run
        stage_map: dict[str, str] = {}
        for run in runs:
            try:
                events = await app.db.events(run["id"])
            except Exception:
                events = []
            if events:
                last_ev = events[-1]
                stage_map[run["id"]] = (
                    last_ev.get("stage") or last_ev.get("status") or ""
                )
            else:
                stage_map[run["id"]] = run.get("status") or ""

        # Fetch sparkline series for each run that has a metric key
        spark_map: dict[str, str] = {}
        for run_id, (key, _val) in latest_metrics.items():
            try:
                series = await app.db.metrics_for_key(run_id, key)
            except Exception:
                series = []
            if series:
                vals = [float(p.get("value", 0)) for p in series]
                spark_map[run_id] = _spark(vals)

        if not self.is_mounted:
            return

        # Preserve cursor position
        selected_id: str | None = None
        if self._run_ids and table.cursor_row < len(self._run_ids):
            selected_id = self._run_ids[table.cursor_row]

        self._run_ids = []
        table.clear()

        for run in runs:
            rid = run["id"]
            self._run_ids.append(rid)

            stage = stage_map.get(rid, "")
            metric_key, metric_val = latest_metrics.get(rid, ("", 0.0))
            spark = spark_map.get(rid, "")

            val_str = f"{metric_val:.4g}" if metric_key else "—"

            table.add_row(
                status_dot_for(run),
                Text(rid[:10],                                  style="#565f89"),
                Text(run.get("name") or "",     overflow="ellipsis"),
                Text(stage[:16],                                style="#7dcfff"),
                Text(metric_key[:16],                           style="#bb9af7"),
                Text(val_str,                                   style="#9ece6a"),
                Text(spark,                                     style="#7aa2f7"),
                Text(duration(run),                             style="#7aa2f7"),
                Text(cost(run),                                 style="#e0af68"),
                key=rid,
            )

        if selected_id and selected_id in self._run_ids:
            table.move_cursor(row=self._run_ids.index(selected_id))

        # Stats bar
        n = len(runs)
        if n:
            summary = (
                f"[bold #9ece6a]● {n} active[/]"
                f"  [#414868]┊[/]  [#565f89]auto-refresh 10s[/]"
            )
        else:
            summary = "[#414868]no active runs[/]  [#565f89]auto-refresh 10s[/]"
        self.query_one("#watch-summary", Static).update(summary)

        empty = self.query_one("#watch-empty", Static)
        has_rows = bool(runs)
        empty.display = not has_rows
        table.display = has_rows
        if not has_rows:
            empty.update("[#414868]No active runs — start one with:  xrun launch <manifest.yaml>[/]")

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
