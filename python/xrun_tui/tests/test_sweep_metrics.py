"""Sweep metric data: deterministic key choice, NULL safety, best over history."""
from __future__ import annotations

import asyncio
import sqlite3
from pathlib import Path

from textual.app import App
from textual.widgets import DataTable

from xrun_tui.db import Database
from xrun_tui.screens.sweep import SweepScreen

NAN = None  # SQLite stores NaN as NULL


def _make_db(tmp_path: Path, rows: list[tuple]) -> Path:
    path = tmp_path / "runs.db"
    con = sqlite3.connect(path)
    # value is nullable here: that is how a stored NaN comes back
    con.execute(
        "CREATE TABLE metrics (run_id TEXT NOT NULL, step INTEGER NOT NULL,"
        " key TEXT NOT NULL, value REAL, ts TEXT NOT NULL DEFAULT '2026-01-01',"
        " PRIMARY KEY (run_id, key, step))"
    )
    con.executemany(
        "INSERT INTO metrics (run_id, step, key, value) VALUES (?,?,?,?)", rows
    )
    con.commit()
    con.close()
    return path


def test_latest_key_follows_priority_not_row_order(tmp_path: Path) -> None:
    path = _make_db(tmp_path, [
        # `zeta` sorts last and `aaa_custom` first; the rule must pick `loss`
        ("r1", 5, "aaa_custom", 9.0), ("r1", 5, "loss", 0.4),
        ("r1", 5, "zeta", 1.0), ("r1", 3, "loss", 0.9),
        # priority key absent: first non-system key, never `step`/`epoch`
        ("r2", 7, "epoch", 7.0), ("r2", 7, "score", 0.7),
    ])

    async def go() -> dict:
        async with Database(path) as db:
            return await db.latest_metrics_for_runs(["r1", "r2", "none"])

    got = asyncio.run(go())
    assert got == {"r1": ("loss", 0.4), "r2": ("score", 0.7)}


def test_null_values_are_skipped_not_fatal(tmp_path: Path) -> None:
    path = _make_db(tmp_path, [
        # NaN at the last step: the latest real value wins
        ("r1", 1, "loss", 0.9), ("r1", 2, "loss", 0.5), ("r1", 3, "loss", NAN),
        # only NaN: the run has no metric
        ("r2", 1, "loss", NAN),
        # NaN on the priority key must not hide a usable other key
        ("r3", 1, "loss", NAN), ("r3", 1, "score", 0.2),
    ])

    async def go() -> tuple[dict, dict]:
        async with Database(path) as db:
            ids = ["r1", "r2", "r3"]
            return (await db.latest_metrics_for_runs(ids),
                    await db.metric_extremes_for_runs(ids))

    latest, ext = asyncio.run(go())
    assert latest == {"r1": ("loss", 0.5), "r3": ("score", 0.2)}
    assert ext["r1"] == {"loss": (0.5, 0.9)}
    assert "r2" not in ext
    assert ext["r3"] == {"score": (0.2, 0.2)}


def test_extremes_cover_whole_history_in_one_call(tmp_path: Path) -> None:
    path = _make_db(tmp_path, [
        ("a", 1, "val_loss", 0.9), ("a", 2, "val_loss", 0.2),
        ("a", 3, "val_loss", 0.6), ("a", 1, "acc", 0.1),
        ("b", 1, "val_loss", 0.4),
    ])

    async def go() -> dict:
        async with Database(path) as db:
            return await db.metric_extremes_for_runs(["a", "b"])

    assert asyncio.run(go()) == {
        "a": {"val_loss": (0.2, 0.9), "acc": (0.1, 0.1)},
        "b": {"val_loss": (0.4, 0.4)},
    }


class _Host(App):
    def __init__(self, db: Database) -> None:
        super().__init__()
        self.db = db


def test_sweep_ranks_on_best_over_history_and_survives_nan(tmp_path: Path) -> None:
    path = _make_db(tmp_path, [
        # a: dips to 0.1 but ends at 0.8; b: steady 0.3; c: NaN at the end
        ("a0000000000", 1, "val_loss", 0.1), ("a0000000000", 2, "val_loss", 0.8),
        ("b0000000000", 1, "val_loss", 0.3), ("b0000000000", 2, "val_loss", 0.3),
        ("c0000000000", 1, "val_loss", 0.5), ("c0000000000", 2, "val_loss", NAN),
    ])
    runs = [
        {"id": rid, "name": f"s-{rid[0]}", "manifest_path": "exp/sw/m.yaml"}
        for rid in ("a0000000000", "b0000000000", "c0000000000")
    ]

    class _Db(Database):
        async def runs(self, status=None, limit=300):
            return runs

    async def scenario() -> str:
        async with _Db(path) as db:
            app = _Host(db)
            async with app.run_test(size=(140, 40)) as pilot:
                await app.push_screen(SweepScreen())
                await pilot.pause()
                await asyncio.sleep(0.3)
                table = app.screen.query_one("#sweep-table", DataTable)
                assert table.row_count == 4  # header + three runs, none dropped
                return str(table.get_row_at(0)[1])

    header = asyncio.run(scenario())
    # best over history is a's 0.1, not the last values (0.8 / 0.3 / 0.5)
    assert "best ↓ val_loss: 0.1" in header
