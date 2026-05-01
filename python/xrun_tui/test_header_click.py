"""Raw file-log debug — bypass all Textual notification."""
import pathlib, time
LOG = pathlib.Path(__file__).parent / "click_debug.log"

from textual.app import App, ComposeResult
from textual.events import Click, MouseDown
from textual.widgets import Footer, Static, Header
from textual.widgets._header import HeaderIcon


def _log(msg: str) -> None:
    with open(LOG, "a") as f:
        f.write(f"{time.time():.3f} {msg}\n")


class _PaletteIcon(HeaderIcon):
    def __init__(self, *a, **kw):
        super().__init__(*a, **kw)
        _log(f"_PaletteIcon.__init__ id={id(self)}")

    async def on_click(self, event: Click) -> None:
        _log("_PaletteIcon.on_click FIRED")
        event.stop()
        event.prevent_default()

    async def on_mouse_down(self, event: MouseDown) -> None:
        _log("_PaletteIcon.on_mouse_down FIRED")


class TestApp(App):
    ENABLE_COMMAND_PALETTE = False

    def compose(self) -> ComposeResult:
        yield Header(show_clock=True)
        yield Static("Click the ● icon top-left. Check click_debug.log")
        yield Footer()

    def on_mount(self) -> None:
        _log("on_mount start")
        try:
            icon = self.query_one(HeaderIcon)
            _log(f"found icon class={type(icon).__name__} id={id(icon)}")
            icon.__class__ = _PaletteIcon
            _log(f"patched to class={type(icon).__name__}")
        except Exception as e:
            _log(f"patch error: {e}")

    async def on_click(self, event: Click) -> None:
        _log(f"App.on_click widget={type(event.widget).__name__ if event.widget else 'None'}")

    async def on_mouse_down(self, event: MouseDown) -> None:
        _log(f"App.on_mouse_down widget={type(event.widget).__name__ if event.widget else 'None'}")


if __name__ == "__main__":
    LOG.write_text("")
    _log("app start")
    TestApp().run()
