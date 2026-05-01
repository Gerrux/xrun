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
    Input,
    RichLog,
    Rule,
    Static,
    TabbedContent,
    TabPane,
)
from xrun_tui.widgets.status_bar import StatusBar
from xrun_tui.widgets.title_bar import TitleBar

from xrun_tui.utils import (
    EVENT_STATUS_STYLE,
    cost,
    duration,
    is_stale,
    rel_time,
)

if TYPE_CHECKING:
    from xrun_tui.app import XrunApp


class RunDetailScreen(Screen):
    BINDINGS = [
        Binding("escape,q", "go_back",        "Back"),
        Binding("s",        "stop_run",       "Stop"),
        Binding("S",        "sync_status",    "Sync"),
        Binding("r",        "rerun",          "Rerun"),
        Binding("R",        "patch_rerun",    "Patch-rerun", show=False),
        Binding("E",        "error_detail",   "Error",       show=False),
        Binding("p",        "pull",           "Pull"),
        Binding("a",        "artifacts",      "Artifacts"),
        Binding("ctrl+f",   "toggle_search",  "Search",      show=False),
        Binding("ctrl+r",   "refresh",        "Refresh",     show=False),
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
        self._log_lines: list[str] = []
        self._search_active = False

    def compose(self) -> ComposeResult:
        yield TitleBar("run detail")
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
                yield Button("Patch [R]",       id="btn-patch",     classes="action-btn")
                yield Button("Pull  [p]",       id="btn-pull",      classes="action-btn")
                yield Button("Artifacts [a]",   id="btn-artifacts", classes="action-btn")
                yield Button("Relaunch",        id="btn-relaunch",  classes="action-btn")
                yield Button("Error detail [E]",id="btn-error",     classes="action-btn danger")
        with TabbedContent(id="detail-tabs"):
            with TabPane("Stages [1]", id="tab-stages"):
                yield DataTable(id="stages-table",
                                cursor_type="row", zebra_stripes=True)
            with TabPane("Logs [2]", id="tab-logs"):
                yield RichLog(id="logs-view",
                              highlight=False, markup=True, wrap=True)
                yield Input(
                    id="log-search-input",
                    placeholder="/ search… (Ctrl+F to toggle)",
                    classes="log-search-input",
                )
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
        # Hide search input and extra buttons by default
        self.query_one("#log-search-input", Input).display = False
        self.query_one("#btn-patch",  Button).display = False
        self.query_one("#btn-error",  Button).display = False

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
        try:
            self._run = await app.db.run(self._run_id)
        except Exception as exc:
            self.notify(f"DB error: {exc}", severity="error", timeout=8)
            return
        if not self._run:
            self.notify("Run not found", severity="error")
            return
        self._render_header()
        table = self.query_one("#stages-table", DataTable)
        table.loading = True
        try:
            await self._load_stages()
        finally:
            table.loading = False

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
        if is_stale(run):
            badge = (
                f"[{style}]{sym}[/]  "
                "[bold #e0af68]⚠ stale (S to sync)[/]"
            )
        else:
            badge = f"[{style}]{sym}[/]"
        self.query_one("#run-badge", Static).update(badge)

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

        self.query_one("#btn-stop",  Button).disabled = not is_active
        self.query_one("#btn-pull",  Button).disabled = run["status"] not in ("running", "done")
        has_manifest = bool(run.get("manifest_path"))
        self.query_one("#btn-relaunch", Button).disabled = not has_manifest
        self.query_one("#btn-patch",    Button).display = has_manifest
        self.query_one("#btn-error",    Button).display = (run["status"] == "failed")

    async def _load_stages(self) -> None:
        app: XrunApp = self.app  # type: ignore[assignment]
        table = self.query_one("#stages-table", DataTable)
        table.clear()
        try:
            events = await app.db.events(self._run_id)
        except Exception as exc:
            table.add_row(
                Text("✗", style="#f7768e"),
                Text(f"error: {exc}", style="#f7768e"),
                Text(""), Text(""),
            )
            return
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

    _TERMINAL_STATUSES = frozenset({"done", "succeeded", "failed", "cancelled"})
    _ACTIVE_STATUSES = frozenset({"running", "provisioning", "uploading"})

    async def _load_logs(self) -> None:
        app: XrunApp = self.app  # type: ignore[assignment]
        log = self.query_one("#logs-view", RichLog)
        log_path = app.db.log_path(self._run_id)
        if log_path.exists():
            try:
                raw = log_path.read_text(encoding="utf-8", errors="replace")
                lines = raw.splitlines()
                if len(lines) > 500:
                    lines = [
                        f"[#414868]… ({len(lines) - 500} earlier lines omitted) …[/]",
                        *lines[-500:],
                    ]
                self._log_lines = lines
            except OSError as exc:
                self._log_lines = [f"[#f7768e]error reading log:[/] {exc}"]
        else:
            self._log_lines = [
                "[#414868]No local log snapshot yet.[/]",
                "",
                "[#565f89]Stream live output with:[/]",
                f"[bold #7aa2f7]  xrun logs -f {self._run_id}[/]",
                "",
                "[#414868]The poller writes a local snapshot every ~5 s once the run is running.[/]",
            ]
        self._render_log()
        status = (self._run or {}).get("status", "")
        if status in self._ACTIVE_STATUSES or status in self._TERMINAL_STATUSES:
            log.scroll_end(animate=False)
        if status in self._TERMINAL_STATUSES:
            self._stop_log_poll()

    def _render_log(self, query: str = "") -> None:
        log = self.query_one("#logs-view", RichLog)
        log.clear()
        q = query.lower().strip()
        for line in self._log_lines:
            # Strip existing markup to search plain text
            plain = line
            if q and q in plain.lower():
                log.write(f"[bold #e0af68 on #2d3149]{plain}[/]")
            else:
                log.write(plain)
        if q:
            matches = sum(1 for l in self._log_lines if q in l.lower())
            # Update search hint inline — handled by on_input_changed

    def _start_log_poll(self) -> None:
        if self._log_timer is None:
            self._log_timer = self.set_interval(10.0, self._poll_logs)

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
        from xrun_tui.widgets.ascii_chart import render_chart
        summary = self.query_one("#metrics-summary", Static)
        table   = self.query_one("#metrics-table",   DataTable)
        chart   = self.query_one("#metrics-chart",   RichLog)
        summary.update("[#e0af68]Loading metrics…[/]")
        table.clear()
        chart.clear()

        app: XrunApp = self.app  # type: ignore[assignment]
        try:
            keys = await app.db.metric_keys(self._run_id)
        except Exception as exc:
            summary.update(f"[#f7768e]Error:[/] {exc}")
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
            try:
                series = await app.db.metrics_for_key(self._run_id, k)
            except Exception:
                series = []
            latest = "—"
            spark  = ""
            if series:
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
                self.run_worker(self._load_manifest())
            elif pid == "tab-metrics":
                metrics_table = self.query_one("#metrics-table", DataTable)
                metrics_table.loading = True
                async def _load_metrics_with_spinner() -> None:
                    try:
                        await self._load_metrics()
                    finally:
                        if self.is_mounted:
                            metrics_table.loading = False
                self.run_worker(_load_metrics_with_spinner())

    def on_input_changed(self, event: Input.Changed) -> None:
        if event.input.id == "log-search-input":
            self._render_log(event.value)

    def on_input_submitted(self, event: Input.Submitted) -> None:
        if event.input.id == "log-search-input":
            self.query_one("#logs-view", RichLog).focus()

    def on_button_pressed(self, event: Button.Pressed) -> None:
        match event.button.id:
            case "btn-stop":      self.run_worker(self.action_stop_run())
            case "btn-rerun":     self.run_worker(self.action_rerun())
            case "btn-patch":     self.run_worker(self.action_patch_rerun())
            case "btn-pull":      self.run_worker(self.action_pull())
            case "btn-artifacts": self.run_worker(self.action_artifacts())
            case "btn-relaunch":  self.run_worker(self._do_relaunch())
            case "btn-error":     self.run_worker(self.action_error_detail())

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
                await self._load_run()
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

    async def action_sync_status(self) -> None:
        if not self._run:
            return
        from xrun_tui import services
        self.notify(f"Reconciling {self._run_id[:8]}…", severity="information")
        ok, msg = await services.fix_status(self._run_id)
        if ok:
            tail = msg.splitlines()[-1] if msg else "no change"
            self.notify(f"Sync ok: {tail}", severity="information", timeout=6)
            await self._load_run()
        else:
            self.notify(
                f"Sync failed: {msg[:200]}", severity="error", timeout=10
            )

    def action_toggle_search(self) -> None:
        inp = self.query_one("#log-search-input", Input)
        self._search_active = not self._search_active
        inp.display = self._search_active
        if self._search_active:
            inp.focus()
            inp.value = ""
        else:
            inp.value = ""
            self._render_log()
            self.query_one("#logs-view", RichLog).focus()

    async def action_patch_rerun(self) -> None:
        if not self._run:
            return
        from xrun_tui.screens.patch_launch import PatchLaunchScreen
        await self.app.push_screen(PatchLaunchScreen(self._run))

    async def action_error_detail(self) -> None:
        if not self._run or self._run.get("status") != "failed":
            self.notify("Run has not failed", severity="warning")
            return
        from xrun_tui.screens.error_detail import ErrorDetailScreen

        async def _on_dismiss(result: str | None) -> None:
            if result == "open_logs":
                self.query_one(TabbedContent).active = "tab-logs"

        await self.app.push_screen(ErrorDetailScreen(self._run), _on_dismiss)

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
