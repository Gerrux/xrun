# xrun

Унифицированный запуск ML-экспериментов на vast.ai и Kaggle: один YAML-манифест, один CLI, один TUI, одна локальная БД с историей и метриками.

## Зачем

- Каждый запуск на vast.ai сейчас — отдельный bash-скрипт, отдельный набор путей. Это шумно (расход токенов на повторяющийся код) и нестандартизовано.
- Kaggle-ноутбуки запускаются вручную, история не агрегируется с vast.
- Метрики и стадии приходится вытаскивать через `vastai logs | grep` каждый раз заново.

`xrun` решает это: описываешь эксперимент один раз в YAML, дальше всё через `xrun launch / ls / pull / metrics`. Claude Code знает только эти команды — bash-обёртки больше не нужны.

## Стек

| Компонент | |
|-----------|--|
| `xrun` CLI | Rust (clap, rusqlite, tokio) |
| TUI | Python Textual (`python/xrun_tui/`) |
| vast.ai адаптер | Rust subprocess → `vastai` CLI + native REST |
| Kaggle адаптер | Rust subprocess → `kaggle` CLI |
| MLflow mirror | Rust REST client (`xrun-mlflow`) |
| Локальный state | SQLite WAL (`runs.db`) |
| Training hook | Python `xrun_hook` (pip-пакет) |

## Quickstart

```bash
# 1. Сборка CLI
cargo build --release
./target/release/xrun config init
./target/release/xrun doctor

# 2. TUI (отдельно)
pip install -e python/xrun_tui
xrun              # открывает TUI если stdout — TTY
```

При первом запуске без credentials TUI показывает сплеш и открывает экран Vendors — нажми `i` чтобы импортировать ключ из `~/.config/vastai/vast_api_key`.

## Основные команды

```bash
xrun launch exp/foo.yaml [--detach]          # запустить эксперимент
xrun launch exp/foo.yaml --dry-run           # проверить без запуска
xrun ls [--status running] [--json]          # список запусков
xrun events <id> [--follow]                  # стадии: provision → upload → train → done
xrun logs <id> [--follow]                    # stdout (--follow = SSH tail -F)
xrun metrics <id> [--ascii] [--json] [--png] # метрики
xrun pull <id> [--ckpt best] [--into dir/]   # скачать чекпоинты/артефакты
xrun stop <id>                               # остановить
xrun balance                                 # баланс vast.ai
xrun doctor                                  # диагностика окружения
```

Все read-команды поддерживают `--json`. Полный справочник: [docs/CLI.md](docs/CLI.md).

## TUI

```bash
xrun          # запускает TUI (Python Textual) если stdout — TTY
xrun-tui      # прямой вызов
```

Экраны: Dashboard · Runs · Run detail (Stages/Logs/Metrics/Manifest) · Instances · Vendors · Launch · Settings · Doctor

Навигация: `g r` Runs · `g v` Vendors · `g s` Settings · `g l` Launch · `?` Help · `:` Command palette

## Budget guards

```bash
xrun launch exp/foo.yaml \
  --max-cost 5.0 \      # auto-destroy при $5 потраченных
  --max-hours 8 \       # auto-destroy через 8 часов
  --idle-timeout 30     # auto-destroy если GPU idle 30+ минут
```

Poll-daemon сам гасит инстанс при превышении cap, записывает `auto_destroyed_reason` в БД.

## Манифест (минимальный)

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

Полная схема: [docs/MANIFEST.md](docs/MANIFEST.md)

## Документация

| Файл | Содержимое |
|------|------------|
| [docs/CLI.md](docs/CLI.md) | Все подкоманды, флаги, exit codes |
| [docs/MANIFEST.md](docs/MANIFEST.md) | Полная YAML-схема с примерами |
| [docs/TUI.md](docs/TUI.md) | Экраны, биндинги, виджеты |
| [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) | Компоненты, поток данных |
| [docs/EVENTS.md](docs/EVENTS.md) | Протокол events.jsonl + Python hook |
| [docs/STATE.md](docs/STATE.md) | SQLite-схема |
| [docs/SKILL.md](docs/SKILL.md) | Claude Code skill |
| [docs/ROADMAP.md](docs/ROADMAP.md) | История версий и backlog |

## Статус: v0.3 complete

- ✅ vast.ai: provision, upload, exec, poll, pull, destroy
- ✅ Kaggle: kernels push/status/output, embedded hook
- ✅ Events и метрики в SQLite в реальном времени
- ✅ MLflow mirror (метрики + ссылка на UI)
- ✅ `xrun metrics --png` (plotters, Tokyo Night)
- ✅ Budget guards (caps, confirm flow, auto-destroy, spend dashboard)
- ✅ Python Textual TUI: 16 экранов, chord-навигация, Tokyo Night тема
- ✅ `xrun events --follow` (SQLite poll), `xrun logs --follow` (SSH tail)
- ✅ Claude Code skill + CLAUDE.md

Следующее: `xrun sweep` (hyperparameter grid), native vast.ai REST, web UI.
