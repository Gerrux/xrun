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

    async def vacuum(self) -> None:
        assert self._conn is not None
        await self._conn.execute("VACUUM")
