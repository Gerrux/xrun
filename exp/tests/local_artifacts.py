"""Artifact-emitting local test — writes checkpoints + plots + metrics.json.

Produces the file layout that the manifest's artifact patterns and checkpoints
watcher expect, so `xrun pull --ckpt best` and the Artifacts tab have something
to show.
"""
from __future__ import annotations

import json
import os
import struct
import time

import xrun_hook

CKPT_DIR = "checkpoints"
OUT_DIR = "output"


def _fake_ckpt(path: str, payload: bytes) -> None:
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "wb") as f:
        f.write(payload)


def _fake_png(path: str) -> None:
    """Write the smallest valid 1×1 PNG so the Artifacts tab sees a real file."""
    os.makedirs(os.path.dirname(path), exist_ok=True)
    sig = b"\x89PNG\r\n\x1a\n"
    ihdr = (b"\x00\x00\x00\rIHDR" + struct.pack(">II", 1, 1)
            + b"\x08\x06\x00\x00\x00\x1f\x15\xc4\x89")
    idat = (b"\x00\x00\x00\x16IDATx\x9cb\x00\x01\x00\x00\xff\xff\x00"
            b"\x00\x00\x02\x00\x01\xe5'\xde\xfc")
    iend = b"\x00\x00\x00\x00IEND\xaeB`\x82"
    with open(path, "wb") as f:
        f.write(sig + ihdr + idat + iend)


def main() -> None:
    print("artifacts: starting", flush=True)

    with xrun_hook.stage("train"):
        best_f1 = 0.0
        for epoch in range(3):
            f1 = 0.6 + 0.1 * epoch
            xrun_hook.metric("val_f1", f1, step=epoch)
            xrun_hook.metric("loss", 1.0 / (epoch + 1), step=epoch)
            _fake_ckpt(f"{CKPT_DIR}/ep{epoch:02d}.pt", b"FAKECKPT" * 64)
            if f1 > best_f1:
                best_f1 = f1
                _fake_ckpt(f"{CKPT_DIR}/best_f1_{f1:.3f}.pt",
                           b"BESTCKPT" * 64)
            xrun_hook.epoch(epoch, {"val_f1": f1})
            print(f"epoch {epoch}: f1={f1:.3f}", flush=True)
            time.sleep(0.1)

    with xrun_hook.stage("eval"):
        _fake_png(f"{OUT_DIR}/confusion_matrix.png")
        _fake_png(f"{OUT_DIR}/pr_curve.png")
        os.makedirs(OUT_DIR, exist_ok=True)
        with open(f"{OUT_DIR}/metrics.json", "w") as f:
            json.dump({"best_f1": best_f1, "epochs": 3}, f, indent=2)

    xrun_hook.done()
    print(f"artifacts: done (best_f1={best_f1:.3f})", flush=True)


if __name__ == "__main__":
    main()
