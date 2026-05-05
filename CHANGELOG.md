# Changelog

All notable changes to this project are documented in this file.

Format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
Versions follow [Semantic Versioning](https://semver.org/).

---

## [Unreleased]

---

## [0.7.0] — 2026-05-05

Pluggable metric sinks. WandB joins MLflow as a first-class fan-out
target; the poller now mirrors one xrun run to N tracking servers in
parallel, with per-sink failure isolation. Comet ML and TensorBoard
plug into the same trait in v0.8 with no further refactor.

Skipped 0.6.x — the field-feedback ramp was already absorbed into 0.5.4
and the next milestone deserves a clear minor bump.

### Added

#### MetricSink trait + WandB

- `xrun_core::metric_sink::MetricSink` (async via `async-trait`):
  `name` / `open_run` / `log_metrics_batch` / `log_artifact` / `finalize`.
  Backends are independent crates; adding one is a single
  `impl MetricSink for X` plus a registry entry in `MetricFanOut::new`.
- `OpenRunCtx<'_>` carries everything a tracking server needs
  (run_id, experiment, run_name, vendor, instance_id, config, tags),
  borrowed so callers don't clone hashmaps per tick.
- `RemoteRunHandle { sink_name, remote_run_id, remote_url }` — the opaque
  token. `remote_url` lets `xrun show` / `xrun metrics --mlflow-url`
  build deep links without rebuilding from raw config (which broke when
  the user later edited `mlflow.url`).
- `MetricSinkError` granular enough to retry on `Network` / `Server`,
  fail loud on `Auth` / `Config`, swallow on `Disabled`.

- `crates/xrun-wandb/` — new crate. `WandbClient` wraps three operations
  on `api.wandb.ai`:
  - `viewer { entity }` GraphQL probe for default-entity resolution.
  - `mutation upsertBucket` to create / reuse a run row (idempotent on
    run name, so a daemon restart doesn't double-create).
  - `POST /files/{entity}/{project}/{run}/file_stream` to append to
    `wandb-history.jsonl` and signal `complete + exitcode` on finalize
    (same path the official wandb Python SDK uses internally).
- `WandbSink: MetricSink` groups `MetricPoint`s by step before posting
  so multi-key updates within one training iteration land on the same
  wandb x-axis tick. Tracks an `AtomicU64` history offset per run for
  retry-safe appends.

#### Poller fan-out

- `MetricFanOut` (was `MlflowMirror`) holds `Vec<Box<dyn MetricSink>>`.
  `start()` opens runs on every enabled sink in sequence; `log_metrics`
  fans batches out concurrently within one `block` boundary, so total
  latency is `max(per_sink)` not the sum. Per-sink `warned` latches
  surface the first failure and swallow the rest.
- `MetricSinksConfig { mlflow: Option<MlflowSubConfig>, wandb:
  Option<WandbSubConfig>, … }` — toggle each sub independently.
- Sinks are gated by both `[metrics] sinks = […]` (config) and
  credential presence; a sink listed without creds warns and is dropped
  (no fail), so a partial setup still gives the user whatever sinks
  remain — including local-only.

#### Persistence

- Schema migration `005_sink_run_ids.sql`:
  - `mlflow_run_url` (resolved at sink-open time).
  - `wandb_run_id`, `wandb_run_url` (paired write — never half-populated).
- `Store::set_mlflow_run_url`, `Store::set_wandb_run` setters.
- `MetricFanOut::start` routes the new `RemoteRunHandle` back to the
  right setter via a sink-name dispatch.

#### CLI

- `xrun init --wandb-key -` flag, mirrors `--vast-key` / `--kaggle-token`
  with `-` to read trimmed line from stdin (one stdin read max per
  invocation).
- `xrun config probe --vendor wandb` — lands in the existing probe
  surface, mirroring vast / kaggle / mlflow shape. Reads
  `XRUN_PROBE_WANDB_KEY` from env so the wizard / TUI can validate a
  pasted key without writing it to disk first. Hits `viewer { entity }`,
  returns 401 cleanly on a bad key.
- `xrun init-manifest --vendor <X> --sink <Y>` — generate a manifest
  skeleton for any (vendor × sink) combination. Every editable spot
  is marked `TODO_<field>`; `grep TODO_ <path>` lights up everything
  needing review before launch. Vendors: `vast`, `kaggle`, `local`,
  `ssh`. Sinks: `mlflow`, `wandb`, `none`. Closes the loop on the
  user-facing goal — Claude Code can produce a working manifest
  without knowing the schema.
- `xrun init --sink wandb` now accepted (was a v0.8 rejection).
- `xrun config show` includes `wandb.api_key` (masked unless
  `--secrets`); `xrun config set wandb.api_key …` works through the
  schema-driven setter.

#### TUI

- New `screens/sinks.py` (Python Textual). Cards: MLflow, WandB,
  Comet ([v0.8] disabled). Status pill with five distinct states:
  EMPTY / PAUSED / CHECK / READY / ERROR — PAUSED makes the
  "key set but not in metrics.sinks list" mistake visible at a glance.
  Actions: edit / test / toggle-default / revoke. Routing: `g m` chord
  + `Go: Sinks` palette entry.
- `SinkEditScreen` with masked inputs: 4 fields for MLflow (url goes
  to global config, auth fields to credentials.toml), 1 for WandB
  (entity is auto-probed at first launch).
- Wizard catalog: wandb `available_now` flag flipped to `True`. Wizard
  step still treats wandb as "tick to add to default list, configure
  key on Sinks screen" — full wizard form is a follow-up.

#### Schema

- `WandbCredentials { api_key }` in `Credentials`.
- `Run` carries `mlflow_run_url`, `wandb_run_id`, `wandb_run_url`
  (`Option<String>`).

### Changed

- `mlflow_mirror.rs` → `metric_fanout.rs`. `MlflowMirror` →
  `MetricFanOut`, `MlflowMirrorConfig` → `MetricSinksConfig`.
  `Poller::with_mlflow` → `Poller::with_metric_sinks`. The wiremock-
  based poller integration tests still validate the same wire-level
  MLflow calls (create / log-batch / update-run) — they don't care
  that the path goes through a trait now.

### Tests

- 11 new unit + wiremock tests in `xrun-wandb` covering viewer-probe,
  upsertBucket, file_stream batching, finalize exit code, 401 auth
  surfacing.
- 8 unit tests in `init_manifest` covering the (vendor × sink) matrix
  and the `TODO_` token contract.
- 1 multi-sink integration test in `xrun-poller` runs the poller
  end-to-end against two wiremock servers (one MLflow, one wandb) and
  asserts both received the metric batch.
- Full workspace: 404 passing, 0 failures across 50 suites.

### Live smoke

- Run `01KQWJSX0VDNEEMBQGPK8KH6HN` — quickstart.yaml with
  `metrics.sinks = ["wandb"]`. WandB run created, finalized, page
  exists at `https://wandb.ai/data_force/quickstart/runs/xrun-…`
  (HTTP 200), DB row populated with `wandb_run_id` + `wandb_run_url`.
- `xrun init-manifest --vendor local --sink mlflow` parses cleanly
  through `xrun doctor --manifest <path>`.

### Not yet shipped

- WandB artifacts API — `log_artifact` returns `Disabled` for now.
  Needs the separate `manifests + S3-presigned PUT` surface; deferred
  to v0.7.x patch or v0.8.
- Wizard form for wandb (just `api_key` input) — checkbox enables
  the sink in `metrics.sinks` but credential entry still goes through
  the Sinks screen.
- Comet ML — slot in TUI catalog, no impl. v0.8.

---

## [0.5.4] — 2026-05-05

Field-feedback patch from the arborust v9-skipalpha session: closes
`issue_2026_05_05_notebook_mode_no_livestream.md`. The headline fix
restores live telemetry for `run.notebook` Kaggle kernels (silently
broken since 0.5.3); four bonus observations from the same session
(stdout phantom metrics, kaggle CLI noise, `kernels list` JSON parse,
template pip-eagerness) are also addressed.

### Added
- `crates/xrun-kaggle/src/notebook_inject.rs` — prepends a synthetic
  `xrun-bootstrap` cell to the user's `.ipynb` before push. The cell
  base64-decodes the embedded `xrun_hook` wheel, pip-installs it
  `--no-deps --quiet`, and sets `MLFLOW_TRACKING_URI` /
  `MLFLOW_TRACKING_USERNAME` / `MLFLOW_TRACKING_PASSWORD` from xrun
  config — exactly what script-mode `main.py` already does. Notebook
  cells now have the same telemetry plumbing as `run.cmd` kernels.
  Cell carries a `xrun-bootstrap` tag + `xrun_generated: true` metadata
  so it's identifiable in the kernel viewer.
- `XRUN_LOG_STREAM_DISABLE=1` env var: prevents subprocess re-imports
  of `xrun_hook` from spawning a duplicate streamer that would push
  to a separate MLflow run. User cells can still call `metric()` /
  `epoch()` / `done()` — only the auto-streamer at import time is
  suppressed.
- Polyline overlay in TUI metrics chart: `render_chart_multi(..., lines=True)`
  joins consecutive samples with `─ ╱ ╲ │` glyphs; press `C` in the
  Run-detail Metrics tab to toggle. Toolbar reports `lines: on/off`.

### Fixed
- **Notebook-mode silently no live telemetry.** `xrun_hook.metric(...)`
  calls inside notebook kernels would silently no-op because
  `MLFLOW_TRACKING_URI` was never injected, and the wheel was only
  base64-embedded into `main.py` (script-mode). `xrun events <id>`
  showed only host-side `queued:start` / `running:start`, never any
  user-side `stage:*` / metric points. Reproduced 4× during the
  arborust v9-skipalpha launch sweep.
- **Phantom metrics from stdout-parser.** `parse_stdout_metrics` was
  too greedy — it would scrape `numpy>` (from `numpy>=2.0`),
  `dropout`, `in_ch`, `w[0]`, `tobler`, etc. as `count: 1` keys with
  no real value. Tightened to (a) require strict Python identifier
  on the key and (b) require an explicit `epoch=`/`step=` anchor on
  the line for the `key=value` form. The `key: value` form is
  unchanged — it's a stronger training-output signal.
- **`kaggle kernels list` parse failure breaking `xrun resume`.** The
  CLI emits a tab-padded text table by default and doesn't support
  `--json`; `xrun resume` was JSON-parsing it and failing with
  `Expecting value: line 1 column 1 (char 0)`. Switched to `--csv`
  with quote-aware row splitting. JSON path retained for backward
  compat with existing test mocks.
- **Kaggle CLI 1.8.x outdated-version banner corrupting JSON parsers.**
  The kaggle CLI emits a literal-template warning to stdout (with
  un-substituted `{current_version}` placeholders), which our
  `serde_json::from_str` would choke on. New `strip_kaggle_cli_noise`
  filter drops top-level `Warning:` lines and the literal-template
  variant before downstream parsers see them. New
  `annotate_kaggle_cli_failure` appends a `pip install --upgrade
  kaggle` hint when the cryptic JSON-parse error reaches the user.
- **Streamed terminal events not promoting Kaggle runs.**
  `ingest_telemetry_chunks` now returns `Option<RunStatus>` and
  `poll_completion` promotes the run + cancels the kernel when an
  ingested event signals `status=fail` / `stage=done` — so a CUDA-OOM
  or training crash doesn't keep burning compute while Kaggle's
  coarse-grained `KernelState` lags behind.

### Documentation
- `docs/MANIFEST.md`: rewrote the `run.notebook` and live-tail bullets
  to reflect 0.5.4 parity (auto-bootstrap cell, MLflow side-channel).
- `exp/templates/README.md`: new "Kaggle: что НЕ переустанавливать"
  section warning that `pip install torch` on Kaggle costs 15-20 min,
  the container already ships cu128 + sm_60 support, and old
  `P100 → reinstall torch 2.2.2+cu118` recipes are obsolete.

---

## [0.5.3] — 2026-05-05

Field-feedback sweep: closes the eight items in `ISSUES.md` from the
arborust evening session, plus one latent `Store::open` bug surfaced
while wiring live telemetry. End-to-end live metrics + events on Kaggle
now work via the existing MLflow side-channel.

### Added
- `xrun pull` is no longer a stub — resolves run → vendor adapter →
  `adapter.pull()`, defaults destination to `runs/<id>/artifacts/`,
  supports `--into`, and reports matching files for `--ckpt
  best|latest|all`. (#4)
- Live telemetry on Kaggle: `xrun_hook`'s log streamer now also tails
  `events.jsonl` / `metrics.jsonl` and pushes them as MLflow artifact
  chunks alongside stdout. The Kaggle adapter ingests new chunks every
  poll tick — events and metrics appear in `xrun events`/`xrun metrics`
  while the kernel is still running. Requires `mlflow.url` configured.
  (#8)
- Streamer also mirrors metric records to MLflow's native
  `/api/2.0/mlflow/runs/log-batch` endpoint so the MLflow UI's Metrics
  tab plots them. The artifact JSONL is still the source of truth for
  the local poller; the native logging is purely for the human
  dashboard. (Without it, the UI shows "No model metrics recorded".)
- Kaggle dataset version pinning: after `datasets status = ready`, the
  adapter resolves the dataset's `currentVersionNumber` via the REST
  API and rewrites slugs to `owner/name/N`, so kernels never mount a
  stale snapshot due to the kernel-creation cache lag. Slugs that
  already carry an explicit `/N` are left alone. (#1)
- TUI Artifacts screen: walks the run's local artifacts dir and lists
  real files (was previously a `xrun pull not yet implemented` stub).
  Empty state hints `press \`a\` to pull`; `a` runs the real CLI and
  auto-refreshes. (Surfaced after #4 unblocked it.)

### Fixed
- `xrun launch --detach` no longer hangs after `kaggle kernels push`
  finishes. The push subprocess's pipes are now drained on background
  threads, so `try_wait` returns instead of waiting on a full OS pipe
  buffer. (#3)
- Duplicate `running:start` events after a poll-daemon restart. The
  Kaggle adapter rehydrates its last kernel state from the DB on the
  first poll so already-emitted transitions don't re-fire. (#6)
- TUI `⚠ stale` warning for healthy runs whose poll-daemon died
  mid-session: the app now runs auto-resume every 60 s, so a poller
  killed by a binary upgrade self-heals without user action. (#7)
- `xrun_hook` install path on Kaggle: the wheel is base64-embedded in
  the kernel `main.py` and bootstrapped before user `setup`, so the
  resolution-atomic `pip install` no longer drops siblings when
  `xrun_hook` isn't on PyPI. (#2/#5)
- Latent: `Store::open` was being called on the data directory in
  three Kaggle adapter callsites (post-pull ingest, kernel-state
  recovery, live telemetry ingest), failing silently with
  `unable to open database file`. New `db_path()` helper appends
  `runs.db`; without this the live-telemetry ingest never persisted
  any events or metrics.

### Changed
- `crates/xrun-kaggle/src/log_stream.rs` exports
  `parse_chunk_seq_with(stem, ext, path)` so adapters can reuse the
  same chunk reassembly path for `logs/`, `events/`, and `metrics/`
  prefixes.

---

## [0.5.0] — 2026-05-03

### Added
- `xrun-local` adapter — run manifests directly on the host as a first-class
  vendor (`vendor: local`). Full lifecycle parity with vast (provision →
  upload → execute → tail → pull → destroy), cross-platform (bash/sh on Unix,
  pwsh/powershell on Windows), idempotent destroy via PID file.
- `xrun-ssh` adapter — same lifecycle against your own server / NAS / VPS over
  SSH; reuses the local execution model.
- `xrun init` — first-run wizard. TTY → spawns `xrun-tui --wizard` (4 steps:
  local capabilities → vendors → logging mode → recap with live `xrun doctor`).
  Non-interactive flags for the Claude skill / CI: `--probe-local --json`,
  `--non-interactive --mark-completed --sink mlflow`. Credential flags
  (`--vast-key`, `--kaggle-token`, `--kaggle-username`, `--kaggle-key`) accept
  `-` to read one stdin line, so secrets stay out of shell history.
- `xrun doctor` categorized output: env / vendors / manifests groups with
  per-category counts, `--json` for skill consumption.
- `xrun config probe --vendor <name>` — validate credentials read from
  `XRUN_PROBE_*` env (used by the wizard).
- `xrun metrics --per-key` — emit one chart per metric key instead of overlay.
- `xrun_hook.metrics(values: dict, step: int)` — batch shortcut: writes one row
  per key sharing a single timestamp. Avoids N separate `metric()` calls in the
  training loop.
- Kaggle live log streaming end-to-end via `xrun_hook` → MLflow chunked
  artifacts (the public Kaggle API has no live-log endpoint).
- `exp/templates/` — starter manifests + train.py for common ML tasks:
  `quickstart` (zero-config smoke test), `classification`
  (loss/acc/f1_macro/precision/recall), `regression` (loss/mae/rmse/r2).
  Templates run end-to-end without torch so they smoke-test the structure
  before adaptation.
- TUI Vendors screen: brand-coloured cards (vast orange `#ff6b35`, kaggle cyan
  `#20beff` via `border-left thick` accent), status pills
  (READY/CHECK/ERROR/EMPTY), pulse animation on the connectivity probe dot,
  double-click opens edit.
- TUI Vast edit screen: "Region filter" section reusing the existing
  `CountryExcludeScreen` — pulls and saves `search.exclude_countries` via
  `xrun config`, surfaces current exclusions inline.
- TUI country picker: pills tinted by continent (EU blue, AS red, NA green,
  ME orange, AF violet, OC cyan, SA amber) so country codes render and group
  visually in any terminal — including Windows Terminal which doesn't compose
  regional-indicator codepoints into flag glyphs.
- TUI `u` binding on Vendors screen — opens the vendor's quota/billing page
  in a browser (`kaggle.com/settings`, `cloud.vast.ai/billing`).
- TUI wizard split into a sub-package with `image_view` / `metrics_view` /
  `report_view` widgets for reuse in run detail.

### Changed
- Wizard rebuilt for keyboard-first UX: `Checkbox` widgets (Tab/Space toggle),
  `o` opens API-key page of *focused* card (works before selecting), Esc-skip
  now requires Y/N confirmation, probe shows a loading indicator, Recap runs
  `xrun doctor --json` and prints ✓/⚠/✗ per check. Toggling no longer rebuilds
  the body — pasted keys keep focus.
- TUI Kaggle card surfaces free-tier limits (CPU ∞ 24h/session,
  GPU 30h/wk 12h/session, TPU 20h/wk 9h/session) instead of an opaque
  "competitions visible: N" probe artifact.
- TUI vendor cards: flat `Horizontal` rows replaced with `Vertical` cards;
  `vendor-row`/`vendor-dot`/`vendor-name-col` CSS retired in favour of
  `vendor-card`/`vendor-card-head`/`vendor-card-foot`. Old class names removed.
- `xrun gc` filters non-vast records — local cleanup is via `xrun stop`.

### Removed
- `xrun init --vendor` flag. It was informational-only (echoed in JSON, never
  wrote anything). The wizard now relies on `--sink` and the credential flags
  for non-interactive setup.

### Configuration
- `[ui] wizard_completed: bool` (default false) — TUI auto-launches the
  wizard when false; finishing the wizard sets both this flag and
  `[metrics] sinks` via the CLI.
- `[metrics] sinks: Vec<String>` (default `["mlflow"]`) — editable through
  `xrun config set`. WandB and Comet sink checkboxes are visible but disabled
  with a `[v0.8]` badge.

---

## [0.4.0] — 2026-04-30

### Added
- Installation scripts for all platforms (`install.sh`, `install.ps1`)
- GitHub Actions CI workflow (cargo test, clippy, fmt)
- GitHub Actions release workflow (cross-platform binaries: linux-musl, macos-x86, macos-arm64, windows)
- `CHANGELOG.md`, `LICENSE` file

### Changed
- `xrun tui` description in `docs/CLI.md` updated to Python Textual (was: ratatui)
- Python package versions bumped to match Rust workspace (0.4.0)

---

## [0.3.0] — 2026-04-29

### Added
- `xrun balance` — vast.ai account balance
- `xrun gc` — remove orphan instances
- `xrun shell <id>` — SSH session to running instance
- `xrun cp` — streaming tar transfer between instances
- `xrun fix-status` — repair stuck runs in the local DB
- MLflow REST mirror: metrics written to local MLflow server in parallel
- `xrun metrics --png` — export metric chart as PNG (plotters, Tokyo Night palette)
- Kaggle adapter: kernel push/status/output, embedded `xrun_hook`
- `xrun dataset` — manage Kaggle datasets
- Budget guards: `--max-cost`, `--max-hours`, `--idle-timeout`; auto-destroy via poll-daemon

### Changed
- TUI fully rewritten in Python Textual (replaces Rust ratatui prototype)
- TUI screens: Dashboard, Runs, Run detail (Stages/Logs/Metrics/Artifacts/Manifest),
  Instances, Vendors, Launch, Compare, Settings, Doctor, Help
- Chord navigation (`g→r`, `g→v`, `g→s`, …), command palette (`:`)
- Budget dashboard: burn rate card, today card, runway warning in status bar
- Vendors screen: masked API key input, import from `vastai` config, balance display
- Poll-daemon respawns automatically on crash; writes `auto_destroyed_reason` to DB

---

## [0.2.3] — 2026-04-29

### Added
- Budget caps in `xrun launch`: `--max-cost`, `--max-hours`, `--idle-timeout`
- Confirmation prompt before launch (overridable with `--yes` in CI)
- Auto-destroy logic in poll-daemon when caps exceeded
- TUI Dashboard budget cards: active burn, cap-left, today spend

---

## [0.2.2] — 2026-04-29

### Added
- TUI UX polish: header click-to-navigate, run-detail tab hotkeys, status colours
- Help screen with all chord bindings

---

## [0.2.1] — 2026-04-28

### Added
- Vendors screen in TUI: vast.ai key import, masked edit, balance display
- Splash screen shown when no credentials configured

---

## [0.2.0] — 2026-04-27

### Added
- Python Textual TUI (`xrun-tui` binary, `pip install -e python/xrun_tui`)
- Live event/metric polling via aiosqlite
- Tokyo Night colour theme

---

## [0.1.0] — 2026-04-27

### Added
- `xrun launch` — provision → upload → exec → poll full pipeline for vast.ai
- `xrun ls`, `xrun show`, `xrun logs`, `xrun events`, `xrun metrics`
- `xrun pull` — download checkpoints and artifacts
- `xrun stop`, `xrun rerun`
- `xrun doctor`, `xrun config`
- `xrun_hook` Python package — emits `events.jsonl` + `metrics.jsonl` from training scripts
- SQLite local state (`runs.db`)
- `--detach` mode with background poll-daemon
- `--dry-run` manifest validation
