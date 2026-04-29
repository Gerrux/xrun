"""Theme system.

We ship Tokyo Night as the canonical stylesheet (`tokyo_night.tcss`).
Alternative themes are produced by colour-mapping that base file at runtime,
written into the user's config dir, and loaded via App.CSS_PATH at startup.
"""
from __future__ import annotations

from pathlib import Path

THEMES_DIR = Path(__file__).parent

# Tokyo Night canonical palette (substring keys -> replacement value)
TOKYO_NIGHT = {
    "#1a1b26": "#1a1b26",
    "#1e2030": "#1e2030",
    "#24283b": "#24283b",
    "#2d3149": "#2d3149",
    "#414868": "#414868",
    "#565f89": "#565f89",
    "#7aa2f7": "#7aa2f7",
    "#7dcfff": "#7dcfff",
    "#9ece6a": "#9ece6a",
    "#e0af68": "#e0af68",
    "#f7768e": "#f7768e",
    "#bb9af7": "#bb9af7",
    "#c0caf5": "#c0caf5",
    "#a9b1d6": "#a9b1d6",
    "#3d59a1": "#3d59a1",
    "#4a6bb5": "#4a6bb5",
    "#2d1b2e": "#2d1b2e",
    "#e0def4": "#e0def4",
}

CATPPUCCIN_MOCHA = {
    "#1a1b26": "#1e1e2e",
    "#1e2030": "#181825",
    "#24283b": "#313244",
    "#2d3149": "#45475a",
    "#414868": "#585b70",
    "#565f89": "#9399b2",
    "#7aa2f7": "#89b4fa",
    "#7dcfff": "#94e2d5",
    "#9ece6a": "#a6e3a1",
    "#e0af68": "#f9e2af",
    "#f7768e": "#f38ba8",
    "#bb9af7": "#cba6f7",
    "#c0caf5": "#cdd6f4",
    "#a9b1d6": "#bac2de",
    "#3d59a1": "#74c7ec",
    "#4a6bb5": "#89dceb",
    "#2d1b2e": "#3a2333",
    "#e0def4": "#cdd6f4",
}

GRUVBOX_DARK = {
    "#1a1b26": "#282828",
    "#1e2030": "#1d2021",
    "#24283b": "#3c3836",
    "#2d3149": "#504945",
    "#414868": "#665c54",
    "#565f89": "#928374",
    "#7aa2f7": "#83a598",
    "#7dcfff": "#8ec07c",
    "#9ece6a": "#b8bb26",
    "#e0af68": "#fabd2f",
    "#f7768e": "#fb4934",
    "#bb9af7": "#d3869b",
    "#c0caf5": "#ebdbb2",
    "#a9b1d6": "#d5c4a1",
    "#3d59a1": "#458588",
    "#4a6bb5": "#689d6a",
    "#2d1b2e": "#3a2526",
    "#e0def4": "#fbf1c7",
}

PALETTES: dict[str, dict[str, str]] = {
    "tokyo-night": TOKYO_NIGHT,
    "catppuccin":  CATPPUCCIN_MOCHA,
    "gruvbox":     GRUVBOX_DARK,
}


def _base_css() -> str:
    return (THEMES_DIR / "tokyo_night.tcss").read_text(encoding="utf-8")


def render_theme(name: str) -> str:
    """Return CSS text with Tokyo Night colours remapped to `name`."""
    palette = PALETTES.get(name, TOKYO_NIGHT)
    css = _base_css()
    if palette is TOKYO_NIGHT:
        return css
    # Substitute longest first to avoid partial overlaps (none in our palette
    # but the discipline is cheap).
    for src in sorted(TOKYO_NIGHT.keys(), key=len, reverse=True):
        dst = palette.get(src, src)
        if src != dst:
            css = css.replace(src, dst)
    return css


def write_theme_for_app(name: str, target_dir: Path) -> Path:
    """Render the chosen theme into `target_dir / theme.tcss` and return path."""
    target_dir.mkdir(parents=True, exist_ok=True)
    out = target_dir / "theme.tcss"
    out.write_text(render_theme(name), encoding="utf-8")
    return out
