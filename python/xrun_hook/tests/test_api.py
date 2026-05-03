"""Tests for the xrun_hook public API."""

import json
from pathlib import Path

import pytest

import xrun_hook
from xrun_hook._writer import JsonlWriter, StdoutWriter, sanitize_extra


@pytest.fixture(autouse=True)
def isolated_run_dir(tmp_path, monkeypatch):
    monkeypatch.setenv("XRUN_RUN_DIR", str(tmp_path))
    xrun_hook._reset()
    yield tmp_path
    xrun_hook._reset()


# ---------------------------------------------------------------------------
# stage()
# ---------------------------------------------------------------------------


def test_stage_writes_start_event(isolated_run_dir):
    xrun_hook.stage("unpack")
    lines = _read_events(isolated_run_dir)
    assert len(lines) == 1
    ev = json.loads(lines[0])
    assert ev["stage"] == "unpack"
    assert ev["status"] == "start"
    assert "ts" in ev


def test_stage_context_manager_ok(isolated_run_dir):
    with xrun_hook.stage("validation"):
        pass
    lines = _read_events(isolated_run_dir)
    assert len(lines) == 2
    assert json.loads(lines[0])["status"] == "start"
    assert json.loads(lines[1])["status"] == "ok"


def test_stage_context_manager_fail(isolated_run_dir):
    with pytest.raises(ValueError):
        with xrun_hook.stage("train"):
            raise ValueError("exploded")
    lines = _read_events(isolated_run_dir)
    assert len(lines) == 2
    fail_ev = json.loads(lines[1])
    assert fail_ev["status"] == "fail"
    assert "ValueError" in fail_ev["extra"]["error"]


def test_stage_with_msg_and_extra(isolated_run_dir):
    xrun_hook.stage("unpack", msg="cache.tar", extra={"size_gb": 3.2})
    ev = json.loads(_read_events(isolated_run_dir)[0])
    assert ev["msg"] == "cache.tar"
    assert ev["extra"]["size_gb"] == 3.2


def test_stage_custom_status(isolated_run_dir):
    xrun_hook.stage("foo", status="ok")
    ev = json.loads(_read_events(isolated_run_dir)[0])
    assert ev["status"] == "ok"


# ---------------------------------------------------------------------------
# metric()
# ---------------------------------------------------------------------------


def test_metric_writes_correct_structure(isolated_run_dir):
    xrun_hook.metric("val_f1", 0.812, step=1)
    lines = _read_metrics(isolated_run_dir)
    assert len(lines) == 1
    row = json.loads(lines[0])
    assert row["key"] == "val_f1"
    assert row["value"] == pytest.approx(0.812)
    assert row["step"] == 1
    assert "ts" in row


# ---------------------------------------------------------------------------
# metrics() — batch shortcut
# ---------------------------------------------------------------------------


def test_metrics_batch_writes_one_row_per_key(isolated_run_dir):
    xrun_hook.metrics({"loss": 0.42, "val_f1": 0.81, "lr": 1e-3}, step=5)
    rows = [json.loads(line) for line in _read_metrics(isolated_run_dir)]
    assert len(rows) == 3
    by_key = {r["key"]: r for r in rows}
    assert by_key["loss"]["value"] == pytest.approx(0.42)
    assert by_key["val_f1"]["value"] == pytest.approx(0.81)
    assert by_key["lr"]["value"] == pytest.approx(1e-3)
    assert all(r["step"] == 5 for r in rows)
    assert len({r["ts"] for r in rows}) == 1  # все три под одним timestamp


def test_metrics_batch_empty_dict_is_noop(isolated_run_dir):
    xrun_hook.metrics({}, step=0)
    assert _read_metrics(isolated_run_dir) == []


# ---------------------------------------------------------------------------
# epoch()
# ---------------------------------------------------------------------------


def test_epoch_writes_stage_epoch_ok(isolated_run_dir):
    xrun_hook.epoch(3, {"val_f1": 0.831})
    ev = json.loads(_read_events(isolated_run_dir)[0])
    assert ev["stage"] == "epoch"
    assert ev["status"] == "ok"
    assert ev["extra"]["epoch"] == 3
    assert ev["extra"]["val_f1"] == pytest.approx(0.831)


# ---------------------------------------------------------------------------
# done()
# ---------------------------------------------------------------------------


def test_done_writes_done_event(isolated_run_dir):
    xrun_hook.done()
    lines = _read_events(isolated_run_dir)
    assert len(lines) == 1
    ev = json.loads(lines[0])
    assert ev["stage"] == "done"
    assert ev["status"] == "ok"


def test_done_clears_writers_so_subsequent_calls_reopen(isolated_run_dir):
    xrun_hook.done()
    # After done(), writers must be None so a subsequent write opens a new file
    # rather than writing to a closed file descriptor (which would raise ValueError).
    xrun_hook.stage("after_done")
    lines = _read_events(isolated_run_dir)
    assert len(lines) == 2  # done event + after_done event


# ---------------------------------------------------------------------------
# fail()
# ---------------------------------------------------------------------------


def test_fail_writes_event_and_exits(isolated_run_dir):
    with pytest.raises(SystemExit) as exc_info:
        xrun_hook.fail("something went wrong", extra={"step": 42})
    assert exc_info.value.code == 1
    lines = _read_events(isolated_run_dir)
    assert len(lines) == 1
    ev = json.loads(lines[0])
    assert ev["stage"] == "error"
    assert ev["status"] == "fail"
    assert ev["msg"] == "something went wrong"
    assert ev["extra"]["step"] == 42


def test_fail_closes_writers_before_exit(isolated_run_dir):
    with pytest.raises(SystemExit):
        xrun_hook.fail("crash")
    # After fail(), module-level writers should be None.
    # Calling metric() should not raise ValueError (it should reopen writers).
    xrun_hook.metric("loss", 1.0, step=0)
    rows = _read_metrics(isolated_run_dir)
    assert len(rows) == 1


# ---------------------------------------------------------------------------
# Secret key filtering
# ---------------------------------------------------------------------------


def test_sanitize_extra_drops_secret_keys():
    result = sanitize_extra({"a": 1, "_secret_key": "pw", "_secretX": "x"})
    assert result == {"a": 1}


def test_sanitize_extra_none():
    assert sanitize_extra(None) is None


def test_sanitize_extra_empty():
    assert sanitize_extra({}) is None


# ---------------------------------------------------------------------------
# StdoutWriter fallback
# ---------------------------------------------------------------------------


def test_stdout_writer_fallback(capsys):
    w = StdoutWriter()
    w.append({"stage": "done", "status": "ok"})
    captured = capsys.readouterr()
    assert captured.out.startswith("[xrun-event] ")
    payload = json.loads(captured.out[len("[xrun-event] "):].strip())
    assert payload["stage"] == "done"


# ---------------------------------------------------------------------------
# Rank guard
# ---------------------------------------------------------------------------


def test_rank_guard_suppresses_writes(tmp_path, monkeypatch):
    monkeypatch.setenv("RANK", "1")
    monkeypatch.delenv("XRUN_HOOK_ALL_RANKS", raising=False)
    f = tmp_path / "events.jsonl"
    w = JsonlWriter(f)
    w.append({"stage": "train", "status": "start"})
    w.close()
    assert f.read_bytes() == b""


def test_rank_guard_all_ranks_override(tmp_path, monkeypatch):
    monkeypatch.setenv("RANK", "1")
    monkeypatch.setenv("XRUN_HOOK_ALL_RANKS", "1")
    f = tmp_path / "events.jsonl"
    w = JsonlWriter(f)
    w.append({"stage": "train", "status": "start"})
    w.close()
    assert f.read_bytes() != b""


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


def _read_events(run_dir: Path) -> list[str]:
    p = run_dir / "events.jsonl"
    return p.read_text(encoding="utf-8").splitlines() if p.exists() else []


def _read_metrics(run_dir: Path) -> list[str]:
    p = run_dir / "metrics.jsonl"
    return p.read_text(encoding="utf-8").splitlines() if p.exists() else []
