from __future__ import annotations

import os
import sys
import json
import subprocess
from pathlib import Path
from typing import Optional

import aiosqlite


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
        q = "SELECT * FROM runs"
        p: list = []
        if status == "active":
            q += " WHERE status IN ('provisioning','uploading','running')"
        elif status == "recent":
            q += " WHERE status NOT IN ('provisioning','uploading','running')"
        elif status:
            q += " WHERE status = ?"
            p.append(status)
        q += " ORDER BY created_at DESC LIMIT ?"
        p.append(limit)
        assert self._conn is not None
        async with self._conn.execute(q, p) as cur:
            rows = await cur.fetchall()
        return [dict(r) for r in rows]

    async def run(self, run_id: str) -> dict | None:
        assert self._conn is not None
        async with self._conn.execute("SELECT * FROM runs WHERE id=?", [run_id]) as cur:
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
