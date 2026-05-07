from __future__ import annotations

import asyncio
import time
from typing import TYPE_CHECKING

from rich.text import Text
from textual.app import ComposeResult
from textual.binding import Binding
from textual.containers import Grid, Horizontal, Vertical
from textual.screen import Screen
from textual.widgets import DataTable, Footer, Static
from xrun_tui.widgets.status_bar import StatusBar
from xrun_tui.widgets.title_bar import TitleBar

from xrun_tui.utils import (
    cost,
    fmt_metric_value,
    is_stale,
    pick_metric_key,
    rel_time,
    render_sparkline,
    status_dot_for,
    status_label_for,
)

if TYPE_CHECKING:
    from xrun_tui.app import XrunApp


_ACTIVE_STATES = ("provisioning", "uploading", "running")

# Slow-tick TTL for Doctor + Sinks probes. They're network-bound, so we don't
# spam them on every 5s tick — just refresh them every minute and on Ctrl+R.
_HEALTH_TTL_SEC = 60.0


def _kpi(label: str, value: str, value_style: str, sub: str | None = None) -> str:
    out = (
        f"[#565f89]{label}[/]\n"
        f"[{value_style}]{value}[/]"
    )
    if sub:
        # Append on the value line so card stays 2-line tall (CSS height: 4).
        out = (
            f"[#565f89]{label}[/]\n"
            f"[{value_style}]{value}[/]  {sub}"
        )
    return out


def _health(label: str, value: str, value_style: str) -> str:
    """Health-strip card. Same shape as KPI but distinct id so CSS can tune
    typography later (smaller value text, brand accents) without touching
    KPIs."""
    return (
        f"[#565f89]{label}[/]\n"
        f"[{value_style}]{value}[/]"
    )


class DashboardScreen(Screen):
    """Home screen — at-a-glance overview with quick navigation.

    Layout:
      ┌─ KPIs (4 cards) ───────────────────────────────┐
      │ Active │ Done │ Failed │ Spent ($X +$Y/hr)     │
      └────────────────────────────────────────────────┘
      ┌─ Health (2 cards) ─────────────────────────────┐
      │ Doctor: ✓ all green │ Sinks: ✓ MLflow ⊘ WandB   │
      └────────────────────────────────────────────────┘
      ┌─ Active runs (sparkline col) ──────────────────┐
      └────────────────────────────────────────────────┘
      ┌─ Recently completed ───────────────────────────┐
      └────────────────────────────────────────────────┘

    Refresh cadence:
      - 5s   → runs/KPIs/sparklines/burn (cheap, all-DB)
      - 60s  → Doctor + Sinks probes (network-bound, separate worker)
    """

    TITLE = "xrun"
    SUB_TITLE = "dashboard"

    BINDINGS = [
        Binding("enter",     "open_runs",       "Runs"),
        Binding("l",         "goto_launch",     "Launch"),
        Binding("i",         "goto_instances",  "Instances"),
        Binding("v",         "goto_vendors",    "Vendors"),
        Binding("s",         "goto_sinks",      "Sinks"),
        Binding("h",         "goto_doctor",     "Doctor"),
        Binding("comma",     "goto_settings",   "Settings"),
        Binding("ctrl+r,f5", "refresh",         "Refresh"),
        Binding("q",         "quit_app",        "Quit"),
    ]

    def __init__(self) -> None:
        super().__init__()
        # Health-card cache. Doctor/Sinks probes are network-bound; we render
        # last known state instantly and refresh on slow tick.
        self._doctor_text:  tuple[str, str] = ("checking…", "#e0af68")
        self._sinks_text:   tuple[str, str] = ("checking…", "#e0af68")
        self._health_last:  float = 0.0

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

    def on_mount(self) -> None:
        self._setup_active_table()
        self._setup_recent_table()
        # Both ticks fire on workers so the screen message pump stays free:
        # `_refresh` does ~100ms of widget work (clear + 16 add_row + 4 KPI
        # updates with rich markup), and `_refresh_health` shells out to
        # `xrun doctor --json` (~1.3s on Windows). Awaiting either one on the
        # screen's task slot makes the dashboard feel unresponsive right after
        # first paint. `exclusive=True` collapses overlapping ticks (e.g. user
        # mashing Ctrl+R) into one in-flight refresh.
        self.set_interval(5, self._kick_refresh)
        self.set_interval(_HEALTH_TTL_SEC, self._kick_health)
        self._kick_refresh()
        self._kick_health()

    def _kick_refresh(self) -> None:
        self.run_worker(self._refresh(), exclusive=True, group="refresh")

    def _kick_health(self) -> None:
        self.run_worker(self._refresh_health(), exclusive=True, group="health")

    # ── Table setup ──────────────────────────────────────────────────────────

    def _setup_active_table(self) -> None:
        t = self.query_one("#dash-active", DataTable)
        t.add_columns(
            Text(" ",      style="#565f89"),
            Text("ID",     style="#565f89"),
            Text("Name",   style="#565f89"),
            Text("Vendor", style="#565f89"),
            Text("Status", style="#565f89"),
            Text("When",   style="#565f89"),
            Text("Cost",   style="#565f89"),
            Text("Metric", style="#565f89"),
        )

    def _setup_recent_table(self) -> None:
        t = self.query_one("#dash-recent", DataTable)
        t.add_columns(
            Text(" ",      style="#565f89"),
            Text("ID",     style="#565f89"),
            Text("Name",   style="#565f89"),
            Text("Vendor", style="#565f89"),
            Text("Status", style="#565f89"),
            Text("When",   style="#565f89"),
            Text("Cost",   style="#565f89"),
        )

    # ── Refresh: fast tick (runs/KPIs) ───────────────────────────────────────

    async def _refresh(self) -> None:
        app: XrunApp = self.app  # type: ignore[assignment]
        try:
            all_runs = await app.db.runs(status=None, limit=200)
        except Exception as exc:
            self.notify(f"DB error: {exc}", severity="error", timeout=8)
            return

        active = [r for r in all_runs if r["status"] in _ACTIVE_STATES][:8]
        recent = [r for r in all_runs if r["status"] not in _ACTIVE_STATES][:8]

        # Sparklines: fetch series only for active runs (capped at 8).
        sparks = await self._collect_sparklines([r["id"] for r in active])

        # Burn rate: sum dph across active local-DB instances.
        burn_dph = await self._compute_burn_rate()

        # Yield between table fills so a key press during refresh isn't held
        # up behind 16 add_row calls + 4 KPI updates with rich markup. The
        # refresh runs on a worker (`_kick_refresh`), but its sync widget
        # work still hogs the asyncio loop without these yield points.
        self._fill_active("#dash-active", active, sparks,
                          "[#414868]No active runs[/]")
        await asyncio.sleep(0)
        self._fill_recent("#dash-recent", recent,
                          "[#414868]No completed runs yet[/]")
        await asyncio.sleep(0)
        self._update_kpis(all_runs, burn_dph)

    async def _collect_sparklines(
        self, run_ids: list[str]
    ) -> dict[str, tuple[str, float]]:
        """For each active run, return (sparkline_str, last_value).

        Two queries per run: one to enumerate keys, one to fetch the chosen
        series. With ≤8 active runs, this is bounded ≤16 SQLite queries per
        5s tick — well within budget. Runs with no metrics are simply absent
        from the result map.
        """
        if not run_ids:
            return {}
        app: XrunApp = self.app  # type: ignore[assignment]
        out: dict[str, tuple[str, float]] = {}
        for run_id in run_ids:
            try:
                key_rows = await app.db.metric_keys(run_id)
            except Exception:
                continue
            if not key_rows:
                continue
            keys = [r["key"] for r in key_rows]
            picked = pick_metric_key(keys)
            if not picked:
                continue
            try:
                points = await app.db.metrics_for_key(run_id, picked)
            except Exception:
                continue
            if not points:
                continue
            values = [float(p["value"]) for p in points]
            spark = render_sparkline(values, width=10)
            label = f"{picked[:8]} {spark} {fmt_metric_value(values[-1])}"
            out[run_id] = (label, values[-1])
        return out

    async def _compute_burn_rate(self) -> float:
        """Σ price_per_hour across non-destroyed local-DB instances.

        We use the local DB rather than the vast.ai REST API because
        (a) it covers all vendors uniformly, (b) it works offline, and
        (c) it's what the poller already records — so the number matches
        what the budget guard sees.
        """
        app: XrunApp = self.app  # type: ignore[assignment]
        try:
            instances = await app.db.instances()
        except Exception:
            return 0.0
        return sum(
            float(i.get("price_per_hour") or 0)
            for i in instances
            if not i.get("destroyed_at")
        )

    # ── Refresh: slow tick (health) ──────────────────────────────────────────

    async def _refresh_health(self) -> None:
        """Probe Doctor + Sinks in parallel, then push results into widgets.

        Runs concurrently so a slow MLflow probe doesn't stall the Doctor
        update. Both render "checking…" until the first probe completes.
        """
        await asyncio.gather(
            self._refresh_doctor(),
            self._refresh_sinks(),
        )
        self._health_last = time.time()

    async def _refresh_doctor(self) -> None:
        from xrun_tui import services

        ok, data, _err = await services.doctor()
        if not ok:
            self._set_doctor("✗ unavailable", "bold #f7768e")
            return

        # `xrun doctor --json` may return either a bare list of check dicts
        # (current shape: [{check, category, status, detail}, …]) OR a dict
        # with a "checks" list. Normalise both — the dashboard screen and the
        # full Doctor screen used to disagree on this; align with the latter.
        if isinstance(data, list):
            raw = data
        elif isinstance(data, dict):
            raw = data.get("checks") if isinstance(data.get("checks"), list) else []
        else:
            raw = []

        warns = 0
        fails = 0
        for c in raw:
            if not isinstance(c, dict):
                continue
            raw_status = c.get("status")
            status = (raw_status.lower()
                      if isinstance(raw_status, str) else "")
            if not status:
                status = "ok" if c.get("ok") else "fail"
            if status == "warn":
                warns += 1
            elif status not in ("ok", "pass"):
                fails += 1

        if fails:
            self._set_doctor(f"✗ {fails} fail · {warns} warn",
                             "bold #f7768e")
        elif warns:
            self._set_doctor(f"! {warns} warn", "bold #e0af68")
        else:
            self._set_doctor("✓ all green", "bold #9ece6a")

    def _set_doctor(self, text: str, style: str) -> None:
        self._doctor_text = (text, style)
        try:
            self.query_one("#health-doctor", Static).update(
                _health("Doctor", text, style)
            )
        except Exception:
            pass  # widget not mounted yet

    async def _refresh_sinks(self) -> None:
        """Render compact per-sink status from config + a probe.

        We do NOT probe sinks that aren't *both* configured (creds present)
        and active (listed in `metrics.sinks`). That keeps the dashboard
        out of authentication failures for sinks the user hasn't asked us
        to use anyway.
        """
        from xrun_tui import config
        from xrun_tui.screens.sinks import _SINKS, _sink_configured

        creds = config.read_credentials()
        glob = config.read_global_config()
        sinks_list = glob.get("metrics", {}).get("sinks", [])
        if not isinstance(sinks_list, list):
            sinks_list = []

        # Render an instant "static" view first so the card never sits empty.
        parts: list[str] = []
        probe_targets: list[tuple[str, str]] = []  # (sid, label)
        for sid, name, _desc, enabled in _SINKS:
            if not enabled:
                continue
            if not _sink_configured(creds, sid):
                parts.append(f"[#414868]⊘ {name}[/]")
                continue
            if sid not in sinks_list:
                parts.append(f"[#7aa2f7]◌ {name}[/]")  # configured but paused
                continue
            # Configured & active — probe will overwrite this entry.
            parts.append(f"[#e0af68]⋯ {name}[/]")
            probe_targets.append((sid, name))

        if not parts:
            self._set_sinks("none configured", "#414868")
            return

        # Show static state immediately, then run probes in parallel.
        self._set_sinks("  ".join(parts), "#c0caf5")

        if not probe_targets:
            return

        results = await asyncio.gather(
            *(self._probe_one_sink(sid) for sid, _ in probe_targets),
            return_exceptions=True,
        )

        # Rebuild parts with probe verdicts replacing the ⋯ entries.
        rebuilt: list[str] = []
        probe_iter = iter(zip(probe_targets, results))
        for sid, name, _desc, enabled in _SINKS:
            if not enabled:
                continue
            if not _sink_configured(creds, sid):
                rebuilt.append(f"[#414868]⊘ {name}[/]")
                continue
            if sid not in sinks_list:
                rebuilt.append(f"[#7aa2f7]◌ {name}[/]")
                continue
            try:
                _, res = next(probe_iter)
            except StopIteration:
                res = None
            ok = isinstance(res, tuple) and bool(res[0])
            if ok:
                rebuilt.append(f"[#9ece6a]✓ {name}[/]")
            else:
                rebuilt.append(f"[#f7768e]✗ {name}[/]")
        self._set_sinks("  ".join(rebuilt), "#c0caf5")

    async def _probe_one_sink(self, sid: str) -> tuple[bool, str]:
        """Run `xrun config probe --vendor <sid>` with stored creds piped via
        env so secrets stay out of argv.

        Mirrors `SinksScreen._probe` but lives here too — duplicating the
        ~20 lines beats a circular-import refactor for a non-core feature.
        """
        from xrun_tui import config

        creds = config.read_credentials()
        env: dict[str, str] = {}
        extra: list[str] = []
        if sid == "mlflow":
            m = creds.get("mlflow", {})
            if m.get("token"):
                env["XRUN_PROBE_MLFLOW_TOKEN"] = m["token"]
            if m.get("username") and m.get("password"):
                env["XRUN_PROBE_MLFLOW_USERNAME"] = m["username"]
                env["XRUN_PROBE_MLFLOW_PASSWORD"] = m["password"]
            url = config.read_global_config().get("mlflow", {}).get("url", "")
            if url:
                extra = ["--mlflow-url", url]
        elif sid == "wandb":
            key = creds.get("wandb", {}).get("api_key", "")
            if key:
                env["XRUN_PROBE_WANDB_KEY"] = key
        else:
            return False, "unsupported sink"

        from xrun_tui import services
        try:
            obj = await services.probe(sid, env=env, extra_args=extra,
                                       timeout=12)
            return bool(obj.get("ok")), str(obj.get("detail", ""))
        except Exception as exc:
            return False, str(exc)

    def _set_sinks(self, text: str, style: str) -> None:
        self._sinks_text = (text, style)
        try:
            self.query_one("#health-sinks", Static).update(
                _health("Sinks", text, style)
            )
        except Exception:
            pass

    # ── Table fillers ────────────────────────────────────────────────────────

    def _fill_active(
        self,
        sel: str,
        runs: list[dict],
        sparks: dict[str, tuple[str, float]],
        empty_msg: str,
    ) -> None:
        t = self.query_one(sel, DataTable)
        t.clear()
        if not runs:
            t.add_row(
                Text(""),
                Text.from_markup(empty_msg),
                *[Text("") for _ in range(6)],
            )
            return
        for r in runs:
            spark = sparks.get(r["id"])
            spark_cell = (
                Text(spark[0], style="#7aa2f7")
                if spark else Text("—", style="#414868")
            )
            t.add_row(
                status_dot_for(r),
                Text(r["id"][:10], style="#565f89"),
                Text(r.get("name") or "", overflow="ellipsis"),
                Text(r.get("vendor") or "", style="#7dcfff"),
                status_label_for(r),
                Text(rel_time(r.get("started_at") or r.get("created_at")),
                     style="#565f89"),
                Text(cost(r), style="#e0af68"),
                spark_cell,
                key=r["id"],
            )

    def _fill_recent(self, sel: str, runs: list[dict], empty_msg: str) -> None:
        t = self.query_one(sel, DataTable)
        t.clear()
        if not runs:
            t.add_row(
                Text(""),
                Text.from_markup(empty_msg),
                *[Text("") for _ in range(5)],
            )
            return
        for r in runs:
            t.add_row(
                status_dot_for(r),
                Text(r["id"][:10], style="#565f89"),
                Text(r.get("name") or "", overflow="ellipsis"),
                Text(r.get("vendor") or "", style="#7dcfff"),
                status_label_for(r),
                Text(rel_time(r.get("started_at") or r.get("created_at")),
                     style="#565f89"),
                Text(cost(r), style="#e0af68"),
                key=r["id"],
            )

    # ── KPI cards ────────────────────────────────────────────────────────────

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

    # ── Row clicks → run detail ──────────────────────────────────────────────

    def on_data_table_row_selected(
        self, event: DataTable.RowSelected
    ) -> None:
        run_id = (event.row_key.value if event.row_key else None) or ""
        if not run_id:
            return
        self.run_worker(self._open_detail(run_id), exclusive=True)

    async def _open_detail(self, run_id: str) -> None:
        from xrun_tui.screens.run_detail import RunDetailScreen
        await self.app.push_screen(RunDetailScreen(run_id))

    # ── Actions ──────────────────────────────────────────────────────────────

    async def action_open_runs(self) -> None:
        from xrun_tui.screens.runs import RunsScreen
        await self.app.push_screen(RunsScreen())

    async def action_goto_launch(self) -> None:
        from xrun_tui.screens.launch import LaunchScreen
        await self.app.push_screen(LaunchScreen())

    async def action_goto_doctor(self) -> None:
        from xrun_tui.screens.doctor import DoctorScreen
        await self.app.push_screen(DoctorScreen())

    async def action_goto_instances(self) -> None:
        from xrun_tui.screens.instances import InstancesScreen
        await self.app.push_screen(InstancesScreen())

    async def action_goto_vendors(self) -> None:
        from xrun_tui.screens.vendors import VendorsScreen
        await self.app.push_screen(VendorsScreen())

    async def action_goto_sinks(self) -> None:
        from xrun_tui.screens.sinks import SinksScreen
        await self.app.push_screen(SinksScreen())

    async def action_goto_settings(self) -> None:
        from xrun_tui.screens.settings import SettingsScreen
        await self.app.push_screen(SettingsScreen())

    def action_quit_app(self) -> None:
        self.app.exit()

    async def action_refresh(self) -> None:
        # Both via kick so Ctrl+R returns instantly and the refresh runs on
        # a worker — no per-keystroke wait for widget updates / `xrun doctor`.
        self._kick_refresh()
        self._kick_health()
