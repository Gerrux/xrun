from __future__ import annotations

from typing import Any

from rich.markup import escape
from rich.text import Text
from textual.app import ComposeResult
from textual.binding import Binding
from textual.containers import Vertical
from xrun_tui.live import LiveScreen
from textual.widgets import DataTable, Footer, Static
from xrun_tui.widgets.status_bar import StatusBar
from xrun_tui.widgets.title_bar import TitleBar


def _cell(text: str, style: str) -> Text:
    """Single-line cell that ellipsizes instead of widening the column."""
    return Text(text.replace("\n", " "), style=style,
                no_wrap=True, overflow="ellipsis")


def fit_doctor_widths(total: int, names: list[str]) -> dict[str, int]:
    """Column widths (content, without cell padding) filling `total` cells."""
    pad = 2  # DataTable cell padding is 1 on each side
    dot, status = 1, 6
    check = min(max([len(n) for n in names] + [5]), 24)
    detail = total - (dot + check + status) - 4 * pad
    if detail < 10:  # very narrow terminal: squeeze Check instead
        check = max(5, check - (10 - detail))
        detail = max(4, total - (dot + check + status) - 4 * pad)
    return {"dot": dot, "check": check, "status": status, "detail": detail}


class DoctorScreen(LiveScreen):
    """System health diagnostics — wraps `xrun doctor --json`."""

    TITLE = "xrun — doctor"
    BINDINGS = [
        Binding("escape,q",  "go_back", "Back"),
        Binding("ctrl+r,f5", "refresh", "Refresh"),
    ]

    def __init__(self) -> None:
        super().__init__()
        # Parallel to the table rows: (name, status, full detail) per check
        self._rows: list[tuple[str, str, str]] = []
        self._paths_line = ""

    def compose(self) -> ComposeResult:
        yield TitleBar("doctor")
        yield Static("System health", classes="screen-title")
        yield Static("", id="doctor-summary", classes="stats-bar")
        with Vertical(id="doctor-body"):
            yield DataTable(id="doctor-table",
                            cursor_type="row", zebra_stripes=True)
            yield Static("", id="doctor-footer", classes="doctor-footer")
        yield StatusBar()
        yield Footer()

    def on_mount(self) -> None:
        t = self.query_one("#doctor-table", DataTable)
        t.add_column(Text(" ",      style="#565f89"), key="dot", width=1)
        t.add_column(Text("Check",  style="#565f89"), key="check", width=12)
        t.add_column(Text("Status", style="#565f89"), key="status", width=6)
        t.add_column(Text("Detail", style="#565f89"), key="detail", width=20)
        self.kick(self._refresh)

    # ── Layout ──────────────────────────────────────────────────────────────

    def _fit_columns(self) -> None:
        """Detail takes the width the other columns leave; no h-scroll."""
        t = self.query_one("#doctor-table", DataTable)
        width = t.scrollable_content_region.width
        if width <= 0:
            return
        widths = fit_doctor_widths(width, [r[0] for r in self._rows])
        for key, w in widths.items():
            t.columns[key].width = w  # type: ignore[index]
            t.columns[key].auto_width = False  # type: ignore[index]
        t._require_update_dimensions = True
        t.refresh(layout=True)

    def on_resize(self, event) -> None:
        self._fit_columns()

    def on_data_table_row_highlighted(
        self, event: DataTable.RowHighlighted
    ) -> None:
        self._update_footer(event.cursor_row)

    def _update_footer(self, row: int | None = None) -> None:
        footer = self.query_one("#doctor-footer", Static)
        if row is None:
            row = self.query_one("#doctor-table", DataTable).cursor_row
        parts: list[Text] = []
        if 0 <= row < len(self._rows):
            name, status, detail = self._rows[row]
            head = Text(name, style="bold #c0caf5")
            head.append(f"  {status}", style="#565f89")
            parts.append(head)
            if detail:
                parts.append(Text(detail, style="#c0caf5"))
        if self._paths_line:
            parts.append(Text.from_markup(self._paths_line))
        footer.update(Text("\n").join(parts) if parts else "")

    async def _refresh(self) -> None:
        from xrun_tui import services
        self.query_one("#doctor-summary", Static).update(
            "[#e0af68]Running diagnostics…[/]"
        )
        ok, data, err = await services.doctor()
        table = self.query_one("#doctor-table", DataTable)
        table.clear()
        self._rows = []
        self._paths_line = ""

        if not ok:
            self.query_one("#doctor-summary", Static).update(
                f"[bold #f7768e]✗ doctor failed:[/] [#c0caf5]{err or 'unknown'}[/]"
            )
            table.add_row(
                Text("✗", style="#f7768e"),
                Text("doctor invocation"),
                Text("failed", style="bold #f7768e"),
                _cell(err[:120] if err else "", "#f7768e"),
            )
            self._rows = [("doctor invocation", "failed", err or "")]
            self._fit_columns()
            self._update_footer(0)
            return

        checks = self._extract_checks(data)
        meta = data if isinstance(data, dict) else {}
        self._render_summary(meta, checks)
        for c in checks:
            status = c["status"]
            if status == "ok":
                dot, dot_style, st_style = "✓", "bold #9ece6a", "#9ece6a"
            elif status == "warn":
                dot, dot_style, st_style = "!", "bold #e0af68", "#e0af68"
            else:
                dot, dot_style, st_style = "✗", "bold #f7768e", "bold #f7768e"
            table.add_row(
                Text(dot, style=dot_style),
                Text(str(c.get("name", "?")), style="#c0caf5"),
                Text(status, style=st_style),
                _cell(str(c.get("detail", "")), "#565f89"),
            )
            self._rows.append(
                (str(c.get("name", "?")), status, str(c.get("detail", "")))
            )

        # Footer hint with key paths from JSON
        footer_bits: list[str] = []
        for k in ("db_path", "config_dir", "data_dir"):
            v = meta.get(k)
            if v:
                footer_bits.append(
                    f"[#565f89]{k}:[/] [#7dcfff]{escape(str(v))}[/]"
                )
        self._paths_line = "   ".join(footer_bits)
        self._fit_columns()
        self.call_after_refresh(self._fit_columns)
        self._update_footer()

    def _extract_checks(self, data: Any) -> list[dict[str, Any]]:
        # `xrun doctor --json` may return either a bare list of check dicts,
        # or a dict with a "checks" list plus metadata. Normalize both.
        raw: list[Any]
        if isinstance(data, list):
            raw = data
        elif isinstance(data, dict) and isinstance(data.get("checks"), list):
            raw = data["checks"]
        elif isinstance(data, dict):
            # Last-resort fallback: flatten boolean-ish keys.
            return [
                {"name": k, "status": "ok" if v else "fail", "detail": ""}
                for k, v in data.items() if isinstance(v, bool)
            ]
        else:
            return []

        norm: list[dict[str, Any]] = []
        for c in raw:
            if not isinstance(c, dict):
                continue
            name = c.get("name") or c.get("check") or "?"
            raw_status = c.get("status")
            if isinstance(raw_status, str):
                status = raw_status.lower()
            else:
                status = "ok" if c.get("ok") else "fail"
            if status not in ("ok", "warn", "fail"):
                status = "fail"
            norm.append({
                "name":   name,
                "status": status,
                "detail": c.get("detail", ""),
            })
        return norm

    def _render_summary(self, data: dict[str, Any],
                        checks: list[dict[str, Any]]) -> None:
        passed = sum(1 for c in checks if c["status"] == "ok")
        warns  = sum(1 for c in checks if c["status"] == "warn")
        fails  = sum(1 for c in checks if c["status"] == "fail")
        parts = [
            f"[bold #9ece6a]✓ {passed} pass[/]",
            f"[#e0af68]! {warns} warn[/]" if warns
                else "[#565f89]! 0 warn[/]",
            f"[bold #f7768e]✗ {fails} fail[/]" if fails
                else "[#565f89]✗ 0 fail[/]",
        ]
        version = data.get("version") or data.get("xrun_version") or ""
        if version:
            parts.append(f"[#414868]┊[/]  [#565f89]xrun v{version}[/]")
        self.query_one("#doctor-summary", Static).update("  ".join(parts))

    def action_go_back(self) -> None:
        self.app.pop_screen()

    def action_refresh(self) -> None:
        self.kick(self._refresh)
