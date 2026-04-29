from __future__ import annotations

from textual.app import ComposeResult
from textual.binding import Binding
from textual.screen import ModalScreen
from textual.widgets import DataTable, Footer, Static

from xrun_tui.utils import cost, duration, rel_time, status_label


class CompareScreen(ModalScreen[None]):
    """Side-by-side comparison of two runs."""

    DEFAULT_CSS = """
    CompareScreen {
        align: center middle;
    }
    CompareScreen > Vertical {
        width: 90;
        height: auto;
        max-height: 80vh;
        background: #1e2030;
        border: round #414868;
        padding: 1 2;
    }
    CompareScreen .cmp-title {
        text-align: center;
        color: #7aa2f7;
        padding-bottom: 1;
    }
    """

    BINDINGS = [
        Binding("escape,q", "dismiss", "Close"),
    ]

    def __init__(self, run_a: dict, run_b: dict) -> None:
        super().__init__()
        self._a = run_a
        self._b = run_b

    def compose(self) -> ComposeResult:
        from textual.containers import Vertical
        with Vertical():
            na = self._a.get("name") or self._a["id"][:12]
            nb = self._b.get("name") or self._b["id"][:12]
            yield Static(
                f"[bold #7aa2f7]Compare[/]  [#565f89]{na}[/]  [#414868]vs[/]  [#565f89]{nb}[/]",
                classes="cmp-title",
            )
            yield DataTable(id="cmp-table", cursor_type="none", zebra_stripes=True)
            yield Static(
                "[#414868]escape / q — close[/]",
                classes="form-hint",
            )
        yield Footer()

    def on_mount(self) -> None:
        from rich.text import Text
        a, b = self._a, self._b
        t = self.query_one("#cmp-table", DataTable)
        na = (a.get("name") or a["id"][:10])[:22]
        nb = (b.get("name") or b["id"][:10])[:22]
        t.add_columns(
            Text("Field",    style="#565f89"),
            Text(na,         style="#7aa2f7"),
            Text(nb,         style="#bb9af7"),
            Text("Delta",    style="#565f89"),
        )
        self._populate(t, a, b)

    def _populate(self, t: DataTable, a: dict, b: dict) -> None:
        from rich.text import Text

        def row(label: str, va: str, vb: str, delta: str = "",
                style_a: str = "#c0caf5", style_b: str = "#c0caf5",
                delta_style: str = "#565f89") -> None:
            t.add_row(
                Text(label,  style="#565f89"),
                Text(va,     style=style_a),
                Text(vb,     style=style_b),
                Text(delta,  style=delta_style),
            )

        row("ID",     a["id"][:14],            b["id"][:14])
        row("Vendor", a.get("vendor") or "?",   b.get("vendor") or "?",
            style_a="#7dcfff", style_b="#7dcfff")

        sa, sb = a.get("status","?"), b.get("status","?")
        row("Status", sa, sb,
            style_a=_status_color(sa), style_b=_status_color(sb))

        row("Started",  rel_time(a.get("started_at") or a.get("created_at")),
                        rel_time(b.get("started_at") or b.get("created_at")))
        row("Duration", duration(a), duration(b))

        ca_str, cb_str = cost(a), cost(b)
        ca = a.get("cost_usd") or a.get("cost_usd_estimate") or 0.0
        cb = b.get("cost_usd") or b.get("cost_usd_estimate") or 0.0
        if ca > 0 or cb > 0:
            diff = cb - ca
            sign = "+" if diff >= 0 else ""
            delta_style = "#9ece6a" if diff < 0 else ("#f7768e" if diff > 0 else "#565f89")
            row("Cost", ca_str, cb_str,
                f"{sign}${diff:.2f}", "#e0af68", "#e0af68", delta_style)
        else:
            row("Cost", ca_str, cb_str)

        row("Manifest",
            _basename(a.get("manifest_path") or ""),
            _basename(b.get("manifest_path") or ""))

    def action_dismiss(self) -> None:
        self.dismiss()


def _status_color(status: str) -> str:
    return {
        "running":  "bold #9ece6a",
        "done":     "#565f89",
        "failed":   "bold #f7768e",
        "cancelled":"#bb9af7",
    }.get(status, "#c0caf5")


def _basename(path: str) -> str:
    if not path:
        return "—"
    from pathlib import Path
    return Path(path).name or path[:20]
