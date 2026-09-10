from pathlib import Path

from xrun_tui.themes import render_theme


def test_settings_theme_ids_render_distinct_palettes() -> None:
    tokyo = render_theme("tokyo-night")

    catppuccin = render_theme("catppuccin-mocha")
    gruvbox = render_theme("gruvbox-dark")

    assert "#1e1e2e" in catppuccin
    assert "#282828" in gruvbox
    assert catppuccin != tokyo
    assert gruvbox != tokyo


def test_opencode_dark_renders_distinct_terminal_palette() -> None:
    tokyo = render_theme("tokyo-night")
    opencode = render_theme("opencode-dark")

    assert "#090b0f" in opencode
    assert "#6aa9ff" in opencode
    assert opencode != tokyo


def test_settings_exposes_opencode_dark_theme() -> None:
    from xrun_tui.screens.settings import _THEMES

    assert ("opencode-dark", "OpenCode Dark") in _THEMES


def test_apply_theme_writes_css_and_refreshes_app(tmp_path: Path) -> None:
    from xrun_tui.screens.settings import _apply_theme_to_app

    class DummyApp:
        theme_name = "tokyo-night"

        def __init__(self) -> None:
            self.refreshed = False

        def refresh_css(self, animate: bool = True) -> None:
            assert animate is False
            self.refreshed = True

    app = DummyApp()

    _apply_theme_to_app(app, "gruvbox-dark", tmp_path)

    assert app.theme_name == "gruvbox-dark"
    assert app.refreshed is True
    assert "#282828" in (tmp_path / "theme.tcss").read_text(encoding="utf-8")
