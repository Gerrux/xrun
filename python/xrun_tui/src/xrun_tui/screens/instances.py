from __future__ import annotations

import asyncio
import json
from collections import Counter
from typing import TYPE_CHECKING

from rich.text import Text
from textual.app import ComposeResult
from textual.binding import Binding
from xrun_tui.live import LiveScreen
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


TAB_ALL = "tab-all"
TAB_VAST = "tab-vast"

# Local table column widths (cells are cut to these with a visible ellipsis).
LOCAL_ID_W = 30
LOCAL_ID_W_WIDE = 44  # terminals >= 140 cols
LOCAL_GPU_W = 16


def _ellipsize(text: str, width: int) -> str:
    """Cut `text` to `width` cells, marking the cut with an ellipsis."""
    return text if len(text) <= width else text[: width - 1] + "…"


class InstancesScreen(LiveScreen):
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
        # One summary line per tab; `#inst-summary` shows the active tab's
        self._summary_all = ""
        self._summary_vast = ""
        # Set by the last remote refresh; the 20 s timer skips vast without a key
        self._has_vast_key = True

    def compose(self) -> ComposeResult:
        yield TitleBar("instances")
        yield Static("Instances", classes="screen-title", id="inst-title")
        yield Static("", id="inst-summary", classes="stats-bar")
        # The DB tab is vendor-neutral, so it comes first; the vast.ai live
        # tab only matters to people with a vast key. Tables keep their ids
        # (the tcss selects on them); tabs are addressed by id, not position.
        with TabbedContent(initial=TAB_ALL, id="inst-tabs"):
            with TabPane("All vendors", id=TAB_ALL):
                yield DataTable(id="local-table", cursor_type="row", zebra_stripes=True)
            with TabPane("vast.ai (live)", id=TAB_VAST):
                yield DataTable(id="remote-table", cursor_type="row", zebra_stripes=True)
                yield Static(
                    "[#565f89]x[/] [#c0caf5]Destroy instance[/]   "
                    "[#565f89]ctrl+r[/] [#c0caf5]Refresh[/]",
                    classes="vendor-hint",
                )
        yield StatusBar()
        yield Footer()

    def on_mount(self) -> None:
        self._setup_remote_table()
        self._setup_local_table()
        self.set_interval(20, self._tick_remote)
        self.kick(self._load_all)

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
        # Fixed widths (cells are ellipsized to them) so the row never scrolls
        # sideways at 120 cols and ID gets the biggest share.
        self._local_id_w = self._id_width()
        for label, width in (
            (" ", 1), ("ID", self._local_id_w), ("Vendor", 7), ("Run", 8),
            ("GPU", LOCAL_GPU_W), ("$/hr", 6), ("Created", 9), ("State", 9),
        ):
            t.add_column(Text(label, style="#565f89"), width=width)

    def _id_width(self) -> int:
        """ID column share: wider on roomy terminals, fixed otherwise."""
        return LOCAL_ID_W_WIDE if self.app.size.width >= 140 else LOCAL_ID_W

    # ── Loading ──────────────────────────────────────────────────────────────

    async def _load_all(self) -> None:
        # Local first: it is a few ms of SQLite, the remote half is a
        # vast.ai round trip.
        await self._refresh_local()
        await self._refresh_remote()

    async def _tick_remote(self) -> None:
        # Without a key the refresh would only rebuild the "not configured"
        # row; ctrl+r and opening the vast tab re-check for a new key.
        if self._has_vast_key:
            await self._refresh_remote()

    def _show_summary(self) -> None:
        if not self.is_mounted:
            return
        active_vast = self.query_one(TabbedContent).active == TAB_VAST
        self.query_one("#inst-summary", Static).update(
            self._summary_vast if active_vast else self._summary_all
        )

    async def _refresh_remote(self) -> None:
        api_key = config.get_vast_api_key()
        self._has_vast_key = bool(api_key)
        table = self.query_one("#remote-table", DataTable)

        # Fetch before touching the table: the old rows stay readable (and
        # selectable) while the request is in flight, and two overlapping
        # refreshes cannot interleave their rows — everything below the
        # await is synchronous.
        instances: list[dict] = []
        error: Exception | None = None
        if api_key:
            try:
                from xrun_tui.screens.vendors import fetch_vast_instances
                instances = await fetch_vast_instances(api_key)
            except Exception as exc:
                error = exc
        if not self.is_mounted:
            return

        # `clear()` puts the cursor back on row 0 and the API order is not
        # stable, so remember the instance under the cursor by id: the 20 s
        # tick must not slide the highlight (and `x`) onto another instance.
        prev = self._selected_remote_instance(any_tab=True)
        prev_key = str(prev.get("id", "")) if prev else ""
        table.clear()
        self._remote_instances = []

        if not api_key:
            # Neutral: a user on local/ssh/kaggle has no reason to want a
            # vast key, their instances are in the "All vendors" tab.
            self._summary_vast = "[#565f89]vast.ai is not configured[/]"
            self._show_summary()
            table.add_row(
                Text(""),
                Text(
                    "This tab lists live vast.ai instances only. vast.ai is not "
                    "configured; instances of other vendors are in the "
                    "\"All vendors\" tab.",
                    style="#565f89",
                ),
                *[Text("") for _ in range(7)],
            )
            return

        if error is not None:
            self._summary_vast = f"[#f7768e]Error: {error}[/]"
            self._show_summary()
            table.add_row(
                Text("✗", style="#f7768e"),
                Text(str(error)[:60], style="#f7768e"),
                *[Text("") for _ in range(7)],
            )
            return

        self._remote_instances = instances
        self._render_remote_summary(instances)

        if not instances:
            table.add_row(
                Text(""), Text("No instances running on vast.ai", style="#565f89"),
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
        if prev_key:
            try:
                table.move_cursor(row=table.get_row_index(prev_key))
            except Exception:
                pass  # that instance is gone: row 0 is as good as any

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
            parts.append("[#565f89]no instances[/]")

        if total_dph > 0:
            parts.append(f"[#e0af68]${total_dph:.3f}/hr total[/]")
        if total_up > 0:
            parts.append(f"[#565f89]{_fmt_uptime(total_up)} uptime[/]")

        self._summary_vast = "  ".join(parts)
        self._show_summary()

    def _render_local_summary(self, instances: list[dict]) -> None:
        active = [i for i in instances if not i.get("destroyed_at")]
        parts: list[str] = []
        if active:
            parts.append(f"[bold #9ece6a]● {len(active)} active[/]")
            by_vendor = Counter(i.get("vendor") or "?" for i in active)
            parts.append(
                "[#7dcfff]"
                + " · ".join(f"{v} {n}" for v, n in sorted(by_vendor.items()))
                + "[/]"
            )
        elif instances:
            parts.append("[#565f89]none active[/]")
        else:
            parts.append("[#565f89]no instances recorded — they appear "
                         "after `xrun launch`[/]")
        if len(instances) > len(active):
            parts.append(f"[#565f89]{len(instances) - len(active)} destroyed[/]")
        self._summary_all = "  ".join(parts)
        self._show_summary()

    async def _refresh_local(self) -> None:
        app: XrunApp = self.app  # type: ignore[assignment]
        try:
            instances = await app.db.instances()
        except Exception as exc:
            if self.is_mounted:
                self.notify(f"DB error: {exc}", severity="error", timeout=8)
            return
        if not self.is_mounted:
            return

        table = self.query_one("#local-table", DataTable)
        table.clear()
        self._render_local_summary(instances)

        if not instances:
            table.add_row(
                Text(""),
                Text("No instances · xrun launch", style="#565f89"),
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
                Text(_ellipsize(inst.get("id") or "", self._local_id_w), style="#565f89"),
                Text(_ellipsize(inst.get("vendor") or "", 7), style="#7dcfff"),
                Text(run_id,                      style="#565f89"),
                Text(_ellipsize(gpu, LOCAL_GPU_W) if gpu else "—", style="#c0caf5"),
                Text(f"${price:.3f}" if price is not None else "—", style="#e0af68"),
                Text(rel_time(inst.get("created_at")), style="#565f89"),
                state,
            )

    # ── Tab switch ───────────────────────────────────────────────────────────

    def on_tabbed_content_tab_activated(self, event: TabbedContent.TabActivated) -> None:
        if not event.pane:
            return
        if event.pane.id == TAB_ALL:
            self.call_after_refresh(self._refresh_local)
        elif event.pane.id == TAB_VAST:
            # Also picks up a key added since the timer stopped polling
            self.call_after_refresh(self._refresh_remote)
        self.call_after_refresh(self._show_summary)

    # ── Actions ──────────────────────────────────────────────────────────────

    def _active_table(self) -> DataTable:
        tabs = self.query_one(TabbedContent)
        if tabs.active == TAB_VAST:
            return self.query_one("#remote-table", DataTable)
        return self.query_one("#local-table", DataTable)

    def _selected_remote_instance(self, *, any_tab: bool = False) -> dict | None:
        tabs = self.query_one(TabbedContent)
        if tabs.active != TAB_VAST and not any_tab:
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

    def action_refresh(self) -> None:
        self.kick(self._load_all)

    async def action_destroy(self) -> None:
        if self.query_one(TabbedContent).active != TAB_VAST:
            self.notify(
                "Destroy works on the vast.ai (live) tab; stop runs of other "
                "vendors with `s` on the Runs screen",
                severity="warning",
            )
            return
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
            ConfirmScreen(f"Destroy {gpu} (id {inst_id})?", default_no=True), _do
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
