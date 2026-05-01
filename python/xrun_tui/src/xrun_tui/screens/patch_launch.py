from __future__ import annotations

import re
from pathlib import Path

from textual.app import ComposeResult
from textual.binding import Binding
from textual.containers import Horizontal, ScrollableContainer, Vertical
from textual.screen import ModalScreen
from textual.widgets import Button, Input, Label, Static


def _parse_run_args(yaml_text: str) -> dict[str, str]:
    """Extract run.args mapping from manifest YAML without yaml dependency."""
    lines = yaml_text.splitlines()
    in_run = in_args = False
    args: dict[str, str] = {}
    args_indent = 0
    for line in lines:
        stripped = line.lstrip()
        indent = len(line) - len(stripped)
        if not in_run:
            if re.match(r'^run\s*:', line):
                in_run = True
            continue
        if not in_args:
            if indent == 0 and stripped and not stripped.startswith('#'):
                in_run = False
                continue
            if re.match(r'^\s+args\s*:', line):
                in_args = True
                args_indent = indent
            continue
        # In args block
        if not stripped or stripped.startswith('#'):
            continue
        if indent <= args_indent:
            break  # left args block
        m = re.match(r"^\s+(['\"]?)(-{0,2}[\w.-]+)\1\s*:\s*(.*)", line)
        if m:
            key = m.group(2)
            val = m.group(3).strip().strip("'\"")
            args[key] = val
    return args


def _safe_id(key: str) -> str:
    return re.sub(r'[^a-zA-Z0-9_-]', '_', key.lstrip('-'))


class PatchLaunchScreen(ModalScreen[bool | None]):
    BINDINGS = [
        Binding("escape",      "dismiss_none", show=False),
        Binding("ctrl+s",      "action_submit", show=False),
        Binding("ctrl+enter",  "action_submit", show=False),
    ]

    DEFAULT_CSS = """
    PatchLaunchScreen { align: center middle; }
    #patch-box {
        background: #1e2030;
        border: solid #7aa2f7;
        width: 80;
        height: auto;
        max-height: 36;
        padding: 1 2;
    }
    #patch-title {
        color: #bb9af7;
        text-style: bold;
        height: 1;
        margin-bottom: 0;
    }
    #patch-subtitle {
        color: #565f89;
        height: 1;
        margin-bottom: 1;
    }
    #patch-scroll {
        height: auto;
        max-height: 24;
        margin-bottom: 1;
    }
    #patch-empty {
        color: #565f89;
        height: 3;
        content-align: center middle;
    }
    .patch-row {
        height: 3;
        align: left middle;
    }
    .patch-label {
        width: 24;
        content-align: right middle;
        padding-right: 2;
        color: #7dcfff;
    }
    .patch-input {
        width: 40;
    }
    #patch-actions {
        height: 3;
        align: left middle;
    }
    #patch-actions Button {
        margin-right: 1;
    }
    """

    def __init__(self, run: dict) -> None:
        super().__init__()
        self._run = run
        self._args: dict[str, str] = {}
        manifest = run.get("manifest_path") or ""
        if manifest:
            try:
                text = Path(manifest).read_text(encoding="utf-8")
                self._args = _parse_run_args(text)
            except Exception:
                pass

    def compose(self) -> ComposeResult:
        run = self._run
        name = run.get("name") or (run.get("id") or "")[:16]
        manifest = run.get("manifest_path") or ""
        filename = Path(manifest).name if manifest else "unknown"

        with Vertical(id="patch-box"):
            yield Static(
                f"[bold #bb9af7]Rerun with patches — {name}[/]",
                id="patch-title",
            )
            yield Static(
                f"[#565f89]manifest: {filename}  Ctrl+S to launch[/]",
                id="patch-subtitle",
            )
            with ScrollableContainer(id="patch-scroll"):
                if self._args:
                    for key, val in self._args.items():
                        safe = _safe_id(key)
                        with Horizontal(classes="patch-row"):
                            yield Label(key, classes="patch-label")
                            yield Input(
                                val,
                                id=f"patch-{safe}",
                                classes="patch-input",
                            )
                else:
                    yield Static(
                        "[#565f89]No args found in manifest. "
                        "Run will restart unchanged.[/]",
                        id="patch-empty",
                    )
            with Horizontal(id="patch-actions"):
                yield Button("Launch  [Ctrl+S]", id="btn-launch", variant="primary")
                yield Button("Cancel  [Esc]",    id="btn-cancel")

    def on_button_pressed(self, event: Button.Pressed) -> None:
        match event.button.id:
            case "btn-launch":
                self.run_worker(self.action_submit())
            case "btn-cancel":
                self.dismiss(None)

    async def action_submit(self) -> None:
        run_id = self._run.get("id") or ""

        patches: dict[str, str] = {}
        for key, original in self._args.items():
            safe = _safe_id(key)
            try:
                widget = self.query_one(f"#patch-{safe}", Input)
                current = widget.value
            except Exception:
                current = original
            if current != original:
                patches[key] = current

        from xrun_tui import services
        ok, msg = await services.rerun_with_patches(run_id, patches)
        if ok:
            self.notify("Rerun launched", severity="information")
            self.dismiss(True)
        else:
            self.notify(f"Launch failed: {msg[:80]}", severity="error", timeout=8)

    def action_dismiss_none(self) -> None:
        self.dismiss(None)
