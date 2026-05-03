"""Failing local test — runs two epochs, then exits 1 inside a stage.

Verifies: failed-stage UI, error_detail screen, exit_code propagation,
runs list FAILED badge.
"""
from __future__ import annotations

import sys
import time

import xrun_hook


def main() -> None:
    print("fail-test: starting", flush=True)

    with xrun_hook.stage("train"):
        for epoch in range(2):
            xrun_hook.metric("loss", 1.0 - 0.1 * epoch, step=epoch)
            print(f"epoch {epoch}", flush=True)
            time.sleep(0.1)

    with xrun_hook.stage("eval"):
        print("fail-test: simulating eval crash", flush=True)
        raise RuntimeError("intentional eval failure for xrun TUI test")


if __name__ == "__main__":
    try:
        main()
    except Exception as e:
        print(f"FATAL: {e}", flush=True, file=sys.stderr)
        sys.exit(1)
