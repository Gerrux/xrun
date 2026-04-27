# xrun

Унифицированный запуск ML-экспериментов на vast.ai и Kaggle: один YAML-манифест, один CLI, один TUI, одна локальная БД с историей и метриками.

## Зачем

- Каждый запуск на vast.ai сейчас — отдельный bash-скрипт, отдельный набор путей, отдельный поиск чекпоинтов. Это шумно (расход токенов на повторяющийся код) и нестандартизовано.
- Kaggle-ноутбуки запускаются вручную, история не агрегируется с vast.
- Метрики и стадии (download → unpack → train → done) сейчас приходится вытаскивать через `vastai logs | grep` каждый раз заново.

`xrun` решает это: ты описываешь эксперимент один раз в YAML, дальше всё через `xrun launch / ls / pull / metrics`. Скилл Claude знает только эти команды — bash-обёртки больше не нужны.

## Стек

- **Rust workspace**, единый бинарь `xrun` с подкомандами (`launch`, `ls`, `tui`, `pull`, …).
- **ratatui + crossterm** для TUI, plotters / ratatui::Chart для графиков.
- **SQLite (rusqlite)** — локальный state (runs, события, артефакты).
- **MLflow** (локальный server, REST) — метрики и артефакты с UI «поделиться».
- **Python sidecar** `xrun_hook` — pip-пакет, который тренировочный скрипт импортит, чтобы писать `events.jsonl` и `metrics.jsonl` на vast volume.
- Адаптеры: `vastai` CLI (через subprocess), `kaggle` CLI (через subprocess), позже — нативный REST.

## Документация

| Файл | О чём |
|------|-------|
| [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) | Компоненты, поток данных, почему так |
| [docs/MANIFEST.md](docs/MANIFEST.md) | YAML-схема эксперимента |
| [docs/CLI.md](docs/CLI.md) | Подкоманды и примеры |
| [docs/TUI.md](docs/TUI.md) | Экраны, биндинги, виджеты |
| [docs/EVENTS.md](docs/EVENTS.md) | Протокол events.jsonl + Python hook |
| [docs/STATE.md](docs/STATE.md) | SQLite-схема и граница с MLflow |
| [docs/SKILL.md](docs/SKILL.md) | Дизайн Claude skill для xrun |
| [docs/ROADMAP.md](docs/ROADMAP.md) | v0.1 / v0.2 / v0.3 scope |

## Status

v0.1 foundation: workspace, manifest parser, sqlite, config, CLI scaffolding ready; vast adapter and xrun_hook pending.

Существующие запуски, которые уже идут через старый `train-vast` flow, не переписываем — `xrun` для нового.

## Quickstart

```bash
cargo build --release
target/release/xrun config init
target/release/xrun launch exp/foo.yaml --dry-run
```
