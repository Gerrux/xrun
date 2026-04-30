from __future__ import annotations

import csv
import io
import json
from pathlib import Path
from typing import TYPE_CHECKING

from rich.text import Text
from textual.app import ComposeResult
from textual.binding import Binding
from textual.screen import Screen
from textual.widgets import DataTable, Footer, Header, Static, Tab, Tabs
from xrun_tui.widgets.fuzzy_filter import FilterBar
from xrun_tui.widgets.status_bar import StatusBar

from xrun_tui.utils import cost, duration, rel_time, status_dot, status_label

if TYPE_CHECKING:
    from xrun_tui.app import XrunApp


def _manifest_group(run: dict) -> str:
    mp = run.get("manifest_path") or ""
    if mp:
        from pathlib import Path
        return Path(mp).stem or Path(mp).name
    name = run.get("name") or ""
    if name:
        return name.split("-")[0] or name[:16]
    return "other"


def _matches(run: dict, query: str) -> bool:
    if not query:
        return True
    q = query.lower()
    haystack = " ".join([
        run.get("id") or "",
        run.get("name") or "",
        run.get("vendor") or "",
        run.get("status") or "",
    ]).lower()
    return all(word in haystack for word in q.split())


class RunsScreen(Screen):
    TITLE = "xrun"
    BINDINGS = [
        Binding("j,down",    "cursor_down",    "Down",      show=False),
        Binding("k,up",      "cursor_up",      "Up",        show=False),
        Binding("enter",     "open_detail",    "Detail"),
        Binding("escape",    "go_back",        "Back"),
        Binding("s",         "stop_run",       "Stop"),
        Binding("r",         "rerun",          "Rerun"),
        Binding("p",         "pull_run",       "Pull"),
        Binding("f,slash",   "toggle_filter",  "Filter"),
        Binding("e",         "export",         "Export"),
        Binding("c",         "compare_toggle", "Compare"),
        Binding("C",         "compare_open",   "Compare ✓", show=False),
        Binding("G",         "toggle_group",   "Group",     show=False),
        Binding("l",         "goto_launch",    "Launch"),
        Binding("i",         "goto_instances", "Instances"),
        Binding("v",         "goto_vendors",   "Vendors"),
        Binding("d",         "goto_dashboard", "Dashboard"),
        Binding("h",         "goto_doctor",    "Doctor"),
        Binding("comma",     "goto_settings",  "Settings"),
        Binding("ctrl+r,f5", "refresh",        "Refresh"),
        Binding("q",         "quit_app",       "Quit"),
    ]

    def __init__(self) -> None:
        super().__init__()
        self._runs: list[dict] = []
        self._run_ids: list[str] = []
        self._filter = "all"
        self._filter_text = ""
        self._grouped = False
        self._compare_ids: list[str] = []

    def compose(self) -> ComposeResult:
        yield Header(show_clock=True)
        yield Tabs(
            Tab("All",    id="tab-all"),
            Tab("Active", id="tab-active"),
            Tab("Recent", id="tab-recent"),
        )
        yield FilterBar(
            on_change=self._on_filter_change,
            on_close=self._on_filter_close,
            placeholder="filter by name / id / vendor / status…",
            id="runs-filter",
        )
        yield Static("", id="runs-stats", classes="stats-bar")
        yield DataTable(id="runs-table", cursor_type="row", zebra_stripes=True)
        yield Static("", id="runs-empty", classes="empty-state")
        yield StatusBar()
        yield Footer()

    def on_mount(self) -> None:
        self.query_one("#runs-empty", Static).display = False
        table = self.query_one("#runs-table", DataTable)
        table.add_columns(
            Text(" ",        style="#565f89"),
            Text("ID",       style="#565f89"),
            Text("Name",     style="#565f89"),
            Text("Vendor",   style="#565f89"),
            Text("Status",   style="#565f89"),
            Text("Started",  style="#565f89"),
            Text("Duration", style="#565f89"),
            Text("Cost",     style="#565f89"),
        )
        table.focus()
        self.set_interval(5, self._refresh)
        self.call_after_refresh(self._refresh)

    def _on_filter_change(self, value: str) -> None:
        self._filter_text = value
        self._render_table(self._runs)

    def _on_filter_close(self) -> None:
        self.query_one("#runs-table", DataTable).focus()

    async def _refresh(self) -> None:
        app: XrunApp = self.app  # type: ignore[assignment]
        try:
            from xrun_tui import config as _cfg
            limit = (_cfg.get_settings() or {}).get("history_limit", 300)
            runs = await app.db.runs(
                status=None if self._filter == "all" else self._filter,
                limit=int(limit),
            )
        except Exception as exc:
            self.notify(f"DB error: {exc}", severity="error", timeout=8)
            return
        self._runs = runs
        self._render_table(runs)

    def _render_table(self, runs: list[dict]) -> None:
        visible = [r for r in runs if _matches(r, self._filter_text)]

        table = self.query_one("#runs-table", DataTable)
        selected_id: str | None = None
        if self._run_ids and table.cursor_row < len(self._run_ids):
            selected_id = self._run_ids[table.cursor_row]

        self._run_ids = []
        table.clear()

        if self._grouped:
            groups: dict[str, list[dict]] = {}
            for r in visible:
                key = _manifest_group(r)
                groups.setdefault(key, []).append(r)
            for group_label, group_runs in groups.items():
                table.add_row(
                    Text(""),
                    Text(f"── {group_label} ──", style="bold #565f89"),
                    *[Text("") for _ in range(6)],
                )
                for run in group_runs:
                    self._run_ids.append(run["id"])
                    self._add_run_row(table, run)
        else:
            for run in visible:
                self._run_ids.append(run["id"])
                self._add_run_row(table, run)

        if selected_id and selected_id in self._run_ids:
            table.move_cursor(row=self._run_ids.index(selected_id))

        self._update_stats(visible, len(runs))

    def _add_run_row(self, table: DataTable, run: dict) -> None:
        in_cmp = run["id"] in self._compare_ids
        name_style = "bold #bb9af7" if in_cmp else ""
        marker = Text("◈", style="bold #bb9af7") if in_cmp else status_dot(run["status"])
        table.add_row(
            marker,
            Text(run["id"][:12], style="#565f89"),
            Text(run.get("name") or "", overflow="ellipsis", style=name_style),
            Text(run.get("vendor") or "", style="#7dcfff"),
            status_label(run["status"]),
            Text(rel_time(run.get("created_at")), style="#565f89"),
            Text(duration(run), style="#7aa2f7"),
            Text(cost(run), style="#e0af68"),
            key=run["id"],
        )

    def _update_stats(self, visible: list[dict], total: int) -> None:
        by_status: dict[str, int] = {}
        for r in visible:
            by_status[r["status"]] = by_status.get(r["status"], 0) + 1

        running = by_status.get("running", 0)
        done    = by_status.get("done", 0)
        failed  = by_status.get("failed", 0)
        other   = sum(v for k, v in by_status.items() if k not in ("running", "done", "failed"))
        total_cost = sum(
            (r.get("cost_usd") or r.get("cost_usd_estimate") or 0.0) for r in visible
        )

        parts: list[str] = []
        if running:
            parts.append(f"[bold #9ece6a]● {running} running[/]")
        if done:
            parts.append(f"[#565f89]✓ {done} done[/]")
        if failed:
            parts.append(f"[bold #f7768e]✗ {failed} failed[/]")
        if other:
            parts.append(f"[#e0af68]◌ {other} pending[/]")

        summary = "  ".join(parts) if parts else "[#414868]no runs[/]"
        if visible:
            if self._filter_text:
                summary += f"  [#414868]┊[/]  [#7aa2f7]{len(visible)}/{total}[/] [#565f89]matching[/]"
            else:
                summary += f"  [#414868]┊[/]  [#565f89]{total} total[/]"
        if total_cost > 0:
            summary += f"  [#e0af68]${total_cost:.2f} spent[/]"

        if self._compare_ids:
            summary += (
                f"  [#414868]┊[/]  [#bb9af7]◈ {len(self._compare_ids)} selected[/]"
                f" [#565f89](C to compare)[/]"
            )
        if self._grouped:
            summary += "  [#414868]┊[/]  [#e0af68]grouped[/]"

        self.query_one("#runs-stats", Static).update(summary)

        empty = self.query_one("#runs-empty", Static)
        table = self.query_one("#runs-table", DataTable)
        has_rows = bool(visible)
        empty.display = not has_rows
        table.display = has_rows
        if not has_rows:
            if self._filter_text:
                empty.update(f"[#414868]No runs match '{self._filter_text}'[/]")
            else:
                label = {
                    "all":    "No runs yet — launch with:  xrun launch <manifest.yaml>",
                    "active": "No active runs",
                    "recent": "No completed runs",
                }.get(self._filter, "No runs")
                empty.update(f"[#414868]{label}[/]")

    # ── Tab filter ──────────────────────────────────────────────────────────

    def on_tabs_tab_activated(self, event: Tabs.TabActivated) -> None:
        mapping = {"tab-all": "all", "tab-active": "active", "tab-recent": "recent"}
        self._filter = mapping.get(event.tab.id or "", "all")
        self.call_after_refresh(self._refresh)

    # ── Actions ─────────────────────────────────────────────────────────────

    def action_cursor_down(self) -> None:
        self.query_one(DataTable).action_cursor_down()

    def action_cursor_up(self) -> None:
        self.query_one(DataTable).action_cursor_up()

    async def action_open_detail(self) -> None:
        run_id = self._selected_run_id()
        if run_id:
            from xrun_tui.screens.run_detail import RunDetailScreen
            await self.app.push_screen(RunDetailScreen(run_id))

    async def action_stop_run(self) -> None:
        run_id = self._selected_run_id()
        if not run_id:
            return
        idx = self._run_ids.index(run_id) if run_id in self._run_ids else -1
        visible = [r for r in self._runs if _matches(r, self._filter_text)]
        if idx < 0 or idx >= len(visible):
            return
        run = visible[idx]
        if run["status"] not in ("running", "provisioning", "uploading"):
            self.notify("Run is not active", severity="warning")
            return
        from xrun_tui.screens.confirm import ConfirmScreen

        async def _do(confirmed: bool) -> None:
            if not confirmed:
                return
            from xrun_tui import cli
            ok, msg = await cli.stop_run(run_id)
            if ok:
                self.notify(f"Stopped {run_id[:8]}", severity="information")
            else:
                self.notify(f"Stop failed: {msg}", severity="error", timeout=8)
            await self._refresh()

        await self.app.push_screen(ConfirmScreen(f"Stop run {run_id[:8]}?"), _do)

    async def action_rerun(self) -> None:
        run_id = self._selected_run_id()
        if not run_id:
            return
        from xrun_tui.screens.confirm import ConfirmScreen

        async def _do(confirmed: bool) -> None:
            if not confirmed:
                return
            from xrun_tui import cli
            ok, msg = await cli.rerun_run(run_id)
            if ok:
                self.notify("Rerun launched", severity="information")
            else:
                self.notify(f"Rerun failed: {msg}", severity="error", timeout=8)
            await self._refresh()

        await self.app.push_screen(ConfirmScreen(f"Rerun {run_id[:8]}?"), _do)

    async def action_pull_run(self) -> None:
        run_id = self._selected_run_id()
        if not run_id:
            return
        from xrun_tui.screens.confirm import ConfirmScreen

        async def _do(confirmed: bool) -> None:
            if not confirmed:
                return
            from xrun_tui import services
            self.notify("Pulling latest checkpoint…", severity="information")
            ok, msg = await services.pull(run_id, ckpt="latest")
            if ok:
                self.notify("Pull complete", severity="information")
            else:
                self.notify(f"Pull failed: {msg[:80]}", severity="error", timeout=10)

        await self.app.push_screen(
            ConfirmScreen(f"Pull artifacts for {run_id[:8]}?"), _do
        )

    def action_toggle_filter(self) -> None:
        bar = self.query_one("#runs-filter", FilterBar)
        if "-visible" in bar.classes:
            bar.hide()
        else:
            bar.show()

    def action_export(self) -> None:
        visible = [r for r in self._runs if _matches(r, self._filter_text)]
        if not visible:
            self.notify("No runs to export", severity="warning")
            return
        self.run_worker(self._do_export(visible), exclusive=True)

    async def _do_export(self, runs: list[dict]) -> None:
        from xrun_tui.screens.confirm import ConfirmScreen

        async def _do(confirmed: bool) -> None:
            if not confirmed:
                return
            out = Path.cwd() / "xrun_runs_export.json"
            out.write_text(json.dumps(runs, indent=2, default=str), encoding="utf-8")
            self.notify(f"Exported {len(runs)} runs → {out.name}", severity="information")

        await self.app.push_screen(
            ConfirmScreen(f"Export {len(runs)} runs to xrun_runs_export.json?"), _do
        )

    def action_compare_toggle(self) -> None:
        run_id = self._selected_run_id()
        if not run_id:
            return
        if run_id in self._compare_ids:
            self._compare_ids.remove(run_id)
            self.notify(f"Removed {run_id[:8]} from compare", severity="information")
        else:
            if len(self._compare_ids) >= 2:
                self._compare_ids.pop(0)
            self._compare_ids.append(run_id)
            self.notify(
                f"Added {run_id[:8]} ({len(self._compare_ids)}/2)",
                severity="information",
            )
        self._render_table(self._runs)

    async def action_compare_open(self) -> None:
        if len(self._compare_ids) < 2:
            self.notify("Select 2 runs with [c] first", severity="warning")
            return
        id_a, id_b = self._compare_ids[-2], self._compare_ids[-1]
        run_a = next((r for r in self._runs if r["id"] == id_a), None)
        run_b = next((r for r in self._runs if r["id"] == id_b), None)
        if not run_a or not run_b:
            self.notify("Runs not found in current view", severity="warning")
            return
        from xrun_tui.screens.compare import CompareScreen
        await self.app.push_screen(CompareScreen(run_a, run_b))

    def action_toggle_group(self) -> None:
        self._grouped = not self._grouped
        self._render_table(self._runs)
        state = "on" if self._grouped else "off"
        self.notify(f"Grouping {state}", severity="information")

    async def action_goto_launch(self) -> None:
        from xrun_tui.screens.launch import LaunchScreen
        await self.app.push_screen(LaunchScreen())

    async def action_goto_dashboard(self) -> None:
        from xrun_tui.screens.dashboard import DashboardScreen
        await self.app.switch_screen(DashboardScreen())

    async def action_goto_doctor(self) -> None:
        from xrun_tui.screens.doctor import DoctorScreen
        await self.app.push_screen(DoctorScreen())

    async def action_goto_instances(self) -> None:
        from xrun_tui.screens.instances import InstancesScreen
        await self.app.push_screen(InstancesScreen())

    async def action_goto_vendors(self) -> None:
        from xrun_tui.screens.vendors import VendorsScreen
        await self.app.push_screen(VendorsScreen())

    async def action_goto_settings(self) -> None:
        from xrun_tui.screens.settings import SettingsScreen
        await self.app.push_screen(SettingsScreen())

    async def action_refresh(self) -> None:
        await self._refresh()

    def action_go_back(self) -> None:
        if len(self.app.screen_stack) > 1:
            self.app.pop_screen()

    def action_quit_app(self) -> None:
        self.app.exit()

    def _selected_run_id(self) -> str | None:
        table = self.query_one(DataTable)
        row = table.cursor_row
        return self._run_ids[row] if row < len(self._run_ids) else None
