import os
from pathlib import Path


def find_run_dir() -> Path | None:
    """Return first writable run directory, creating it if needed. None = stdout fallback."""
    candidates = [
        os.environ.get("XRUN_RUN_DIR"),
        "/workspace/run",
        "/kaggle/working/run",
        "./run",
    ]
    for candidate in candidates:
        if candidate is None:
            continue
        path = Path(candidate)
        try:
            path.mkdir(parents=True, exist_ok=True)
            probe = path / ".xrun_write_probe"
            probe.touch()
            probe.unlink()
            return path
        except (OSError, PermissionError):
            continue
    return None
