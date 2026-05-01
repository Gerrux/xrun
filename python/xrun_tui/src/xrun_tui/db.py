from __future__ import annotations

import datetime
import os
import sys
import json
import subprocess
from pathlib import Path
from typing import Optional

import aiosqlite

_CREATE_NO_WINDOW = subprocess.CREATE_NO_WINDOW if sys.platform == "win32" else 0


def _default_data_dir() -> Path:
    if env := os.environ.get("XRUN_DATA_DIR"):
        return Path(env)
    if sys.platform == "win32":
        appdata = os.environ.get("APPDATA", str(Path.home()))
        return Path(appdata) / "xrun"
    if sys.platform == "darwin":
        return Path.home() / "Library" / "Application Support" / "xrun"
    return Path.home() / ".local" / "share" / "xrun"


def find_db_path() -> Path:
    try:
        r = subprocess.run(
            ["xrun", "doctor", "--json"],
            capture_output=True, text=True, timeout=5,
            creationflags=_CREATE_NO_WINDOW,
        )
        if r.returncode == 0:
            d = json.loads(r.stdout)
            if p := d.get("db_path"):
                return Path(p)
    except Exception:
        pass
    base = _default_data_dir()
    # Try data/ subdirectory first (actual layout on Windows)
    for candidate in (base / "data" / "runs.db", base / "runs.db"):
        if candidate.exists():
            return candidate
    return base / "data" / "runs.db"


class Database:
    def __init__(self, path: Path) -> None:
        self.path = path
        self._conn: Optional[aiosqlite.Connection] = None

    async def connect(self) -> None:
        self._conn = await aiosqlite.connect(self.path)
        self._conn.row_factory = aiosqlite.Row

    async def close(self) -> None:
        if self._conn:
            await self._conn.close()
            self._conn = None

    async def __aenter__(self) -> "Database":
        await self.connect()
        return self

    async def __aexit__(self, *_: object) -> None:
        await self.close()

    # ── Queries ──────────────────────────────────────────────────────────────

    async def runs(self, status: str | None = None, limit: int = 300) -> list[dict]:
        q = (
            "SELECT r.*, "
            "  (SELECT MAX(ts) FROM events e WHERE e.run_id = r.id) "
            "    AS last_event_ts "
            "FROM runs r"
        )
        p: list = []
        if status == "active":
            q += " WHERE r.status IN ('provisioning','uploading','running')"
        elif status == "recent":
            q += " WHERE r.status NOT IN ('provisioning','uploading','running')"
        elif status:
            q += " WHERE r.status = ?"
            p.append(status)
        q += " ORDER BY r.created_at DESC LIMIT ?"
        p.append(limit)
        assert self._conn is not None
        async with self._conn.execute(q, p) as cur:
            rows = await cur.fetchall()
        return [dict(r) for r in rows]

    async def run(self, run_id: str) -> dict | None:
        assert self._conn is not None
        q = (
            "SELECT r.*, "
            "  (SELECT MAX(ts) FROM events e WHERE e.run_id = r.id) "
            "    AS last_event_ts "
            "FROM runs r WHERE r.id = ?"
        )
        async with self._conn.execute(q, [run_id]) as cur:
            row = await cur.fetchone()
        return dict(row) if row else None

    async def events(self, run_id: str) -> list[dict]:
        assert self._conn is not None
        async with self._conn.execute(
            "SELECT * FROM events WHERE run_id=? ORDER BY ts ASC", [run_id]
        ) as cur:
            rows = await cur.fetchall()
        return [dict(r) for r in rows]

    async def instances(self) -> list[dict]:
        assert self._conn is not None
        async with self._conn.execute(
            "SELECT * FROM instances ORDER BY created_at DESC"
        ) as cur:
            rows = await cur.fetchall()
        return [dict(r) for r in rows]

    async def metric_keys(self, run_id: str) -> list[dict]:
        assert self._conn is not None
        async with self._conn.execute(
            "SELECT key, COUNT(*) as count FROM metrics WHERE run_id=? GROUP BY key ORDER BY key",
            [run_id],
        ) as cur:
            rows = await cur.fetchall()
        return [dict(r) for r in rows]

    async def metrics_for_key(self, run_id: str, key: str) -> list[dict]:
        assert self._conn is not None
        async with self._conn.execute(
            "SELECT step, key, value, ts FROM metrics WHERE run_id=? AND key=? ORDER BY step",
            [run_id, key],
        ) as cur:
            rows = await cur.fetchall()
        return [dict(r) for r in rows]

    def log_path(self, run_id: str) -> Path:
        return self.path.parent / "runs" / run_id / "stdout.log"

    # ── Maintenance ──────────────────────────────────────────────────────────

    async def db_size_bytes(self) -> int:
        try:
            return self.path.stat().st_size
        except Exception:
            return 0

    async def count_finished_runs(self) -> int:
        assert self._conn is not None
        async with self._conn.execute(
            "SELECT COUNT(*) FROM runs"
            " WHERE status NOT IN ('provisioning','uploading','running')"
        ) as cur:
            row = await cur.fetchone()
        return row[0] if row else 0

    async def cleanup_runs(self, keep_days: int) -> int:
        """Delete finished runs older than keep_days (0 = delete all finished)."""
        assert self._conn is not None
        if keep_days <= 0:
            q = (
                "SELECT id FROM runs"
                " WHERE status NOT IN ('provisioning','uploading','running')"
            )
            params: list = []
        else:
            cutoff = (
                datetime.datetime.utcnow() - datetime.timedelta(days=keep_days)
            ).isoformat()
            q = (
                "SELECT id FROM runs"
                " WHERE status NOT IN ('provisioning','uploading','running')"
                " AND created_at < ?"
            )
            params = [cutoff]
        async with self._conn.execute(q, params) as cur:
            rows = await cur.fetchall()
        ids = [r[0] for r in rows]
        if not ids:
            return 0
        ph = ",".join("?" * len(ids))
        await self._conn.execute(f"DELETE FROM events  WHERE run_id IN ({ph})", ids)
        await self._conn.execute(f"DELETE FROM metrics WHERE run_id IN ({ph})", ids)
        await self._conn.execute(f"DELETE FROM runs    WHERE id      IN ({ph})", ids)
        await self._conn.commit()
        return len(ids)

    async def latest_metrics_for_runs(
        self, run_ids: list[str]
    ) -> dict[str, tuple[str, float]]:
        """Return {run_id: (metric_key, latest_value)} for the most recently
        recorded metric point per run.  Only run_ids that actually have metric
        rows are included in the result."""
        if not run_ids:
            return {}
        assert self._conn is not None
        ph = ",".join("?" * len(run_ids))
        # For each run pick the row with the highest step; break ties by ts.
        q = f"""
            SELECT m.run_id, m.key, m.value
            FROM metrics m
            INNER JOIN (
                SELECT run_id, MAX(step) AS max_step
                FROM metrics
                WHERE run_id IN ({ph})
                GROUP BY run_id
            ) latest ON m.run_id = latest.run_id AND m.step = latest.max_step
            WHERE m.run_id IN ({ph})
            GROUP BY m.run_id
        """
        async with self._conn.execute(q, run_ids + run_ids) as cur:
            rows = await cur.fetchall()
        return {r[0]: (r[1], float(r[2])) for r in rows}

    async def spend_by_day(self, days: int = 14) -> list[dict]:
        """Return list of {day: 'YYYY-MM-DD', spend: float} for the last
        *days* calendar days (most-recent last).  Days with no finished runs
        are included with spend=0."""
        assert self._conn is not None
        # Aggregate finished runs by their creation date
        q = """
            SELECT DATE(created_at) AS day,
                   SUM(COALESCE(cost_usd, cost_usd_estimate, 0.0)) AS spend
            FROM runs
            WHERE created_at >= DATE('now', ?)
            GROUP BY DATE(created_at)
        """
        offset_arg = f"-{days} days"
        async with self._conn.execute(q, [offset_arg]) as cur:
            rows = await cur.fetchall()
        # Build a dense series so every day is present
        import datetime as _dt
        today = _dt.date.today()
        spend_map: dict[str, float] = {r[0]: float(r[1] or 0) for r in rows}
        result: list[dict] = []
        for i in range(days - 1, -1, -1):
            d = today - _dt.timedelta(days=i)
            day_str = d.isoformat()
            result.append({"day": day_str, "spend": spend_map.get(day_str, 0.0)})
        return result

    async def vacuum(self) -> None:
        assert self._conn is not None
        await self._conn.execute("VACUUM")
