"""Шаблон training-скрипта для многоклассовой классификации.

Заменяй внутренности (загрузку данных, модель, train/eval-петли) — структура
со стейджами и батч-логированием метрик через xrun_hook оставляй.

Этот файл специально написан без torch, чтобы шаблон запускался везде. Когда
будешь адаптировать — просто замени `_fake_train_step` / `_fake_eval` на
реальные.
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
    p.add_argument("--num-classes", type=int, default=4)
    p.add_argument("--seed", type=int, default=0)
    return p.parse_args()


def _fake_train_step(epoch: int, lr: float) -> float:
    """Замени на реальный training step. Возвращает loss за эпоху."""
    return 1.5 * math.exp(-0.4 * epoch) + random.uniform(-0.02, 0.02)


def _fake_eval(epoch: int, num_classes: int) -> dict[str, float]:
    """Замени на реальную валидацию. Должна вернуть словарь метрик."""
    progress = 1 - math.exp(-0.5 * (epoch + 1))
    base = 0.4 + 0.5 * progress
    jitter = lambda: random.uniform(-0.01, 0.01)  # noqa: E731
    return {
        "val_loss": 1.2 * (1 - progress) + jitter(),
        "val_acc": base + jitter(),
        "val_f1_macro": base - 0.02 + jitter(),
        "val_precision_macro": base - 0.01 + jitter(),
        "val_recall_macro": base - 0.03 + jitter(),
    }


def main() -> None:
    args = parse_args()
    random.seed(args.seed)

    with xrun_hook.stage("data_load"):
        # TODO: загрузить датасет, train_loader / val_loader
        pass

    with xrun_hook.stage("model_init"):
        # TODO: построить модель, оптимизатор, loss
        pass

    best_f1 = -1.0
    ckpt_dir = Path("checkpoints")
    ckpt_dir.mkdir(exist_ok=True)

    with xrun_hook.stage("train"):
        for ep in range(args.epochs):
            train_loss = _fake_train_step(ep, args.lr)
            val = _fake_eval(ep, args.num_classes)

            # Батч-лог: одна точка timestamp на всё, по одной строке на ключ
            xrun_hook.metrics(
                {"train_loss": train_loss, "lr": args.lr, **val},
                step=ep,
            )
            xrun_hook.epoch(ep, {"val_f1_macro": val["val_f1_macro"]})

            if val["val_f1_macro"] > best_f1:
                best_f1 = val["val_f1_macro"]
                # TODO: torch.save(model.state_dict(), ckpt_dir / f"best_e{ep}.pt")
                (ckpt_dir / f"best_e{ep}.json").write_text(
                    json.dumps({"epoch": ep, "val_f1_macro": best_f1}, indent=2),
                    encoding="utf-8",
                )

    with xrun_hook.stage("eval"):
        # TODO: финальный test-set eval, отчёт по классам
        pass

    xrun_hook.done()


if __name__ == "__main__":
    main()
