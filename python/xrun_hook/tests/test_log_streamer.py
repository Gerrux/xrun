"""Tests for the xrun_hook log streamer."""

from __future__ import annotations

import threading
import time
from pathlib import Path

import pytest

from xrun_hook import _log_streamer
from xrun_hook._log_streamer import LogStreamer, start_if_configured


class FakeClient:
    """In-memory MLflow stand-in. Records every artifact PUT."""

    def __init__(self) -> None:
        self.artifacts: list[tuple[str, bytes]] = []
        self.experiment_calls = 0
        self.run_calls = 0
        self.last_run_status: str | None = None
        self.lock = threading.Lock()

    def get_or_create_experiment(self, name: str) -> str:
        self.experiment_calls += 1
        return "exp-1"

    def create_run(self, exp_id: str, tags):
        self.run_calls += 1
        return "run-42", "1/run-42/artifacts"

    def update_run(self, run_id: str, status_str: str) -> None:
        self.last_run_status = status_str

    def put_artifact(self, artifact_path: str, remote_path: str, content: bytes) -> None:
        with self.lock:
            self.artifacts.append((remote_path, content))


@pytest.fixture
def log_file(tmp_path: Path) -> Path:
    p = tmp_path / "stdout.log"
    p.write_bytes(b"")
    return p


# ---------------------------------------------------------------------------
# LogStreamer mechanics
# ---------------------------------------------------------------------------


def test_streamer_pushes_appended_bytes_in_order(log_file: Path):
    client = FakeClient()
    streamer = LogStreamer(client, "run-42", log_file, interval_sec=0.1)
    streamer.start()
    try:
        with log_file.open("ab") as f:
            f.write(b"line one\n")
            f.flush()
        time.sleep(0.3)
        with log_file.open("ab") as f:
            f.write(b"line two\n")
            f.flush()
        time.sleep(0.3)
    finally:
        streamer.stop(timeout=2.0)

    # Counter is monotonic; chunks reassemble to the full file content.
    paths = [p for p, _ in client.artifacts]
    assert paths == sorted(paths), f"chunk paths not monotonic: {paths}"
    reassembled = b"".join(blob for _, blob in client.artifacts)
    assert reassembled == b"line one\nline two\n"


def test_streamer_chunk_path_format(log_file: Path):
    client = FakeClient()
    streamer = LogStreamer(client, "run-42", log_file, interval_sec=0.1)
    streamer.start()
    try:
        log_file.write_bytes(b"hi")
        time.sleep(0.3)
    finally:
        streamer.stop(timeout=2.0)

    assert client.artifacts, "expected at least one chunk"
    path, _ = client.artifacts[0]
    # logs/log_NNNNNN.txt — zero-padded so lexical sort matches numeric sort.
    assert path == "logs/log_000001.txt"


def test_streamer_handles_truncation_without_crashing(log_file: Path):
    client = FakeClient()
    streamer = LogStreamer(client, "run-42", log_file, interval_sec=0.1)
    streamer.start()
    try:
        log_file.write_bytes(b"first batch\n")
        time.sleep(0.3)
        # Simulate rotation: file shrinks below current offset.
        log_file.write_bytes(b"x")
        time.sleep(0.3)
    finally:
        streamer.stop(timeout=2.0)

    # Both chunks were captured — the truncation reset offset to 0 instead of
    # silently swallowing the new bytes.
    contents = b"".join(blob for _, blob in client.artifacts)
    assert b"first batch" in contents
    assert b"x" in contents


def test_streamer_skips_when_no_new_bytes(log_file: Path):
    client = FakeClient()
    streamer = LogStreamer(client, "run-42", log_file, interval_sec=0.1)
    streamer.start()
    try:
        time.sleep(0.4)  # several intervals, but file stays empty
    finally:
        streamer.stop(timeout=2.0)
    assert client.artifacts == []


def test_streamer_final_flush_on_stop(log_file: Path):
    client = FakeClient()
    streamer = LogStreamer(client, "run-42", log_file, interval_sec=10.0)
    streamer.start()
    log_file.write_bytes(b"last words")
    # Stop is called before the next interval — final flush must catch this.
    streamer.stop(timeout=2.0)
    assert any(b"last words" in blob for _, blob in client.artifacts)


def test_streamer_handles_missing_file(tmp_path: Path):
    client = FakeClient()
    log_path = tmp_path / "never_created.log"
    streamer = LogStreamer(client, "run-42", log_path, interval_sec=0.1)
    streamer.start()
    try:
        time.sleep(0.3)
    finally:
        streamer.stop(timeout=2.0)
    assert client.artifacts == []


# ---------------------------------------------------------------------------
# start_if_configured() activation gate
# ---------------------------------------------------------------------------


@pytest.fixture(autouse=True)
def reset_streamer_module():
    _log_streamer._reset_for_test()
    yield
    _log_streamer._reset_for_test()


def test_start_if_configured_inert_without_env(monkeypatch, tmp_path: Path):
    # Strip every env var the streamer cares about.
    for k in (
        "MLFLOW_TRACKING_URI",
        "XRUN_RUN_ID",
        "XRUN_LOG_STREAM_FILE",
        "XRUN_LOG_STREAM_DISABLE",
    ):
        monkeypatch.delenv(k, raising=False)
    assert start_if_configured() is None


def test_start_if_configured_inert_when_disabled(monkeypatch, tmp_path: Path):
    monkeypatch.setenv("MLFLOW_TRACKING_URI", "http://localhost:5000")
    monkeypatch.setenv("XRUN_RUN_ID", "abc123")
    monkeypatch.setenv("XRUN_LOG_STREAM_DISABLE", "1")
    assert start_if_configured() is None


def test_start_if_configured_inert_on_non_zero_rank(monkeypatch):
    monkeypatch.setenv("MLFLOW_TRACKING_URI", "http://localhost:5000")
    monkeypatch.setenv("XRUN_RUN_ID", "abc123")
    monkeypatch.setenv("RANK", "1")
    monkeypatch.delenv("XRUN_HOOK_ALL_RANKS", raising=False)
    assert start_if_configured() is None


def test_start_if_configured_swallows_mlflow_init_failure(
    monkeypatch, tmp_path: Path
):
    monkeypatch.setenv("MLFLOW_TRACKING_URI", "http://127.0.0.1:1")  # nothing listening
    monkeypatch.setenv("XRUN_RUN_ID", "abc123")
    monkeypatch.chdir(tmp_path)
    (tmp_path / "__xrun_stdout.log").write_bytes(b"")
    # Must not raise even though MLflow is unreachable.
    assert start_if_configured() is None
