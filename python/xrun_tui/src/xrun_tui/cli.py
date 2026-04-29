"""Compatibility shim — see `xrun_tui.services` for the actual implementation."""
from __future__ import annotations

from xrun_tui.services import (  # noqa: F401
    doctor,
    get_logs,
    launch,
    metrics,
    pull,
    read_manifest,
    rerun_run,
    stop_run,
)


async def get_manifest(path: str) -> str:  # legacy name
    return read_manifest(path)
