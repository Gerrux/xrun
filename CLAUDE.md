# xrun — ML experiment runner

Rust CLI + Python Textual TUI для запуска ML-экспериментов на vast.ai, Kaggle
и локальной машине. Один YAML-манифест → provision GPU → upload data → run
training → poll events/metrics → SQLite.

## Стек

| Компонент | Язык | Путь |
|-----------|------|------|
| CLI binary `xrun` | Rust | `crates/xrun-cli/` |
| Core (manifest, db, vendor trait) | Rust | `crates/xrun-core/` |
| vast.ai адаптер | Rust | `crates/xrun-vast/` |
| Kaggle адаптер | Rust | `crates/xrun-kaggle/` |
| Local адаптер (host subprocess, без сети) | Rust | `crates/xrun-local/` |
| Poll daemon engine | Rust | `crates/xrun-poller/` |
| MLflow REST client | Rust | `crates/xrun-mlflow/` |
| TUI (Python Textual) | Python | `python/xrun_tui/` |
| Training hook | Python | `xrun_hook` (отдельный pip-пакет) |

**Entry points:**
- `xrun` без аргументов + TTY → запускает `xrun-tui` (Python Textual binary)
- `xrun tui` → то же самое через feature-flag (Rust crate `xrun-tui` legacy, не использовать)
- `xrun-tui` — Python Textual binary, ставится отдельно через pip

**БД:** `~/.local/share/xrun/runs.db` (Linux) · `~/Library/Application Support/xrun/runs.db` (Mac) · `%APPDATA%\xrun\runs.db` (Windows)

**Конфиг:** `~/.config/xrun/config.toml` + `credentials.toml`

## Сборка и запуск

```bash
# Rust CLI
cargo build --release
./target/release/xrun config init
./target/release/xrun doctor

# Python TUI (отдельно)
cd python/xrun_tui && pip install -e .
xrun-tui          # прямой запуск TUI
xrun              # тоже запустит TUI если stdout — TTY
```

## Ключевые CLI-команды

```bash
xrun launch exp/foo.yaml [--detach] [--dry-run]   # запустить эксперимент
xrun ls [--status running|done|failed] [--json]   # список запусков
xrun show <run-id> [--json]                        # карточка запуска
xrun logs <run-id> [--follow] [--grep pat]         # stdout лога
xrun events <run-id> [--follow]                    # стадии: provision/upload/train/done
xrun metrics <run-id> [--key val_f1] [--ascii] [--json] [--png out.png]
xrun pull <run-id> [--ckpt best|latest|all] [--into models/]
xrun stop <run-id> [--force]
xrun rerun <run-id> [--patch run.args.--lr=5e-4]
xrun balance                                       # баланс vast.ai
xrun config init|show|set <key> <val>
xrun doctor                                        # проверка окружения
xrun gc                                            # удалить orphan инстансы
```

Все read-команды поддерживают `--json` для парсинга.

## TUI — навигация

```
g d → Dashboard     g r → Runs          g i → Instances
g v → Vendors       g s → Settings      g l → Launch
g h → Doctor

?   → Help          :   → Command palette    q/Esc → Назад/выход
V   → Vendors       (прямая клавиша из Runs screen)
```

Экраны: Dashboard, Runs, Run detail (Stages/Logs/Metrics/Manifest), Instances,
Vendors, Launch, Artifacts, Compare, Settings, Doctor, Help.

## Структура манифеста (минимальный)

```yaml
name: my_experiment
vendor: vast          # или kaggle
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

Полная схема: `docs/MANIFEST.md`

## Типичный workflow (для Claude Code)

```bash
# 1. Создать/скопировать манифест
cp exp/base.yaml exp/v2.yaml
# отредактировать поля

# 2. Запустить
xrun launch exp/v2.yaml --detach

# 3. Следить за стадиями
xrun events <id> --follow

# 4. Смотреть метрики
xrun metrics <id> --key val_f1 --ascii

# 5. Забрать чекпоинт
xrun pull <id> --ckpt best --into models/
```

## Запуск тестов

```bash
cargo test --workspace
cargo clippy --workspace -- -D warnings
cargo fmt --check
```

## Что НЕ делать

```
❌ vastai create instance ...       → xrun launch exp/foo.yaml
❌ kaggle kernels push ...          → xrun launch exp/foo.yaml
❌ ssh root@... "tar ..."           → xrun pull <id>
❌ sqlite3 runs.db "SELECT ..."     → xrun ls/show/metrics --json
❌ cat events.jsonl                 → xrun events <id> --json
❌ xrun-tui напрямую               → используй xrun (он сам вызовет xrun-tui)
```

## Документация

| Файл | Содержимое |
|------|------------|
| `docs/CLI.md` | Все подкоманды, флаги, exit codes |
| `docs/MANIFEST.md` | Полная YAML-схема с примерами |
| `docs/TUI.md` | Экраны, биндинги, виджеты |
| `docs/ARCHITECTURE.md` | Компоненты, поток данных |
| `docs/EVENTS.md` | Протокол events.jsonl + Python hook |
| `docs/STATE.md` | SQLite-схема |
| `docs/SKILL.md` | Дизайн Claude skill |
| `docs/ROADMAP.md` | История версий и backlog |

## Важные особенности

- `--detach` спавнит фоновый `__poll-daemon` — он пишет события/метрики в SQLite
- Budget guards: `--max-cost`, `--max-hours`, `--idle-timeout` в `xrun launch`
- Poll-daemon сам гасит инстанс при превышении caps (auto-destroy)
- Credentials: `xrun config set vast.api_key ...` или `V → i` в TUI для импорта
- Windows: все subprocess-вызовы используют `CREATE_NO_WINDOW`
