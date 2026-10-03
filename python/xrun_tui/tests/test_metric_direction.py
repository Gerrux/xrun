"""Which metric value is "best": name rule, early_stop, Sweep selection.

The name cases mirror the Rust tests in crates/xrun-cli/src/commands/diff.rs.
"""
from __future__ import annotations

import asyncio

import pytest
from textual.app import App
from textual.widgets import DataTable

from xrun_tui.screens.sweep import SweepScreen, pick_best
from xrun_tui.utils import is_better, metric_direction

MIN_KEYS = [
    "mae", "val_rmse", "test/mse", "perplexity", "val_ppl", "wer", "cer", "fid",
    "val_MAE", "errors", "mseloss", "train.Loss", "top5_error", "val_loss",
    "rel_err", "FIDScore", "valMAE", "top5Error", "test_nll", "eer", "val_bpb",
    "bpc",
]
MAX_KEYS = [
    "accuracy", "f1", "val_f1", "reward", "auc", "iou", "bleu", "overall",
    "merge_rate", "kernel_score", "fidelity", "certainty", "terminal_reward",
    "lossless_ratio", "AUCScore",
]


# Same table as `PARITY` in crates/xrun-cli/src/commands/diff.rs; the two
# implementations must agree on every key (including the pinned misses:
# loss_weight).
PARITY = [
    ("val_loss", "min"), ("train.Loss", "min"), ("mseloss", "min"),
    ("error", "min"), ("top5_error", "min"), ("errors", "min"), ("mae", "min"),
    ("MAE", "min"), ("rmse", "min"), ("mse", "min"), ("perplexity", "min"),
    ("ppl", "min"), ("wer", "min"), ("cer", "min"), ("fid", "min"),
    ("valLoss", "min"), ("valError", "min"), ("valMAE", "min"),
    ("top5Error", "min"), ("FIDScore", "min"), ("nll", "min"), ("eer", "min"),
    ("bpb", "min"), ("bpc", "min"),
    ("loss_weight", "min"), ("lossless_ratio", "max"), ("losslessRatio", "max"),
    ("accuracy", "max"), ("val_f1", "max"), ("reward", "max"),
    ("fidelity", "max"), ("certainty", "max"), ("overall", "max"),
    ("merge_rate", "max"), ("kernel_score", "max"), ("terminal_reward", "max"),
    ("valFidelity", "max"), ("mAP", "max"),
]


@pytest.mark.parametrize(("key", "want"), PARITY)
def test_parity_table_with_rust(key: str, want: str) -> None:
    assert metric_direction(key) == want


def test_parity_table_matches_rust_source() -> None:
    """The Rust copy of the table must be the same list, not a drifted one."""
    import re
    from pathlib import Path

    src = (Path(__file__).resolve().parents[3]
           / "crates" / "xrun-cli" / "src" / "commands" / "diff.rs")
    if not src.exists():
        pytest.skip("Rust sources not next to the TUI package")
    text = src.read_text(encoding="utf-8")
    block = text[text.index("const PARITY"):]
    block = block[:block.index("];")]
    rust = re.findall(r'\("([^"]+)",\s*"(min|max)"\)', block)
    assert rust == PARITY


@pytest.mark.parametrize("key", MIN_KEYS)
def test_name_rule_min(key: str) -> None:
    assert metric_direction(key) == "min"


@pytest.mark.parametrize("key", MAX_KEYS)
def test_name_rule_max(key: str) -> None:
    assert metric_direction(key) == "max"


def test_early_stop_mode_wins_over_name() -> None:
    es_max = {"policy": {"early_stop": {"metric": "val_loss", "patience": 3}}}
    es_min = {"policy": {"early_stop": {"metric": "score", "mode": "min"}}}
    assert metric_direction("val_loss", es_max) == "max"  # default mode is max
    assert metric_direction("score", es_min) == "min"
    # early_stop on another metric does not apply
    assert metric_direction("val_loss", es_min) == "min"
    assert metric_direction("accuracy", es_min) == "max"
    assert metric_direction("val_loss", {"name": "x"}) == "min"


def test_is_better() -> None:
    assert is_better(1, 2, "min") and not is_better(2, 1, "min")
    assert is_better(2, 1, "max") and not is_better(1, 2, "max")


def test_pick_best_lowest_loss() -> None:
    runs = [{"id": "a"}, {"id": "b"}, {"id": "c"}, {"id": "d"}]
    latest = {
        "a": ("val_loss", 0.5),
        "b": ("val_loss", 0.1),
        "c": ("val_loss", 0.9),
        "d": ("val_loss", float("nan")),
    }
    assert pick_best(runs, latest, "val_loss", "min") == ("b", 0.1)
    assert pick_best(runs, latest, "val_loss", "max") == ("c", 0.9)
    assert pick_best(runs, latest, "other", "min") == ("", None)


class _FakeDB:
    def __init__(self, runs, latest) -> None:
        self._runs = runs
        self._latest = latest

    async def runs(self, status=None, limit=300):
        return self._runs

    async def latest_metrics_for_runs(self, ids):
        return self._latest

    async def metric_extremes_for_runs(self, ids):
        # one-point histories: min == max == the latest value
        return {rid: {k: (v, v)} for rid, (k, v) in self._latest.items()}


class _Host(App):
    def __init__(self, db) -> None:
        super().__init__()
        self.db = db


def test_sweep_header_shows_direction_and_m_flips_it() -> None:
    runs = [
        {"id": f"run{i}0000000", "name": f"s-{i}", "manifest_path": "exp/sw/m.yaml"}
        for i in range(3)
    ]
    latest = {r["id"]: ("val_loss", v) for r, v in zip(runs, (0.5, 0.1, 0.9))}

    def header(screen) -> str:
        t = screen.query_one("#sweep-table", DataTable)
        return str(t.get_row_at(0)[1])

    async def scenario() -> None:
        app = _Host(_FakeDB(runs, latest))
        async with app.run_test(size=(140, 40)) as pilot:
            await app.push_screen(SweepScreen())
            await pilot.pause()
            await asyncio.sleep(0.3)
            screen = app.screen
            assert "best ↓ val_loss: 0.1" in header(screen)
            await pilot.press("down")  # off the header row, into the group
            await pilot.press("m")
            await pilot.pause()
            await asyncio.sleep(0.3)
            assert "best ↑ val_loss: 0.9" in header(screen)

    asyncio.run(scenario())


def test_m_on_a_group_header_keeps_cursor_on_that_group() -> None:
    """`table.clear()` resets the cursor to row 0; on a header row (no run id)
    the refresh used to lose the position, so a second `m` flipped the first
    group instead of un-flipping the one under the cursor."""
    runs = [
        {"id": f"a{i}000000000", "name": f"a-{i}", "manifest_path": "exp/ga/m.yaml"}
        for i in range(2)
    ] + [
        {"id": f"b{i}000000000", "name": f"b-{i}", "manifest_path": "exp/gb/m.yaml"}
        for i in range(2)
    ] + [
        {"id": f"c{i}000000000", "name": f"c-{i}", "manifest_path": "exp/gc/m.yaml"}
        for i in range(2)
    ]
    # gc has no metrics at all
    latest = {
        "a0000000000": ("val_loss", 0.5), "a1000000000": ("val_loss", 0.2),
        "b0000000000": ("val_loss", 0.4), "b1000000000": ("val_loss", 0.8),
    }

    async def scenario() -> None:
        app = _Host(_FakeDB(runs, latest))
        async with app.run_test(size=(140, 40)) as pilot:
            await app.push_screen(SweepScreen())
            await pilot.pause()
            await asyncio.sleep(0.3)
            screen = app.screen
            table = screen.query_one("#sweep-table", DataTable)
            assert screen._row_groups[3] == "gb" and screen._run_ids[3] is None
            table.move_cursor(row=3)
            await pilot.press("m")
            await pilot.pause()
            assert screen._flipped == {"gb"}
            assert table.cursor_row == 3
            await pilot.press("m")
            await pilot.pause()
            assert screen._flipped == set()  # un-flipped gb, ga untouched

            # A group with no metric: nothing to flip, and it says so.
            seen: list[str] = []
            screen.notify = lambda msg, **kw: seen.append(msg)  # type: ignore[method-assign]
            table.move_cursor(row=6)
            assert screen._row_groups[6] == "gc"
            await pilot.press("m")
            await pilot.pause()
            assert screen._flipped == set()
            assert seen and "No metric" in seen[0]

    asyncio.run(scenario())
