# Changelog

All notable changes to this project are documented in this file.

Format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
Versions follow [Semantic Versioning](https://semver.org/).

---

## [Unreleased]

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
