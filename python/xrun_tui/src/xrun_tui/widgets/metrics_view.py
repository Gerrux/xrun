"""Master-detail Metrics view for the Run Detail screen.

Layout (matches W&B / TensorBoard conventions):

    ┌─ toolbar ────────────────────────────────────────┐
    │ group:on  log:off  smooth:off       /: filter    │
    ├─ master ──────────┬─ detail ────────────────────┤
    │ key  n  last  Δ   │ last 0.16  best 0.16 @ 4    │
    │ ▔▔▔▔▔▔▔▔▔▔▔▔▔▔▔   │ Δ -0.84  n=5  ↓ lower=better │
    │ loss     5  0.20  │                              │
    │ val_loss 5  0.16  │      ●●●●●●●● big chart      │
    │ acc      5  0.82  │                              │
    │ …                 │ ● loss    ● val_loss          │
    └───────────────────┴──────────────────────────────┘

Auto-grouping puts `loss/val_loss/test_loss` on the same chart, etc. Toggle
with `g`. Log Y with `L`, EMA smoothing with `M`, fuzzy filter with `/`.
"""
from __future__ import annotations

import os
import subprocess
import sys
import tempfile
import time
from pathlib import Path
from typing import Iterable

from rich.text import Text
from textual.app import ComposeResult
from textual.binding import Binding
from textual.containers import Horizontal, Vertical
from textual.widgets import DataTable, Input, Static

from xrun_tui.services import _run as _xrun
from xrun_tui.widgets.ascii_chart import render_chart_multi
from xrun_tui.widgets.metrics_palette import (
    color_for,
    ema,
    group_keys,
    is_lower_better,
)

_TERMINAL_STATUSES = {"done", "failed", "cancelled"}
_SPARK_BARS = "▁▂▃▄▅▆▇█"


def _sparkline(values: list[float]) -> str:
    if not values:
        return ""
    lo, hi = min(values), max(values)
    if hi - lo < 1e-12:
        return _SPARK_BARS[3] * len(values[-32:])
    n = len(_SPARK_BARS) - 1
    return "".join(
        _SPARK_BARS[int((v - lo) / (hi - lo) * n)] for v in values[-32:]
    )


class MetricsView(Vertical):
    """Container widget composed into the Metrics tab of RunDetailScreen."""

    DEFAULT_CSS = """
    MetricsView { layout: vertical; height: 1fr; }
    MetricsView #mv-toolbar { height: 1; padding: 0 1; background: #1e2030; }
    MetricsView #mv-toolbar-text { width: 1fr; color: #565f89; }
    MetricsView #mv-filter { display: none; height: 1; border: none;
                              background: #1e2030; }
    MetricsView #mv-filter.shown { display: block; }
    MetricsView #mv-body { layout: horizontal; height: 1fr; }
    MetricsView #mv-master { width: 42; border-right: solid #2d3149; }
    MetricsView #mv-table { height: 1fr; }
    MetricsView #mv-detail { width: 1fr; padding: 0 1;
                              background: #1a1b26; }
    MetricsView #mv-chips { height: auto; min-height: 2; padding: 1 0;
                             color: #c0caf5; }
    MetricsView #mv-chart { height: 1fr; padding: 0 0 1 0;
                             color: #c0caf5; }
    """

    BINDINGS = [
        Binding("g", "toggle_group",  "Group"),
        Binding("L", "toggle_log",    "Log-y"),
        Binding("M", "toggle_smooth", "Smooth"),
        Binding("slash", "focus_filter", "Filter"),
        Binding("P", "export_png", "Open PNG"),
    ]

    def __init__(self, run_id: str = "") -> None:
        super().__init__()
        self._run_id = run_id
        self._series: dict[str, list[float]] = {}
        self._status: str = ""
        self._group = True
        self._log = False
        self._smooth = False
        self._filter = ""
        self._focus_key: str | None = None  # row currently selected in master

    def compose(self) -> ComposeResult:
        with Horizontal(id="mv-toolbar"):
            yield Static(self._toolbar_text(), id="mv-toolbar-text")
        yield Input(id="mv-filter", placeholder="filter keys… (Esc to clear)")
        with Horizontal(id="mv-body"):
            with Vertical(id="mv-master"):
                yield DataTable(id="mv-table",
                                cursor_type="row", zebra_stripes=True)
            with Vertical(id="mv-detail"):
                yield Static("", id="mv-chips")
                yield Static("", id="mv-chart")

    def on_mount(self) -> None:
        t = self.query_one("#mv-table", DataTable)
        t.add_columns(
            Text("Key",    style="#565f89"),
            Text("n",      style="#565f89"),
            Text("Last",   style="#565f89"),
            Text("Δ",      style="#565f89"),
            Text("Spark",  style="#565f89"),
        )

    # ── Public API ────────────────────────────────────────────────────────────

    def update_metrics(
        self, series: dict[str, list[float]], status: str,
    ) -> None:
        """Replace state and re-render. Idempotent — safe to call on poll tick."""
        self._series = series
        self._status = status
        self._render_all()

    # ── Actions ───────────────────────────────────────────────────────────────

    def action_toggle_group(self) -> None:
        self._group = not self._group
        self._render_all()

    def action_toggle_log(self) -> None:
        self._log = not self._log
        self._render_chart()
        self._render_toolbar()

    def action_toggle_smooth(self) -> None:
        self._smooth = not self._smooth
        self._render_chart()
        self._render_toolbar()

    def action_focus_filter(self) -> None:
        inp = self.query_one("#mv-filter", Input)
        inp.set_class(True, "shown")
        inp.focus()

    async def action_export_png(self) -> None:
        """Render all metrics as a PNG grid (one subplot per key) and open it."""
        if not self._run_id:
            self.notify("No run id wired into MetricsView", severity="error")
            return
        if not self._series:
            self.notify("No metrics to export yet", severity="warning")
            return
        out_dir = Path(tempfile.gettempdir()) / "xrun-png"
        out_dir.mkdir(parents=True, exist_ok=True)
        ts = time.strftime("%Y%m%d-%H%M%S")
        out_path = out_dir / f"{self._run_id[:8]}-{ts}.png"
        self.notify("Rendering PNG…", timeout=2)
        code, _out, err = await _xrun(
            "metrics", self._run_id,
            "--png", str(out_path),
            "--per-key",
            timeout=60,
        )
        if code != 0 or not out_path.exists():
            self.notify(
                f"PNG render failed: {(err or '').strip()[:120]}",
                severity="error", timeout=10,
            )
            return
        if _open_in_default_app(out_path):
            self.notify(f"Opened {out_path.name}", timeout=4)
        else:
            self.notify(f"Saved to {out_path}", timeout=8)

    def on_key(self, event) -> None:  # type: ignore[override]
        # Esc clears the filter when the input is shown, otherwise propagates
        # so the screen-level binding (go back) still fires.
        if event.key != "escape":
            return
        inp = self.query_one("#mv-filter", Input)
        if not inp.has_class("shown"):
            return
        inp.value = ""
        self._filter = ""
        inp.set_class(False, "shown")
        self.query_one("#mv-table", DataTable).focus()
        self._render_master()
        event.stop()

    def on_input_changed(self, event: Input.Changed) -> None:
        if event.input.id != "mv-filter":
            return
        self._filter = event.value.strip().lower()
        self._render_master()

    def on_input_submitted(self, event: Input.Submitted) -> None:
        if event.input.id == "mv-filter":
            self.query_one("#mv-table", DataTable).focus()

    def on_data_table_row_highlighted(
        self, event: DataTable.RowHighlighted,
    ) -> None:
        if event.data_table.id != "mv-table":
            return
        key = event.row_key.value if event.row_key else None
        if key:
            self._focus_key = key
            self._render_chips()
            self._render_chart()

    # ── Rendering ─────────────────────────────────────────────────────────────

    def _render_all(self) -> None:
        if not self.is_mounted:
            return
        self._render_toolbar()
        self._render_master()
        self._render_chips()
        self._render_chart()

    def _toolbar_text(self) -> str:
        live = (
            "" if self._status in _TERMINAL_STATUSES
            else "  [#7aa2f7]live ↻[/]"
        )
        return (
            f"[#565f89]group:[/] [{'#9ece6a' if self._group else '#565f89'}]"
            f"{'on' if self._group else 'off'}[/]   "
            f"[#565f89]log-y:[/] [{'#9ece6a' if self._log else '#565f89'}]"
            f"{'on' if self._log else 'off'}[/]   "
            f"[#565f89]smooth:[/] [{'#9ece6a' if self._smooth else '#565f89'}]"
            f"{'on' if self._smooth else 'off'}[/]   "
            f"[#565f89]g · L · M · /  filter[/]"
            f"{live}"
        )

    def _render_toolbar(self) -> None:
        try:
            self.query_one("#mv-toolbar-text", Static).update(
                self._toolbar_text(),
            )
        except Exception:
            pass

    def _filtered_keys(self) -> list[str]:
        keys = list(self._series.keys())
        if self._filter:
            keys = [k for k in keys if self._filter in k.lower()]
        return keys

    def _render_master(self) -> None:
        table = self.query_one("#mv-table", DataTable)
        prev = self._focus_key
        table.clear()

        keys = self._filtered_keys()
        if not keys:
            self._focus_key = None
            return

        for k in keys:
            vals = self._series.get(k, [])
            color = color_for(k)
            n = len(vals)
            last = f"{vals[-1]:.4g}" if vals else "—"
            delta = (vals[-1] - vals[0]) if len(vals) >= 2 else 0.0
            delta_good = (
                (delta < 0) == is_lower_better(k) if abs(delta) > 1e-12 else None
            )
            delta_style = (
                "#9ece6a" if delta_good is True
                else "#f7768e" if delta_good is False
                else "#565f89"
            )
            spark = _sparkline(vals)
            table.add_row(
                Text(k, style=color),
                Text(str(n), style="#7aa2f7"),
                Text(last, style="#c0caf5"),
                Text(f"{delta:+.3g}" if abs(delta) > 1e-12 else "·",
                     style=delta_style),
                Text(spark, style=color),
                key=k,
            )

        # Restore cursor position
        target = prev if prev in keys else keys[0]
        try:
            row = keys.index(target)
            table.move_cursor(row=row, animate=False)
            self._focus_key = target
        except (ValueError, Exception):
            self._focus_key = keys[0] if keys else None

    def _peers(self, key: str) -> list[str]:
        """Keys that share a stem with `key` and exist in the series."""
        if not self._group:
            return [key]
        groups = group_keys(list(self._series.keys()))
        for stem, members in groups.items():
            if key in members:
                return members
        return [key]

    def _render_chips(self) -> None:
        chips = self.query_one("#mv-chips", Static)
        if not self._focus_key or self._focus_key not in self._series:
            chips.update("[#414868]Select a metric on the left[/]")
            return
        k = self._focus_key
        vals = self._series[k]
        if not vals:
            chips.update(f"[bold]{k}[/]  [#414868]no points[/]")
            return
        lower = is_lower_better(k)
        best = min(vals) if lower else max(vals)
        best_at = vals.index(best)
        delta = vals[-1] - vals[0] if len(vals) >= 2 else 0.0
        arrow = "↓ lower=better" if lower else "↑ higher=better"
        peers = self._peers(k)
        peers_label = (
            f"  [#565f89]grouped:[/] " +
            " ".join(f"[{color_for(p)}]●[/] [#c0caf5]{p}[/]" for p in peers)
            if len(peers) > 1 else ""
        )
        chips.update(
            f"[bold {color_for(k)}]{k}[/]  [#565f89]({arrow})[/]\n"
            f"[#565f89]last[/] [#c0caf5]{vals[-1]:.4g}[/]   "
            f"[#565f89]best[/] [bold #e0af68]{best:.4g}[/] "
            f"[#565f89]@[/] [#c0caf5]{best_at}[/]   "
            f"[#565f89]Δ[/] [{'#9ece6a' if (delta < 0) == lower else '#f7768e'}]"
            f"{delta:+.3g}[/]   "
            f"[#565f89]n[/] [#7aa2f7]{len(vals)}[/]"
            f"{peers_label}"
        )

    def _render_chart(self) -> None:
        chart = self.query_one("#mv-chart", Static)
        if not self._focus_key or self._focus_key not in self._series:
            chart.update("")
            return
        peers = self._peers(self._focus_key)
        series = []
        for p in peers:
            vals = self._series.get(p, [])
            if not vals:
                continue
            if self._smooth:
                vals = ema(vals)
            series.append((p, vals, color_for(p)))
        if not series:
            chart.update("[#414868](no data)[/]")
            return
        size = chart.size
        width = max(40, (size.width or 80) - 12)
        height = max(8, (size.height or 14) - 4)
        chart.update(render_chart_multi(
            series, width=width, height=height, log_y=self._log,
        ))

    def on_resize(self, _event) -> None:  # type: ignore[override]
        self._render_chart()

    # ── External cursor control (used by tests / keybindings) ────────────────

    def select_keys(self, keys: Iterable[str]) -> None:
        """Force-select the first key from `keys` that exists. No-op otherwise."""
        existing = [k for k in keys if k in self._series]
        if not existing:
            return
        target = existing[0]
        table = self.query_one("#mv-table", DataTable)
        try:
            row = self._filtered_keys().index(target)
            table.move_cursor(row=row, animate=False)
        except (ValueError, Exception):
            pass


def _open_in_default_app(path: Path) -> bool:
    """Open a file with the OS default viewer. Returns True on success."""
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
