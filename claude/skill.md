# xrun skill — ML experiment runner

xrun runs ML experiments via a single YAML manifest.
Vendors: `local` (host subprocess, no creds), `ssh` (your own box), `vast`
(vast.ai), `kaggle`. **Use xrun for all runs** — never call `vastai` /
`kaggle` / `ssh` / `rsync` directly.

## First-run decision tree (READ THIS FIRST)

When the user asks for an experiment, before launching anything:

```bash
xrun doctor --json                # exit 0 = healthy core
xrun config show                  # what's configured (no secrets)
```

Then branch on what the user actually wants:

| User asks for…           | What to do                                                       |
|--------------------------|------------------------------------------------------------------|
| "try it" / "smoke test"  | `xrun launch exp/templates/quickstart.yaml` — zero config        |
| local CPU/GPU run        | copy a `vendor: local` template, edit, launch                    |
| vast.ai / kaggle / ssh   | check creds first (below); if missing → ask user to run wizard   |

**If the user wants a vendor that needs creds and they aren't set:**
do NOT try to write `credentials.toml` yourself, and do NOT try to launch
the wizard from a non-interactive agent shell - `xrun init` requires a TTY
and exits immediately under non-TTY harnesses.

Tell the user, in plain text, to **open a separate terminal** and run:

```
xrun init
```

That gives the interactive Textual wizard. After it exits, the user comes
back to the agent; you re-run `xrun doctor --json` and proceed.

If the user pastes a key into chat and asks the agent to set it, prefer the
non-interactive form (no TTY needed, key never echoed back):

```bash
printf '%s' "$KEY_FROM_USER" | xrun init --non-interactive --mark-completed --vast-key -
# or, kaggle JWT
printf '%s' "$KEY"          | xrun init --non-interactive --mark-completed --kaggle-token -
```

Use `printf` (not `echo`) and pass via stdin with `-`; never put the key on
the argv. After setting, do not read it back. Never run `xrun config show
--secrets`, `cat ~/.config/xrun/credentials.toml`, or any equivalent — creds
must stay out of the transcript.

## Quick commands

```bash
xrun launch exp/foo.yaml --detach        # run experiment (background)
xrun launch exp/foo.yaml --dry-run       # validate manifest, no action
xrun ls [--status running] [--json]      # list runs
xrun show <id> [--json]                  # run card
xrun logs <id> [--follow]                # stdout; --follow streams via SSH
xrun events <id> [--follow]              # stages: provision/upload/train/done
xrun metrics <id> [--key val_f1] [--ascii] [--json] [--png out.png] [--per-key]
xrun pull <id> [--ckpt best|latest|all] [--into models/]
xrun stop <id>
xrun rerun <id> [--patch run.args.--lr=5e-4]
xrun sweep <manifest> --grid run.args.--lr=1e-3,5e-4 --launch --detach --yes
xrun fix-status [<id>]                   # heal stuck running rows when poller died
xrun balance                             # vast.ai balance
xrun doctor [--manifest exp/foo.yaml]    # env + pre-flight
xrun config show                         # current config (no secrets)
```

## Templates — start here, don't write manifests from scratch

| Template                              | Vendor  | Needs creds | Use for                              |
|---------------------------------------|---------|-------------|--------------------------------------|
| `exp/templates/quickstart.yaml`       | local   | no          | smoke-test xrun itself               |
| `exp/templates/classification.yaml`   | local   | no          | multiclass classification skeleton   |
| `exp/templates/regression.yaml`       | local   | no          | regression skeleton                  |

Copy → rename → edit `name`, `run.cmd`, `run.args`, `data`, `artifacts`,
optionally bump `vendor` to `vast`/`kaggle`/`ssh`. Then `xrun launch`.

## Typical flow

```bash
cp exp/templates/classification.yaml exp/v2.yaml   # copy + edit
xrun doctor --manifest exp/v2.yaml                 # pre-flight
xrun launch exp/v2.yaml --detach                   # → prints run_id
xrun events <id> --follow                          # provision → ... → done
xrun metrics <id> --key val_f1 --ascii
xrun pull <id> --ckpt best --into models/
```

## Minimal manifest — local (zero config)

```yaml
name: my_local
vendor: local
local:
  gpu: cpu                       # or "cuda:0" for GPU on host
run:
  cmd: python train.py
  args:
    --epochs: 5
artifacts:
  patterns: ["checkpoints/best*.pt"]
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
gpu: T4x2                        # P100, TPU also valid
dataset:
  - owner/my-dataset             # kaggle dataset slug
run:
  cmd: python train.py
  args:
    --lr: 5e-4
artifacts:
  patterns: ["checkpoints/best*.pt"]
```

Kaggle: no live metrics or logs (available after kernel completes).
Stages: provision → running → done/failed (no intermediates).

## Budget guards (vast.ai)

```bash
xrun launch exp/foo.yaml --max-cost 5.0 --max-hours 8 --idle-timeout 30
xrun launch exp/foo.yaml --yes      # skip billable confirm in scripts
```

## Parsing output — always prefer `--json`

```bash
# latest successful run id
xrun ls --status done --json | python -c "import json,sys; print(json.load(sys.stdin)[0]['id'])"

# last value of a metric
xrun metrics <id> --key val_f1 --json | python -c "import json,sys; print(json.load(sys.stdin)[-1]['value'])"

# pre-flight a manifest, fail fast on schema/cred errors
xrun doctor --manifest exp/foo.yaml --json
```

## Recipes

### Run a sweep
```bash
xrun sweep exp/base.yaml \
  --grid run.args.--lr=1e-3,5e-4,1e-4 \
  --grid run.args.--batch-size=4,8 \
  --launch --detach --yes --json
```

### Stuck `running` row (poller died)
```bash
xrun fix-status            # all stuck rows
xrun fix-status <id>       # one
```

### Zero-config sanity check (use when user reports "xrun broken")
```bash
xrun launch exp/templates/quickstart.yaml
xrun events <last_id>
```

## Anti-patterns

```
❌  vastai create instance ...      →  xrun launch exp/foo.yaml
❌  kaggle kernels push ...         →  xrun launch exp/foo.yaml (vendor: kaggle)
❌  ssh root@<host> "python ..."    →  xrun launch / xrun rerun
❌  rsync ... root@<host>:/ws/      →  xrun pull <id>
❌  sqlite3 runs.db "SELECT ..."    →  xrun ls/show/metrics --json
❌  cat events.jsonl                →  xrun events <id> --json
❌  cat ~/.config/xrun/credentials.toml  → ask user; don't read creds
❌  xrun config show --secrets      →  never; secrets must not enter transcript
```

If a feature is missing — add it to xrun, don't work around it.

## TUI

```bash
xrun                  # opens TUI when stdout is a TTY
xrun-tui              # direct launch (requires pip install -e python/xrun_tui)
xrun init             # first-run wizard (TUI), or non-interactive flags
```

Navigation: `g r` Runs · `g v` Vendors · `g s` Settings · `g h` Doctor ·
`?` Help · `:` Command palette · `q`/`Esc` back/exit.

In non-interactive agent shells, do **not** try to force `xrun` or
`xrun init` through shell escapes - the embedded shell has no TTY and the
TUI exits immediately. Ask the user to open a separate terminal window and
run `xrun` / `xrun init` themselves.

## Docs

- `docs/CLI.md` — all commands and flags
- `docs/MANIFEST.md` — full YAML schema
- `docs/EVENTS.md` — events.jsonl protocol + xrun_hook
- `AGENTS.md` / `CLAUDE.md` - project overview for the active agent harness
