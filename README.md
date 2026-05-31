# xrun

[![CI](https://github.com/gerrux/xrun/actions/workflows/ci.yml/badge.svg)](https://github.com/gerrux/xrun/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Version](https://img.shields.io/github/v/release/gerrux/xrun)](https://github.com/gerrux/xrun/releases)

**ML experiment runner** — one YAML manifest → provision GPU → upload data → train → track metrics → pull checkpoints.

Works with **vast.ai** and **Kaggle**. Keeps the full history in a local SQLite database. No third-party tracking service required.

```
xrun launch exp/resnet50.yaml --detach   # kick off on a vast.ai GPU
xrun events <id> --follow                # watch stages: provision → upload → running → done
xrun metrics <id> --ascii                # live loss / accuracy curves
xrun pull <id> --ckpt best               # download the best checkpoint
```

---

## Install

### macOS / Linux

```sh
curl -sSf https://raw.githubusercontent.com/gerrux/xrun/master/install.sh | sh
```

Installs `xrun` to `~/.local/bin/xrun` and installs the Python TUI (`xrun-tui`)
with `pip --user`. Pass `--prefix /usr/local` to change the binary location.

If Python 3.11+ is present but pip is missing, let the installer try
`python -m ensurepip --upgrade`:

```sh
curl -sSf https://raw.githubusercontent.com/gerrux/xrun/master/install.sh | sh -s -- --install-pip
```

For CLI-only install without the TUI:

```sh
curl -sSf https://raw.githubusercontent.com/gerrux/xrun/master/install.sh | sh -s -- --no-tui
```

### Windows (PowerShell)

```powershell
irm https://raw.githubusercontent.com/gerrux/xrun/master/install.ps1 | iex
```

Installs `xrun.exe` to `%LOCALAPPDATA%\xrun\bin\xrun.exe`, adds it to your user
`PATH`, and installs the Python TUI (`xrun-tui`) with `pip --user`.

If Python 3.11+ is present but pip is missing:

```powershell
& ([scriptblock]::Create((irm 'https://raw.githubusercontent.com/gerrux/xrun/master/install.ps1'))) -InstallPip
```

For CLI-only install without the TUI:

```powershell
& ([scriptblock]::Create((irm 'https://raw.githubusercontent.com/gerrux/xrun/master/install.ps1'))) -NoTui
```

### Specific version

```sh
curl -sSf https://raw.githubusercontent.com/gerrux/xrun/master/install.sh | sh -s -- --version v0.7.1
```

### From source

```sh
cargo install --git https://github.com/gerrux/xrun --branch master xrun-cli
```

### Python TUI

The install scripts install the TUI by default because `xrun` without arguments
opens it on a TTY. To install or repair only the TUI:

```sh
curl -sSf https://raw.githubusercontent.com/gerrux/xrun/master/install.sh | sh -s -- --tui-only
```

```powershell
& ([scriptblock]::Create((irm 'https://raw.githubusercontent.com/gerrux/xrun/master/install.ps1'))) -TuiOnly
```

From a local clone during development:

```sh
pip install -e python/xrun_tui
```

After install, `xrun` without arguments opens the TUI automatically when stdout is a TTY.

### Updates

On interactive startup (`xrun` or `xrun tui`), xrun checks GitHub Releases for a
newer version. If one is available, it shows a confirmation prompt before
running the official installer.

```sh
xrun update --check   # check only
xrun update           # ask, then install
xrun update --yes     # install without prompt
```

Use `xrun update --no-tui` to update only the Rust CLI. Set
`XRUN_NO_UPDATE_CHECK=1` to disable the startup check in scripted environments.

### Agent skill (optional)

Teaches Codex or Claude Code how to use xrun correctly — which commands to call, how to parse output, what to avoid.

For a repository-local install, run this inside the project that uses xrun:

```sh
xrun install skill --codex   # writes .codex/skills/xrun/SKILL.md + AGENTS.md
xrun install skill --claude  # writes .claude/skills/xrun/SKILL.md + CLAUDE.md
```

The legacy global Claude installer is still available:

```sh
# macOS / Linux — install binary + skill together
curl -sSf https://raw.githubusercontent.com/gerrux/xrun/master/install.sh | sh -s -- --with-skill

# skill only (if xrun is already installed)
curl -sSf https://raw.githubusercontent.com/gerrux/xrun/master/install.sh | sh -s -- --skill-only
```

```powershell
# Windows — install binary + skill together
& ([scriptblock]::Create((irm 'https://raw.githubusercontent.com/gerrux/xrun/master/install.ps1'))) -WithSkill

# skill only
& ([scriptblock]::Create((irm 'https://raw.githubusercontent.com/gerrux/xrun/master/install.ps1'))) -SkillOnly
```

The global installer writes `~/.claude/skills/xrun/SKILL.md`.

---

## Quick start

```sh
# 1. Check your environment
xrun doctor

# 2. Set your vast.ai API key (or use the TUI: xrun → V → i)
xrun config set vast.api_key <YOUR_KEY>

# 3. Create a manifest
cp exp/base.yaml exp/my_run.yaml   # edit gpu, data, run.cmd, …

# 4. Launch (detached background run)
xrun launch exp/my_run.yaml --detach
#  → prints run ID, e.g. 01J2KX...

# 5. Follow stages
xrun events <id> --follow
#  provision → upload → running → done

# 6. Watch metrics
xrun metrics <id> --ascii

# 7. Retrieve the best checkpoint
xrun pull <id> --ckpt best --into models/
```

---

## Commands

| Command | Description |
|---------|-------------|
| `xrun launch <manifest>` | Provision → upload → exec; `--detach` returns immediately |
| `xrun ls` | List runs; `--status running\|done\|failed`, `--json` |
| `xrun show <id>` | Full run card from local DB |
| `xrun events <id>` | Stage timeline; `--follow` polls until terminal |
| `xrun logs <id>` | stdout log; `--follow` streams via SSH |
| `xrun metrics <id>` | Metrics table/chart; `--ascii`, `--json`, `--png` |
| `xrun pull <id>` | Download checkpoints; `--ckpt best\|latest\|all` |
| `xrun stop <id>` | Graceful stop → pull artifacts → destroy instance |
| `xrun rerun <id>` | Re-run with optional `--patch run.args.--lr=5e-4` |
| `xrun balance` | vast.ai account balance |
| `xrun doctor` | Check credentials, CLI tools, connectivity |
| `xrun config` | `init \| show \| set <key> <val>` |
| `xrun gc` | Remove orphan instances |

All read commands support `--json` for scripting.  
Full reference: [`docs/CLI.md`](docs/CLI.md)

---

## Manifest

```yaml
name: resnet50_baseline
vendor: vast          # or: kaggle
gpu: RTX_4090

# budget guards (auto-destroy when exceeded)
max_cost: 5.0         # USD
max_hours: 8

data:
  - src: data/train.h5
    dst: /workspace/data/train.h5

run:
  cmd: python train.py
  args:
    --lr: 5e-4
    --epochs: 30
    --batch-size: 16

artifacts:
  patterns:
    - "checkpoints/best*.pt"
    - "logs/metrics.json"
```

Full schema: [`docs/MANIFEST.md`](docs/MANIFEST.md)

---

## TUI

```sh
xrun        # opens TUI if stdout is a TTY
xrun-tui    # direct launch (after pip install)
```

**Screens** (chord navigation with `g→X`):

| Key | Screen |
|-----|--------|
| `g d` | Dashboard — burn rate, active runs, runway warning |
| `g r` | Runs — filterable list with live status |
| `g i` | Instances — raw vast.ai / Kaggle instances |
| `g v` | Vendors — API key management, balance |
| `g l` | Launch — manifest picker |
| `g s` | Settings — config editor |
| `?`   | Help |
| `:`   | Command palette |

Run detail opens on `Enter` and has tabs: **Stages · Logs · Metrics · Artifacts · Manifest**

---

## Training hook

Add `xrun_hook` to your training script to emit structured events and metrics:

```python
# pip install git+https://github.com/gerrux/xrun.git#subdirectory=python/xrun_hook
from xrun_hook import XRunHook

hook = XRunHook()
hook.event("epoch_start", stage="train", msg=f"epoch {epoch}")

for epoch in range(epochs):
    loss = train_one_epoch(...)
    hook.metric("train_loss", loss, step=epoch)
    hook.metric("val_f1",   eval_f1, step=epoch)

hook.event("done", stage="train")
```

Metrics appear in `xrun metrics <id>` and the TUI in real time.

---

## Budget guards

```sh
xrun launch exp/foo.yaml \
  --max-cost 5.0      \  # auto-destroy after $5 spent
  --max-hours 8       \  # auto-destroy after 8 hours
  --idle-timeout 30      # auto-destroy if GPU idle for 30 minutes
```

The background poll-daemon monitors spend and destroys the instance automatically, writing `auto_destroyed_reason` to the local DB.

---

## Architecture

```
┌─────────────────────────────────────────────────────┐
│                 Local machine                        │
│                                                      │
│  xrun (Rust CLI)  ──spawn──▶  xrun-tui (Python)     │
│        │                           │                 │
│        ▼                           ▼                 │
│     xrun-core  ◀──────────  SQLite runs.db           │
│        │                                             │
│   ┌────┴────┐   ┌────────────┐                       │
│   │xrun-vast│   │xrun-kaggle │                       │
│   └────┬────┘   └─────┬──────┘                       │
└────────┼──────────────┼──────────────────────────────┘
         ▼              ▼
    vast.ai GPU    Kaggle Kernel
    /workspace/    output/
```

- **`xrun-cli`** — command routing, user-facing UX
- **`xrun-core`** — manifest types, SQLite schema, vendor trait
- **`xrun-vast`** — vast.ai: provision, SSH upload, exec, poll, transfer
- **`xrun-kaggle`** — Kaggle: kernel push, status poll, output download
- **`xrun-poller`** — background daemon: events/metrics → SQLite, budget enforcement
- **`xrun-mlflow`** — optional MLflow REST mirror for metric storage
- **`xrun-tui`** (Python) — Textual TUI, reads SQLite, calls CLI via subprocess

---

## Requirements

**For the CLI:**
- [vastai CLI](https://github.com/vast-ai/vast-python) (`pip install vastai`) — for vast.ai runs
- [kaggle CLI](https://github.com/Kaggle/kaggle-api) (`pip install kaggle`) — for Kaggle runs
- SSH key configured for vast.ai (checked by `xrun doctor`)

**For the TUI:**
- Python ≥ 3.11
- pip for that Python. The install scripts can try `ensurepip` via
  `--install-pip` / `-InstallPip`.
- For local development: `pip install -e python/xrun_tui`

**Building from source:**
- Rust stable (≥ 1.75) — [install via rustup](https://rustup.rs)

---

## Documentation

| File | Contents |
|------|----------|
| [`docs/CLI.md`](docs/CLI.md) | All subcommands, flags, exit codes |
| [`docs/MANIFEST.md`](docs/MANIFEST.md) | Full YAML schema with examples |
| [`docs/TUI.md`](docs/TUI.md) | Screens, key bindings, widgets |
| [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) | Components and data flow |
| [`docs/EVENTS.md`](docs/EVENTS.md) | events.jsonl protocol + Python hook |
| [`docs/STATE.md`](docs/STATE.md) | SQLite schema |
| [`docs/ROADMAP.md`](docs/ROADMAP.md) | Version history and backlog |
| [`CHANGELOG.md`](CHANGELOG.md) | Release notes |

---

## Status: v0.7.1

- ✅ vast.ai: provision, upload, exec, poll, pull, destroy
- ✅ Kaggle: kernel push, status poll, output download
- ✅ Live events and metrics in SQLite
- ✅ MLflow mirror (metrics + UI link)
- ✅ Budget guards (caps, auto-destroy, spend dashboard)
- ✅ Python Textual TUI: 16 screens, chord navigation, Tokyo Night theme
- ✅ `xrun events --follow`, `xrun logs --follow`
- ✅ Install scripts for macOS, Linux, Windows
- ✅ Agent skill (`xrun install skill --codex` / `--claude`)
- ✅ `xrun sweep` — hyperparameter grid

---

## License

MIT — see [LICENSE](LICENSE)
