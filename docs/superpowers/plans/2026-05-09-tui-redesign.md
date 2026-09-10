# TUI Redesign: OpenCode-inspired — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Redesign Dashboard, Runs, and Run Detail screens of the Python TUI (`python/xrun_tui`) to be clean and borderless, inspired by OpenCode's style — removing card/panel borders, collapsing KPI cards into a single status bar, flattening tab navigation, and stripping the StatusBar/Footer.

**Architecture:** Pure cosmetic changes via TCSS overrides and targeted Python compose/method edits. No behavioral changes: all keybindings, data loading, and navigation stay identical. The 5 tasks are fully independent once Task 1 (TCSS) is done — Tasks 2–5 each touch one Python file only.

**Tech Stack:** Python 3.11+, Textual 0.x, Rich markup, Tokyo Night color palette

---

## Files Modified

| File | What changes |
|------|-------------|
| `python/xrun_tui/src/xrun_tui/app.tcss` | Global: hide Footer/StatusBar; flatten Tabs; restyle cards, detail header, action buttons |
| `python/xrun_tui/src/xrun_tui/widgets/title_bar.py` | Remove `_MenuBtn` class and its `yield` |
| `python/xrun_tui/src/xrun_tui/screens/dashboard.py` | `compose()` restructure; `_render_kpi_bar()`; `_update_kpis`, `_set_doctor`, `_set_sinks` wired to new bar |
| `python/xrun_tui/src/xrun_tui/screens/runs.py` | Remove `StatusBar()` / `Footer()` from `compose()` |
| `python/xrun_tui/src/xrun_tui/screens/run_detail.py` | `compose()`: Label buttons + remove StatusBar/Footer; new click handler; CSS fixup |

---

## Task 1: TCSS — Flatten global styles

**Files:**
- Modify: `python/xrun_tui/src/xrun_tui/app.tcss`

- [ ] **Step 1: Read the current app.tcss** to orient before editing

```bash
# In workspace root
head -n 100 python/xrun_tui/src/xrun_tui/app.tcss
```

- [ ] **Step 2: Add global hide rules for Footer and StatusBar at top of file, just after the `Screen` base styles block (after line ~13)**

In `app.tcss`, find:
```css
Screen {
    background: #1a1b26;
    color: #c0caf5;
    scrollbar-color: #2d3149;
    scrollbar-color-hover: #414868;
}
```
Add immediately after that closing `}`:
```css

/* ── OpenCode redesign: hide chrome widgets ──────────────────────────────── */
Footer    { display: none; }
StatusBar { display: none; }
```

- [ ] **Step 3: Flatten standalone `Tabs` widget (used in Runs screen)**

Find in `app.tcss` (lines ~77–97):
```css
Tabs {
    background: #24283b;
    border-bottom: solid #414868;
    height: 3;
}

Tab {
    color: #565f89;
    padding: 0 3;
}

Tab.-active {
    color: #7aa2f7;
    text-style: bold;
    background: #1a1b26;
}

Tab:hover {
    color: #7dcfff;
    background: #1e2030;
}
```
Replace with:
```css
Tabs {
    background: #1a1b26;
    border-bottom: none;
    height: 1;
    padding: 0 2;
}

Tab {
    color: #565f89;
    padding: 0 2;
}

Tab.-active {
    color: #7aa2f7;
    text-style: bold underline;
    background: #1a1b26;
}

Tab:hover {
    color: #7dcfff;
    background: #1a1b26;
}
```

- [ ] **Step 4: Flatten `TabbedContent` inner Tabs (used in Run Detail)**

Find in `app.tcss` (lines ~150–169):
```css
TabbedContent Tabs {
    background: #1e2030;
    border-bottom: solid #2d3149;
}

TabbedContent Tab {
    color: #565f89;
    padding: 0 2;
}

TabbedContent Tab.-active {
    color: #7dcfff;
    text-style: bold;
    background: #1a1b26;
}
```
Replace with:
```css
TabbedContent Tabs {
    background: #1a1b26;
    border-bottom: none;
    height: 1;
    padding: 0 2;
}

TabbedContent Tab {
    color: #565f89;
    padding: 0 2;
}

TabbedContent Tab.-active {
    color: #7aa2f7;
    text-style: bold underline;
    background: #1a1b26;
}

TabbedContent Tab:hover {
    color: #7dcfff;
    background: #1a1b26;
}
```

- [ ] **Step 5: Replace Dashboard card + grid CSS with KPI bar style**

Find in `app.tcss` (lines ~564–623):
```css
#dash-root {
    height: 1fr;
    padding: 0;
}

#dash-kpi-grid {
    grid-size: 4 1;
    grid-gutter: 1 1;
    height: 5;
    padding: 1 2 0 2;
}

.kpi-card {
    background: #24283b;
    border: tall #2d3149;
    padding: 0 2;
    height: 4;
    content-align: left middle;
}

#dash-health-row {
    grid-size: 2 1;
    grid-gutter: 1 1;
    height: 5;
    padding: 0 2 0 2;
}

.health-card {
    background: #24283b;
    border: tall #2d3149;
    padding: 0 2;
    height: 4;
    content-align: left middle;
}

#dash-active-col, #dash-recent-col {
    width: 1fr;
    margin-right: 1;
    height: 1fr;
}

.dash-section {
    color: #bb9af7;
    text-style: bold;
    height: 1;
    padding: 0 1 0 1;
    border-bottom: solid #2d3149;
}
```
Replace with:
```css
#dash-root {
    height: 1fr;
    padding: 1 2;
}

#kpi-bar {
    height: 1;
    padding: 0 0 1 0;
    color: #c0caf5;
}

#dash-active-col, #dash-recent-col {
    width: 1fr;
    height: 1fr;
}

.dash-section {
    color: #565f89;
    text-style: bold;
    height: 1;
    padding: 1 0 0 0;
}
```

- [ ] **Step 6: Replace Run Detail header and action-button CSS**

Find in `app.tcss` (lines ~355–405):
```css
#detail-header {
    background: #24283b;
    border-bottom: solid #414868;
    padding: 1 2 0 2;
    height: auto;
}

#detail-chips {
    height: 1;
    align: left middle;
    padding: 0 0 1 0;
}

.chip {
    color: #565f89;
    width: auto;
    padding-right: 3;
}

#detail-actions {
    height: 3;
    align: left middle;
    padding: 0 0 0 0;
}

.action-btn {
    min-width: 12;
    margin-right: 1;
}
```
Replace with:
```css
#detail-header {
    background: #1a1b26;
    border-bottom: none;
    padding: 1 2 0 2;
    height: auto;
}

#detail-chips {
    height: 1;
    align: left middle;
    padding: 0 0 1 0;
}

.chip {
    color: #565f89;
    width: auto;
    padding-right: 3;
}

#detail-actions {
    height: 1;
    align: left middle;
    padding: 0 0 1 0;
}

.action-btn {
    width: auto;
    height: 1;
    padding: 0 1;
    margin-right: 1;
    color: #565f89;
    background: transparent;
}

.action-btn:hover {
    color: #7aa2f7;
    background: transparent;
}

.action-btn.danger {
    color: #f7768e;
    background: transparent;
}

.action-btn.danger:hover {
    color: #f7768e;
    background: transparent;
}
```

- [ ] **Step 7: Verify TCSS is valid by importing the app**

```bash
cd python/xrun_tui
python -c "from xrun_tui.app import XrunApp; print('TCSS OK')"
```

Expected: `TCSS OK` with no errors. If Textual reports a CSS parse error, it will print it to stderr.

- [ ] **Step 8: Commit**

```bash
cd python/xrun_tui
rtk git add ../../python/xrun_tui/src/xrun_tui/app.tcss
rtk git commit -m "style(tui): flatten tabs, remove card borders, hide Footer/StatusBar globally"
```

---

## Task 2: TitleBar — Remove the Menu button

**Files:**
- Modify: `python/xrun_tui/src/xrun_tui/widgets/title_bar.py`

- [ ] **Step 1: Read title_bar.py**

Current file (lines 1–60):
```python
from __future__ import annotations
from datetime import datetime
from textual.app import ComposeResult
from textual.containers import Horizontal
from textual.events import Click
from textual.widgets import Static

class _MenuBtn(Static):
    DEFAULT_CSS = """
    _MenuBtn { width: auto; height: 1; color: #7aa2f7; padding: 0 1; }
    _MenuBtn:hover { background: #2d3149; }
    """
    def on_click(self, event: Click) -> None:
        event.stop()
        self.run_worker(self.app.action_open_palette(), exclusive=True)

class _StatusClock(Static):
    # ... (unchanged)

class TitleBar(Horizontal):
    DEFAULT_CSS = """
    TitleBar { dock: top; height: 1; background: #24283b; padding: 0 0; align: left middle; }
    TitleBar #tb-title { width: 1fr; content-align: center middle; color: #c0caf5; }
    """
    def __init__(self, subtitle: str = "") -> None:
        super().__init__()
        self._subtitle = subtitle
    def compose(self) -> ComposeResult:
        yield _MenuBtn("⊞ Menu")
        label = f"xrun  —  {self._subtitle}" if self._subtitle else "xrun"
        yield Static(label, id="tb-title")
        yield _StatusClock()
```

- [ ] **Step 2: Remove `_MenuBtn` class and the `yield _MenuBtn(...)` line, update label format and TitleBar DEFAULT_CSS**

Replace the entire file content with:
```python
from __future__ import annotations
from datetime import datetime
from textual.app import ComposeResult
from textual.containers import Horizontal
from textual.widgets import Static


class _StatusClock(Static):
    """Right-side status: active-runs dot + HH:MM clock."""

    DEFAULT_CSS = """
    _StatusClock { width: auto; height: 1; padding: 0 1; color: #565f89; }
    """

    def __init__(self) -> None:
        super().__init__("[#414868]○[/]  [#7aa2f7]--:--[/]")
        self._active: int | None = None

    def on_mount(self) -> None:
        self._paint()
        self._clock_timer = self.set_interval(1.0, self._paint)
        self._poll_timer  = self.set_interval(5.0, self._refresh_async)
        self.run_worker(self._refresh_async(), exclusive=True)

    def on_unmount(self) -> None:
        for attr in ("_clock_timer", "_poll_timer"):
            try:
                getattr(self, attr).stop()
            except Exception:
                pass

    async def _refresh_async(self) -> None:
        try:
            runs = await self.app.db.runs(status="active")
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
    """Slim title bar: [bold cyan]xrun[/]  <subtitle>   ● N  HH:MM."""

    DEFAULT_CSS = """
    TitleBar { dock: top; height: 1; background: #1a1b26; padding: 0 2; align: left middle; }
    TitleBar #tb-title { width: 1fr; color: #c0caf5; }
    """

    def __init__(self, subtitle: str = "") -> None:
        super().__init__()
        self._subtitle = subtitle

    def compose(self) -> ComposeResult:
        if self._subtitle:
            label = f"[bold #7aa2f7]xrun[/]  [#565f89]{self._subtitle}[/]"
        else:
            label = "[bold #7aa2f7]xrun[/]"
        yield Static(label, id="tb-title")
        yield _StatusClock()
```

- [ ] **Step 3: Verify import works**

```bash
cd python/xrun_tui
python -c "from xrun_tui.widgets.title_bar import TitleBar; print('OK')"
```

Expected: `OK`

- [ ] **Step 4: Commit**

```bash
rtk git add src/xrun_tui/widgets/title_bar.py
rtk git commit -m "style(tui): remove menu button from TitleBar, use bold cyan xrun label"
```

---

## Task 3: Dashboard — Single KPI bar, remove card grids

**Files:**
- Modify: `python/xrun_tui/src/xrun_tui/screens/dashboard.py`

- [ ] **Step 1: Update imports — remove `Grid`, `Footer`**

Find at line 10–12:
```python
from textual.containers import Grid, Horizontal, Vertical
from textual.screen import Screen
from textual.widgets import DataTable, Footer, Static
from xrun_tui.widgets.status_bar import StatusBar
```
Replace with:
```python
from textual.containers import Horizontal, Vertical
from textual.screen import Screen
from textual.widgets import DataTable, Static
```

- [ ] **Step 2: Add `_kpi_cache` to `__init__`**

Find (lines 97–103):
```python
    def __init__(self) -> None:
        super().__init__()
        # Health-card cache. Doctor/Sinks probes are network-bound; we render
        # last known state instantly and refresh on slow tick.
        self._doctor_text:  tuple[str, str] = ("checking…", "#e0af68")
        self._sinks_text:   tuple[str, str] = ("checking…", "#e0af68")
        self._health_last:  float = 0.0
```
Replace with:
```python
    def __init__(self) -> None:
        super().__init__()
        # Health-card cache. Doctor/Sinks probes are network-bound; we render
        # last known state instantly and refresh on slow tick.
        self._doctor_text:  tuple[str, str] = ("checking…", "#e0af68")
        self._sinks_text:   tuple[str, str] = ("checking…", "#e0af68")
        self._health_last:  float = 0.0
        # KPI cache — updated by _update_kpis(), read by _render_kpi_bar()
        self._kpi_cache: dict = {
            "active": 0, "done": 0, "failed": 0,
            "stale": 0, "spent": 0.0, "burn": 0.0,
        }
```

- [ ] **Step 3: Replace `compose()` — remove Grid/cards, add KPI bar**

Find (lines 105–132):
```python
    def compose(self) -> ComposeResult:
        yield TitleBar("dashboard")
        with Vertical(id="dash-root"):
            with Grid(id="dash-kpi-grid"):
                yield Static(_kpi("Active runs", "—", "#9ece6a"),
                             id="kpi-active",  classes="kpi-card")
                yield Static(_kpi("Done (last)", "—", "#7aa2f7"),
                             id="kpi-done",    classes="kpi-card")
                yield Static(_kpi("Failed",      "—", "#f7768e"),
                             id="kpi-failed",  classes="kpi-card")
                yield Static(_kpi("Spent",       "—", "#e0af68"),
                             id="kpi-spent",   classes="kpi-card")
            with Grid(id="dash-health-row"):
                yield Static(_health("Doctor", "checking…", "#e0af68"),
                             id="health-doctor", classes="health-card")
                yield Static(_health("Sinks",  "checking…", "#e0af68"),
                             id="health-sinks",  classes="health-card")
            with Horizontal(id="dash-cols"):
                with Vertical(id="dash-active-col"):
                    yield Static("Active runs", classes="dash-section")
                    yield DataTable(id="dash-active",
                                    cursor_type="row", zebra_stripes=True)
                with Vertical(id="dash-recent-col"):
                    yield Static("Recently completed", classes="dash-section")
                    yield DataTable(id="dash-recent",
                                    cursor_type="row", zebra_stripes=True)
        yield StatusBar()
        yield Footer()
```
Replace with:
```python
    def compose(self) -> ComposeResult:
        yield TitleBar("dashboard")
        with Vertical(id="dash-root"):
            yield Static("", id="kpi-bar")
            with Horizontal(id="dash-cols"):
                with Vertical(id="dash-active-col"):
                    yield Static("ACTIVE RUNS", classes="dash-section")
                    yield DataTable(id="dash-active",
                                    cursor_type="row", zebra_stripes=True)
                with Vertical(id="dash-recent-col"):
                    yield Static("RECENTLY COMPLETED", classes="dash-section")
                    yield DataTable(id="dash-recent",
                                    cursor_type="row", zebra_stripes=True)
```

- [ ] **Step 4: Add `_render_kpi_bar()` method — add it just before `_update_kpis()`**

Find the line `def _update_kpis(self, runs: list[dict], burn_dph: float) -> None:` and insert this method immediately before it:

```python
    def _render_kpi_bar(self) -> None:
        """Build and push the single-line KPI status bar from cached state."""
        d = self._kpi_cache
        active, done, failed = d["active"], d["done"], d["failed"]
        stale, spent, burn   = d["stale"],  d["spent"],  d["burn"]

        run_label = f"▸ {active}" + (f" +{stale}⚠" if stale else "") + " running"
        run_col   = "bold #e0af68" if stale else ("#9ece6a" if active else "#565f89")

        parts = [
            f"[{run_col}]{run_label}[/]",
            f"[{'#7dcfff' if done else '#565f89'}]✓ {done} done[/]",
            f"[{'bold #f7768e' if failed else '#565f89'}]✗ {failed} failed[/]",
            f"[{'#e0af68' if spent else '#565f89'}]${spent:.2f}[/]",
        ]
        if burn > 0:
            parts.append(f"[#565f89]~$[/][#e0af68]{burn:.2f}[/][#565f89]/hr[/]")

        doc_val, doc_col = self._doctor_text
        parts.append(f"[#565f89]doctor[/] [{doc_col}]{doc_val}[/]")

        snk_val, snk_col = self._sinks_text
        if snk_val:
            parts.append(f"[#565f89]sinks[/] [{snk_col}]{snk_val}[/]")

        try:
            self.query_one("#kpi-bar", Static).update("  ".join(parts))
        except Exception:
            pass  # not yet mounted

```

- [ ] **Step 5: Replace `_update_kpis()` to use the cache + bar**

Find (lines 513–555):
```python
    def _update_kpis(self, runs: list[dict], burn_dph: float) -> None:
        active = sum(1 for r in runs if r["status"] in _ACTIVE_STATES)
        done   = sum(1 for r in runs if r["status"] == "done")
        failed = sum(1 for r in runs if r["status"] == "failed")
        stale  = sum(1 for r in runs if is_stale(r))
        spent  = sum(
            (r.get("cost_usd") or r.get("cost_usd_estimate") or 0.0)
            for r in runs
        )

        # Fold stale into the Active card so the warning is impossible to miss
        # without us claiming a whole second card for it.
        if stale:
            active_value = f"{active}  ⚠ {stale}"
        else:
            active_value = str(active)
        self.query_one("#kpi-active", Static).update(
            _kpi(
                "Active runs",
                active_value,
                "bold #e0af68" if stale else
                ("bold #9ece6a" if active else "#414868"),
            )
        )
        self.query_one("#kpi-done", Static).update(
            _kpi("Done", str(done), "#7aa2f7" if done else "#414868")
        )
        self.query_one("#kpi-failed", Static).update(
            _kpi("Failed", str(failed), "bold #f7768e" if failed else "#414868")
        )

        # Spent KPI: cumulative $ + live burn-rate subline. We only show the
        # subline when there's actual burn — otherwise the card stays clean.
        spent_str = f"${spent:.2f}"
        if burn_dph > 0:
            sub = f"[#e0af68]+${burn_dph:.2f}/hr[/]"
        else:
            sub = None
        self.query_one("#kpi-spent", Static).update(
            _kpi("Spent", spent_str,
                 "#e0af68" if spent else "#414868",
                 sub=sub)
        )
```
Replace with:
```python
    def _update_kpis(self, runs: list[dict], burn_dph: float) -> None:
        self._kpi_cache = {
            "active": sum(1 for r in runs if r["status"] in _ACTIVE_STATES),
            "done":   sum(1 for r in runs if r["status"] == "done"),
            "failed": sum(1 for r in runs if r["status"] == "failed"),
            "stale":  sum(1 for r in runs if is_stale(r)),
            "spent":  sum(
                (r.get("cost_usd") or r.get("cost_usd_estimate") or 0.0)
                for r in runs
            ),
            "burn": burn_dph,
        }
        self._render_kpi_bar()
```

- [ ] **Step 6: Update `_set_doctor()` to call `_render_kpi_bar()` instead of querying `#health-doctor`**

Find (lines 325–332):
```python
    def _set_doctor(self, text: str, style: str) -> None:
        self._doctor_text = (text, style)
        try:
            self.query_one("#health-doctor", Static).update(
                _health("Doctor", text, style)
            )
        except Exception:
            pass  # widget not mounted yet
```
Replace with:
```python
    def _set_doctor(self, text: str, style: str) -> None:
        self._doctor_text = (text, style)
        self._render_kpi_bar()
```

- [ ] **Step 7: Update `_set_sinks()` to call `_render_kpi_bar()` instead of querying `#health-sinks`**

Find (lines 442–449):
```python
    def _set_sinks(self, text: str, style: str) -> None:
        self._sinks_text = (text, style)
        try:
            self.query_one("#health-sinks", Static).update(
                _health("Sinks", text, style)
            )
        except Exception:
            pass
```
Replace with:
```python
    def _set_sinks(self, text: str, style: str) -> None:
        self._sinks_text = (text, style)
        self._render_kpi_bar()
```

- [ ] **Step 8: Verify import**

```bash
cd python/xrun_tui
python -c "from xrun_tui.screens.dashboard import DashboardScreen; print('OK')"
```

Expected: `OK`

- [ ] **Step 9: Commit**

```bash
rtk git add src/xrun_tui/screens/dashboard.py
rtk git commit -m "style(tui): replace KPI cards with single status bar on Dashboard"
```

---

## Task 4: Runs screen — Remove StatusBar and Footer

**Files:**
- Modify: `python/xrun_tui/src/xrun_tui/screens/runs.py`

- [ ] **Step 1: Remove `StatusBar` and `Footer` imports**

Find in runs.py (near top, look for these imports):
```python
from textual.widgets import DataTable, Footer, Static, Tab, Tabs
from xrun_tui.widgets.status_bar import StatusBar
```
Replace with:
```python
from textual.widgets import DataTable, Static, Tab, Tabs
```

Note: If the imports are on different lines or combined differently, adapt accordingly — the goal is to remove `Footer` from the `textual.widgets` import and remove the `StatusBar` import line entirely.

- [ ] **Step 2: Remove `StatusBar()` and `Footer()` from `compose()`**

Find (lines 98–115):
```python
    def compose(self) -> ComposeResult:
        yield TitleBar("runs")
        yield Tabs(
            Tab("All",    id="tab-all"),
            Tab("Active", id="tab-active"),
            Tab("Recent", id="tab-recent"),
        )
        yield FilterBar(
            on_change=self._on_filter_change,
            on_close=self._on_filter_close,
            placeholder="filter by name / id / vendor / status…",
            id="runs-filter",
        )
        yield Static("", id="runs-stats", classes="stats-bar")
        yield DataTable(id="runs-table", cursor_type="row", zebra_stripes=True)
        yield Static("", id="runs-empty", classes="empty-state")
        yield StatusBar()
        yield Footer()
```
Replace with:
```python
    def compose(self) -> ComposeResult:
        yield TitleBar("runs")
        yield Tabs(
            Tab("All",    id="tab-all"),
            Tab("Active", id="tab-active"),
            Tab("Recent", id="tab-recent"),
        )
        yield FilterBar(
            on_change=self._on_filter_change,
            on_close=self._on_filter_close,
            placeholder="filter by name / id / vendor / status…",
            id="runs-filter",
        )
        yield Static("", id="runs-stats", classes="stats-bar")
        yield DataTable(id="runs-table", cursor_type="row", zebra_stripes=True)
        yield Static("", id="runs-empty", classes="empty-state")
```

- [ ] **Step 3: Verify**

```bash
cd python/xrun_tui
python -c "from xrun_tui.screens.runs import RunsScreen; print('OK')"
```

Expected: `OK`

- [ ] **Step 4: Commit**

```bash
rtk git add src/xrun_tui/screens/runs.py
rtk git commit -m "style(tui): remove StatusBar and Footer from Runs screen"
```

---

## Task 5: Run Detail — Label buttons, remove StatusBar/Footer

**Files:**
- Modify: `python/xrun_tui/src/xrun_tui/screens/run_detail.py`

- [ ] **Step 1: Update imports — remove `Button`, `Footer`; add `Label`; remove `StatusBar`**

Find in run_detail.py (near top, look for the textual imports):
```python
from textual.widgets import Button, DataTable, Footer, Input, RichLog, Static, TabbedContent, TabPane
from xrun_tui.widgets.status_bar import StatusBar
```
Replace with:
```python
from textual.events import Click
from textual.widgets import DataTable, Input, Label, RichLog, Static, TabbedContent, TabPane
```

Note: `Click` may already be imported — check first and skip if so. The `StatusBar` import line should be removed. If `Button` is the only widget using the `Button` identifier in the file (other than `compose()`), removing it here is sufficient.

- [ ] **Step 2: Replace Button widgets with Label widgets in `compose()`**

Find (lines 84–91 inside `compose()`):
```python
            with Horizontal(id="detail-actions"):
                yield Button("Stop  [s]",       id="btn-stop",      classes="action-btn danger")
                yield Button("Rerun [r]",       id="btn-rerun",     classes="action-btn")
                yield Button("Patch [R]",       id="btn-patch",     classes="action-btn")
                yield Button("Pull  [p]",       id="btn-pull",      classes="action-btn")
                yield Button("Artifacts [a]",   id="btn-artifacts", classes="action-btn")
                yield Button("Relaunch",        id="btn-relaunch",  classes="action-btn")
                yield Button("Error detail [E]",id="btn-error",     classes="action-btn danger")
```
Replace with:
```python
            with Horizontal(id="detail-actions"):
                yield Label("[s] stop",        id="btn-stop",      classes="action-btn danger")
                yield Label("[r] rerun",       id="btn-rerun",     classes="action-btn")
                yield Label("[R] patch",       id="btn-patch",     classes="action-btn")
                yield Label("[p] pull",        id="btn-pull",      classes="action-btn")
                yield Label("[a] artifacts",   id="btn-artifacts", classes="action-btn")
                yield Label("relaunch",        id="btn-relaunch",  classes="action-btn")
                yield Label("[E] error",       id="btn-error",     classes="action-btn danger")
```

- [ ] **Step 3: Remove `StatusBar()` and `Footer()` from `compose()`**

Find (lines 111–112 at end of `compose()`):
```python
        yield StatusBar()
        yield Footer()
```
Delete both lines.

- [ ] **Step 4: Replace `on_button_pressed` with `on_click` handler**

Find (lines 407–415):
```python
    def on_button_pressed(self, event: Button.Pressed) -> None:
        match event.button.id:
            case "btn-stop":      self.run_worker(self.action_stop_run())
            case "btn-rerun":     self.run_worker(self.action_rerun())
            case "btn-patch":     self.run_worker(self.action_patch_rerun())
            case "btn-pull":      self.run_worker(self.action_pull())
            case "btn-artifacts": self.run_worker(self.action_artifacts())
            case "btn-relaunch":  self.run_worker(self._do_relaunch())
            case "btn-error":     self.run_worker(self.action_error_detail())
```
Replace with:
```python
    def on_click(self, event: Click) -> None:
        wid = getattr(event.widget, "id", None)
        if not isinstance(wid, str) or not wid.startswith("btn-"):
            return
        event.stop()
        match wid:
            case "btn-stop":      self.run_worker(self.action_stop_run())
            case "btn-rerun":     self.run_worker(self.action_rerun())
            case "btn-patch":     self.run_worker(self.action_patch_rerun())
            case "btn-pull":      self.run_worker(self.action_pull())
            case "btn-artifacts": self.run_worker(self.action_artifacts())
            case "btn-relaunch":  self.run_worker(self._do_relaunch())
            case "btn-error":     self.run_worker(self.action_error_detail())
```

- [ ] **Step 5: Verify**

```bash
cd python/xrun_tui
python -c "from xrun_tui.screens.run_detail import RunDetailScreen; print('OK')"
```

Expected: `OK`

- [ ] **Step 6: Commit**

```bash
rtk git add src/xrun_tui/screens/run_detail.py
rtk git commit -m "style(tui): replace Button with Label in Run Detail, remove StatusBar/Footer"
```

---

## Final Verification

- [ ] **Install package in editable mode and run smoke import**

```bash
pip install -e python/xrun_tui
python -c "
from xrun_tui.app import XrunApp
from xrun_tui.screens.dashboard import DashboardScreen
from xrun_tui.screens.runs import RunsScreen
from xrun_tui.screens.run_detail import RunDetailScreen
from xrun_tui.widgets.title_bar import TitleBar
print('All imports OK')
"
```

Expected: `All imports OK`

- [ ] **Launch TUI and visually verify**

```bash
# On a real TTY: xrun
# Or force the Python TUI directly:
python -m xrun_tui
```

Check:
1. TitleBar: `xrun  dashboard  ● N  HH:MM` — no Menu button, `xrun` in cyan
2. Dashboard: single status line at top (running / done / failed / cost / doctor)
3. Dashboard: no bordered KPI cards, no bordered health cards
4. Runs: tab row is flat, height 1, underlined active tab
5. Run Detail: action row is single-line dim text labels, not tall bordered buttons
6. Run Detail: tab bar (Stages / Logs / Manifest / Metrics / Report) is flat, height 1
7. No Footer bar at the bottom of any screen
8. No StatusBar above the Footer on any screen

- [ ] **Final commit (if any fixups were needed)**

```bash
rtk git add -A
rtk git commit -m "fix(tui): post-integration fixups from visual review"
```
