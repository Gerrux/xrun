import json
import logging
import os
import sys
from pathlib import Path

_log = logging.getLogger(__name__)


def _is_rank_zero() -> bool:
    return int(os.environ.get("RANK", "0")) == 0 or os.environ.get("XRUN_HOOK_ALL_RANKS") == "1"


def sanitize_extra(extra: "dict | None") -> "dict | None":
    """Drop keys starting with '_secret', warn on each dropped key."""
    if not extra:
        return None
    out = {}
    for k, v in extra.items():
        if str(k).startswith("_secret"):
            _log.warning("xrun_hook: dropping secret key %r from extra", k)
        else:
            out[k] = v
    return out if out else None


class JsonlWriter:
    def __init__(self, path: Path) -> None:
        self._path = path
        self._fd = open(path, "ab")

    def append(self, record: dict) -> None:
        if not _is_rank_zero():
            return
        encoded = (json.dumps(record, separators=(",", ":")) + "\n").encode("utf-8")
        self._write_locked(encoded)

    def _write_locked(self, encoded: bytes) -> None:
        if sys.platform == "win32":
            import msvcrt

            # Lock byte-range starting at 0 as an advisory mutex sentinel.
            # Windows byte-range locking works even beyond EOF.
            self._fd.seek(0)
            msvcrt.locking(self._fd.fileno(), msvcrt.LK_LOCK, 1)
            try:
                self._fd.seek(0, 2)
                self._fd.write(encoded)
                self._fd.flush()
                if os.environ.get("XRUN_HOOK_FSYNC") == "1":
                    os.fsync(self._fd.fileno())
            finally:
                self._fd.seek(0)
                msvcrt.locking(self._fd.fileno(), msvcrt.LK_UNLCK, 1)
        else:
            import fcntl

            fcntl.flock(self._fd, fcntl.LOCK_EX)
            try:
                self._fd.write(encoded)
                self._fd.flush()
                if os.environ.get("XRUN_HOOK_FSYNC") == "1":
                    os.fsync(self._fd.fileno())
            finally:
                fcntl.flock(self._fd, fcntl.LOCK_UN)

    def close(self) -> None:
        try:
            self._fd.flush()
        finally:
            self._fd.close()


class StdoutWriter:
    """Fallback writer used when no run directory is writable."""

    def append(self, record: dict) -> None:
        if not _is_rank_zero():
            return
        print(f"[xrun-event] {json.dumps(record, separators=(',', ':'))}", flush=True)

    def close(self) -> None:
        pass
