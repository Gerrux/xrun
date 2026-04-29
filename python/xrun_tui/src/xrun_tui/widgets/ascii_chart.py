"""ASCII line chart renderer.

Pure-Python; no extra deps. Renders a 1-line-per-row Rich-friendly text block
so it can be passed to RichLog or Static.
"""
from __future__ import annotations

from rich.text import Text


_BARS = "·▁▂▃▄▅▆▇█"


def render_chart(
    values: list[float],
    *,
    width: int = 60,
    height: int = 12,
    title: str = "",
    color: str = "#7aa2f7",
) -> Text:
    """Render `values` as a vertical bar chart with axis annotations.

    Width and height in CHARACTER cells. Returns a `rich.text.Text` instance
    ready for RichLog.write or Static.update.
    """
    if not values:
        return Text("(no data)", style="#414868")

    # Down-sample / up-pad to width
    if len(values) > width:
        # Even subsample
        step = len(values) / width
        sampled = [values[int(i * step)] for i in range(width)]
    else:
        sampled = list(values)

    lo, hi = min(sampled), max(sampled)
    if hi - lo < 1e-12:
        hi = lo + 1.0

    # Build columns (bottom-up)
    nbars = len(_BARS) - 1
    canvas = [[" " for _ in range(width)] for _ in range(height)]
    for x, v in enumerate(sampled):
        norm = (v - lo) / (hi - lo)
        cells = norm * height  # how tall in cells (fractional)
        full = int(cells)
        partial = cells - full
        for y in range(full):
            canvas[height - 1 - y][x] = "█"
        if full < height:
            idx = int(partial * nbars)
            if idx > 0:
                canvas[height - 1 - full][x] = _BARS[idx]

    rendered = Text()
    if title:
        rendered.append(f"  {title}\n", style=f"bold {color}")

    # Axis labels (left): hi at top, lo at bottom
    label_width = max(len(f"{hi:.4g}"), len(f"{lo:.4g}")) + 1
    for y, row in enumerate(canvas):
        if y == 0:
            label = f"{hi:>{label_width - 1}.4g} "
            rendered.append(label, style="#565f89")
        elif y == height - 1:
            label = f"{lo:>{label_width - 1}.4g} "
            rendered.append(label, style="#565f89")
        else:
            rendered.append(" " * label_width, style="#414868")
        rendered.append("│ ", style="#2d3149")
        rendered.append("".join(row), style=color)
        rendered.append("\n")

    # X-axis line
    rendered.append(" " * label_width)
    rendered.append("└" + "─" * (width + 1), style="#2d3149")
    rendered.append("\n")
    rendered.append(" " * (label_width + 2))
    rendered.append(f"0", style="#565f89")
    rendered.append(" " * max(1, width - 4 - len(str(len(values)))))
    rendered.append(f"{len(values)} pts", style="#565f89")
    return rendered
