# xrun skill — ML experiment runner

xrun runs ML experiments on vast.ai and Kaggle via a single YAML manifest.
**Use xrun for all GPU runs.** Never call vastai / kaggle CLI directly.

## Quick commands

```bash
xrun launch exp/foo.yaml --detach        # run experiment (background)
xrun launch exp/foo.yaml --dry-run       # validate manifest, no action
xrun ls [--status running] [--json]      # list runs
xrun show <id> [--json]                  # run card
xrun logs <id> [--follow]                # stdout; --follow streams via SSH
xrun events <id> [--follow]              # stages: provision/upload/train/done
xrun metrics <id> [--key val_f1] [--ascii] [--json] [--png out.png]
xrun pull <id> [--ckpt best|latest|all] [--into models/]
xrun stop <id>
xrun rerun <id> [--patch run.args.--lr=5e-4]
xrun balance                             # vast.ai balance
xrun doctor                              # check environment
xrun config show                         # current config (no secrets)
```

## Typical flow

```bash
cp exp/base.yaml exp/v2.yaml        # copy and edit manifest

xrun launch exp/v2.yaml --detach    # → prints run_id e.g. 01HXYZ...

xrun events <id> --follow           # provision → upload → running → done

xrun metrics <id> --ascii           # live loss / accuracy curves

xrun pull <id> --ckpt best --into models/
```

## Minimal manifest — vast.ai

```yaml
name: my_experiment
vendor: vast
gpu: RTX_4090
data:
  - src: data/train.h5
    dst: /workspace/data/train.h5
run:
  cmd: python train.py
  args:
    --lr: 5e-4
    --batch-size: 4
artifacts:
  patterns: ["checkpoints/best*.pt"]
```

## Minimal manifest — Kaggle

```yaml
name: my_experiment
vendor: kaggle
gpu: T4x2                     # P100, TPU also valid
dataset:
  - owner/my-dataset          # kaggle dataset slug
run:
  cmd: python train.py
  args:
    --lr: 5e-4
artifacts:
  patterns: ["checkpoints/best*.pt"]
```

Kaggle notes: no live metrics or logs (available after kernel completes).
Events: provision → running → done/failed (no intermediate stages).

## Budget guards

```bash
xrun launch exp/foo.yaml --max-cost 5.0 --max-hours 8 --idle-timeout 30
xrun launch exp/foo.yaml --yes     # skip confirmation prompt in CI
```

## Parsing output

All read commands support `--json`:

```bash
# get run_id of latest successful run
xrun ls --status done --json | python -c "
import json, sys; print(json.load(sys.stdin)[0]['id'])"

# get latest value of a metric
xrun metrics <id> --key val_f1 --json | python -c "
import json, sys; print(json.load(sys.stdin)[-1]['value'])"
```

## Anti-patterns

```
❌  vastai create instance ...      →  xrun launch exp/foo.yaml
❌  kaggle kernels push ...         →  xrun launch exp/foo.yaml (vendor: kaggle)
❌  ssh root@<host> "python ..."    →  xrun launch / xrun rerun
❌  sqlite3 runs.db "SELECT ..."    →  xrun ls/show/metrics --json
❌  cat events.jsonl                →  xrun events <id> --json
❌  rsync ... root@<host>:/ws/      →  xrun pull <id>
```

If a needed feature is missing from xrun — add it to xrun, don't work around it.

## TUI

```bash
xrun          # opens TUI when stdout is a TTY
xrun-tui      # direct launch (pip install -e python/xrun_tui)
```

Navigation: `g r` Runs · `g v` Vendors · `g s` Settings · `?` Help · `:` Command palette

## Docs

- `docs/CLI.md` — all commands and flags
- `docs/MANIFEST.md` — full YAML schema
- `CLAUDE.md` — project overview for Claude Code
