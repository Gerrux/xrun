from __future__ import annotations

from typing import TYPE_CHECKING

from rich.text import Text
from textual.app import ComposeResult
from textual.binding import Binding
from textual.containers import Grid, Horizontal, Vertical
from textual.screen import Screen
from textual.widgets import Button, DataTable, Footer, Static
from xrun_tui.widgets.status_bar import StatusBar

from xrun_tui.utils import cost, duration, rel_time, status_dot, status_label

if TYPE_CHECKING:
    from xrun_tui.app import XrunApp


class _TitleBar(Horizontal):
    """Custom title bar: [⊞ Menu] xrun — dashboard  (clock via StatusBar)."""

    DEFAULT_CSS = """
    _TitleBar {
        dock: top;
        height: 1;
        background: $panel;
        padding: 0 1;
        align: left middle;
    }
    _TitleBar Button {
        height: 1;
        min-width: 8;
        background: transparent;
        border: none;
        padding: 0 1;
        color: $accent;
    }
    _TitleBar Button:hover { background: $foreground 10%; }
    _TitleBar #tb-title {
        width: 1fr;
        content-align: center middle;
        color: $foreground;
    }
    """

    def compose(self) -> ComposeResult:
        yield Button("⊞ Menu", id="tb-menu")
        yield Static("xrun  —  dashboard", id="tb-title")

    def on_button_pressed(self, event: Button.Pressed) -> None:
        if event.button.id == "tb-menu":
            event.stop()
            self.run_worker(self.app.action_open_palette(), exclusive=True)  # type: ignore[attr-defined]


def _kpi(label: str, value: str, value_style: str) -> str:
    return (
        f"[#565f89]{label}[/]\n"
        f"[{value_style}]{value}[/]"
    )


class DashboardScreen(Screen):
    """Home screen — at-a-glance overview with quick navigation."""

    TITLE = "xrun"
    SUB_TITLE = "dashboard"

    BINDINGS = [
        Binding("enter",     "open_runs",       "Runs"),
        Binding("l",         "goto_launch",     "Launch"),
        Binding("i",         "goto_instances",  "Instances"),
        Binding("v",         "goto_vendors",    "Vendors"),
        Binding("h",         "goto_doctor",     "Doctor"),
        Binding("comma",     "goto_settings",   "Settings"),
        Binding("ctrl+r,f5", "refresh",         "Refresh"),
        Binding("q",         "quit_app",        "Quit"),
    ]

    def compose(self) -> ComposeResult:
        yield _TitleBar()
        with Vertical(id="dash-root"):
            with Grid(id="dash-kpi-grid"):
                yield Static(_kpi("Active runs",  "—", "#9ece6a"),
                             id="kpi-active",  classes="kpi-card")
                yield Static(_kpi("Done (last)",  "—", "#7aa2f7"),
                             id="kpi-done",    classes="kpi-card")
                yield Static(_kpi("Failed",       "—", "#f7768e"),
                             id="kpi-failed",  classes="kpi-card")
                yield Static(_kpi("Spent",        "—", "#e0af68"),
                             id="kpi-spent",   classes="kpi-card")
            with Horizontal(id="dash-cols"):
                with Vertical(id="dash-active-col"):
                    yield Static("Active runs",     classes="dash-section")
                    yield DataTable(id="dash-active",
                                    cursor_type="row", zebra_stripes=True)
                with Vertical(id="dash-recent-col"):
                    yield Static("Recently completed", classes="dash-section")
                    yield DataTable(id="dash-recent",
                                    cursor_type="row", zebra_stripes=True)
        yield StatusBar()
        yield Footer()

    def on_mount(self) -> None:
        self._setup_table("#dash-active")
        self._setup_table("#dash-recent")
        self.set_interval(5, self._refresh)
        self.call_after_refresh(self._refresh)

    def _setup_table(self, sel: str) -> None:
        t = self.query_one(sel, DataTable)
        t.add_columns(
            Text(" ",       style="#565f89"),
            Text("ID",      style="#565f89"),
            Text("Name",    style="#565f89"),
            Text("Vendor",  style="#565f89"),
            Text("Status",  style="#565f89"),
            Text("When",    style="#565f89"),
            Text("Cost",    style="#565f89"),
        )

    async def _refresh(self) -> None:
        app: XrunApp = self.app  # type: ignore[assignment]
        try:
            all_runs = await app.db.runs(status=None, limit=200)
        except Exception as exc:
            self.notify(f"DB error: {exc}", severity="error", timeout=8)
            return

        active_states = {"provisioning", "uploading", "running"}
        active = [r for r in all_runs if r["status"] in active_states][:8]
        recent = [r for r in all_runs if r["status"] not in active_states][:8]

        self._fill_table("#dash-active", active, "[#414868]No active runs[/]")
        self._fill_table("#dash-recent", recent, "[#414868]No completed runs yet[/]")
        self._update_kpis(all_runs)

    def _fill_table(self, sel: str, runs: list[dict], empty_msg: str) -> None:
        t = self.query_one(sel, DataTable)
        t.clear()
        if not runs:
            t.add_row(
                Text(""),
                Text.from_markup(empty_msg),
                *[Text("") for _ in range(5)],
            )
            return
        for r in runs:
            t.add_row(
                status_dot(r["status"]),
                Text(r["id"][:10], style="#565f89"),
                Text(r.get("name") or "", overflow="ellipsis"),
                Text(r.get("vendor") or "", style="#7dcfff"),
                status_label(r["status"]),
                Text(rel_time(r.get("started_at") or r.get("created_at")),
                     style="#565f89"),
                Text(cost(r), style="#e0af68"),
                key=r["id"],
            )

    def _update_kpis(self, runs: list[dict]) -> None:
        active = sum(1 for r in runs
                     if r["status"] in ("provisioning", "uploading", "running"))
        done   = sum(1 for r in runs if r["status"] == "done")
        failed = sum(1 for r in runs if r["status"] == "failed")
        spent  = sum(
            (r.get("cost_usd") or r.get("cost_usd_estimate") or 0.0) for r in runs
        )
        self.query_one("#kpi-active", Static).update(
            _kpi("Active runs", str(active), "bold #9ece6a" if active else "#414868")
        )
        self.query_one("#kpi-done", Static).update(
            _kpi("Done", str(done), "#7aa2f7" if done else "#414868")
        )
        self.query_one("#kpi-failed", Static).update(
            _kpi("Failed", str(failed), "bold #f7768e" if failed else "#414868")
        )
        self.query_one("#kpi-spent", Static).update(
            _kpi("Spent", f"${spent:.2f}", "#e0af68" if spent else "#414868")
        )

    def on_data_table_row_selected(
        self, event: DataTable.RowSelected
    ) -> None:
        # Click a row in either table → open run detail
        run_id = (event.row_key.value if event.row_key else None) or ""
        if not run_id:
            return
        self.run_worker(self._open_detail(run_id), exclusive=True)

    async def _open_detail(self, run_id: str) -> None:
        from xrun_tui.screens.run_detail import RunDetailScreen
        await self.app.push_screen(RunDetailScreen(run_id))

    def on_button_pressed(self, event: Button.Pressed) -> None:
        targets = {
            "nav-launch":    "go:launch",
            "nav-runs":      "go:runs",
            "nav-instances": "go:instances",
            "nav-vendors":   "go:vendors",
            "nav-doctor":    "go:doctor",
            "nav-settings":  "go:settings",
            "nav-help":      "go:help",
        }
        target = targets.get(event.button.id or "")
        if not target:
            return
        from xrun_tui.screens.palette import run_target
        self.run_worker(run_target(self.app, target), exclusive=True)

    # ── Actions ──────────────────────────────────────────────────────────────

    async def action_open_runs(self) -> None:
        from xrun_tui.screens.runs import RunsScreen
        await self.app.push_screen(RunsScreen())

    async def action_goto_launch(self) -> None:
        from xrun_tui.screens.launch import LaunchScreen
        await self.app.push_screen(LaunchScreen())

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

    def action_quit_app(self) -> None:
        self.app.exit()

    async def action_refresh(self) -> None:
        await self._refresh()
