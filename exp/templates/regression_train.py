"""Шаблон training-скрипта для регрессии.

Структуру (стейджи + батч-метрики через xrun_hook) оставляй, нутро (данные,
модель, train/eval) подменяй на своё. Скрипт работает без torch — это нарочно,
чтобы шаблон запускался везде до подмены.
"""

from __future__ import annotations

import argparse
import json
import math
import random
from pathlib import Path

import xrun_hook


def parse_args() -> argparse.Namespace:
    p = argparse.ArgumentParser()
    p.add_argument("--epochs", type=int, default=5)
    p.add_argument("--lr", type=float, default=1e-3)
    p.add_argument("--seed", type=int, default=0)
    return p.parse_args()


def _fake_train_loss(epoch: int) -> float:
    return 2.0 * math.exp(-0.35 * epoch) + random.uniform(-0.03, 0.03)


def _fake_eval(epoch: int) -> dict[str, float]:
    """Замени на реальный eval. Метрики регрессии: MAE, RMSE, R²."""
    progress = 1 - math.exp(-0.4 * (epoch + 1))
    mae = 0.8 * (1 - progress) + random.uniform(-0.01, 0.01)
    rmse = mae * 1.3
    r2 = 0.95 * progress + random.uniform(-0.005, 0.005)
    return {"val_mae": mae, "val_rmse": rmse, "val_r2": r2}


def main() -> None:
    args = parse_args()
    random.seed(args.seed)

    with xrun_hook.stage("data_load"):
        # TODO: загрузка датасета
        pass

    with xrun_hook.stage("model_init"):
        # TODO: модель, оптимизатор, loss (например MSELoss)
        pass

    best_rmse = float("inf")
    ckpt_dir = Path("checkpoints")
    ckpt_dir.mkdir(exist_ok=True)

    with xrun_hook.stage("train"):
        for ep in range(args.epochs):
            train_loss = _fake_train_loss(ep)
            val = _fake_eval(ep)

            xrun_hook.metrics(
                {"train_loss": train_loss, "lr": args.lr, **val},
                step=ep,
            )
            xrun_hook.epoch(ep, {"val_rmse": val["val_rmse"]})

            if val["val_rmse"] < best_rmse:
                best_rmse = val["val_rmse"]
                # TODO: torch.save(model.state_dict(), ...)
                (ckpt_dir / f"best_e{ep}.json").write_text(
                    json.dumps({"epoch": ep, "val_rmse": best_rmse}, indent=2),
                    encoding="utf-8",
                )

    with xrun_hook.stage("eval"):
        # TODO: финальный test-set eval
        pass

    xrun_hook.done()


if __name__ == "__main__":
    main()
