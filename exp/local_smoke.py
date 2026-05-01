"""Tiny local-smoke training script — no real training, just exercises the
xrun_hook event/metric pipeline so xrun-local can verify end-to-end."""

import time

import xrun_hook


def main() -> None:
    print("local_smoke: starting", flush=True)
    with xrun_hook.stage("env_ready"):
        time.sleep(0.05)
    print("local_smoke: training", flush=True)
    with xrun_hook.stage("train"):
        for step in range(3):
            xrun_hook.metric("loss", 1.0 / (step + 1), step=step)
            xrun_hook.metric("val_f1", 0.5 + 0.1 * step, step=step)
            time.sleep(0.05)
    xrun_hook.done()
    print("local_smoke: done", flush=True)


if __name__ == "__main__":
    main()
