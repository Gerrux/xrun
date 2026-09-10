# opencode-dark Theme Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a selectable compact dark `opencode-dark` theme to the Python Textual TUI.

**Architecture:** Reuse the existing palette-mapped theme renderer. Add one palette and one Settings option, then cover the behavior with focused Python tests.

**Tech Stack:** Python 3.11+, Textual, pytest, existing `xrun_tui.themes` palette renderer.

---

## File Structure

- Modify `python/xrun_tui/src/xrun_tui/themes/__init__.py`: add the `OPENCODE_DARK` palette and register it in `PALETTES`.
- Modify `python/xrun_tui/src/xrun_tui/screens/settings.py`: add `OpenCode Dark` to `_THEMES`.
- Modify `python/xrun_tui/tests/test_themes.py`: add tests for rendering and Settings exposure.

---

### Task 1: Test opencode-dark Theme Availability

**Files:**
- Modify: `python/xrun_tui/tests/test_themes.py`

- [ ] **Step 1: Write failing tests**

Add these tests to `python/xrun_tui/tests/test_themes.py`:

```python
def test_opencode_dark_renders_distinct_terminal_palette() -> None:
    tokyo = render_theme("tokyo-night")
    opencode = render_theme("opencode-dark")

    assert "#090b0f" in opencode
    assert "#6aa9ff" in opencode
    assert opencode != tokyo


def test_settings_exposes_opencode_dark_theme() -> None:
    from xrun_tui.screens.settings import _THEMES

    assert ("opencode-dark", "OpenCode Dark") in _THEMES
```

- [ ] **Step 2: Run tests to verify failure**

Run from `python/xrun_tui`:

```powershell
rtk python -m pytest tests/test_themes.py -q
```

Expected: the new tests fail because `opencode-dark` is not registered and not exposed in Settings.

---

### Task 2: Implement opencode-dark Palette

**Files:**
- Modify: `python/xrun_tui/src/xrun_tui/themes/__init__.py`
- Modify: `python/xrun_tui/src/xrun_tui/screens/settings.py`

- [ ] **Step 1: Add palette**

Add this constant after `GRUVBOX_DARK` in `python/xrun_tui/src/xrun_tui/themes/__init__.py`:

```python
OPENCODE_DARK = {
    "#1a1b26": "#090b0f",
    "#1e2030": "#0d1117",
    "#24283b": "#11161d",
    "#2d3149": "#161d27",
    "#414868": "#222a35",
    "#565f89": "#7d8997",
    "#7aa2f7": "#6aa9ff",
    "#7dcfff": "#78dce8",
    "#9ece6a": "#7bd88f",
    "#e0af68": "#f0b86a",
    "#f7768e": "#ff6b7a",
    "#bb9af7": "#b392f0",
    "#c0caf5": "#d7dde6",
    "#a9b1d6": "#a8b3c2",
    "#3d59a1": "#254f7a",
    "#4a6bb5": "#316391",
    "#2d1b2e": "#21131a",
    "#e0def4": "#f2f5f8",
}
```

- [ ] **Step 2: Register palette**

Update `PALETTES` in `python/xrun_tui/src/xrun_tui/themes/__init__.py` to include:

```python
PALETTES: dict[str, dict[str, str]] = {
    "tokyo-night":      TOKYO_NIGHT,
    "catppuccin-mocha": CATPPUCCIN_MOCHA,
    "gruvbox-dark":     GRUVBOX_DARK,
    "opencode-dark":    OPENCODE_DARK,
}
```

- [ ] **Step 3: Expose theme in Settings**

Update `_THEMES` in `python/xrun_tui/src/xrun_tui/screens/settings.py` to include:

```python
_THEMES = [
    ("tokyo-night",      "Tokyo Night (default)"),
    ("catppuccin-mocha", "Catppuccin Mocha"),
    ("gruvbox-dark",     "Gruvbox Dark"),
    ("opencode-dark",    "OpenCode Dark"),
]
```

- [ ] **Step 4: Run focused tests**

Run from `python/xrun_tui`:

```powershell
rtk python -m pytest tests/test_themes.py -q
```

Expected: all tests in `tests/test_themes.py` pass.

---

### Task 3: Verify Package Tests

**Files:**
- No production edits.

- [ ] **Step 1: Run Python TUI tests**

Run from `python/xrun_tui`:

```powershell
rtk python -m pytest -q
```

Expected: tests pass. Existing collection warnings may remain if unrelated to this change.

- [ ] **Step 2: Inspect diff**

Run from repo root:

```powershell
rtk git diff -- python/xrun_tui/src/xrun_tui/themes/__init__.py python/xrun_tui/src/xrun_tui/screens/settings.py python/xrun_tui/tests/test_themes.py
```

Expected: diff only adds the `opencode-dark` palette, Settings option, and tests.

---

## Self-Review

- Spec coverage: plan adds `opencode-dark`, preserves palette renderer, exposes Settings option, and verifies render output plus Settings exposure.
- Placeholder scan: no TBD/TODO placeholders.
- Type consistency: theme IDs use `opencode-dark` everywhere; label uses `OpenCode Dark` everywhere.
