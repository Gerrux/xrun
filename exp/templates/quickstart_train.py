"""Zero-config quickstart — exercises the full xrun_hook pipeline (stages,
metrics, epochs, artifact, done) without GPU, data, or external services.

Pair with exp/templates/quickstart.yaml. Use to confirm xrun works end-to-end
before configuring vast/kaggle creds."""

from __future__ import annotations

import argparse
import math
import os
import time

import xrun_hook


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--epochs", type=int, default=3)
    args = ap.parse_args()

    with xrun_hook.stage("setup"):
        os.makedirs("output", exist_ok=True)
        time.sleep(0.05)

    with xrun_hook.stage("train"):
        for epoch in range(args.epochs):
            loss = math.exp(-epoch * 0.5)
            acc = 1.0 - loss * 0.5
            xrun_hook.metrics({"loss": loss, "acc": acc}, step=epoch)
            xrun_hook.epoch(epoch, {"loss": loss, "acc": acc})
            time.sleep(0.1)

    with open("output/result.txt", "w", encoding="utf-8") as f:
        f.write(f"trained {args.epochs} epochs, final acc={acc:.4f}\n")

    xrun_hook.done()


if __name__ == "__main__":
    main()
