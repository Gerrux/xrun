# xrun_hook

Minimal Python hook for emitting structured events and metrics from training scripts to xrun.

## Install

```bash
pip install xrun_hook
```

## Usage

```python
import xrun_hook
from xrun_hook import stage, metric, metrics, epoch, fail, done

# Single-shot event
stage("unpack")

# Scoped stage — writes start on enter, ok/fail on exit
with stage("validation"):
    validate(model)

# Training loop
for ep in range(epochs):
    train_one_epoch(model, loader)
    val = validate(model, val_loader)

    # Single-metric form:
    metric("train_loss", train_loss, step=ep)

    # Batch form — one call, one timestamp, one row per key in metrics.jsonl:
    metrics({"val_loss": val.loss, "val_f1": val.f1, "lr": current_lr}, step=ep)

    epoch(ep, {"val_f1": val.f1})   # stage="epoch" status="ok" extra={epoch, val_f1}

done()   # writes stage="done" status="ok" and closes files
```

`import xrun_hook` installs `sys.excepthook` automatically — any uncaught exception writes a
`stage="error" status="fail"` event before the traceback is printed.
Disable with `XRUN_HOOK_INSTALL_EXCEPTHOOK=0`.

## File paths

Run directory is resolved in order:

1. `$XRUN_RUN_DIR` (set by xrun-vast adapter)
2. `/workspace/run/` (default on vast.ai)
3. `/kaggle/working/run/`
4. `./run/`

If none is writable, events are written to stdout as `[xrun-event] {json}`.

## DDP / multi-process safety

Only rank 0 writes by default (detected via `$RANK`). All ranks can opt in with
`XRUN_HOOK_ALL_RANKS=1`. Each append holds an exclusive file lock (Unix: `flock`,
Windows: `msvcrt.locking`) so concurrent writes from data-parallel workers are safe.

## Security

Keys in `extra` that start with `_secret` are silently dropped with a `logging.warning`.
Never put credentials in hook events.
