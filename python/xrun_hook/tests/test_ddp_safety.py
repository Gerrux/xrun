"""DDP safety test: N concurrent processes write to the same JSONL file without corruption."""

import json
import os
import subprocess
import sys
from pathlib import Path


_WRITER_SCRIPT = """\
import sys, os
from pathlib import Path
src_dir, shared_file, worker_id = sys.argv[1], sys.argv[2], int(sys.argv[3])
sys.path.insert(0, src_dir)
from xrun_hook._writer import JsonlWriter
w = JsonlWriter(Path(shared_file))
for i in range(100):
    w.append({"worker": worker_id, "step": i, "key": "loss", "value": float(i)})
w.close()
"""

_SRC_DIR = str(Path(__file__).parent.parent / "src")


def test_concurrent_writes_no_corruption(tmp_path):
    """400 total writes from 4 concurrent processes must yield 400 valid JSON lines."""
    shared = tmp_path / "metrics.jsonl"

    procs = [
        subprocess.Popen(
            [sys.executable, "-c", _WRITER_SCRIPT, _SRC_DIR, str(shared), str(i)],
            env={**os.environ},
        )
        for i in range(4)
    ]
    for i, p in enumerate(procs):
        ret = p.wait(timeout=30)
        assert ret == 0, f"worker {i} exited with code {ret}"

    lines = shared.read_text(encoding="utf-8").splitlines()
    assert len(lines) == 400, f"expected 400 lines, got {len(lines)}"
    for lineno, line in enumerate(lines):
        try:
            obj = json.loads(line)
        except json.JSONDecodeError as exc:
            raise AssertionError(f"line {lineno} is not valid JSON: {line!r}") from exc
        assert "worker" in obj, f"line {lineno} missing 'worker' field: {obj}"
        assert "step" in obj, f"line {lineno} missing 'step' field: {obj}"


def test_rank_guard_no_write(tmp_path):
    """RANK=1 process must not write anything."""
    shared = tmp_path / "events.jsonl"

    env = {**os.environ, "RANK": "1"}
    env.pop("XRUN_HOOK_ALL_RANKS", None)

    script = f"""\
import sys
from pathlib import Path
sys.path.insert(0, {_SRC_DIR!r})
from xrun_hook._writer import JsonlWriter
w = JsonlWriter(Path({str(shared)!r}))
w.append({{"stage": "train", "status": "start"}})
w.close()
"""
    ret = subprocess.run([sys.executable, "-c", script], env=env, timeout=10)
    assert ret.returncode == 0

    # File was created (open("ab") always creates it) but should be empty
    assert not shared.exists() or shared.read_bytes() == b""
