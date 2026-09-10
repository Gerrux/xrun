# TUI Redesign: OpenCode-inspired — Design Spec

**Date:** 2026-05-09
**Scope:** Python TUI (`python/xrun_tui`) — Dashboard, Runs, Run Detail screens
**Approach:** Remove panel borders, replace with color+spacing, strip redundant widgets

---

## Problem Statement

The current TUI uses Textual's default `border: tall/solid/round` on every panel, card, and table wrapper. This creates visual noise without adding information. The StatusBar duplicates info already in TitleBar. The Footer takes vertical space showing hints that are rarely looked at. Buttons and tabs have heavy widget borders that feel cluttered.

Goal: clean, information-dense UI where color carries meaning instead of boxes.

---

## Color System (unchanged — Tokyo Night palette)

| Role | Color | Usage |
|------|-------|-------|
| `$accent` | `#7aa2f7` | Active tab underline, TitleBar "xrun" label, focused elements |
| `$success` | `#9ece6a` | `●` running status, doctor ✓, active count >0 |
| `$info` | `#7dcfff` | `✓` done status |
| `$warning` | `#e0af68` | Cost/money values, queued/pending status |
| `$error` | `#f7768e` | `✗` failed status |
| `$section-label` | `#565f89` | Section headers (ACTIVE RUNS), dim metadata labels |
| `$text` | `#c0caf5` | Normal body text |
| `$bg` | `#1a1b26` | App background |
| `$surface` | `#24283b` | (no longer used for bordered panels; kept for modals only) |

---

## Global Changes (all 3 screens)

### Remove: StatusBar widget
- File: `python/xrun_tui/src/xrun_tui/widgets/status_bar.py`
- Remove `StatusBar()` from `compose()` in Dashboard, Runs, and Run Detail screens only
- Do NOT delete the file — other screens (watch, instances, etc.) may still use it
- The active-run count and clock are already in TitleBar; vendor balance is dropped from always-visible UI

### Remove: Footer widget
- Textual's built-in `Footer` (keybinding hints bar at bottom) is hidden globally
- Add to `app.tcss`: `Footer { display: none; }`
- Critical bindings surface as inline hints where needed (e.g., dim text in empty states)

### Modify: TitleBar widget (`widgets/title_bar.py`)
**Current:** `[⊞ Menu]  xrun — <subtitle>   ● N  HH:MM`

**New:** `xrun  <subtitle>  ● N  HH:MM`

Changes:
- Remove `_MenuBtn` component (the `[⊞ Menu]` button). Ctrl+P still works for palette
- `xrun` label: `color: #7aa2f7; text-style: bold`
- Subtitle: `color: #565f89`
- `● N`: green (`#9ece6a`) when N > 0, dim (`#565f89`) when 0
- Clock: `color: #565f89; text-align: right`
- No border on TitleBar itself (already height 1, just ensure `border: none`)

### Global TCSS rules to add (`app.tcss`)
```css
Footer { display: none; }
StatusBar { display: none; }

/* Strip borders from DataTable wrappers */
.panel { border: none; padding: 0 2; }

/* Section label style */
.section-label {
    color: #565f89;
    text-style: bold;
    padding: 1 0 0 0;
}

/* KPI bar */
#kpi-bar {
    height: 1;
    padding: 0 2;
}

/* Flat tab row */
.flat-tabs {
    height: 1;
    padding: 0 2;
}
.flat-tabs .tab-active {
    color: #7aa2f7;
    text-style: bold underline;
}
.flat-tabs .tab-inactive {
    color: #565f89;
}
```

---

## Dashboard Screen (`screens/dashboard.py`)

### Layout — Before
```
TitleBar
Grid#dash-kpi-grid          ← 4 BorderedCard widgets
Grid#dash-health-row        ← 2 BorderedCard widgets
Horizontal#dash-cols
  Vertical#dash-active-col  ← Label + DataTable in Block wrapper
  Vertical#dash-recent-col  ← Label + DataTable in Block wrapper
StatusBar
Footer
```

### Layout — After
```
TitleBar
Vertical#dash-root (padding: 0 2)
  Static#kpi-bar
    "▸ 2 running  ✓ 14 done  ✗ 1 failed  $0.43    doctor ✓  mlflow ✓"
  Rule (color: #2d3149)
  Static.section-label "ACTIVE RUNS"
  DataTable#dash-active
  Static.section-label "RECENTLY COMPLETED"
  DataTable#dash-recent
```

### KPI bar formatting
- `▸ N running` — `▸` and count in `#9ece6a`
- `✓ N done` — `✓` and count in `#7dcfff`
- `✗ N failed` — `✗` and count in `#f7768e`
- `$X.XX` — in `#e0af68`
- `doctor ✓` — `✓` in `#9ece6a`; if unhealthy: `doctor ✗` in `#f7768e`
- `mlflow ✓` / `wandb ✓` — same pattern; hidden if sink not configured
- Implemented as a single `Static` with Rich markup, refreshed on the existing 5s timer

### DataTables
- Remove the `Block` wrapper widget that was adding a border + title
- DataTable header row: `text-style: bold; color: #565f89`
- Status column: Rich markup `[green]●[/]` / `[cyan]✓[/]` / `[red]✗[/]` / `[yellow]○[/]`
- Cost column: `color: #e0af68`

### What's removed
- `Grid#dash-kpi-grid` and 4 `KpiCard` widgets
- `Grid#dash-health-row` and 2 health card widgets
- `Block` wrappers around both DataTables

---

## Runs Screen (`screens/runs.py`)

### Layout — Before
```
TitleBar
Tabs (Textual widget with borders): All | Active | Recent
Static#runs-stats
DataTable#runs-table in Block wrapper
StatusBar
Footer
```

### Layout — After
```
TitleBar
Horizontal#runs-nav (height: 1, padding: 0 2)
  Static#tab-all   "all"
  Static          "  ·  "
  Static#tab-active "active"
  Static          "  ·  "
  Static#tab-recent "recent"
  Static#runs-stats  (right-aligned, dim)  "24 runs"
Rule (color: #2d3149)
DataTable#runs-table
```

### Tab navigation
- Replace Textual `Tabs` widget with a `Horizontal` of `Static` widgets
- Active tab: `color: #7aa2f7; text-style: bold underline`
- Inactive tabs: `color: #565f89`
- Clicking or pressing `1`/`2`/`3` (existing bindings) updates which Static has `.tab-active` class

### DataTable
- No Block wrapper, no border
- Same status-column color coding as Dashboard
- FilterBar: already borderless (just an Input), keep as-is

---

## Run Detail Screen (`screens/run_detail.py`)

### Layout — Before
```
TitleBar
Vertical#detail-header
  Horizontal#detail-title-row
    Static#run-name (bold)
    Static#run-badge (● running)
  Horizontal#detail-chips (chips with borders)
    id | vendor | started | duration | cost | $/hr
  Rule
  Horizontal#detail-actions
    Button "Stop" | Button "Pull" | ...  (Textual bordered buttons)
TabbedContent (Textual, with tab bar + borders)
  TabPane x5
StatusBar
Footer
```

### Layout — After
```
TitleBar  (subtitle = run name, status dot in TitleBar active-count area)
Horizontal#detail-meta (padding: 0 2, height: 1)
  "#abc123  vast  2h ago  1h 45m  $0.23  ~$0.12/hr"
Horizontal#detail-actions (padding: 0 2, height: 1)
  "[stop]  [pull]  [artifacts]  [rerun]  [patch]"
Rule (color: #2d3149)
Horizontal#detail-tabs-row (height: 1, padding: 0 2)
  "stages · logs · manifest · metrics · report"
Rule (color: #2d3149)
ContentSwitcher (no border)
  (active tab content)
```

### Meta line
- Leading status: `● running` in `#9ece6a` / `✓ done` in `#7dcfff` / `✗ failed` in `#f7768e`
- Labels dim (`#565f89`): `#`, `vendor`, `started`, `dur`, `cost`, `rate`
- Values normal (`#c0caf5`): `abc123`, `vast`, `2h ago`, `1h 45m`
- Cost value: `#e0af68`
- Status is shown in the meta line (not moved to TitleBar — TitleBar subtitle stays as run name only)

### Action buttons
- Replace Textual `Button` widgets with a `Horizontal` of slim `Label` widgets (no border, `can_focus=True`)
- Format: `[stop]  [pull]  [artifacts]  [rerun]  [patch]` — each label is its own widget
- Color: `color: #565f89` by default, `color: #7aa2f7` on hover/focus (`:hover` and `:focus` TCSS rules)
- Click handlers remain bound to the same action methods as before

### Tab row
- Replace `TabbedContent` + `TabPane` with a `Horizontal` of `Static` labels + `ContentSwitcher`
- Active tab: `color: #7aa2f7; text-style: bold underline`
- Existing bindings (`1`–`5`) switch the active tab via `ContentSwitcher.current`
- `ContentSwitcher` children are the existing tab content widgets (no change to their internals)

---

## Files to Modify

| File | Change |
|------|--------|
| `widgets/title_bar.py` | Remove `_MenuBtn`, update styles |
| `widgets/status_bar.py` | Not deleted (used elsewhere?), but hidden via CSS |
| `app.tcss` | Global rules: hide Footer/StatusBar, strip panel borders, add `.section-label`, `.flat-tabs` |
| `screens/dashboard.py` | Replace KPI cards + health cards + Block wrappers with `#kpi-bar` Static + bare DataTables |
| `screens/runs.py` | Replace Textual `Tabs` with flat `Horizontal` nav row |
| `screens/run_detail.py` | Replace TabbedContent with flat tabs + ContentSwitcher; replace Button row with Static |

## Files NOT modified in this iteration
- All other screens (launch, instances, vendors, settings, doctor, etc.)
- MetricsView, MetricsPalette, AsciiChart widgets
- FilterBar, FuzzyFilter widgets
- Modal screens (confirm, error_detail, etc.)
- Python logic, CLI calls, DB queries — zero behavioral changes

---

## Success Criteria

1. All 3 screens render without Textual exceptions
2. No visible border boxes around DataTables or panels on Dashboard, Runs, Run Detail
3. KPI bar shows correct counts with color markup
4. Flat tab navigation works (click + keyboard)
5. TitleBar shows `xrun  <screen>  ● N  HH:MM` without Menu button
6. StatusBar and Footer are not visible
7. Existing keybindings (g-chords, Ctrl+P, Ctrl+O, ?. n) still work
