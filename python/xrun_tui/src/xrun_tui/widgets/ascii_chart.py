"""ASCII line / bar chart renderers.

Pure-Python; no extra deps. Returns `rich.text.Text` for Static / RichLog.

Two flavours:

* `render_chart(values, …)` — single-series bar chart used in the legacy
  per-metric tile grid. Kept for back-compat.
* `render_chart_multi(series, …)` — overlay of N series as colored dots
  with a left Y-axis (4 ticks) and bottom legend. Used by MetricsView.
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
    """Single-series vertical bar chart with axis labels."""
    if not values:
        return Text("(no data)", style="#414868")

    sampled = _sample(values, width)
    lo, hi = min(sampled), max(sampled)
    if hi - lo < 1e-12:
        hi = lo + 1.0

    nbars = len(_BARS) - 1
    canvas = [[" " for _ in range(width)] for _ in range(height)]
    for x, v in enumerate(sampled):
        norm = (v - lo) / (hi - lo)
        cells = norm * height
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

    label_w = max(len(f"{hi:.4g}"), len(f"{lo:.4g}")) + 1
    for y, row in enumerate(canvas):
        if y == 0:
            rendered.append(f"{hi:>{label_w - 1}.4g} ", style="#565f89")
        elif y == height - 1:
            rendered.append(f"{lo:>{label_w - 1}.4g} ", style="#565f89")
        else:
            rendered.append(" " * label_w, style="#414868")
        rendered.append("│ ", style="#2d3149")
        rendered.append("".join(row), style=color)
        rendered.append("\n")

    rendered.append(" " * label_w)
    rendered.append("└" + "─" * (width + 1), style="#2d3149")
    rendered.append("\n")
    rendered.append(" " * (label_w + 2))
    rendered.append("0", style="#565f89")
    rendered.append(" " * max(1, width - 4 - len(str(len(values)))))
    rendered.append(f"{len(values)} pts", style="#565f89")
    return rendered


def render_chart_multi(
    series: list[tuple[str, list[float], str]],
    *,
    width: int = 80,
    height: int = 14,
    log_y: bool = False,
) -> Text:
    """Overlay multiple series as colored dots with Y-axis and legend.

    Each item in `series` is `(name, values, color)`. Y-axis prints four ticks
    (top / two midpoints / bottom). Bottom strip lists `● name` per series.
    """
    series = [(n, list(v), c) for n, v, c in series if v]
    if not series:
        return Text("(no data)", style="#414868")

    if log_y:
        from xrun_tui.widgets.metrics_palette import safe_log
        series = [(n, safe_log(v), c) for n, v, c in series]

    flat = [x for _, vs, _ in series for x in vs]
    lo, hi = min(flat), max(flat)
    if hi - lo < 1e-12:
        hi = lo + 1.0

    canvas = [[(" ", "") for _ in range(width)] for _ in range(height)]
    max_n = max(len(vs) for _, vs, _ in series)
    for _name, vs, color in series:
        sampled = _sample(vs, width)
        n = len(sampled)
        if n == 0:
            continue
        # Spread points evenly across the full width so few-epoch runs don't
        # cluster on the left edge. Single-point series → centred dot.
        for i, v in enumerate(sampled):
            x = width // 2 if n == 1 else round(i * (width - 1) / (n - 1))
            norm = (v - lo) / (hi - lo)
            y = int(round(norm * (height - 1)))
            y = max(0, min(height - 1, y))
            canvas[height - 1 - y][x] = ("●", color)

    label_w = max(len(_fmt(hi)), len(_fmt(lo))) + 1
    tick_rows = {
        0:              hi,
        height // 3:    lo + (hi - lo) * 2 / 3,
        2 * height // 3: lo + (hi - lo) / 3,
        height - 1:     lo,
    }

    rendered = Text()
    if log_y:
        rendered.append("  log10 Y\n", style="#565f89")
    for y, row in enumerate(canvas):
        if y in tick_rows:
            rendered.append(f"{_fmt(tick_rows[y]):>{label_w - 1}} ",
                            style="#565f89")
        else:
            rendered.append(" " * label_w, style="#414868")
        rendered.append("│", style="#2d3149")
        for ch, color in row:
            if color:
                rendered.append(ch, style=color)
            else:
                rendered.append(ch)
        rendered.append("\n")

    # X-axis
    rendered.append(" " * label_w)
    rendered.append("└" + "─" * width, style="#2d3149")
    rendered.append("\n")
    rendered.append(" " * (label_w + 1))
    rendered.append("0", style="#565f89")
    pad = max(1, width - 1 - len(f"step {max_n}"))
    rendered.append(" " * pad)
    rendered.append(f"step {max_n}", style="#565f89")
    rendered.append("\n")

    # Legend
    rendered.append("\n")
    for i, (name, _vs, color) in enumerate(series):
        if i:
            rendered.append("   ", style="#414868")
        rendered.append("●", style=color)
        rendered.append(f" {name}", style="#c0caf5")
    return rendered


def _sample(values: list[float], width: int) -> list[float]:
    if not values:
        return []
    if len(values) >= width:
        step = len(values) / width
        return [values[int(i * step)] for i in range(width)]
    return list(values)


def _fmt(v: float) -> str:
    return f"{v:.4g}"
