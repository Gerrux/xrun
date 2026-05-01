from __future__ import annotations

import asyncio
import json
from typing import TYPE_CHECKING

from rich.text import Text
from textual.app import ComposeResult
from textual.binding import Binding
from textual.screen import Screen
from textual.widgets import (
    DataTable,
    Footer,
    Static,
    TabbedContent,
    TabPane,
)
from xrun_tui.widgets.status_bar import StatusBar
from xrun_tui.widgets.title_bar import TitleBar

from xrun_tui import config
from xrun_tui.utils import rel_time

if TYPE_CHECKING:
    from xrun_tui.app import XrunApp


class InstancesScreen(Screen):
    TITLE = "xrun — instances"
    BINDINGS = [
        Binding("escape,q",  "go_back",     "Back"),
        Binding("j,down",    "cursor_down", "Down",    show=False),
        Binding("k,up",      "cursor_up",   "Up",      show=False),
        Binding("ctrl+r,f5", "refresh",     "Refresh"),
        Binding("x",         "destroy",     "Destroy"),
    ]

    def __init__(self) -> None:
        super().__init__()
        self._remote_instances: list[dict] = []

    def compose(self) -> ComposeResult:
        yield TitleBar("instances")
        yield Static("Instances", classes="screen-title", id="inst-title")
        yield Static("", id="inst-summary", classes="stats-bar")
        with TabbedContent(id="inst-tabs"):
            with TabPane("Remote  (vast.ai)", id="tab-remote"):
                yield DataTable(id="remote-table", cursor_type="row", zebra_stripes=True)
                yield Static(
                    "[#565f89]x[/] [#c0caf5]Destroy instance[/]   "
                    "[#565f89]ctrl+r[/] [#c0caf5]Refresh[/]",
                    classes="vendor-hint",
                )
            with TabPane("Local  (DB)", id="tab-local"):
                yield DataTable(id="local-table", cursor_type="row", zebra_stripes=True)
        yield StatusBar()
        yield Footer()

    def on_mount(self) -> None:
        self._setup_remote_table()
        self._setup_local_table()
        self.set_interval(20, self._refresh_remote)
        self.call_after_refresh(self._load_all)

    # ── Column setup ─────────────────────────────────────────────────────────

    def _setup_remote_table(self) -> None:
        t = self.query_one("#remote-table", DataTable)
        t.add_columns(
            Text(" ",       style="#565f89"),
            Text("ID",      style="#565f89"),
            Text("GPU",     style="#565f89"),
            Text("#",       style="#565f89"),
            Text("$/hr",    style="#565f89"),
            Text("Status",  style="#565f89"),
            Text("Uptime",  style="#565f89"),
            Text("SSH",     style="#565f89"),
            Text("Region",  style="#565f89"),
        )

    def _setup_local_table(self) -> None:
        t = self.query_one("#local-table", DataTable)
        t.add_columns(
            Text(" ",       style="#565f89"),
            Text("ID",      style="#565f89"),
            Text("Vendor",  style="#565f89"),
            Text("Run",     style="#565f89"),
            Text("GPU",     style="#565f89"),
            Text("$/hr",    style="#565f89"),
            Text("Created", style="#565f89"),
            Text("State",   style="#565f89"),
        )

    # ── Loading ──────────────────────────────────────────────────────────────

    async def _load_all(self) -> None:
        await self._refresh_remote()
        await self._refresh_local()

    async def _refresh_remote(self) -> None:
        api_key = config.get_vast_api_key()
        table = self.query_one("#remote-table", DataTable)
        table.clear()
        self._remote_instances = []

        if not api_key:
            self.query_one("#inst-summary", Static).update(
                "[#565f89]No API key configured — press [/][#7aa2f7]v[/][#565f89] to open Vendors[/]"
            )
            table.add_row(
                Text(""), Text("No API key — go to Vendors [v]", style="#414868"),
                *[Text("") for _ in range(7)],
            )
            return

        try:
            from xrun_tui.screens.vendors import fetch_vast_instances
            instances = await fetch_vast_instances(api_key)
        except Exception as exc:
            self.query_one("#inst-summary", Static).update(f"[#f7768e]Error: {exc}[/]")
            table.add_row(
                Text("✗", style="#f7768e"),
                Text(str(exc)[:60], style="#f7768e"),
                *[Text("") for _ in range(7)],
            )
            return

        self._remote_instances = instances
        self._render_remote_summary(instances)

        if not instances:
            table.add_row(
                Text(""), Text("No instances running on vast.ai", style="#414868"),
                *[Text("") for _ in range(7)],
            )
            return

        for inst in instances:
            status     = inst.get("actual_status") or inst.get("cur_state") or "?"
            is_running = status == "running"
            dot        = Text("●", style="bold #9ece6a" if is_running else "#565f89")
            status_t   = Text(status, style="bold #9ece6a" if is_running else "#565f89")

            gpu        = inst.get("gpu_name") or "—"
            num_gpus   = inst.get("num_gpus") or 1
            dph        = inst.get("dph_total")
            uptime     = _fmt_uptime(inst.get("duration") or 0)
            ssh_host   = inst.get("ssh_host") or ""
            ssh_port   = inst.get("ssh_port")
            ssh        = f"{ssh_host}:{ssh_port}" if ssh_host and ssh_port else (ssh_host or "—")
            region     = (inst.get("geolocation") or "—")[:18]

            table.add_row(
                dot,
                Text(str(inst.get("id", "—")), style="#565f89"),
                Text(gpu[:24], style="#c0caf5"),
                Text(str(num_gpus), style="#7aa2f7"),
                Text(f"${dph:.3f}" if dph is not None else "—", style="#e0af68"),
                status_t,
                Text(uptime, style="#565f89"),
                Text(ssh[:26], style="#7dcfff"),
                Text(region, style="#565f89"),
                key=str(inst.get("id", "")),
            )

    def _render_remote_summary(self, instances: list[dict]) -> None:
        running   = [i for i in instances if (i.get("actual_status") or "") == "running"]
        total_dph = sum(i.get("dph_total") or 0 for i in running)
        total_up  = sum(i.get("duration") or 0 for i in running)

        parts: list[str] = []
        if running:
            parts.append(f"[bold #9ece6a]● {len(running)} running[/]")
        elif instances:
            parts.append(f"[#565f89]{len(instances)} instances[/]")
        else:
            parts.append("[#414868]no instances[/]")

        if total_dph > 0:
            parts.append(f"[#e0af68]${total_dph:.3f}/hr total[/]")
        if total_up > 0:
            parts.append(f"[#565f89]{_fmt_uptime(total_up)} uptime[/]")

        self.query_one("#inst-summary", Static).update("  ".join(parts))

    async def _refresh_local(self) -> None:
        app: XrunApp = self.app  # type: ignore[assignment]
        try:
            instances = await app.db.instances()
        except Exception as exc:
            self.notify(f"DB error: {exc}", severity="error", timeout=8)
            return

        table = self.query_one("#local-table", DataTable)
        table.clear()

        if not instances:
            table.add_row(
                Text(""), Text("No local instances in DB", style="#414868"),
                *[Text("") for _ in range(6)],
            )
            return

        for inst in instances:
            is_active = not inst.get("destroyed_at")
            dot   = Text("●", style="bold #9ece6a" if is_active else "#565f89")
            state = Text("active" if is_active else "destroyed",
                         style="bold #9ece6a" if is_active else "#565f89")

            gpu = inst.get("gpu_type") or ""
            if not gpu and inst.get("state_json"):
                try:
                    sj  = json.loads(inst["state_json"])
                    gpu = sj.get("gpu") or sj.get("gpu_type") or ""
                except Exception:
                    pass

            price  = inst.get("price_per_hour")
            run_id = (inst.get("run_id") or "—")
            if run_id != "—":
                run_id = run_id[:8]

            table.add_row(
                dot,
                Text((inst.get("id") or "")[:20], style="#565f89"),
                Text(inst.get("vendor") or "",    style="#7dcfff"),
                Text(run_id,                      style="#565f89"),
                Text(gpu[:20] if gpu else "—",    style="#c0caf5"),
                Text(f"${price:.3f}" if price is not None else "—", style="#e0af68"),
                Text(rel_time(inst.get("created_at")), style="#565f89"),
                state,
            )

    # ── Tab switch ───────────────────────────────────────────────────────────

    def on_tabbed_content_tab_activated(self, event: TabbedContent.TabActivated) -> None:
        if event.pane and event.pane.id == "tab-local":
            self.call_after_refresh(self._refresh_local)

    # ── Actions ──────────────────────────────────────────────────────────────

    def _active_table(self) -> DataTable:
        tabs = self.query_one(TabbedContent)
        if tabs.active == "tab-remote":
            return self.query_one("#remote-table", DataTable)
        return self.query_one("#local-table", DataTable)

    def _selected_remote_instance(self) -> dict | None:
        tabs = self.query_one(TabbedContent)
        if tabs.active != "tab-remote":
            return None
        table = self.query_one("#remote-table", DataTable)
        row = table.cursor_row
        if 0 <= row < len(self._remote_instances):
            return self._remote_instances[row]
        return None

    def action_cursor_down(self) -> None:
        self._active_table().action_cursor_down()

    def action_cursor_up(self) -> None:
        self._active_table().action_cursor_up()

    def action_go_back(self) -> None:
        self.app.pop_screen()

    async def action_refresh(self) -> None:
        await self._load_all()

    async def action_destroy(self) -> None:
        inst = self._selected_remote_instance()
        if not inst:
            self.notify("Select a remote instance first", severity="warning")
            return
        inst_id = inst.get("id")
        if not inst_id:
            return
        from xrun_tui.screens.confirm import ConfirmScreen

        async def _do(confirmed: bool) -> None:
            if not confirmed:
                return
            ok, msg = await _vast_destroy(inst_id)
            if ok:
                self.notify(f"Instance {inst_id} destroyed", severity="information")
                await self._refresh_remote()
            else:
                self.notify(f"Destroy failed: {msg[:80]}", severity="error", timeout=8)

        gpu = inst.get("gpu_name") or str(inst_id)
        await self.app.push_screen(
            ConfirmScreen(f"Destroy {gpu} (id {inst_id})?"), _do
        )


# ── Helpers ───────────────────────────────────────────────────────────────────

def _fmt_uptime(secs: float) -> str:
    s = int(secs)
    if s < 60:
        return f"{s}s"
    if s < 3600:
        return f"{s // 60}m"
    if s < 86400:
        return f"{s // 3600}h {(s % 3600) // 60}m"
    return f"{s // 86400}d {(s % 86400) // 3600}h"


async def _vast_destroy(instance_id: int | str) -> tuple[bool, str]:
    """Call vast.ai REST API to destroy an instance."""
    import urllib.request
    api_key = config.get_vast_api_key()
    if not api_key:
        return False, "no API key configured"

    def _do() -> tuple[bool, str]:
        req = urllib.request.Request(
            f"https://console.vast.ai/api/v0/instances/{instance_id}/",
            method="DELETE",
            headers={"Authorization": f"Bearer {api_key}"},
        )
        try:
            with urllib.request.urlopen(req, timeout=15) as r:
                return True, ""
        except urllib.request.HTTPError as e:
            return False, f"HTTP {e.code}: {e.reason}"

    return await asyncio.to_thread(_do)
