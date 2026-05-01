from __future__ import annotations

import asyncio
from pathlib import Path

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
    Label,
    RichLog,
    Static,
)
from xrun_tui.widgets.status_bar import StatusBar
from xrun_tui.widgets.title_bar import TitleBar


class LaunchScreen(Screen):
    """Pick a manifest, preview it, dry-run or launch."""

    TITLE = "xrun — launch"
    BINDINGS = [
        Binding("escape,q",   "go_back",   "Back"),
        Binding("j,down",     "cursor_down", "Down", show=False),
        Binding("k,up",       "cursor_up",   "Up",   show=False),
        Binding("ctrl+r,f5",  "refresh",   "Refresh"),
        Binding("ctrl+d",     "dry_run",   "Dry-run"),
        Binding("ctrl+enter", "launch",    "Launch", show=False),
        Binding("enter",      "launch",    "Launch"),
    ]

    def __init__(self) -> None:
        super().__init__()
        self._manifests: list[Path] = []
        self._selected: Path | None = None

    def compose(self) -> ComposeResult:
        yield TitleBar("launch")
        yield Static("Launch a manifest", classes="screen-title")
        yield Static("", id="launch-summary", classes="stats-bar")
        with Horizontal(id="launch-cols"):
            with Vertical(id="launch-list-col"):
                yield Static("Manifests (cwd / exp/)", classes="dash-section")
                yield DataTable(id="launch-table",
                                cursor_type="row", zebra_stripes=True)
            with Vertical(id="launch-preview-col"):
                yield Static("Preview", classes="dash-section")
                yield RichLog(id="launch-preview",
                              highlight=False, markup=False, wrap=False)
        with Vertical(id="launch-form"):
            with Horizontal(classes="form-row"):
                yield Label("Run name (optional):", classes="form-label")
                yield Input(id="launch-name",
                            placeholder="leave blank for manifest default",
                            classes="form-input")
            with Horizontal(classes="form-row"):
                yield Label("Manifest path:", classes="form-label")
                yield Input(id="launch-path",
                            placeholder="…or type a path manually",
                            classes="form-input")
            yield Static("", id="launch-result", classes="form-hint")
            with Horizontal(classes="form-actions"):
                yield Button("Launch  [enter]",   id="btn-launch",
                             variant="primary")
                yield Button("Dry-run  [ctrl+d]", id="btn-dryrun")
                yield Button("Back  [esc]",      id="btn-back")
        yield StatusBar()
        yield Footer()

    def on_mount(self) -> None:
        t = self.query_one("#launch-table", DataTable)
        t.add_columns(
            Text(" ",        style="#565f89"),
            Text("Manifest", style="#565f89"),
            Text("Modified", style="#565f89"),
        )
        self.query_one("#launch-preview", RichLog).write(
            "[#414868]select a manifest to preview[/]"
        )
        t.focus()
        self.call_after_refresh(self._refresh)

    async def _refresh(self) -> None:
        from xrun_tui import services
        exp_dir = getattr(self.app, "_exp_dir", None)
        self._manifests = await asyncio.to_thread(
            services.discover_manifests, exp_dir
        )
        t = self.query_one("#launch-table", DataTable)
        t.clear()
        cwd = Path.cwd()
        if not self._manifests:
            scoped = exp_dir or "exp/  experiments/  manifests/"
            t.add_row(
                Text(""),
                Text(
                    f"no manifests found in {scoped} — "
                    f"pass a path to `xrun launch <file>` "
                    f"or set defaults.exp_dir",
                    style="#414868",
                ),
                Text(""),
            )
        else:
            for p in self._manifests:
                try:
                    rel = p.relative_to(cwd)
                except ValueError:
                    rel = p
                from datetime import datetime
                mtime = datetime.fromtimestamp(p.stat().st_mtime).strftime(
                    "%Y-%m-%d %H:%M"
                )
                t.add_row(
                    Text("•",       style="#7aa2f7"),
                    Text(str(rel),  style="#c0caf5"),
                    Text(mtime,     style="#565f89"),
                )
        self.query_one("#launch-summary", Static).update(
            f"[#565f89]{len(self._manifests)} manifests in[/] "
            f"[#7dcfff]{cwd}[/]"
        )

    # ── Selection ───────────────────────────────────────────────────────────

    def on_data_table_row_highlighted(
        self, event: DataTable.RowHighlighted
    ) -> None:
        idx = event.cursor_row
        if 0 <= idx < len(self._manifests):
            self._select(self._manifests[idx])

    def _select(self, path: Path) -> None:
        self._selected = path
        self.query_one("#launch-path", Input).value = str(path)
        self.query_one("#launch-name", Input).value = ""
        log = self.query_one("#launch-preview", RichLog)
        log.clear()
        try:
            content = path.read_text(encoding="utf-8")
        except OSError as e:
            log.write(f"[#f7768e]cannot read manifest: {e}[/]")
            return
        if len(content) > 8000:
            content = content[:8000] + "\n# … (truncated)"
        log.write(Syntax(content, "yaml", theme="nord", line_numbers=True))

    # ── Buttons ─────────────────────────────────────────────────────────────

    def on_button_pressed(self, event: Button.Pressed) -> None:
        match event.button.id:
            case "btn-launch": self.run_worker(self._do_launch(False),
                                               exclusive=True)
            case "btn-dryrun": self.run_worker(self._do_launch(True),
                                               exclusive=True)
            case "btn-back":   self.action_go_back()

    async def action_dry_run(self) -> None:
        await self._do_launch(True)

    async def action_launch(self) -> None:
        if isinstance(self.focused, Input):
            return  # Don't fire on Enter inside text inputs
        await self._do_launch(False)

    async def _do_launch(self, dry: bool) -> None:
        manifest_path = self.query_one("#launch-path", Input).value.strip()
        if not manifest_path:
            self.notify("Choose a manifest first", severity="warning")
            return
        if not Path(manifest_path).is_file():
            self.notify(f"File not found: {manifest_path}", severity="error")
            return

        name = self.query_one("#launch-name", Input).value.strip() or None
        result = self.query_one("#launch-result", Static)
        result.update("[#e0af68]Running…[/]" if dry else "[#e0af68]Launching…[/]")

        from xrun_tui import services
        ok, msg = await services.launch(manifest_path, dry_run=dry, name=name)
        excerpt = (msg or "").splitlines()[-1] if msg else ""
        if ok:
            result.update(
                f"[bold #9ece6a]✓ {'plan ready' if dry else 'launched'}[/]  "
                f"[#565f89]{excerpt[:120]}[/]"
            )
            self.notify(
                "Plan ready" if dry else "Launched",
                severity="information",
            )
        else:
            result.update(f"[bold #f7768e]✗ {excerpt[:200]}[/]")
            self.notify(
                f"{'Dry-run' if dry else 'Launch'} failed: {excerpt[:80]}",
                severity="error", timeout=10,
            )

    # ── Misc ────────────────────────────────────────────────────────────────

    def action_go_back(self) -> None:
        self.app.pop_screen()

    def action_cursor_down(self) -> None:
        self.query_one("#launch-table", DataTable).action_cursor_down()

    def action_cursor_up(self) -> None:
        self.query_one("#launch-table", DataTable).action_cursor_up()

    async def action_refresh(self) -> None:
        await self._refresh()
