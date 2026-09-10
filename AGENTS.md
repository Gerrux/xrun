# AGENTS.md

## Project Shape

- `xrun` is a Rust CLI plus separate Python Textual TUI for ML experiment runs: one YAML manifest provisions a vendor, uploads data, runs training, polls events/metrics, and records state in SQLite.
- Rust workspace crates: `xrun-cli` is the binary, `xrun-core` owns manifests/DB/vendor traits, `xrun-poller` owns polling, and vendor adapters live in `xrun-vast`, `xrun-kaggle`, `xrun-local`, `xrun-ssh`; `xrun-mlflow`/`xrun-wandb` are metric sinks.
- `crates/xrun-tui` is legacy Rust TUI behind a feature flag; the real TUI is `python/xrun_tui` and installs the `xrun-tui` binary.
- `python/xrun_hook` is a separate Python package used by training code to emit `events.jsonl` and `metrics.jsonl`.

## Commands To Trust

- Full Rust verification, matching CI order: `cargo fmt --all -- --check`, then `cargo clippy --workspace --all-targets -- -D warnings`, then `cargo test --workspace`.
- Build only the CLI release artifact with `cargo build --release --package xrun-cli`.
- Focus Rust tests with normal Cargo filters, for example `cargo test --package xrun-cli <test_name>` or `cargo test --workspace <test_name>`.
- Python hook tests run from `python/xrun_hook` with `python -m pytest tests/ -q`; CI installs `python/xrun_hook[dev]` first.
- Install the Python TUI locally with `pip install -e python/xrun_tui`; it requires Python >=3.11. The hook package requires Python >=3.9 and has dev extras.
- On Linux CI, `libfontconfig1-dev` is installed before Rust checks because chart rendering uses `plotters` font support.

## CLI And Runtime Gotchas

- `xrun` with no args on a TTY shells out to the Python `xrun-tui` binary; `xrun tui` uses the legacy Rust TUI path and is not the default user flow.
- Read commands generally support `--json`; prefer `xrun ls/show/events/metrics --json` over reading SQLite or raw run files directly.
- `xrun launch --detach` spawns hidden `xrun __poll-daemon <run-id>`; pollers write events/metrics to SQLite and can be resumed/reconciled with `xrun resume` or `xrun fix-status`.
- `xrun launch --dry-run` validates and prints a plan without touching the DB/vendor; use it before any billable or remote launch when changing manifest behavior.
- Non-TTY billable launches may need `--yes`; budget caps are exposed as `--max-cost`, `--max-hours`, and `--idle-timeout`.
- Windows matters: subprocess code is expected to avoid extra console windows, and Windows cannot replace an open `xrun.exe`, so stale `running` rows may need `xrun fix-status`.

## Vendor Workflows

- Use xrun commands, not raw vendor CLIs: launch with `xrun launch`, pull with `xrun pull`, inspect with `xrun events/logs/metrics`, and clean orphans with `xrun gc`.
- `vendor: local` runs `run.cmd` as a host subprocess and is the zero-credential smoke path; `exp/templates/quickstart.yaml` is the safest first launch.
- Local manifests are shell-sensitive: Unix uses `bash -c`/`sh -c`; Windows prefers `pwsh` but may fall back to PowerShell 5.1, where `&&` is invalid.
- `vendor: ssh` requires preconfigured credentials, SSH key auth, and Unix tools on the remote (`rsync`, `bash`, `tail`, `wc`, optionally `nvidia-smi`); destroy only kills the run process, not the machine.
- Kaggle live telemetry during a running kernel depends on configured `mlflow.url`; without MLflow expect limited synthetic status until output is collected.
- For Kaggle templates, do not reinstall heavy packages like Torch unless a quick import/CUDA smoke proves it is necessary; the templates warn that this can waste 15-20 minutes.

## Manifests And Templates

- Full schema is in `docs/MANIFEST.md`; CLI reference is in `docs/CLI.md`; architecture and DB lifecycle are in `docs/ARCHITECTURE.md` and `docs/STATE.md`.
- `exp/templates/quickstart.yaml` is local and zero-config; `classification.yaml` and `regression.yaml` are local skeletons; `kaggle_smoke.yaml` and `kaggle_classification.yaml` exercise Kaggle + live telemetry.
- Prefer copying a template into `exp/` and editing `cmd`, `args`, `data`, and `artifacts` over inventing manifest structure from memory.
- `xrun init-manifest` generates valid YAML with `TODO_` placeholders for a chosen vendor/sink combination.

## Credentials And Secrets

- Never read or print `credentials.toml`, `.env`, `~/.kaggle/kaggle.json`, `~/.ssh/id_*`, `*.key`, or `*.pem`; ask for redacted output if debugging config.
- Do not run commands that expose secrets, including `xrun config show --secrets`, verbose doctor modes that print credentials, or environment dumps filtered for keys.
- Config and credentials live outside the repo (`~/.config/xrun/` on Linux, `~/Library/Application Support/xrun/` on macOS, `%APPDATA%\xrun\` on Windows); do not create or commit them inside the workspace.
- If the user provides a key in chat, write it through stdin with `xrun init --non-interactive --mark-completed --vast-key -` or `--kaggle-token -`, then do not read it back.
- For tests that need a key literal, use fake values such as `test-key-abc`.

## First-Run Handling

- If asked to "run something" on an unconfigured setup, start with `xrun doctor --json` and `xrun launch exp/templates/quickstart.yaml`.
- Do not start the interactive `xrun init` wizard from a non-TTY harness; ask the user to run `xrun init` in a real terminal, then continue with `xrun doctor --json` and the launch.
- If a remote vendor is requested and credentials are missing, stop before vendor launch unless the user provides credentials or confirms they completed `xrun init`.
