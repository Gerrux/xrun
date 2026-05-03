"""Final-Report view for the Run Detail screen.

Single scrollable page that ties together everything you'd want to see when
a run is finished: summary table of scalar metrics, a thumbnail grid of all
metric curves, and a file browser of the run-dir + any output artifacts.

Composed as a tab pane in RunDetailScreen alongside Stages / Logs / Manifest /
Metrics.

Why this lives separately from MetricsView:
* MetricsView is interactive exploration — focus one metric, smooth, log-y.
* ReportView is "printout / archive" — at-a-glance final state of the run,
  with side-by-side images and stats. Less knob-twisting, more reading.
"""
from __future__ import annotations

import os
import sys
from pathlib import Path
from typing import Iterable

from rich.console import Group
from rich.table import Table
from rich.text import Text
from textual.app import ComposeResult
from textual.binding import Binding
from textual.containers import Grid, Vertical, VerticalScroll
from textual.widgets import DataTable, Digits, Static

from xrun_tui.widgets.ascii_chart import render_chart
from xrun_tui.widgets.metrics_palette import color_for, is_lower_better

_TERMINAL_STATUSES = {"done", "failed", "cancelled"}
_IMAGE_SUFFIXES = {".png", ".jpg", ".jpeg", ".gif", ".svg", ".webp", ".bmp"}
_TEXT_SUFFIXES = {".txt", ".log", ".jsonl", ".json", ".yaml", ".yml",
                   ".md", ".csv", ".tsv"}


def _file_kind(path: Path) -> str:
    s = path.suffix.lower()
    if s in _IMAGE_SUFFIXES:
        return "image"
    if s in _TEXT_SUFFIXES:
        return "text"
    if s == ".pt" or s == ".bin" or s == ".safetensors":
        return "ckpt"
    return "file"


def _human_size(n: int) -> str:
    for unit in ("B", "KB", "MB", "GB"):
        if n < 1024:
            return f"{n:.0f} {unit}" if unit == "B" else f"{n:.1f} {unit}"
        n /= 1024  # type: ignore[assignment]
    return f"{n:.1f} TB"


class ReportView(VerticalScroll):
    """Full-page final report: stats + thumbnail grid + artifact browser."""

    DEFAULT_CSS = """
    ReportView { height: 1fr; padding: 0 1; background: #1a1b26; }
    ReportView .rv-section-title {
        color: #c0caf5;
        padding: 1 0 0 0;
        text-style: bold;
    }
    ReportView #rv-status { padding: 1 0; color: #c0caf5; }
    ReportView #rv-summary { height: auto; padding: 0 0 1 0; color: #c0caf5; }
    ReportView #rv-scalars {
        grid-size: 4;
        grid-gutter: 1 1;
        height: auto;
        padding: 0 0 1 0;
    }
    ReportView .rv-stat {
        height: 6;
        border: tall #2d3149;
        background: #1e2030;
        padding: 0 1;
        layout: vertical;
    }
    ReportView .rv-stat .rv-stat-key {
        height: 1;
        color: #565f89;
        text-style: bold;
    }
    ReportView .rv-stat Digits {
        height: 3;
        width: 1fr;
        content-align: center middle;
    }
    ReportView #rv-grid {
        grid-size: 3;
        grid-gutter: 1 1;
        height: auto;
        padding: 0 0 1 0;
    }
    ReportView .rv-tile {
        height: 9;
        border: tall #2d3149;
        background: #1e2030;
        padding: 0 1;
        color: #c0caf5;
    }
    ReportView #rv-files {
        height: auto;
        max-height: 18;
        background: #1e2030;
        border: tall #2d3149;
    }
    ReportView #rv-empty {
        color: #565f89;
        padding: 2 0;
        text-align: center;
    }
    """

    BINDINGS = [
        Binding("o",     "open_file",     "Open"),
        Binding("i",     "inline_preview", "Inline preview"),
        Binding("ctrl+r,f5", "refresh",   "Refresh", show=False),
    ]

    def __init__(self, run_id: str = "") -> None:
        super().__init__()
        self._run_id = run_id
        self._series: dict[str, list[float]] = {}
        self._status: str = ""
        self._files: list[Path] = []
        self._run_dir: Path | None = None

    def compose(self) -> ComposeResult:
        yield Static("", id="rv-status")
        yield Static("", id="rv-section-summary", classes="rv-section-title")
        yield Static("", id="rv-summary")
        yield Static("", id="rv-section-scalars", classes="rv-section-title")
        yield Grid(id="rv-scalars")
        yield Static("", id="rv-section-grid", classes="rv-section-title")
        yield Grid(id="rv-grid")
        yield Static("", id="rv-section-files", classes="rv-section-title")
        yield DataTable(id="rv-files",
                        cursor_type="row", zebra_stripes=True)
        yield Static("", id="rv-empty")

    def on_mount(self) -> None:
        files = self.query_one("#rv-files", DataTable)
        files.add_columns(
            Text(" ",      style="#565f89"),
            Text("Path",    style="#565f89"),
            Text("Size",    style="#565f89"),
            Text("Type",    style="#565f89"),
        )

    # ── Public API ────────────────────────────────────────────────────────────

    def update_report(
        self,
        run_id: str,
        run_dir: Path | None,
        series: dict[str, list[float]],
        status: str,
        workdir: Path | None = None,
    ) -> None:
        self._run_id = run_id
        self._run_dir = run_dir
        self._series = series
        self._status = status
        self._files = (list(_collect_files(run_dir, workdir))
                       if run_dir else [])
        self.run_worker(self._render_all_async(),
                        exclusive=True, group="report")

    def on_data_table_row_selected(
        self, event: DataTable.RowSelected,
    ) -> None:
        """Enter on the file table opens / previews the selected row."""
        if event.data_table.id == "rv-files":
            self.action_open_file()

    # ── Actions ───────────────────────────────────────────────────────────────

    def _focused_file(self) -> Path | None:
        if not self._files:
            return None
        try:
            t = self.query_one("#rv-files", DataTable)
            row = t.cursor_row
            if row is None or not (0 <= row < len(self._files)):
                return None
            return self._files[row]
        except Exception:
            return None

    def action_open_file(self) -> None:
        """Default: hand off to the OS so PNGs open at native resolution."""
        path = self._focused_file()
        if path is None:
            return
        if not _open_in_default_app(path):
            self.notify(f"Cannot open externally — path:\n{path}",
                        title="Open file", timeout=10)

    def action_inline_preview(self) -> None:
        """Optional half-block inline preview (low-fi, useful only as a sanity check)."""
        path = self._focused_file()
        if path is None:
            return
        if _file_kind(path) != "image":
            self.notify("Inline preview only works for images.",
                        severity="warning")
            return
        from xrun_tui.screens.image_view import ImagePreviewScreen
        self.app.push_screen(ImagePreviewScreen(path))

    async def action_refresh(self) -> None:
        if self._run_dir:
            self._files = list(_collect_files(self._run_dir))
        await self._render_all_async()

    # ── Rendering ─────────────────────────────────────────────────────────────

    def _render_all(self) -> None:  # kept for sync callers (not used now)
        if not self.is_mounted:
            return
        self._render_status()
        self._render_summary()
        self._render_files()
        self._render_empty()

    async def _render_all_async(self) -> None:
        if not self.is_mounted:
            return
        self._render_status()
        self._render_summary()
        await self._render_grid_async()
        self._render_files()
        self._render_empty()

    def _render_status(self) -> None:
        s = self._status or "?"
        glyph, color = {
            "done":      ("✓", "#9ece6a"),
            "failed":    ("✗", "#f7768e"),
            "cancelled": ("○", "#bb9af7"),
            "running":   ("●", "#7aa2f7"),
        }.get(s, ("·", "#c0caf5"))
        n = len(self._series)
        files = len(self._files)
        live = ("[#e0af68]live — re-open this tab when the run finishes "
                "for the full report[/]"
                if s not in _TERMINAL_STATUSES else
                "[#565f89]final report[/]")
        self.query_one("#rv-status", Static).update(
            f"[bold {color}]{glyph} {s}[/]   "
            f"[#565f89]·[/]   [#7aa2f7]{n}[/] [#565f89]metric keys[/]   "
            f"[#565f89]·[/]   [#7aa2f7]{files}[/] [#565f89]files in run-dir[/]   "
            f"[#565f89]·[/]   {live}"
        )

    def _render_summary(self) -> None:
        title = self.query_one("#rv-section-summary", Static)
        body  = self.query_one("#rv-summary", Static)
        if not self._series:
            title.update("")
            body.update("")
            return
        title.update("─── Scalar metrics (final) ───")

        rows = _compute_summary(self._series)
        if not rows:
            body.update("[#414868](no scalar metrics emitted)[/]")
            return
        tbl = Table(
            show_header=True, header_style="bold #565f89",
            box=None, padding=(0, 1, 0, 0), expand=False,
        )
        tbl.add_column("Key",   style="#c0caf5")
        tbl.add_column("n",     style="#7aa2f7", justify="right")
        tbl.add_column("First", style="#565f89", justify="right")
        tbl.add_column("Last",  style="#9ece6a", justify="right")
        tbl.add_column("Best",  style="bold #e0af68", justify="right")
        tbl.add_column("@step", style="#565f89", justify="right")
        tbl.add_column("Δ",     justify="right")
        for r in rows:
            delta = r["delta"]
            if abs(delta) < 1e-12:
                d_style = "#565f89"
            else:
                d_style = ("#9ece6a" if (delta < 0) == r["lower_better"]
                           else "#f7768e")
            arrow = "↓" if r["lower_better"] else "↑"
            tbl.add_row(
                f"[{color_for(r['key'])}]●[/] {r['key']} {arrow}",
                str(r["n"]),
                f"{r['first']:.4g}",
                f"{r['last']:.4g}",
                f"{r['best']:.4g}",
                str(r["best_at"]),
                Text(f"{delta:+.4g}", style=d_style),
            )
        body.update(Group(tbl))

    async def _render_grid_async(self) -> None:
        await self._render_scalars_async()
        await self._render_curves_async()

    async def _render_scalars_async(self) -> None:
        title = self.query_one("#rv-section-scalars", Static)
        grid = self.query_one("#rv-scalars", Grid)
        await grid.remove_children()
        scalars = [(k, v) for k, v in self._series.items() if len(v) == 1]
        if not scalars:
            title.update("")
            return
        title.update("─── Final scalars ───")
        tiles = [_ScalarTile(k, v[0]) for k, v in scalars]
        await grid.mount_all(tiles)

    async def _render_curves_async(self) -> None:
        title = self.query_one("#rv-section-grid", Static)
        grid = self.query_one("#rv-grid", Grid)
        await grid.remove_children()
        curves = [(k, v) for k, v in self._series.items() if len(v) >= 2]
        if not curves:
            title.update("")
            return
        title.update("─── Curves ───")
        tiles: list[Static] = []
        for k, vals in curves:
            chart = render_chart(vals, width=28, height=6,
                                 title=k, color=color_for(k))
            tile = Static(chart, classes="rv-tile")
            tile.border_subtitle = f"last {vals[-1]:.4g}  ·  n={len(vals)}"
            tiles.append(tile)
        if tiles:
            await grid.mount_all(tiles)

    def _render_files(self) -> None:
        title = self.query_one("#rv-section-files", Static)
        t = self.query_one("#rv-files", DataTable)
        t.clear()
        if not self._files:
            title.update("")
            return
        title.update(
            "─── Run-dir files ─── "
            "[#565f89]Enter / o = open externally · i = low-res inline preview[/]"
        )
        kind_color = {
            "image": "#bb9af7",
            "ckpt":  "#e0af68",
            "text":  "#7aa2f7",
            "file":  "#565f89",
        }
        base = self._run_dir
        for f in self._files:
            try:
                size = f.stat().st_size
            except OSError:
                size = 0
            kind = _file_kind(f)
            try:
                rel = f.relative_to(base) if base else f.name
            except ValueError:
                rel = f
            t.add_row(
                Text("●", style=kind_color.get(kind, "#565f89")),
                Text(str(rel), style="#c0caf5"),
                Text(_human_size(size), style="#9ece6a", justify="right"),
                Text(kind, style=kind_color.get(kind, "#565f89")),
            )

    def _render_empty(self) -> None:
        empty = self.query_one("#rv-empty", Static)
        if not self._series and not self._files:
            empty.update("[#414868]No data yet — re-open after the run finishes.[/]")
        else:
            empty.update("")


# ── Helpers ───────────────────────────────────────────────────────────────────

_LOWER_IS_BETTER_TOKENS = (
    "loss", "err", "nll", "perplexity", "ppl", "mae", "mse", "rmse", "wer", "cer",
)


def _compute_summary(series: dict[str, list[float]]) -> list[dict]:
    out: list[dict] = []
    for k, vals in series.items():
        if not vals:
            continue
        lower = is_lower_better(k)
        best = min(vals) if lower else max(vals)
        out.append({
            "key": k,
            "n": len(vals),
            "first": vals[0],
            "last": vals[-1],
            "best": best,
            "best_at": vals.index(best),
            "delta": vals[-1] - vals[0],
            "lower_better": lower,
        })
    return out


def _collect_files(
    run_dir: Path | None, workdir: Path | None = None,
) -> Iterable[Path]:
    """Yield interesting files for the report.

    Three sources, deduped:

    1. Top-level files in `run-dir` (events.jsonl, manifest.yaml, …).
    2. Anything under `run-dir/{artifacts,output,checkpoints,plots}/`
       (populated by `xrun pull` for vast/ssh, may be empty for local).
    3. For local runs, the manifest's `artifacts.patterns` + `checkpoints.watch`
       globbed from `workdir` (the launch CWD, fetched from the DB by the
       caller). Lets the Report tab show user-generated PNGs without waiting
       on `xrun pull`.
    """
    if not run_dir or not run_dir.exists():
        return
    skip_names = {"run.pid", "__pycache__"}
    seen: set[Path] = set()

    def _emit(p: Path) -> Iterable[Path]:
        try:
            r = p.resolve()
        except Exception:
            r = p
        if r in seen:
            return
        seen.add(r)
        yield p

    # 1. Run-dir top level
    for entry in sorted(run_dir.iterdir()):
        if entry.name in skip_names or entry.name.startswith("."):
            continue
        if entry.is_file() and entry.suffix != ".pyc":
            yield from _emit(entry)
        elif entry.is_dir() and entry.name in ("artifacts", "output",
                                                "checkpoints", "plots"):
            # 2. Pulled-artifact subdirs
            for sub in sorted(entry.rglob("*")):
                if sub.is_file() and sub.suffix != ".pyc":
                    yield from _emit(sub)

    # 3. Launch-cwd globbing (local vendor escape hatch)
    if not workdir or not workdir.exists():
        return
    patterns = _read_manifest_patterns(run_dir)
    for pat in patterns:
        try:
            matches = sorted(workdir.glob(pat))
        except Exception:
            matches = []
        for m in matches:
            if m.is_file():
                yield from _emit(m)


def _read_manifest_patterns(run_dir: Path) -> list[str]:
    """Best-effort extract `artifacts.patterns` and `checkpoints.watch`."""
    mfile = run_dir / "manifest.yaml"
    if not mfile.exists():
        return []
    try:
        import yaml  # type: ignore
        data = yaml.safe_load(mfile.read_text(encoding="utf-8")) or {}
    except Exception:
        return []
    patterns: list[str] = []
    arts = data.get("artifacts") or {}
    if isinstance(arts.get("patterns"), list):
        patterns += [str(p) for p in arts["patterns"]]
    ckpts = data.get("checkpoints") or {}
    if isinstance(ckpts.get("watch"), str):
        patterns.append(ckpts["watch"])
    return patterns


def _open_in_default_app(path: Path) -> bool:
    try:
        if sys.platform == "win32":
            os.startfile(str(path))  # type: ignore[attr-defined]
        elif sys.platform == "darwin":
            import subprocess
            subprocess.Popen(["open", str(path)])
        else:
            import subprocess
            subprocess.Popen(["xdg-open", str(path)])
        return True
    except Exception:
        return False


class _ScalarTile(Vertical):
    """Single-value metric: small label on top, big 7-segment number below."""

    def __init__(self, key: str, value: float) -> None:
        super().__init__(classes="rv-stat")
        self._key = key
        self._value = value

    def compose(self) -> ComposeResult:
        color = color_for(self._key)
        arrow = "↓" if is_lower_better(self._key) else "↑"
        yield Static(
            f"[{color}]●[/] [#c0caf5]{self._key}[/] "
            f"[#414868]{arrow}[/]",
            classes="rv-stat-key",
        )
        digits = Digits(_format_short(self._value))
        digits.styles.color = color
        yield digits


def _format_short(v: float) -> str:
    """Compact number for Digits: abbreviate big values, keep small ones precise."""
    av = abs(v)
    if av == 0:
        return "0"
    if av >= 1_000_000:
        return f"{v / 1_000_000:.2f}M"
    if av >= 10_000:
        return f"{v / 1000:.1f}K"
    if av >= 1000:
        return f"{v:.0f}"
    if av >= 1:
        return f"{v:.3f}".rstrip("0").rstrip(".")
    if av >= 0.001:
        return f"{v:.4f}".rstrip("0").rstrip(".")
    return f"{v:.2e}"
