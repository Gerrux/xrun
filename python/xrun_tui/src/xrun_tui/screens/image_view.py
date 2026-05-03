"""Inline image preview as a Textual modal.

Uses `rich-pixels` half-block rendering so it works in any truecolor terminal
(Windows Terminal / WezTerm / iTerm2 / kitty / xterm with truecolor).

Resolution is one terminal row = two image rows (the half-block trick), so a
1200×900 PNG mapped to a 120×40 viewport is a low-fidelity thumbnail — enough
for "did the plot come out right?" but not for serious inspection. `o` opens
the file externally for that.
"""
from __future__ import annotations

import os
import subprocess
import sys
from pathlib import Path

from textual.app import ComposeResult
from textual.binding import Binding
from textual.containers import Vertical
from textual.screen import ModalScreen
from textual.widgets import Footer, Static


class ImagePreviewScreen(ModalScreen[None]):
    """Render a PNG/JPG inline. Esc/q closes; `o` opens externally."""

    DEFAULT_CSS = """
    ImagePreviewScreen { align: center middle; }
    ImagePreviewScreen > Vertical {
        width: 90%;
        height: 90%;
        background: #1a1b26;
        border: round #7aa2f7;
        padding: 0 1;
    }
    ImagePreviewScreen #img-title {
        height: 1;
        padding: 0 1;
        background: #24283b;
        color: #c0caf5;
    }
    ImagePreviewScreen #img-canvas {
        width: 1fr;
        height: 1fr;
        content-align: center middle;
        padding: 0;
    }
    ImagePreviewScreen #img-error {
        color: #f7768e;
        padding: 1 2;
    }
    """

    BINDINGS = [
        Binding("escape,q", "dismiss",       "Close"),
        Binding("o",        "open_external", "Open externally"),
    ]

    def __init__(self, path: Path) -> None:
        super().__init__()
        self._path = path

    def compose(self) -> ComposeResult:
        with Vertical():
            yield Static(self._title(), id="img-title")
            yield Static("", id="img-canvas")
            yield Footer()

    def on_mount(self) -> None:
        # Render once after layout settles so we know the canvas size.
        self.call_after_refresh(self._redraw_image)

    def on_resize(self, _event) -> None:  # type: ignore[override]
        self._redraw_image()

    def _title(self) -> str:
        try:
            size = self._path.stat().st_size
        except OSError:
            size = 0
        return (
            f"[bold #c0caf5]{self._path.name}[/]  "
            f"[#565f89]{_human_size(size)}[/]   "
            f"[#414868]Esc / q close · o open externally[/]"
        )

    def _redraw_image(self) -> None:
        canvas = self.query_one("#img-canvas", Static)
        try:
            from PIL import Image
            from rich_pixels import Pixels
        except ImportError as exc:  # pragma: no cover
            canvas.update(f"[#f7768e]rich-pixels / Pillow missing:[/] {exc}")
            return
        try:
            img = Image.open(self._path).convert("RGB")
        except Exception as exc:
            canvas.update(f"[#f7768e]Failed to load image:[/]\n{exc}")
            return

        size = canvas.size
        max_w = max(20, (size.width or 100) - 2)
        # Half-block: one terminal row ≈ 2 image rows. Multiply by 2 so the
        # downsampled height matches the cell budget after pairing.
        max_h = max(10, ((size.height or 40) - 2) * 2)
        img.thumbnail((max_w, max_h), Image.Resampling.LANCZOS)
        canvas.update(Pixels.from_image(img))

    def action_open_external(self) -> None:
        if _open_external(self._path):
            self.notify(f"Opened {self._path.name}", timeout=4)
        else:
            self.notify(f"Could not open externally:\n{self._path}",
                        severity="warning", timeout=8)


def _human_size(n: int) -> str:
    f = float(n)
    for unit in ("B", "KB", "MB", "GB"):
        if f < 1024:
            return f"{f:.0f} {unit}" if unit == "B" else f"{f:.1f} {unit}"
        f /= 1024
    return f"{f:.1f} TB"


def _open_external(path: Path) -> bool:
    try:
        if sys.platform == "win32":
            os.startfile(str(path))  # type: ignore[attr-defined]
        elif sys.platform == "darwin":
            subprocess.Popen(["open", str(path)])
        else:
            subprocess.Popen(["xdg-open", str(path)])
        return True
    except Exception:
        return False
