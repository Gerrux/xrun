# Changelog

All notable changes to this project are documented in this file.

Format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).
Versions follow [Semantic Versioning](https://semver.org/).

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
