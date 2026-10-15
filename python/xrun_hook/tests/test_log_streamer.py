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


def test_start_if_configured_keeps_retrying_after_mlflow_init_failure(
    monkeypatch, tmp_path: Path
):
    monkeypatch.setenv("MLFLOW_TRACKING_URI", "http://127.0.0.1:1")  # nothing listening
    monkeypatch.setenv("XRUN_RUN_ID", "abc123")
    monkeypatch.chdir(tmp_path)
    log = tmp_path / "__xrun_stdout.log"
    log.write_bytes(b"")
    # Must not raise even though MLflow is unreachable, and must not give up:
    # one failed init at kernel start used to disable telemetry for the run.
    streamer = start_if_configured()
    assert streamer is not None
    assert streamer.mlflow_run_id is None
    # The failure is visible in the tailed log, i.e. in `xrun logs`.
    assert b"[xrun_hook] MLflow init failed" in log.read_bytes()


class FlakyInit:
    """Fails the first `failures` calls, then returns a run."""

    def __init__(self, failures: int) -> None:
        self.failures = failures
        self.calls = 0

    def __call__(self) -> "tuple[str, str]":
        self.calls += 1
        if self.calls <= self.failures:
            raise OSError("connection refused")
        return "run-late", "1/run-late/artifacts"


def test_streamer_uploads_backlog_once_init_succeeds(log_file: Path):
    client = FakeClient()
    init = FlakyInit(failures=2)
    streamer = LogStreamer(client, None, log_file, interval_sec=0.01, init=init)
    log_file.write_bytes(b"written while MLflow was down\n")
    streamer.start()
    try:
        deadline = time.monotonic() + 3.0
        while not client.artifacts and time.monotonic() < deadline:
            time.sleep(0.02)
    finally:
        streamer.stop(timeout=2.0)

    assert init.calls == 3
    assert streamer.mlflow_run_id == "run-late"
    reassembled = b"".join(blob for _, blob in client.artifacts)
    # Status line from _say() lands in the tailed log after the backlog.
    assert reassembled.startswith(b"written while MLflow was down\n")
    assert client.artifacts[0][0] == "logs/log_000001.txt"


def test_streamer_without_run_reads_nothing(log_file: Path):
    client = FakeClient()
    streamer = LogStreamer(client, None, log_file, interval_sec=0.01, init=FlakyInit(10**6))
    log_file.write_bytes(b"pending")
    streamer.start()
    time.sleep(0.1)
    streamer.stop(timeout=2.0)
    assert client.artifacts == []
    assert streamer.mlflow_run_id is None


def test_atexit_drain_skips_update_without_run(log_file: Path):
    client = FakeClient()
    streamer = LogStreamer(client, None, log_file, interval_sec=10.0, init=FlakyInit(10**6))
    streamer.start()
    _log_streamer._atexit_drain(streamer, client)
    assert client.last_run_status is None


def test_atexit_drain_finishes_created_run(log_file: Path):
    client = FakeClient()
    streamer = LogStreamer(client, "run-42", log_file, interval_sec=10.0)
    streamer.start()
    _log_streamer._atexit_drain(streamer, client)
    assert client.last_run_status == "FINISHED"


class FailFirstPutClient(FakeClient):
    """The first PUT times out (it may or may not have landed server-side)."""

    def __init__(self) -> None:
        super().__init__()
        self.puts = 0

    def put_artifact(self, artifact_path: str, remote_path: str, content: bytes) -> None:
        self.puts += 1
        if self.puts == 1:
            raise OSError("timed out")
        super().put_artifact(artifact_path, remote_path, content)


def test_failed_put_is_retried_without_a_hole(log_file: Path):
    # A dropped chunk used to be gone for good: the poller's live offset then
    # fell behind the file, and post-run ingest (which skips that many bytes
    # of events.jsonl) re-ingested some lines and never saw others.
    client = FailFirstPutClient()
    streamer = LogStreamer(client, "run-42", log_file, interval_sec=10.0)
    log_file.write_bytes(b"a\n")
    with pytest.raises(OSError):
        streamer._flush_once()
    with log_file.open("ab") as f:
        f.write(b"b\n")
    streamer._flush_once()
    # Same seq, same bytes as the failed attempt, then the rest.
    assert client.artifacts == [
        ("logs/log_000001.txt", b"a\n"),
        ("logs/log_000002.txt", b"b\n"),
    ]


def test_backlog_goes_out_in_capped_line_aligned_chunks(log_file: Path, monkeypatch):
    monkeypatch.setattr(_log_streamer, "MAX_CHUNK_BYTES", 16)
    monkeypatch.setattr(_log_streamer, "MAX_CHUNKS_PER_TICK", 2)
    body = b"".join(f"line-{i:02d}\n".encode() for i in range(10))  # 8 bytes each
    log_file.write_bytes(body)
    client = FakeClient()
    streamer = LogStreamer(client, "run-42", log_file, interval_sec=10.0)

    streamer._flush_once()  # a normal tick is bounded
    assert len(client.artifacts) == 2

    streamer._flush_once(force_init=True)  # the final drain is not
    blobs = [blob for _, blob in client.artifacts]
    assert b"".join(blobs) == body
    assert all(len(b) <= 16 and b.endswith(b"\n") for b in blobs)
    assert [p for p, _ in client.artifacts] == [
        f"logs/log_{i:06d}.txt" for i in range(1, len(blobs) + 1)
    ]
