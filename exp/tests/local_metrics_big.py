"""Big-metrics local test — stress-tests MetricsView.

Emits ~20 keys over 150 steps to exercise: master scroll, fuzzy filter,
auto-grouping (loss/val_loss/test_loss, acc/val_acc/test_acc, f1/val_f1),
log-y (loss spans 4 orders of magnitude), EMA smoothing (noisy curves),
lower-better detection (loss/err/mae) vs higher-better (acc/f1/iou).
"""
from __future__ import annotations

import math
import random
import time

import xrun_hook

random.seed(42)

N_STEPS = 150


def _curve(start: float, end: float, step: int, total: int,
           noise: float = 0.0, kind: str = "exp") -> float:
    """Smooth start→end interpolation with optional gaussian noise."""
    t = step / max(1, total - 1)
    if kind == "exp":
        # Exponential decay (loss-like)
        v = end + (start - end) * math.exp(-3.0 * t)
    elif kind == "log":
        # Saturating growth (acc-like)
        v = start + (end - start) * (1 - math.exp(-3.0 * t))
    else:
        v = start + (end - start) * t
    if noise:
        v += random.gauss(0.0, noise * abs(end - start))
    return v


def main() -> None:
    print("metrics_big: starting", flush=True)

    with xrun_hook.stage("data_load"):
        time.sleep(0.05)
        xrun_hook.metric("dataset_size", 50000.0, step=0)
        xrun_hook.metric("num_classes", 10.0, step=0)

    with xrun_hook.stage("train"):
        for step in range(N_STEPS):
            metrics = {
                # Lower-is-better trio (loss family) — wide range for log-y
                "loss":      _curve(5.0,  0.01,  step, N_STEPS, 0.05, "exp"),
                "val_loss":  _curve(5.2,  0.05,  step, N_STEPS, 0.06, "exp"),
                "test_loss": _curve(5.5,  0.08,  step, N_STEPS, 0.07, "exp"),
                # Higher-is-better (accuracy family)
                "acc":       _curve(0.10, 0.96, step, N_STEPS, 0.02, "log"),
                "val_acc":   _curve(0.12, 0.91, step, N_STEPS, 0.025, "log"),
                "test_acc":  _curve(0.10, 0.89, step, N_STEPS, 0.03, "log"),
                # F1 family
                "f1":        _curve(0.05, 0.93, step, N_STEPS, 0.02, "log"),
                "val_f1":    _curve(0.05, 0.88, step, N_STEPS, 0.025, "log"),
                # Solo curves
                "lr":        1e-3 * (0.95 ** (step // 10)),
                "grad_norm": _curve(8.0,  0.4,  step, N_STEPS, 0.15, "exp"),
                "throughput": _curve(120.0, 480.0, step, N_STEPS, 0.05, "log"),
                # Per-class IoU (segmentation-like — group by stem `iou_*`)
                "iou_bg":     _curve(0.40, 0.95, step, N_STEPS, 0.02, "log"),
                "iou_class1": _curve(0.10, 0.78, step, N_STEPS, 0.03, "log"),
                "iou_class2": _curve(0.05, 0.71, step, N_STEPS, 0.04, "log"),
                "iou_class3": _curve(0.02, 0.55, step, N_STEPS, 0.05, "log"),
                # Regression-style errors
                "mae":       _curve(2.5,  0.18, step, N_STEPS, 0.05, "exp"),
                "rmse":      _curve(3.1,  0.24, step, N_STEPS, 0.05, "exp"),
            }
            xrun_hook.metrics(metrics, step=step)
            if step % 25 == 0:
                xrun_hook.epoch(step // 25,
                                {"loss": metrics["loss"],
                                 "val_loss": metrics["val_loss"]})
                print(f"step {step:3d}: loss={metrics['loss']:.4f} "
                      f"val_acc={metrics['val_acc']:.3f}", flush=True)
            time.sleep(0.02)

    with xrun_hook.stage("eval"):
        time.sleep(0.05)
        # Final aggregate metrics — single-point curves, no group
        xrun_hook.metric("final_test_acc",       0.892, step=0)
        xrun_hook.metric("final_test_f1_macro",  0.871, step=0)
        xrun_hook.metric("final_test_f1_micro",  0.886, step=0)
        xrun_hook.metric("inference_ms_per_img", 4.7,   step=0)

    xrun_hook.done()
    print("metrics_big: done", flush=True)


if __name__ == "__main__":
    main()
