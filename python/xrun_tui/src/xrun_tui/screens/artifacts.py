from __future__ import annotations

from pathlib import Path
from typing import TYPE_CHECKING

from rich.text import Text
from textual.app import ComposeResult
from textual.binding import Binding
from textual.screen import Screen
from textual.widgets import DataTable, Footer, Header, Static
from xrun_tui.widgets.status_bar import StatusBar

if TYPE_CHECKING:
    from xrun_tui.app import XrunApp


class ArtifactsScreen(Screen):
    """Browse and pull run artifacts / checkpoints."""

    TITLE = "xrun — artifacts"
    BINDINGS = [
        Binding("escape,q",  "go_back",     "Back"),
        Binding("j,down",    "cursor_down", "Down",    show=False),
        Binding("k,up",      "cursor_up",   "Up",      show=False),
        Binding("enter,p",   "pull_file",   "Pull"),
        Binding("ctrl+r,f5", "refresh",     "Refresh"),
        Binding("a",         "pull_all",    "Pull all"),
    ]

    def __init__(self, run_id: str, run_name: str = "") -> None:
        super().__init__()
        self._run_id   = run_id
        self._run_name = run_name or run_id[:12]
        self._entries: list[dict] = []

    def compose(self) -> ComposeResult:
        yield Header(show_clock=True)
        yield Static(
            f"[bold #c0caf5]Artifacts[/]  [#565f89]run:[/] [#7aa2f7]{self._run_name}[/]",
            classes="screen-title",
        )
        yield Static("", id="art-summary", classes="stats-bar")
        yield DataTable(id="art-table", cursor_type="row", zebra_stripes=True)
        yield Static("", id="art-result", classes="form-hint")
        yield StatusBar()
        yield Footer()

    def on_mount(self) -> None:
        t = self.query_one("#art-table", DataTable)
        t.add_columns(
            Text(" ",        style="#565f89"),
            Text("Path",     style="#565f89"),
            Text("Size",     style="#565f89"),
            Text("Modified", style="#565f89"),
            Text("Type",     style="#565f89"),
        )
        t.focus()
        self.call_after_refresh(self._load)

    async def _load(self) -> None:
        from xrun_tui import services
        summary = self.query_one("#art-summary", Static)
        table   = self.query_one("#art-table",   DataTable)
        summary.update("[#e0af68]Loading artifacts…[/]")
        table.clear()
        self._entries = []

        ok, entries, err = await services.list_artifacts(self._run_id)
        if not ok:
            summary.update(f"[#f7768e]Error: {err[:120]}[/]")
            table.add_row(
                Text("✗", style="#f7768e"),
                Text(err[:60], style="#f7768e"),
                Text(""), Text(""), Text(""),
            )
            return

        if not entries:
            summary.update("[#414868]No artifacts found[/]")
            table.add_row(
                Text(""), Text("No artifacts", style="#414868"),
                Text(""), Text(""), Text(""),
            )
            return

        self._entries = entries
        total_size = sum(e.get("size") or 0 for e in entries)
        summary.update(
            f"[#7aa2f7]{len(entries)}[/] [#565f89]files[/]  "
            f"[#e0af68]{_fmt_size(total_size)}[/] [#565f89]total[/]   "
            f"[#565f89]↵ to pull selected  a to pull all[/]"
        )

        for entry in entries:
            kind  = entry.get("type") or "file"
            icon  = "📁" if kind == "dir" else "📄"
            path  = entry.get("path") or "?"
            size  = _fmt_size(entry.get("size") or 0) if kind != "dir" else ""
            mtime = (entry.get("modified") or "")[:19].replace("T", " ")
            table.add_row(
                Text(icon),
                Text(path,  style="#c0caf5"),
                Text(size,  style="#e0af68"),
                Text(mtime, style="#565f89"),
                Text(kind,  style="#7dcfff"),
                key=path,
            )

    def _selected_entry(self) -> dict | None:
        table = self.query_one("#art-table", DataTable)
        row   = table.cursor_row
        return self._entries[row] if 0 <= row < len(self._entries) else None

    async def action_pull_file(self) -> None:
        entry = self._selected_entry()
        if not entry:
            return
        path = entry.get("path") or ""
        if not path:
            return
        result = self.query_one("#art-result", Static)
        result.update(f"[#e0af68]Pulling {path}…[/]")
        from xrun_tui import services
        ok, msg = await services.pull(self._run_id, ckpt=path)
        if ok:
            result.update(f"[bold #9ece6a]✓ pulled[/] [#565f89]{path}[/]")
            self.notify(f"Pulled {Path(path).name}", severity="information")
        else:
            result.update(f"[bold #f7768e]✗ {msg[:100]}[/]")
            self.notify(f"Pull failed: {msg[:60]}", severity="error", timeout=8)

    async def action_pull_all(self) -> None:
        from xrun_tui.screens.confirm import ConfirmScreen
        from xrun_tui import services

        async def _do(confirmed: bool) -> None:
            if not confirmed:
                return
            result = self.query_one("#art-result", Static)
            result.update("[#e0af68]Pulling all artifacts…[/]")
            ok, msg = await services.pull(self._run_id, ckpt="latest", artifacts=True)
            if ok:
                result.update("[bold #9ece6a]✓ all artifacts pulled[/]")
                self.notify("All artifacts pulled", severity="information")
            else:
                result.update(f"[bold #f7768e]✗ {msg[:100]}[/]")
                self.notify(f"Pull failed: {msg[:60]}", severity="error", timeout=8)

        await self.app.push_screen(
            ConfirmScreen(f"Pull all artifacts for {self._run_name}?"), _do
        )

    async def action_refresh(self) -> None:
        await self._load()

    def action_go_back(self) -> None:
        self.app.pop_screen()

    def action_cursor_down(self) -> None:
        self.query_one(DataTable).action_cursor_down()

    def action_cursor_up(self) -> None:
        self.query_one(DataTable).action_cursor_up()


def _fmt_size(n: int) -> str:
    if n < 1024:
        return f"{n}B"
    if n < 1024 ** 2:
        return f"{n / 1024:.1f}KB"
    if n < 1024 ** 3:
        return f"{n / 1024 ** 2:.1f}MB"
    return f"{n / 1024 ** 3:.2f}GB"
