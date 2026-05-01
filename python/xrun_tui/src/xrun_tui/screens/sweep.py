from __future__ import annotations

from collections import Counter
from pathlib import Path
from typing import TYPE_CHECKING

from rich.text import Text
from textual.app import ComposeResult
from textual.binding import Binding
from textual.screen import Screen
from textual.widgets import DataTable, Footer, Header, Static
from xrun_tui.widgets.status_bar import StatusBar

from xrun_tui.utils import (
    cost,
    rel_time,
    status_dot_for,
    status_label_for,
)

if TYPE_CHECKING:
    from xrun_tui.app import XrunApp


# ── Grouping key ──────────────────────────────────────────────────────────────

def _group_key(run: dict) -> str:
    mp = run.get("manifest_path") or ""
    if mp:
        parent = Path(mp).parent.name
        return parent if parent not in (".", "") else Path(mp).stem
    name = run.get("name") or ""
    return name.split("-")[0] or name[:16] or "other"


# ── Screen ────────────────────────────────────────────────────────────────────

class SweepScreen(Screen):
    """Sweep results: runs grouped by manifest parent directory."""

    TITLE = "xrun — sweep"

    BINDINGS = [
        Binding("escape,q",  "go_back",     "Back"),
        Binding("enter",     "open_detail", "Detail"),
        Binding("j,down",    "cursor_down", "Down",    show=False),
        Binding("k,up",      "cursor_up",   "Up",      show=False),
        Binding("ctrl+r,f5", "refresh",     "Refresh"),
    ]

    def __init__(self) -> None:
        super().__init__()
        # None entries mark group-header rows (not selectable)
        self._run_ids: list[str | None] = []

    def compose(self) -> ComposeResult:
        yield Header(show_clock=True)
        yield Static("Sweep results", classes="screen-title")
        yield Static("", id="sweep-summary", classes="stats-bar")
        yield DataTable(id="sweep-table", cursor_type="row", zebra_stripes=True)
        yield Static("", id="sweep-empty", classes="empty-state")
        yield StatusBar()
        yield Footer()

    def on_mount(self) -> None:
        self.query_one("#sweep-empty", Static).display = False
        table = self.query_one("#sweep-table", DataTable)
        table.add_columns(
            Text(" ",       style="#565f89"),
            Text("ID",      style="#565f89"),
            Text("Name",    style="#565f89"),
            Text("Status",  style="#565f89"),
            Text("Metric",  style="#565f89"),
            Text("Value",   style="#565f89"),
            Text("Cost",    style="#565f89"),
            Text("When",    style="#565f89"),
        )
        table.focus()
        self.call_after_refresh(self._refresh)

    async def _refresh(self) -> None:
        if not self.is_mounted:
            return

        app: XrunApp = self.app  # type: ignore[assignment]
        table = self.query_one("#sweep-table", DataTable)
        first_load = table.row_count == 0
        if first_load:
            table.loading = True

        try:
            all_runs = await app.db.runs(status=None, limit=300)
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

        # ── Group runs ────────────────────────────────────────────────────────
        raw_groups: dict[str, list[dict]] = {}
        for r in all_runs:
            raw_groups.setdefault(_group_key(r), []).append(r)

        # Groups with only one run go into a synthetic "other" bucket
        groups: dict[str, list[dict]] = {}
        singletons: list[dict] = []
        for label, runs in raw_groups.items():
            if len(runs) > 1:
                groups[label] = runs
            else:
                singletons.extend(runs)
        if singletons:
            groups["other"] = singletons

        # ── Per-group: find dominant metric key and best run ──────────────────
        # key_for_group: group_label -> most common metric key across runs
        group_meta: dict[str, dict] = {}  # label -> {key, best_run_id, best_val}

        all_run_ids = [r["id"] for r in all_runs]
        try:
            latest_metrics = await app.db.latest_metrics_for_runs(all_run_ids)
        except Exception:
            latest_metrics = {}

        for label, runs in groups.items():
            # Count which metric key appears most often across runs in group
            key_counter: Counter[str] = Counter()
            for r in runs:
                mk = latest_metrics.get(r["id"])
                if mk:
                    key_counter[mk[0]] += 1
            dominant_key = key_counter.most_common(1)[0][0] if key_counter else ""

            best_run_id = ""
            best_val: float | None = None
            for r in runs:
                mk = latest_metrics.get(r["id"])
                if mk and mk[0] == dominant_key:
                    if best_val is None or mk[1] > best_val:
                        best_val = mk[1]
                        best_run_id = r["id"]

            group_meta[label] = {
                "key": dominant_key,
                "best_run_id": best_run_id,
                "best_val": best_val,
            }

        if not self.is_mounted:
            return

        # ── Preserve cursor ───────────────────────────────────────────────────
        old_run_ids = self._run_ids
        selected_id: str | None = None
        if old_run_ids and table.cursor_row < len(old_run_ids):
            candidate = old_run_ids[table.cursor_row]
            if candidate is not None:
                selected_id = candidate

        self._run_ids = []
        table.clear()

        n_sweeps = len([g for g in groups if g != "other"])
        n_total = len(all_runs)

        for label, runs in groups.items():
            meta = group_meta[label]
            dominant_key = meta["key"]
            best_val = meta["best_val"]

            # Group header row
            best_str = (
                f"best {dominant_key}: {best_val:.4g}"
                if dominant_key and best_val is not None
                else ""
            )
            n_runs_str = f"({len(runs)} runs)"
            header_text = Text.assemble(
                Text(f"── {label} ──  ", style="bold #bb9af7"),
                Text(f"{n_runs_str}  ", style="#565f89"),
                Text(best_str, style="#9ece6a"),
            )
            table.add_row(
                Text(""),
                header_text,
                Text(""), Text(""), Text(""), Text(""), Text(""), Text(""),
            )
            self._run_ids.append(None)  # not selectable

            # Child rows
            for r in runs:
                rid = r["id"]
                self._run_ids.append(rid)
                mk = latest_metrics.get(rid)
                metric_key_str = mk[0][:16] if mk else ""
                metric_val_str = f"{mk[1]:.4g}" if mk else "—"

                table.add_row(
                    status_dot_for(r),
                    Text("  " + rid[:8],               style="#565f89"),
                    Text("  " + (r.get("name") or ""), overflow="ellipsis"),
                    status_label_for(r),
                    Text(metric_key_str,                style="#bb9af7"),
                    Text(metric_val_str,                style="#9ece6a"),
                    Text(cost(r),                       style="#e0af68"),
                    Text(rel_time(r.get("created_at")), style="#565f89"),
                    key=rid,
                )

        # Restore cursor
        if selected_id and selected_id in self._run_ids:
            table.move_cursor(row=self._run_ids.index(selected_id))

        # Stats bar
        summary = (
            f"[#7aa2f7]{n_sweeps}[/] [#565f89]sweeps[/]"
            f"  [#414868]┊[/]"
            f"  [#7aa2f7]{n_total}[/] [#565f89]total runs[/]"
            f"  [#414868]┊[/]"
            f"  [#565f89][G] group view in Runs[/]"
        )
        self.query_one("#sweep-summary", Static).update(summary)

        empty = self.query_one("#sweep-empty", Static)
        has_rows = bool(all_runs)
        empty.display = not has_rows
        table.display = has_rows
        if not has_rows:
            empty.update("[#414868]No runs yet — launch with:  xrun launch <manifest.yaml>[/]")

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
        """Return the run_id at the current cursor row, or None for group headers."""
        table = self.query_one(DataTable)
        row = table.cursor_row
        if row >= len(self._run_ids):
            return None
        return self._run_ids[row]  # may be None for header rows
