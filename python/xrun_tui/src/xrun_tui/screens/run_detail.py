from __future__ import annotations

from typing import TYPE_CHECKING, Any

from rich.syntax import Syntax
from rich.text import Text
from textual.app import ComposeResult
from textual.binding import Binding
from textual.containers import Horizontal, Vertical
from textual.screen import Screen
from textual.widgets import (
    Button,
    DataTable,
    Footer,
    Header,
    RichLog,
    Rule,
    Static,
    TabbedContent,
    TabPane,
)
from xrun_tui.widgets.status_bar import StatusBar

from xrun_tui.utils import (
    EVENT_STATUS_STYLE,
    cost,
    duration,
    rel_time,
)

if TYPE_CHECKING:
    from xrun_tui.app import XrunApp


class RunDetailScreen(Screen):
    BINDINGS = [
        Binding("escape,q", "go_back",        "Back"),
        Binding("s",        "stop_run",       "Stop"),
        Binding("r",        "rerun",          "Rerun"),
        Binding("p",        "pull",           "Pull"),
        Binding("a",        "artifacts",      "Artifacts"),
        Binding("ctrl+r",   "refresh",        "Refresh", show=False),
        Binding("1",        "tab_stages",     show=False),
        Binding("2",        "tab_logs",       show=False),
        Binding("3",        "tab_manifest",   show=False),
        Binding("4",        "tab_metrics",    show=False),
    ]

    def __init__(self, run_id: str) -> None:
        super().__init__()
        self._run_id = run_id
        self._run: dict | None = None
        self._log_timer: Any = None
        self._log_tab_active = False

    def compose(self) -> ComposeResult:
        yield Header(show_clock=True)
        with Vertical(id="detail-header"):
            with Horizontal(id="detail-title-row"):
                yield Static("", id="run-name",  classes="detail-name")
                yield Static("", id="run-badge", classes="detail-badge")
            with Horizontal(id="detail-chips"):
                yield Static("", id="chip-id",        classes="chip")
                yield Static("", id="chip-vendor",    classes="chip")
                yield Static("", id="chip-started",   classes="chip")
                yield Static("", id="chip-duration",  classes="chip")
                yield Static("", id="chip-cost",      classes="chip")
                yield Static("", id="chip-projected", classes="chip")
            yield Rule(classes="detail-rule")
            with Horizontal(id="detail-actions"):
                yield Button("Stop  [s]",       id="btn-stop",      classes="action-btn danger")
                yield Button("Rerun [r]",       id="btn-rerun",     classes="action-btn")
                yield Button("Pull  [p]",       id="btn-pull",      classes="action-btn")
                yield Button("Artifacts [a]",   id="btn-artifacts", classes="action-btn")
                yield Button("Relaunch",        id="btn-relaunch",  classes="action-btn")
        with TabbedContent(id="detail-tabs"):
            with TabPane("Stages [1]", id="tab-stages"):
                yield DataTable(id="stages-table",
                                cursor_type="row", zebra_stripes=True)
            with TabPane("Logs [2]", id="tab-logs"):
                yield RichLog(id="logs-view",
                              highlight=False, markup=True, wrap=True)
            with TabPane("Manifest [3]", id="tab-manifest"):
                yield RichLog(id="manifest-view",
                              highlight=True, markup=False, wrap=False)
            with TabPane("Metrics [4]", id="tab-metrics"):
                yield Static("", id="metrics-summary", classes="stats-bar")
                yield DataTable(id="metrics-table",
                                cursor_type="row", zebra_stripes=True)
                yield RichLog(id="metrics-chart",
                              highlight=False, markup=False, wrap=False)
        yield StatusBar()
        yield Footer()

    def on_mount(self) -> None:
        t = self.query_one("#stages-table", DataTable)
        t.add_columns(
            Text("Time",    style="#565f89"),
            Text("Stage",   style="#565f89"),
            Text("Status",  style="#565f89"),
            Text("Message", style="#565f89"),
        )
        m = self.query_one("#metrics-table", DataTable)
        m.add_columns(
            Text("Key",     style="#565f89"),
            Text("Points",  style="#565f89"),
            Text("Latest",  style="#565f89"),
            Text("Spark",   style="#565f89"),
        )
        self.call_after_refresh(self._load_run)

    def on_unmount(self) -> None:
        self._stop_log_poll()

    # ── Loading ──────────────────────────────────────────────────────────────

    async def _load_run(self) -> None:
        app: XrunApp = self.app  # type: ignore[assignment]
        self._run = await app.db.run(self._run_id)
        if not self._run:
            self.notify("Run not found", severity="error")
            return
        self._render_header()
        await self._load_stages()

    def _render_header(self) -> None:
        run = self._run
        if not run:
            return
        name = run.get("name") or run["id"][:16]
        self.query_one("#run-name", Static).update(f"[bold #c0caf5]{name}[/]")

        sym, style = {
            "running":   ("● running",   "bold #9ece6a"),
            "done":      ("✓ done",      "#565f89"),
            "failed":    ("✗ failed",    "bold #f7768e"),
            "cancelled": ("○ cancelled", "#bb9af7"),
        }.get(run["status"], (run["status"], "#c0caf5"))
        self.query_one("#run-badge", Static).update(f"[{style}]{sym}[/]")

        def chip(label: str, value: str, val_style: str = "#c0caf5") -> str:
            return f"[#565f89]{label}[/] [{val_style}]{value}[/]"

        self.query_one("#chip-id",       Static).update(
            chip("id",     run["id"][:14], "#565f89"))
        self.query_one("#chip-vendor",   Static).update(
            chip("vendor", run.get("vendor") or "?", "#7dcfff"))
        self.query_one("#chip-started",  Static).update(
            chip("started", rel_time(
                run.get("started_at") or run.get("created_at"))))
        self.query_one("#chip-duration", Static).update(
            chip("duration", duration(run), "#7aa2f7"))
        self.query_one("#chip-cost",     Static).update(
            chip("cost", cost(run), "#e0af68"))

        # Cost projection for active runs
        proj_chip = self.query_one("#chip-projected", Static)
        is_active = run["status"] in ("running", "provisioning", "uploading")
        if is_active:
            price = run.get("price_per_hour") or run.get("dph_total") or 0.0
            if price:
                proj_chip.update(chip("~$/hr", f"${price:.3f}", "#e0af68"))
            else:
                proj_chip.update("")
        else:
            proj_chip.update("")

        self.query_one("#btn-stop", Button).disabled = not is_active
        self.query_one("#btn-pull", Button).disabled = run["status"] not in (
            "running", "done"
        )
        has_manifest = bool(run.get("manifest_path"))
        self.query_one("#btn-relaunch", Button).disabled = not has_manifest

    async def _load_stages(self) -> None:
        app: XrunApp = self.app  # type: ignore[assignment]
        events = await app.db.events(self._run_id)
        table = self.query_one("#stages-table", DataTable)
        table.clear()
        if not events:
            table.add_row(
                Text("—", style="#414868"),
                Text("no events yet", style="#414868"),
                Text(""), Text(""),
            )
            return
        for ev in events:
            ts    = (ev.get("ts") or "")[:19].replace("T", " ")
            stage = ev.get("stage") or ""
            st    = (ev.get("status") or "").lower()
            msg   = (ev.get("msg") or "")[:120]
            style = EVENT_STATUS_STYLE.get(st, "#c0caf5")
            table.add_row(
                Text(ts,    style="#565f89"),
                Text(stage, style="#7aa2f7"),
                Text(st,    style=style),
                Text(msg,   style="#c0caf5"),
            )

    async def _load_logs(self) -> None:
        from xrun_tui import services
        log = self.query_one("#logs-view", RichLog)
        content = await services.get_logs(self._run_id)
        log.clear()
        log.write(content)
        # Auto-scroll to bottom while run is active
        if self._run and self._run.get("status") in ("running", "provisioning", "uploading"):
            log.scroll_end(animate=False)

    def _start_log_poll(self) -> None:
        if self._log_timer is None:
            self._log_timer = self.set_interval(2.0, self._poll_logs)

    def _stop_log_poll(self) -> None:
        if self._log_timer is not None:
            try:
                self._log_timer.stop()
            except Exception:
                pass
            self._log_timer = None

    async def _poll_logs(self) -> None:
        if self._log_tab_active and self.is_mounted:
            await self._load_logs()

    async def _load_manifest(self) -> None:
        if not self._run:
            return
        from xrun_tui import services
        view = self.query_one("#manifest-view", RichLog)
        view.clear()
        content = services.read_manifest(self._run.get("manifest_path") or "")
        view.write(Syntax(content, "yaml", theme="nord", line_numbers=True))

    async def _load_metrics(self) -> None:
        from xrun_tui import services
        from xrun_tui.widgets.ascii_chart import render_chart
        summary = self.query_one("#metrics-summary", Static)
        table   = self.query_one("#metrics-table",   DataTable)
        chart   = self.query_one("#metrics-chart",   RichLog)
        summary.update("[#e0af68]Loading metrics…[/]")
        table.clear()
        chart.clear()

        ok, keys, err = await services.metrics(self._run_id)
        if not ok:
            summary.update(f"[#f7768e]Error:[/] {err[:120]}")
            return
        if not keys:
            summary.update("[#414868]No metrics emitted by this run yet[/]")
            return

        summary.update(
            f"[#7aa2f7]{len(keys)}[/] metric keys  "
            f"[#414868]┊[/]  [#565f89]↑/↓ select a row to chart it[/]"
        )

        self._metrics_series: dict[str, list[float]] = {}
        for entry in keys:
            k     = entry.get("key") or "?"
            count = entry.get("count") or 0
            ok2, series, _ = await services.metrics(self._run_id, key=k)
            latest = "—"
            spark  = ""
            if ok2 and isinstance(series, list) and series:
                vals = [float(p.get("value", 0)) for p in series]
                self._metrics_series[k] = vals
                latest = f"{vals[-1]:.4g}"
                spark  = _sparkline(vals[-40:])
            table.add_row(
                Text(k,           style="#c0caf5"),
                Text(str(count),  style="#7aa2f7"),
                Text(latest,      style="#9ece6a"),
                Text(spark,       style="#7dcfff"),
                key=k,
            )

        # Chart the first metric by default
        first_key = list(self._metrics_series.keys())[0] if self._metrics_series else None
        if first_key:
            self._render_chart(first_key)

    def _render_chart(self, key: str) -> None:
        from xrun_tui.widgets.ascii_chart import render_chart
        chart = self.query_one("#metrics-chart", RichLog)
        chart.clear()
        vals = getattr(self, "_metrics_series", {}).get(key)
        if not vals:
            return
        rendered = render_chart(vals, width=56, height=10, title=key, color="#7aa2f7")
        chart.write(rendered)

    def on_data_table_row_highlighted(
        self, event: DataTable.RowHighlighted
    ) -> None:
        if event.data_table.id == "metrics-table":
            key = (event.row_key.value if event.row_key else None) or ""
            if key:
                self._render_chart(key)

    # ── Events ───────────────────────────────────────────────────────────────

    def on_tabbed_content_tab_activated(
        self, event: TabbedContent.TabActivated
    ) -> None:
        pid = event.pane.id if event.pane else None
        self._log_tab_active = (pid == "tab-logs")
        if pid == "tab-logs":
            self._start_log_poll()
            self.run_worker(self._load_logs(), exclusive=True)
        else:
            self._stop_log_poll()
            if pid == "tab-manifest":
                self.call_after_refresh(self._load_manifest)
            elif pid == "tab-metrics":
                self.call_after_refresh(self._load_metrics)

    def on_button_pressed(self, event: Button.Pressed) -> None:
        match event.button.id:
            case "btn-stop":      self.run_worker(self.action_stop_run())
            case "btn-rerun":     self.run_worker(self.action_rerun())
            case "btn-pull":      self.run_worker(self.action_pull())
            case "btn-artifacts": self.run_worker(self.action_artifacts())
            case "btn-relaunch":  self.run_worker(self._do_relaunch())

    # ── Actions ──────────────────────────────────────────────────────────────

    def action_go_back(self) -> None:
        self.app.pop_screen()

    async def action_stop_run(self) -> None:
        if not self._run:
            return
        from xrun_tui.screens.confirm import ConfirmScreen

        async def _do(confirmed: bool) -> None:
            if not confirmed:
                return
            from xrun_tui import services
            ok, msg = await services.stop_run(self._run_id)
            if ok:
                self.notify(f"Stopped {self._run_id[:8]}", severity="information")
                await self._load_run()
            else:
                self.notify(f"Stop failed: {msg}", severity="error", timeout=8)

        await self.app.push_screen(
            ConfirmScreen(f"Stop {self._run.get('name', self._run_id[:8])}?"), _do
        )

    async def action_rerun(self) -> None:
        if not self._run:
            return
        from xrun_tui.screens.confirm import ConfirmScreen

        async def _do(confirmed: bool) -> None:
            if not confirmed:
                return
            from xrun_tui import services
            ok, msg = await services.rerun_run(self._run_id)
            if ok:
                self.notify("Rerun launched", severity="information")
            else:
                self.notify(f"Rerun failed: {msg}", severity="error", timeout=8)

        await self.app.push_screen(
            ConfirmScreen(f"Rerun {self._run.get('name', self._run_id[:8])}?"), _do
        )

    async def action_pull(self) -> None:
        if not self._run:
            return
        from xrun_tui.screens.confirm import ConfirmScreen

        async def _do(confirmed: bool) -> None:
            if not confirmed:
                return
            from xrun_tui import services
            self.notify("Pulling artifacts…", severity="information")
            ok, msg = await services.pull(self._run_id, ckpt="latest")
            if ok:
                self.notify("Pull complete", severity="information")
            else:
                self.notify(f"Pull failed: {msg[:80]}", severity="error", timeout=10)

        await self.app.push_screen(
            ConfirmScreen(f"Pull latest checkpoint for {self._run_id[:8]}?"), _do
        )

    async def action_artifacts(self) -> None:
        if not self._run:
            return
        from xrun_tui.screens.artifacts import ArtifactsScreen
        name = self._run.get("name") or self._run_id[:12]
        await self.app.push_screen(ArtifactsScreen(self._run_id, name))

    async def _do_relaunch(self) -> None:
        if not self._run:
            return
        manifest = self._run.get("manifest_path") or ""
        if not manifest:
            self.notify("No manifest path recorded for this run", severity="warning")
            return
        from xrun_tui.screens.confirm import ConfirmScreen
        from pathlib import Path as _Path

        async def _do(confirmed: bool) -> None:
            if not confirmed:
                return
            from xrun_tui import services
            ok, msg = await services.launch(manifest)
            if ok:
                self.notify("Relaunched", severity="information")
            else:
                excerpt = (msg or "").splitlines()[-1] if msg else ""
                self.notify(f"Relaunch failed: {excerpt[:80]}", severity="error", timeout=8)

        await self.app.push_screen(
            ConfirmScreen(
                f"Relaunch from {_Path(manifest).name}?"
            ),
            _do,
        )

    async def action_refresh(self) -> None:
        await self._load_run()

    def action_tab_stages(self) -> None:
        self.query_one(TabbedContent).active = "tab-stages"

    def action_tab_logs(self) -> None:
        self.query_one(TabbedContent).active = "tab-logs"

    def action_tab_manifest(self) -> None:
        self.query_one(TabbedContent).active = "tab-manifest"

    def action_tab_metrics(self) -> None:
        self.query_one(TabbedContent).active = "tab-metrics"


# ── Helpers ───────────────────────────────────────────────────────────────────

_SPARK_BARS = "▁▂▃▄▅▆▇█"


def _sparkline(values: list[float]) -> str:
    if not values:
        return ""
    lo, hi = min(values), max(values)
    if hi - lo < 1e-12:
        return _SPARK_BARS[3] * len(values)
    n = len(_SPARK_BARS) - 1
    return "".join(
        _SPARK_BARS[int((v - lo) / (hi - lo) * n)] for v in values
    )
