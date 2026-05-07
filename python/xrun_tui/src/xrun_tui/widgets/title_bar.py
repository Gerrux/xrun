from __future__ import annotations

from datetime import datetime

from textual.app import ComposeResult
from textual.containers import Horizontal
from textual.events import Click
from textual.widgets import Static


class _MenuBtn(Static):
    DEFAULT_CSS = """
    _MenuBtn {
        width: auto;
        height: 1;
        color: #7aa2f7;
        padding: 0 1;
    }
    _MenuBtn:hover { background: #2d3149; }
    """

    def on_click(self, event: Click) -> None:
        event.stop()
        self.run_worker(self.app.action_open_palette(), exclusive=True)  # type: ignore[attr-defined]


class _StatusClock(Static):
    """Right-side status: active-runs dot + HH:MM clock."""

    DEFAULT_CSS = """
    _StatusClock {
        width: auto;
        height: 1;
        padding: 0 1;
        color: #565f89;
    }
    """

    def __init__(self) -> None:
        super().__init__("[#414868]○[/]  [#7aa2f7]--:--[/]")
        self._active: int | None = None

    def on_mount(self) -> None:
        self._paint()
        self._clock_timer = self.set_interval(1.0, self._paint)
        self._poll_timer = self.set_interval(5.0, self._refresh_async)
        self.run_worker(self._refresh_async(), exclusive=True)

    def on_unmount(self) -> None:
        for attr in ("_clock_timer", "_poll_timer"):
            try:
                getattr(self, attr).stop()
            except Exception:
                pass

    async def _refresh_async(self) -> None:
        try:
            runs = await self.app.db.runs(status="active")  # type: ignore[attr-defined]
            self._active = len(runs)
        except Exception:
            self._active = None
        self._paint()

    def _paint(self) -> None:
        if self._active is None:
            dot = "[#414868]○[/]"
        elif self._active > 0:
            dot = f"[bold #9ece6a]●[/] [#c0caf5]{self._active}[/]"
        else:
            dot = "[#414868]○[/]"
        now = datetime.now().strftime("%H:%M")
        if self.is_mounted:
            self.update(f"{dot}  [#7aa2f7]{now}[/]")


class TitleBar(Horizontal):
    """Shared title bar: [⊞ Menu]  xrun — <subtitle>   ● N  HH:MM."""

    DEFAULT_CSS = """
    TitleBar {
        dock: top;
        height: 1;
        background: #24283b;
        padding: 0 0;
        align: left middle;
    }
    TitleBar #tb-title {
        width: 1fr;
        content-align: center middle;
        color: #c0caf5;
    }
    """

    def __init__(self, subtitle: str = "") -> None:
        super().__init__()
        self._subtitle = subtitle

    def compose(self) -> ComposeResult:
        yield _MenuBtn("⊞ Menu")
        label = f"xrun  —  {self._subtitle}" if self._subtitle else "xrun"
        yield Static(label, id="tb-title")
        yield _StatusClock()
