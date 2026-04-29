from __future__ import annotations

from typing import Any

from rich.text import Text
from textual.app import ComposeResult
from textual.binding import Binding
from textual.containers import Vertical
from textual.screen import Screen
from textual.widgets import DataTable, Footer, Header, Static
from xrun_tui.widgets.status_bar import StatusBar


class DoctorScreen(Screen):
    """System health diagnostics — wraps `xrun doctor --json`."""

    TITLE = "xrun — doctor"
    BINDINGS = [
        Binding("escape,q",  "go_back", "Back"),
        Binding("ctrl+r,f5", "refresh", "Refresh"),
    ]

    def compose(self) -> ComposeResult:
        yield Header(show_clock=True)
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
        t.add_columns(
            Text(" ",      style="#565f89"),
            Text("Check",  style="#565f89"),
            Text("Status", style="#565f89"),
            Text("Detail", style="#565f89"),
        )
        self.call_after_refresh(self._refresh)

    async def _refresh(self) -> None:
        from xrun_tui import services
        self.query_one("#doctor-summary", Static).update(
            "[#e0af68]Running diagnostics…[/]"
        )
        ok, data, err = await services.doctor()
        table = self.query_one("#doctor-table", DataTable)
        table.clear()

        if not ok:
            self.query_one("#doctor-summary", Static).update(
                f"[bold #f7768e]✗ doctor failed:[/] [#c0caf5]{err or 'unknown'}[/]"
            )
            table.add_row(
                Text("✗", style="#f7768e"),
                Text("doctor invocation"),
                Text("failed", style="bold #f7768e"),
                Text(err[:120] if err else "", style="#f7768e"),
            )
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
                Text(str(c.get("detail", ""))[:200], style="#565f89"),
            )

        # Footer hint with key paths from JSON
        footer_bits: list[str] = []
        for k in ("db_path", "config_dir", "data_dir"):
            v = meta.get(k)
            if v:
                footer_bits.append(f"[#565f89]{k}:[/] [#7dcfff]{v}[/]")
        self.query_one("#doctor-footer", Static).update(
            "   ".join(footer_bits) or ""
        )

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
                else "[#414868]! 0 warn[/]",
            f"[bold #f7768e]✗ {fails} fail[/]" if fails
                else "[#414868]✗ 0 fail[/]",
        ]
        version = data.get("version") or data.get("xrun_version") or ""
        if version:
            parts.append(f"[#414868]┊[/]  [#565f89]xrun v{version}[/]")
        self.query_one("#doctor-summary", Static).update("  ".join(parts))

    def action_go_back(self) -> None:
        self.app.pop_screen()

    async def action_refresh(self) -> None:
        await self._refresh()
