"""The xrun mark: a tile with a loss curve cut out of it and a detached dot.

The geometry lives here only: `scripts/brand.py` imports it to render the PNGs
for README and the site, and the splash screen draws it in half-block cells.
The SVG in `docs/index.html` is a hand copy of the same numbers. See
`docs/brand.md`.
"""

from __future__ import annotations

import math
from collections.abc import Sequence
from typing import TYPE_CHECKING

from PIL import Image, ImageDraw

if TYPE_CHECKING:
    from rich.text import Text

Rgb = tuple[int, int, int]

# Tokyo Night, the TUI's default palette.
BG: Rgb = (0x1A, 0x1B, 0x26)
BLUE: Rgb = (0x7A, 0xA2, 0xF7)
MAGENTA: Rgb = (0xBB, 0x9A, 0xF7)

# Mark geometry on a 100×100 field.
RADIUS = 22
CURVE = [(20, 24), (28, 62), (44, 70), (64, 70)]  # cubic Bézier
STROKE = 11
DOT = (80, 70, 7.5)

_SS = 4  # supersampling, so the curve's edge is not a staircase


def bezier(p: Sequence[tuple[float, float]], n: int = 256) -> list[tuple[float, float]]:
    (x0, y0), (x1, y1), (x2, y2), (x3, y3) = p
    out = []
    for i in range(n + 1):
        t = i / n
        u = 1 - t
        out.append(
            (
                u**3 * x0 + 3 * u * u * t * x1 + 3 * u * t * t * x2 + t**3 * x3,
                u**3 * y0 + 3 * u * u * t * y1 + 3 * u * t * t * y2 + t**3 * y3,
            )
        )
    return out


_PTS = bezier(CURVE)
# Arc length from the curve's start to each point, so a partial cut grows at
# an even pace along the stroke rather than along the Bézier parameter.
_ARC = [0.0]
for _a, _b in zip(_PTS, _PTS[1:]):
    _ARC.append(_ARC[-1] + math.dist(_a, _b))


def mask(size: int, curve: float = 1.0, dot: float = 1.0) -> Image.Image:
    """Tile coverage, 0..255. `curve` is the cut share of the curve's length,
    `dot` scales the dot's radius; 1 and 1 is the finished mark."""
    big = size * _SS
    k = big / 100
    m = Image.new("L", (big, big), 0)
    d = ImageDraw.Draw(m)
    d.rounded_rectangle((0, 0, big - 1, big - 1), radius=RADIUS * k, fill=255)
    w = STROKE * k
    upto = _ARC[-1] * curve
    # Stamp circles along the curve rather than line(): line() tears the
    # segment joints on the bend and the cut comes out frayed.
    if curve > 0:
        for (x, y), s in zip(_PTS, _ARC):
            if s > upto:
                break
            x, y = x * k, y * k
            d.ellipse((x - w / 2, y - w / 2, x + w / 2, y + w / 2), fill=0)
    cx, cy, r = DOT
    r *= dot
    if r > 0:
        d.ellipse(((cx - r) * k, (cy - r) * k, (cx + r) * k, (cy + r) * k), fill=0)
    return m.resize((size, size), Image.Resampling.LANCZOS)


def gradient(x: int, y: int, size: int) -> Rgb:
    """The brand diagonal, BLUE top-left → MAGENTA bottom-right."""
    t = (x + y) / (2 * (size - 1))
    return (
        round(BLUE[0] + (MAGENTA[0] - BLUE[0]) * t),
        round(BLUE[1] + (MAGENTA[1] - BLUE[1]) * t),
        round(BLUE[2] + (MAGENTA[2] - BLUE[2]) * t),
    )


def _over(fg: Rgb, bg: Rgb, a: float) -> str:
    r, g, b = (round(bg[i] + (fg[i] - bg[i]) * a) for i in range(3))
    return f"#{r:02x}{g:02x}{b:02x}"


def cells(
    size: int,
    curve: float = 1.0,
    dot: float = 1.0,
    alpha: float = 1.0,
    bg: Rgb = BG,
) -> Text:
    """The mark as `size` columns × `size / 2` rows of "▀": the top pixel is
    the foreground, the bottom one the background, so pixels come out about
    square in a terminal cell. `alpha` fades the whole tile in over `bg`."""
    # Imported here: scripts/brand.py uses mask() with Pillow alone.
    from rich.style import Style
    from rich.text import Text

    cov = mask(size, curve, dot).tobytes()  # row-major, one byte per pixel
    out = Text(no_wrap=True, overflow="crop")
    for row in range(0, size - size % 2, 2):
        if row:
            out.append("\n")
        for x in range(size):
            a_top = alpha * cov[row * size + x] / 255
            a_bot = alpha * cov[(row + 1) * size + x] / 255
            top = _over(gradient(x, row, size), bg, a_top)
            bot = _over(gradient(x, row + 1, size), bg, a_bot)
            out.append("▀", Style(color=top, bgcolor=bot))
    return out


def _ease_out(t: float) -> float:
    return 1 - (1 - t) ** 3


def frame_at(t: float) -> tuple[float, float, float]:
    """(curve, dot, alpha) at `t` in 0..1 of the boot animation.

    The tile fades in, then the curve is cut top to bottom — fast down the
    drop, slow along the plateau, like the loss it stands for — and last the
    dot lands: the best checkpoint. The curve starts only once the tile is
    opaque, or its round start cap flashes on a half-faded tile.
    """
    t = min(max(t, 0.0), 1.0)
    alpha = min(1.0, t / 0.25)
    curve = _ease_out(min(1.0, max(0.0, (t - 0.25) / 0.45)))
    d = max(0.0, (t - 0.75) / 0.25)
    # A small overshoot so the dot pops; still clear of the curve at its peak.
    dot = 0.0 if d == 0 else min(1.0, d * 1.25) + 0.15 * math.sin(math.pi * d)
    return curve, dot, alpha
