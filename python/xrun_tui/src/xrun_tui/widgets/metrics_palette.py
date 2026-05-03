"""Helpers for the Metrics view: color palette, grouping, EMA smoothing.

Kept dependency-free so it can be unit-tested without Textual.
"""
from __future__ import annotations

import hashlib

# Tokyo-Night-friendly distinct hues; index by stable hash of metric key so
# the same key keeps the same color across reloads.
PALETTE = (
    "#7aa2f7",  # blue
    "#9ece6a",  # green
    "#e0af68",  # yellow
    "#f7768e",  # red
    "#bb9af7",  # purple
    "#7dcfff",  # cyan
    "#ff9e64",  # orange
    "#c0caf5",  # off-white
)

_LOWER_IS_BETTER_TOKENS = (
    "loss", "err", "nll", "perplexity", "ppl", "mae", "mse", "rmse", "wer", "cer",
)

# Common prefixes that distinguish train/val/test versions of the same metric.
# Order matters: longer first.
_GROUP_PREFIXES = ("train_", "val_", "valid_", "test_", "eval_")


def color_for(key: str) -> str:
    """Deterministic palette pick — same key always maps to same colour."""
    h = int(hashlib.md5(key.encode("utf-8")).hexdigest()[:8], 16)
    return PALETTE[h % len(PALETTE)]


def is_lower_better(key: str) -> bool:
    k = key.lower()
    return any(tok in k for tok in _LOWER_IS_BETTER_TOKENS)


def group_stem(key: str) -> str:
    """Strip a `train_/val_/test_/eval_` prefix.

    `val_loss` → `loss`, `test_acc` → `acc`, `loss` → `loss`.
    """
    k = key.lower()
    for p in _GROUP_PREFIXES:
        if k.startswith(p):
            return key[len(p):]
    return key


def group_keys(keys: list[str]) -> dict[str, list[str]]:
    """Bucket keys by their stem. Preserves first-seen order within each bucket."""
    out: dict[str, list[str]] = {}
    for k in keys:
        out.setdefault(group_stem(k), []).append(k)
    return out


def ema(values: list[float], alpha: float = 0.3) -> list[float]:
    """Exponential moving average smoother. alpha=0.3 ≈ TensorBoard default."""
    if not values:
        return []
    out = [values[0]]
    for v in values[1:]:
        out.append(alpha * v + (1 - alpha) * out[-1])
    return out


def safe_log(values: list[float]) -> list[float]:
    """log10 of positive values; non-positive → smallest positive in series."""
    import math
    positives = [v for v in values if v > 0]
    if not positives:
        return [0.0] * len(values)
    floor = min(positives)
    return [math.log10(v if v > 0 else floor) for v in values]
