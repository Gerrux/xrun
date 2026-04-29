# CLI

Один бинарь `xrun` с подкомандами. Без аргументов — открывает TUI.

## Команды

### `xrun launch <manifest> [flags]`
Создаёт run, валидирует манифест, провижинит инстанс, заливает данные, стартует команду.

```
--detach            возврат в shell сразу после старта (по умолчанию — печатает live-стадии)
--allow-duplicate   разрешить запуск манифеста с уже существующим хешем
--dry-run           распарсить + показать план, ничего не делать
--name <override>   переопределить name (не меняет hash)
```

Exit 0 при `status=done`, 1 при `failed`, 2 при cancellation, 130 при Ctrl-C (если не --detach).

### `xrun ls [flags]`
Список runs — по умолчанию активные + последние 10 завершённых.

```
--all                       показать всё
--vendor vast|kaggle
--status running|done|failed
--tag <tag>                 фильтр по tag
--manifests                 показать манифесты в exp/, помеченные «не запускались»
--json                      машинно-читаемо
```

### `xrun show <run-id>`
Полная карточка run: манифест, события, последние метрики, артефакты, ссылки.

### `xrun logs <run-id> [flags]`
stdout/stderr.

```
--follow / -f      live tail
--since 10m        только последние 10 минут
--grep <pat>       фильтр
```

### `xrun events <run-id> [flags]`
Поток событий стадий (download/unpack/train/epoch/...). По умолчанию — таблица, `--follow` — live.

### `xrun metrics <run-id> [flags]`
Метрики.

```
--key val_f1,val_loss        выбрать конкретные
--ascii                      ASCII chart в stdout (default если TTY)
--png <out>                  дамп PNG (через MLflow или локально через plotters)
--mlflow-url                 распечатать URL run в MLflow UI
```

### `xrun pull <run-id> [flags]`
Синхронизация артефактов и чекпоинтов на локальный диск.

```
--ckpt latest|best|all|<glob>
--artifacts                  забрать всё, что в manifest.artifacts.patterns
--into <local-dir>           default: runs/<id>/output/
```

### `xrun stop <run-id>`
Корректный stop: посылает SIGTERM в команду, ждёт N сек, забирает финальные артефакты, гасит инстанс.

```
--force          сразу destroy, без graceful
--keep-instance  не гасить vast-инстанс (для отладки)
```

### `xrun rerun <run-id> [--patch key=val ...]`
Повтор запуска. Без --patch — точная копия. С --patch — модифицирует args/гиперпараметры (значение лезет внутрь run.args, обозначается через jq-style путь: `--patch run.args.--lr=5e-4`).

### `xrun sweep <manifest> --grid <spec>`
Генерит N манифестов из decart-произведения и лончит. Spec пример:
```
--grid run.args.--lr=1e-3,1e-4 run.args.--batch-size=4,8
```

### `xrun tui`
Открывает интерактивный TUI (ratatui). `xrun` без аргументов делает то же самое, если stdout — TTY; в противном случае выводит help и завершается с кодом 0.

Экраны: Runs (главный), Run detail (Stages/Logs/Manifest), Launch picker, Instances, Settings. Live-обновления через канал поллера. Биндинги: `?` help, `:` command palette, `q`/`Esc` — назад/выход.

### `xrun doctor`
Проверки: креды есть, vastai/kaggle CLI работают, MLflow server поднят, диск, сеть.

### `xrun config`
Управление `~/.config/xrun/`.

```
xrun config init                    создать дефолтные файлы
xrun config set vast.api_key ...
xrun config show                   текущая конфигурация (без секретов)
```

## Глобальные флаги

```
-v / --verbose       DEBUG логи в stderr
-q / --quiet         только ошибки
--db <path>          override SQLite location
--no-color
```

## Идиомы для skill

```bash
# Скилл всегда пишет:
xrun launch exp/arborust_v7_C.yaml --detach
xrun ls --status running --json
xrun pull <run-id> --ckpt best
xrun metrics <run-id> --key val_f1 --ascii

# Скилл НИКОГДА не пишет:
vastai create instance ...
ssh root@... "tar xf ..."
kaggle kernels push -k ...
```

См. [SKILL.md](SKILL.md).

### `xrun __poll-daemon <run-id>` (hidden)

Внутренняя команда, запускаемая автоматически при `--detach`. Запускает поллер событий/метрик в фоне для уже запущенного run.

```
--runs-dir <path>   путь к runs/ каталогу (передаётся лаунчером)
```

Для отладки зависшего поллера:
```bash
xrun __poll-daemon <run-id>   # вручную из терминала, foreground
```

## v0.1 status

| Команда | Статус | Заметки |
|---------|--------|---------|
| `xrun launch <manifest> --dry-run` | Работает | Парсит манифест, считает хеш, показывает DryRunPlan |
| `xrun launch <manifest>` | Работает | Полная цепочка: provision → upload → exec → poller |
| `xrun launch <manifest> --detach` | Работает | Спавнит фоновый поллер, сразу выходит |
| `xrun ls` | Работает | Читает runs из SQLite; `--manifests` возвращает пустой список (v0.2) |
| `xrun show <id>` | Работает | Карточка run из БД |
| `xrun logs <id>` | Работает | Читает stdout.log; `--follow` → exit 64 (not supported in v0.1) |
| `xrun events <id>` | Работает | Из SQLite; `--follow` → exit 64 |
| `xrun metrics <id>` | Работает | Список ключей и точек; `--ascii` → «no data yet» |
| `xrun pull <id>` | Заглушка | «no active runs to act on» |
| `xrun stop <id>` | Заглушка | «no active runs to act on» |
| `xrun rerun <id>` | Заглушка | «no active runs to act on» |
| `xrun doctor` | Работает | Проверяет config_dir, vastai/kaggle в PATH, ssh key, rsync, xrun_hook |
| `xrun config init/show/set` | Работает | Полная реализация |
| `xrun tui` | Работает | TUI с live-обновлениями; Metrics tab — v0.3 |
| `xrun sweep` | Не реализовано | Отложено |

## Exit codes

| Код | Значение |
|-----|----------|
| 0 | Успех / `status=done` |
| 1 | Ошибка стадии / failed run |
| 2 | Cancelled (graceful stop) |
| 64 | Ошибка манифеста / валидации |
| 65 | Конфигурация (нет кредов, неверный API key) |
| 66 | Вендор недоступен (нет offers, kaggle 503) |
| 130 | Ctrl-C |
