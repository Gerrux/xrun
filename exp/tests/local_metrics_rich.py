"""Rich-metrics local test — exercises Run detail → Metrics tab.

Emits 6 metric keys across 5 epochs, three stages (data_load, train, eval).
Each epoch logs all metrics in one batched call so timestamps align.
"""
from __future__ import annotations

import math
import time

import xrun_hook


def main() -> None:
    print("metrics_rich: starting", flush=True)

    with xrun_hook.stage("data_load"):
        time.sleep(0.05)
        xrun_hook.metric("dataset_size", 1024.0, step=0)

    with xrun_hook.stage("train"):
        for epoch in range(5):
            loss     = 1.0 / (epoch + 1)
            val_loss = loss + 0.05 * math.sin(epoch)
            acc      = 0.5 + 0.08 * epoch
            val_acc  = acc - 0.03
            f1       = min(0.95, 0.6 + 0.07 * epoch)
            lr       = 1e-3 * (0.5 ** epoch)
            xrun_hook.metrics(
                {
                    "loss": loss,
                    "val_loss": val_loss,
                    "acc": acc,
                    "val_acc": val_acc,
                    "f1": f1,
                    "lr": lr,
                },
                step=epoch,
            )
            xrun_hook.epoch(epoch, {"loss": loss, "val_loss": val_loss})
            print(f"epoch {epoch}: loss={loss:.4f} val_acc={val_acc:.3f}",
                  flush=True)
            time.sleep(0.1)

    with xrun_hook.stage("eval"):
        time.sleep(0.05)
        xrun_hook.metric("test_acc", 0.823, step=0)
        xrun_hook.metric("test_f1", 0.794, step=0)

    xrun_hook.done()
    print("metrics_rich: done", flush=True)


if __name__ == "__main__":
    main()
