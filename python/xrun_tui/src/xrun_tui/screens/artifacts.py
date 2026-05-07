from __future__ import annotations

from pathlib import Path
from typing import TYPE_CHECKING

from rich.text import Text
from textual.app import ComposeResult
from textual.binding import Binding
from textual.screen import Screen
from textual.widgets import DataTable, Footer, Static
from xrun_tui.widgets.status_bar import StatusBar
from xrun_tui.widgets.title_bar import TitleBar

if TYPE_CHECKING:
    from xrun_tui.app import XrunApp


# Vendors whose adapter can rsync individual remote paths via `xrun pull --ckpt`.
# Kaggle's kernel API only exposes "download all output", so single-file pull
# is rejected upstream in pull.rs and we surface that here instead of silently
# re-downloading everything. Local has no remote to pull from.
_SELECTIVE_PULL_VENDORS: frozenset[str] = frozenset({"vast", "ssh"})


class ArtifactsScreen(Screen):
    """Browse run artifacts on disk; pull more from the remote where supported."""

    TITLE = "xrun — artifacts"
    BINDINGS = [
        Binding("escape,q",  "go_back",      "Back"),
        Binding("j,down",    "cursor_down",  "Down",   show=False),
        Binding("k,up",      "cursor_up",    "Up",     show=False),
        Binding("space",     "toggle_mark",  "Mark"),
        Binding("enter,p",   "pull_marked",  "Pull"),
        Binding("a",         "pull_all",     "Pull all"),
        Binding("o",         "reveal_file",  "Open"),
        Binding("O",         "reveal_dir",   "Open dir"),
        Binding("ctrl+r,f5", "refresh",      "Refresh", show=False),
    ]

    def __init__(self, run_id: str, run_name: str = "", vendor: str = "") -> None:
        super().__init__()
        self._run_id   = run_id
        self._run_name = run_name or run_id[:12]
        self._vendor   = (vendor or "").lower()
        self._entries: list[dict] = []
        self._marked: set[str] = set()

    @property
    def _selective(self) -> bool:
        return self._vendor in _SELECTIVE_PULL_VENDORS

    def compose(self) -> ComposeResult:
        yield TitleBar("artifacts")
        yield Static(
            f"[bold #c0caf5]Artifacts[/]  [#565f89]run:[/] [#7aa2f7]{self._run_name}[/]"
            + (f"  [#565f89]vendor:[/] [#7dcfff]{self._vendor}[/]" if self._vendor else ""),
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
            summary.update(
                "[#414868]No artifacts yet —[/] [#7aa2f7]press `a`[/] "
                "[#565f89]to run `xrun pull --artifacts`[/]"
            )
            table.add_row(
                Text(""),
                Text("(empty — pull to populate)", style="#565f89"),
                Text(""), Text(""), Text(""),
            )
            return

        self._entries = entries
        # Drop stale marks for files no longer present.
        present = {e.get("path") for e in entries}
        self._marked &= present  # type: ignore[arg-type]
        self._render_summary()
        self._render_rows()

    def _render_summary(self) -> None:
        summary = self.query_one("#art-summary", Static)
        total_size = sum(e.get("size") or 0 for e in self._entries)
        marked = len(self._marked)
        head = (
            f"[#7aa2f7]{len(self._entries)}[/] [#565f89]files[/]  "
            f"[#e0af68]{_fmt_size(total_size)}[/] [#565f89]total[/]"
        )
        if marked:
            head += f"   [bold #9ece6a]{marked} marked[/]"
        if self._selective:
            tail = (
                "   [#565f89]space mark · ↵ pull marked · a all · "
                "o open · O open dir[/]"
            )
        else:
            label = (
                "Kaggle output is downloaded as a single archive"
                if self._vendor == "kaggle"
                else "local run — files already on disk"
                if self._vendor == "local"
                else "selective pull not supported for this vendor"
            )
            tail = (
                f"   [#e0af68]{label} —[/] "
                "[#565f89]a pulls all · o open · O open dir[/]"
            )
        summary.update(head + tail)

    def _render_rows(self) -> None:
        table = self.query_one("#art-table", DataTable)
        table.clear()
        for entry in self._entries:
            kind  = entry.get("type") or "file"
            path  = entry.get("path") or "?"
            size  = _fmt_size(entry.get("size") or 0) if kind != "dir" else ""
            mtime = (entry.get("modified") or "")[:19].replace("T", " ")
            mark = "✓" if path in self._marked else ("📁" if kind == "dir" else "📄")
            mark_style = "bold #9ece6a" if path in self._marked else ""
            table.add_row(
                Text(mark, style=mark_style),
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

    def _local_path(self, rel: str) -> Path:
        from xrun_tui import services
        return services.artifacts_dir(self._run_id) / rel

    # ── Actions ───────────────────────────────────────────────────────────────

    def action_toggle_mark(self) -> None:
        if not self._selective:
            self.notify(
                f"Marking is disabled — vendor `{self._vendor or '?'}` cannot pull "
                "files individually. Press `a` to pull all.",
                severity="warning",
                timeout=6,
            )
            return
        entry = self._selected_entry()
        if not entry:
            return
        path = entry.get("path") or ""
        if not path:
            return
        if path in self._marked:
            self._marked.discard(path)
        else:
            self._marked.add(path)
        # Preserve cursor position across rerender.
        table = self.query_one("#art-table", DataTable)
        cursor = table.cursor_row
        self._render_rows()
        self._render_summary()
        if 0 <= cursor < len(self._entries):
            table.move_cursor(row=cursor)

    async def action_pull_marked(self) -> None:
        from xrun_tui import services

        result = self.query_one("#art-result", Static)

        if not self._selective:
            self.notify(
                f"Selective pull not supported for `{self._vendor or '?'}` — "
                "press `a` to pull all.",
                severity="warning",
                timeout=6,
            )
            return

        # Decide which paths to pull: marked set, or fall back to current row.
        if self._marked:
            paths = sorted(self._marked)
        else:
            entry = self._selected_entry()
            paths = [entry["path"]] if entry and entry.get("path") else []
        if not paths:
            return

        result.update(
            f"[#e0af68]Pulling {len(paths)} file(s)…[/]"
            if len(paths) > 1
            else f"[#e0af68]Pulling {paths[0]}…[/]"
        )

        ok_count = 0
        first_err = ""
        for path in paths:
            ok, msg = await services.pull(self._run_id, ckpt=path)
            if ok:
                ok_count += 1
            elif not first_err:
                first_err = msg

        if ok_count == len(paths):
            result.update(
                f"[bold #9ece6a]✓ pulled[/] [#565f89]{ok_count} file(s)[/]"
            )
            self.notify(f"Pulled {ok_count} file(s)", severity="information")
            self._marked.clear()
            await self._load()
        else:
            failed = len(paths) - ok_count
            result.update(
                f"[bold #f7768e]✗ {failed} failed[/]  "
                f"[#9ece6a]{ok_count} ok[/]  [#565f89]{first_err[:80]}[/]"
            )
            self.notify(
                f"{failed}/{len(paths)} pulls failed: {first_err[:60]}",
                severity="error",
                timeout=8,
            )

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
                await self._load()
            else:
                result.update(f"[bold #f7768e]✗ {msg[:100]}[/]")
                self.notify(f"Pull failed: {msg[:60]}", severity="error", timeout=8)

        await self.app.push_screen(
            ConfirmScreen(f"Pull all artifacts for {self._run_name}?"), _do
        )

    def action_reveal_file(self) -> None:
        from xrun_tui import services

        entry = self._selected_entry()
        if not entry or not entry.get("path"):
            self.notify("No file selected", severity="warning")
            return
        target = self._local_path(entry["path"])
        ok, err = services.reveal_in_explorer(target)
        if not ok:
            self.notify(f"Open failed: {err[:80]}", severity="error", timeout=6)

    def action_reveal_dir(self) -> None:
        from xrun_tui import services

        target = services.artifacts_dir(self._run_id)
        if not target.is_dir():
            self.notify(
                "Artifacts dir does not exist yet — pull something first.",
                severity="warning",
            )
            return
        ok, err = services.reveal_in_explorer(target)
        if not ok:
            self.notify(f"Open failed: {err[:80]}", severity="error", timeout=6)

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
