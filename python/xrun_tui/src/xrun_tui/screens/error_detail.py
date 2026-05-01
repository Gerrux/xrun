from __future__ import annotations

from typing import TYPE_CHECKING

from textual.app import ComposeResult
from textual.binding import Binding
from textual.containers import Horizontal, Vertical
from textual.screen import ModalScreen
from textual.widgets import Button, RichLog, Static

if TYPE_CHECKING:
    from xrun_tui.app import XrunApp


class ErrorDetailScreen(ModalScreen[str | None]):
    BINDINGS = [
        Binding("escape", "dismiss_none", show=False),
        Binding("r",      "action_rerun", show=False),
    ]

    DEFAULT_CSS = """
    ErrorDetailScreen { align: center middle; }
    #error-box {
        background: #1e2030;
        border: solid #f7768e;
        width: 90;
        height: 32;
        padding: 1 2;
    }
    #error-title {
        color: #f7768e;
        text-style: bold;
        height: 1;
        margin-bottom: 0;
    }
    #error-meta {
        color: #565f89;
        height: 1;
        margin-bottom: 1;
    }
    #error-log {
        height: 1fr;
        border: solid #414868;
        background: #1a1b26;
        margin-bottom: 1;
    }
    #error-actions {
        height: 3;
        align: left middle;
    }
    #error-actions Button {
        margin-right: 1;
    }
    """

    def __init__(self, run: dict) -> None:
        super().__init__()
        self._run = run

    def compose(self) -> ComposeResult:
        run = self._run
        name = run.get("name") or run["id"][:16]
        run_id = run.get("id") or ""
        vendor = run.get("vendor") or "?"
        exit_code = run.get("exit_code")
        exit_str = str(exit_code) if exit_code is not None else "?"

        with Vertical(id="error-box"):
            yield Static(
                f"[bold #f7768e]✗ Run failed — {name}[/]",
                id="error-title",
            )
            yield Static(
                f"[#565f89]id: {run_id[:14]}  vendor: {vendor}  exit code: {exit_str}[/]",
                id="error-meta",
            )
            yield RichLog(
                id="error-log",
                highlight=False,
                markup=True,
                wrap=True,
            )
            with Horizontal(id="error-actions"):
                yield Button("Rerun  [r]",    id="btn-rerun",  variant="primary")
                yield Button("Full logs",     id="btn-logs")
                yield Button("Close  [Esc]",  id="btn-close")

    def on_mount(self) -> None:
        self.call_after_refresh(self._load_log)

    async def _load_log(self) -> None:
        app: XrunApp = self.app  # type: ignore[assignment]
        log = self.query_one("#error-log", RichLog)
        run_id = self._run.get("id") or ""
        log_path = app.db.log_path(run_id)

        if log_path.exists():
            try:
                raw = log_path.read_text(encoding="utf-8", errors="replace")
                all_lines = raw.splitlines()
                tail_size = 30
                if len(all_lines) > tail_size:
                    skipped = len(all_lines) - tail_size
                    lines = [
                        f"[#414868]… ({skipped} earlier lines) …[/]",
                        *all_lines[-tail_size:],
                    ]
                else:
                    lines = all_lines
                for line in lines:
                    lower = line.lower()
                    if "error" in lower or "traceback" in lower:
                        log.write(f"[bold #f7768e]{line}[/]")
                    elif "warning" in lower or "warn" in lower:
                        log.write(f"[#e0af68]{line}[/]")
                    else:
                        log.write(f"[#c0caf5]{line}[/]")
            except OSError as exc:
                log.write(f"[#f7768e]error reading log:[/] {exc}")
        else:
            short_id = run_id[:8]
            log.write(
                "[#414868]No local log snapshot found.[/]\n\n"
                "[#565f89]View full output with:[/]\n"
                f"[bold #7aa2f7]  xrun logs {short_id}[/]"
            )

        log.scroll_end(animate=False)

    def on_button_pressed(self, event: Button.Pressed) -> None:
        match event.button.id:
            case "btn-close":
                self.dismiss(None)
            case "btn-logs":
                self.dismiss("open_logs")
            case "btn-rerun":
                self.run_worker(self.action_rerun())

    async def action_rerun(self) -> None:
        from xrun_tui.screens.confirm import ConfirmScreen

        run_id = self._run.get("id") or ""
        name = self._run.get("name") or run_id[:8]

        async def _do(confirmed: bool) -> None:
            if not confirmed:
                return
            from xrun_tui import services
            ok, msg = await services.rerun_run(run_id)
            if ok:
                self.notify("Rerun launched", severity="information")
                self.dismiss("rerun_ok")
            else:
                self.notify(f"Rerun failed: {msg[:80]}", severity="error", timeout=8)

        await self.app.push_screen(ConfirmScreen(f"Rerun {name}?"), _do)

    def action_dismiss_none(self) -> None:
        self.dismiss(None)
