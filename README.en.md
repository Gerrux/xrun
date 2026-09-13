<p align="center">
  <img src="docs/brand/mark.png" width="88" alt="">
</p>

<h1 align="center">xrun</h1>

<p align="center">One YAML manifest — from a rented GPU to the best checkpoint.</p>

<p align="center"><a href="https://gerrux.github.io/xrun/">Site</a> · <a href="https://github.com/Gerrux/xrun/releases">Download</a> · <a href="docs/">Docs</a> · <a href="CHANGELOG.md">Changelog</a></p>

<p align="center"><a href="README.md">Русский</a> · <b>English</b></p>
<p align="center">
  <a href="https://github.com/Gerrux/xrun/releases/latest"><img alt="" src="https://img.shields.io/github/v/release/Gerrux/xrun?style=flat-square&labelColor=1A1B26&color=7AA2F7"></a>
  <a href="https://github.com/Gerrux/xrun/actions/workflows/ci.yml"><img alt="" src="https://img.shields.io/github/actions/workflow/status/Gerrux/xrun/ci.yml?branch=master&style=flat-square&labelColor=1A1B26&label=ci"></a>
  <a href="LICENSE"><img alt="" src="https://img.shields.io/github/license/Gerrux/xrun?style=flat-square&labelColor=1A1B26&color=9ECE6A"></a>
  <img alt="" src="https://img.shields.io/badge/Windows%20%7C%20macOS%20%7C%20Linux-1A1B26?style=flat-square">
  <img alt="" src="https://img.shields.io/badge/vast.ai%20%7C%20Kaggle%20%7C%20SSH%20%7C%20local-1A1B26?style=flat-square">
</p>

**An ML experiment runner.** One manifest describes a run end to end: where
the GPU comes from, what to upload, what to train and what to bring back.
`xrun` rents the instance, uploads the data, starts training, follows stages
and metrics and pulls the checkpoints — and once it is all over, or spend has
hit the ceiling, it destroys the instance by itself.

A Rust core in workspace crates with the `xrun` CLI on top, and a Python
Textual TUI above that. Four vendors: vast.ai, Kaggle, your own server over
SSH and the local machine. The full run history lives in a local SQLite
database — no third-party tracking service and no account are needed for it;
MLflow and W&B plug in as mirrors if you want their charts.

The docs under `docs/` are in Russian, the project's source language; the
command reference reads fine with a translator, and the commands themselves
are the same.

```bash
xrun launch exp/resnet50.yaml --detach --max-cost 5   # rent a GPU and walk away
xrun events <id> --follow                             # provision → upload → running → done
xrun metrics <id> --key val_f1 --ascii                # the curve, right in the terminal
xrun pull <id> --ckpt best --into models/             # bring back the best checkpoint
```

## The one promise

An instance that is billing is never left unattended. Everything else in the
architecture follows from that.

`xrun launch --detach` leaves a background poller behind. It pulls events and
metrics, counts the spend and destroys the instance when the run finished,
failed, went silent, hit a metric plateau or ran into `--max-cost` /
`--max-hours`. If it could not destroy it, it sends a push: "instance still
billing".

That leaves one hole the poller cannot report: its own death. `xrun watchdog`
closes it — from the scheduler every five minutes and from the TUI every
minute. The poller writes a heartbeat on every tick; no heartbeat while the
instance is alive means the watchdog notifies and respawns the poller. It also
catches instances no run record knows about.

```
                     done / failed / plateau / ceiling ──► pull → destroy → push
poller (tick) ───────┤
                     could not destroy ──────────────────► push "still billing"

watchdog (5 min) ────► no heartbeat, instance alive ─────► push → respawn poller
```

How it works inside — [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).

## Install

macOS and Linux:

```bash
curl -sSf https://raw.githubusercontent.com/Gerrux/xrun/master/install.sh | sh
```

Windows (PowerShell):

```powershell
irm https://raw.githubusercontent.com/Gerrux/xrun/master/install.ps1 | iex
```

The script drops the `xrun` binary (`~/.local/bin` or `%LOCALAPPDATA%\xrun\bin`)
and installs the TUI with `pip --user` — that needs Python 3.11+. `--no-tui` /
`-NoTui` installs the CLI only, `--version v0.8.0` pins a release,
`--install-pip` / `-InstallPip` tries `ensurepip` when pip is missing.

From source:

```bash
cargo install --git https://github.com/Gerrux/xrun --branch master xrun-cli
pip install -e python/xrun_tui
```

`xrun` updates itself: on interactive start it checks the releases and asks
before installing. `xrun update --check` only checks,
`XRUN_NO_UPDATE_CHECK=1` turns the check off in scripts.

## First run

No credentials, no GPU and no data — just to make sure the chain is alive:

```bash
xrun doctor
xrun launch exp/templates/quickstart.yaml
xrun metrics <id> --ascii
```

Then the vendor keys. The easiest way is the wizard: `xrun` with no arguments
opens the TUI, and on first start — the setup wizard (`xrun init`). It sets up
notifications too. Ready-made manifests for classification, regression and
Kaggle live in [exp/templates](exp/templates/README.md).

## Manifest

```yaml
name: resnet50_baseline
vendor: vast                    # vast | kaggle | ssh | local

vast:
  image: pytorch/pytorch:2.4.1-cuda12.1-cudnn9-devel
  gpu: { type: "RTX 4090", count: 1 }
  price: { max_per_hour: 0.55 }

data:
  - src: data/train.h5
    dst: /workspace/data/train.h5

run:
  cmd: python train.py
  args:
    --lr: 5e-4
    --epochs: 30

artifacts:
  patterns: ["checkpoints/best*.pt"]

policy:
  on_idle_minutes: 30           # no output for half an hour — destroy
  early_stop:                   # metric plateau: pull the best, then destroy
    metric: val_f1
    patience: 5
```

On launch a copy of the manifest is stored with the run record, and its hash
becomes part of the run's identity: edit the original freely,
`xrun rerun <id>` reproduces exactly what ran. Hyperparameter search is
`xrun sweep exp/base.yaml --grid run.args.--lr=1e-3,5e-4 --launch` — it
materializes one manifest per variant instead of templating a single one.

Full schema — [docs/MANIFEST.md](docs/MANIFEST.md).

## Hook in the training script

`xrun_hook` writes stages and metrics to `events.jsonl` / `metrics.jsonl`,
where the poller picks them up. On Kaggle it is embedded into the kernel
automatically.

```python
from xrun_hook import stage, metric, metrics, done, notify

with stage("train"):
    for ep in range(epochs):
        loss = train_one_epoch(model, loader)
        metric("train_loss", loss, step=ep)
        metrics({"val_loss": v.loss, "val_f1": v.f1}, step=ep)

notify("training", f"best val_f1 = {best:.3f}")
done()
```

An uncaught exception is recorded as an `error` event by the hook itself. The
full protocol — [docs/EVENTS.md](docs/EVENTS.md).

## TUI

`xrun` with no arguments in a terminal opens the TUI. Navigation is by chords
starting with `g`:

| | |
| --- | --- |
| `g d` | Dashboard — hourly burn, active runs, how long the balance lasts |
| `g r` | Runs — list with live status; `Enter` — stages, logs, metrics, artifacts, manifest |
| `g i` | Instances — what is rented from vendors right now |
| `g v` | Vendors — keys and balance |
| `g l` | Launch — pick a manifest and start it |
| `g n` | Notifications — push channels, test, watchdog registration |
| `g h` | Doctor — environment check |
| `?` · `Ctrl+P` | help · command palette |

Screens and bindings — [docs/TUI.md](docs/TUI.md).

## Notifications

ntfy, Telegram, a webhook (Slack, Discord) and desktop toasts. The poller
sends: run finished, failed or went idle; 50 % and 80 % of `--max-cost` spent;
instance destroyed by a ceiling or failed to destroy; NaN or an exploding
loss; a plateau stop — and whatever the script sends via
`xrun_hook.notify(...)`. Reply `/stop <id>` to the Telegram bot to kill a run.

```bash
xrun config set notify.channels ntfy,desktop
xrun config set ntfy.topic my-random-topic
xrun notify test                      # exit 1 — the channel is broken, fix it before launching
xrun watchdog schedule --install      # every 5 minutes via schtasks / crontab
```

The TUI does the same on the `g n` screen: the ntfy topic is generated, the
Telegram chat id is detected with one button.

## Principles

- **State is local.** One SQLite database per machine, shared by all projects.
  Whatever the TUI can do, the CLI can do too, and every read command has
  `--json`: a script and an agent see what a human sees.
- **A ceiling is cheaper than a bill.** Budget, idleness and plateau are
  checked on every poller tick, not after the fact. A failed instance destroy is
  not a log line but a push.
- **Credentials do not live in the repository.** Only in `credentials.toml`
  in the user's config directory, never in a manifest; the instance receives a
  copy of the manifest without them.
- **One manifest is one self-contained file.** No `include`, no `extends`, no
  templating: duplication beats a hidden hierarchy.
- **Agents get the same commands.** `xrun install skill --claude` / `--codex`
  teaches Claude Code and Codex to use `xrun` instead of hand-rolled `vastai`
  and `ssh`.

## Documentation

| | |
| --- | --- |
| [CLI](docs/CLI.md) | every subcommand, flags, machine output, exit codes |
| [Manifest](docs/MANIFEST.md) | the full YAML schema with examples for each vendor |
| [Architecture](docs/ARCHITECTURE.md) | components, launch data flow, poller model, failures |
| [Events and metrics](docs/EVENTS.md) | the `events.jsonl` / `metrics.jsonl` protocol and `xrun_hook` |
| [State](docs/STATE.md) | SQLite schema, migrations, backup |
| [TUI](docs/TUI.md) | screens, bindings, themes |
| [Agent skill](docs/SKILL.md) | what the skill does and does not do |
| [Roadmap](docs/ROADMAP.md) | version history and what comes next |

## Contributing

The most valuable thing is a report "ran it on a live vendor, here is what
happened": each of them has an API with its own habits, and half of the fixes
in the history came exactly that way. The rest is in
[CONTRIBUTING.md](CONTRIBUTING.md). Do not file a credential leak, or a way to
leave an instance billing unnoticed, as a public issue:
[SECURITY.md](SECURITY.md).

## License

[MIT](LICENSE).
