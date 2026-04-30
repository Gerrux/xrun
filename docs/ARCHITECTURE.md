# Architecture

## Цели и не-цели

**Цели**
- Один манифест → один запуск на любом из поддерживаемых вендоров (vast.ai, Kaggle).
- Полная история запусков локально, без зависимости от облачного UI.
- Live-метрики и стадии без логин-сессий и без WandB.
- Минимальная поверхность для Claude skill — 6–8 CLI-команд, никаких ad-hoc bash.

**Не-цели**
- Не оркестратор кластера. Один пользователь, одна машина-контроллер.
- Не replacement для MLflow — мы его используем как backend для метрик и UI шаринга.
- Не пытаемся унифицировать всё абстрактным слоем: vast и Kaggle-адаптеры физически разные модули, общий только манифест и БД.

## Компоненты

```
┌──────────────────────────────────────────────────────────────────────┐
│                     Локальная машина (контроллер)                    │
│                                                                      │
│  ┌──────────────┐  spawn   ┌───────────────────────────────────┐    │
│  │  xrun (CLI)  │─────────▶│  xrun-tui (Python Textual)        │    │
│  │  Rust binary │          │  python/xrun_tui/                 │    │
│  └──────┬───────┘          │  - читает SQLite (aiosqlite)      │    │
│         │                  │  - вызывает xrun CLI (subprocess) │    │
│         │                  └───────────────┬───────────────────┘    │
│         │                                  │                        │
│         ▼                                  ▼                        │
│  ┌──────────────┐          ┌───────────────────┐                    │
│  │  xrun-core   │◀─────────│     SQLite        │                    │
│  │  (manifest,  │          │     runs.db       │                    │
│  │   db, vendor │          └─────────┬─────────┘                    │
│  │   trait)     │                    │                              │
│  └──────┬───────┘                    ▼                              │
│         │                  ┌───────────────────┐  ┌─────────────┐  │
│         │                  │  MLflow REST      │◀─│   Browser   │  │
│         │                  └───────────────────┘  └─────────────┘  │
│         ▼                                                            │
│  ┌─────────────┐  ┌─────────────┐                                   │
│  │ vastai CLI  │  │ kaggle CLI  │                                   │
│  └─────────────┘  └─────────────┘                                   │
└────────────────────────────────────────────────────────────────────┘
         │                 │
         ▼                 ▼
  ┌─────────────┐    ┌─────────────┐
  │ vast.ai GPU │    │ Kaggle      │
  │ /workspace/ │    │ Kernel      │
  │   ├ events  │    │ stdout +    │
  │   │  .jsonl │    │  output/    │
  │   ├ metrics │    │             │
  │   │  .jsonl │    │             │
  │   └ ckpts/  │    │             │
  └─────────────┘    └─────────────┘
```

## Crates (Rust)

```
xrun-core      — manifest types, sqlite, event/metric model, budget, vendor trait
xrun-poller    — polling loop engine (Poller, CancellationToken, PollerLock); used by xrun-cli
xrun-vast      — vast.ai адаптер (provision, upload, exec, tail, pull, CREATE_NO_WINDOW on Windows)
xrun-kaggle    — kaggle адаптер (kernels push/status/output, embedded xrun_hook wheel)
xrun-mlflow    — REST клиент для tracking server (metric mirror, retry, wiremock tests)
xrun-cli       — clap-парсер, все subcommands; spawn xrun-tui при запуске без аргументов
xrun-tui       — legacy Rust ratatui TUI (за feature-флагом, не используется по умолчанию)
```

## Python TUI (xrun-tui)

```
python/xrun_tui/          — Python Textual TUI
  src/xrun_tui/
    app.py                — главное приложение, chord-навигация, screen stack
    db.py                 — async SQLite (aiosqlite), read-only, тот же runs.db
    services.py           — asyncio subprocess wrappers вокруг xrun CLI
    screens/              — 16 экранов: dashboard, runs, run_detail, instances,
                            vendors, launch, artifacts, compare, settings,
                            doctor, help, splash, confirm, ...
    widgets/              — status_bar, ascii_chart, ...
    themes/               — Tokyo Night, Catppuccin, Gruvbox CSS
```

**Интеграция с Rust CLI:**
- `xrun` без аргументов на TTY: `std::process::Command::new("xrun-tui").status()`
- TUI читает SQLite напрямую (aiosqlite, WAL mode — safe concurrent read/write)
- Мутирующие операции (stop, pull, launch) вызывают `xrun` CLI через `asyncio.create_subprocess_exec`
- На Windows: все subprocess-вызовы используют `CREATE_NO_WINDOW`

Бинарь `xrun-tui` устанавливается через `pip install -e python/xrun_tui` (`pyproject.toml` → `hatchling`).

## Поток данных запуска

1. **`xrun launch exp.yaml`** — CLI парсит манифест, создаёт строку в `runs` (`status=provisioning`), хеширует манифест, пишет copy в `runs/{id}/manifest.yaml`.
2. **Адаптер vast** запрашивает offer, создаёт инстанс (`provision`), заливает данные через `vastai copy` или `rsync` (`upload`), стартует контейнер с командой:
   ```
   pip install xrun_hook && python script.py [args] | tee /workspace/run/stdout.log
   ```
3. **Тренировочный скрипт** через `xrun_hook` пишет на vast volume:
   - `events.jsonl` — стадии и состояние
   - `metrics.jsonl` — числа
   - `output/` — артефакты (PNG, ckpt, JSON)
4. **Poller** (фон-таск в локальной машине, живёт пока есть активные runs) каждые 5–15s делает `vastai execute <id> "tail -c+OFFSET /workspace/run/events.jsonl"` инкрементально, парсит и пишет в SQLite + зеркалит метрики в MLflow.
5. **Pull** (`xrun pull <run> --ckpt latest`) — синхронизирует артефакты на локальный диск через `vastai copy`.
6. **Stop** — при `done` событии или ручной команде poller инициирует `vastai destroy`.

Kaggle flow тот же, кроме шагов 2 и 4: provision = `kaggle kernels push`, polling = `kaggle kernels status` + финальный `kaggle kernels output` (нет live-tail). Метрики восстанавливаются после завершения, чарты постфактум.

## Граница с MLflow

- **SQLite — primary** для всего, что про процесс: какие runs существуют, какие инстансы крутятся, на какой стадии, какие чекпоинты выкачаны, ссылки на артефакты.
- **MLflow — secondary** для метрик-числовых рядов и ссылок на артефакты-файлы, которыми хочется поделиться (PNG, GeoTIFF). TUI читает свежие значения из обеих БД, но «истина» по run lifecycle — в SQLite.
- В MLflow один **experiment = manifest.name**, один **run = xrun run id**. Tags: `vendor`, `instance_id`, `manifest_hash`, `gpu_type`.

Подробнее в [STATE.md](STATE.md).

## Poller process model

Поллер — polling-loop, который читает `events.jsonl` / `metrics.jsonl` с инстанса через инкрементальный tail.

**Lock-файл**: `~/.local/share/xrun/runs/<id>/poller.pid` (in-memory registry + файл). Предотвращает двойной поллинг одного run. При попытке запустить второй поллер возвращает `AlreadyPolling`.

**Daemon spawn**: `xrun launch --detach` запускает `xrun __poll-daemon <run-id>` как отдельный процесс через `CREATE_NEW_PROCESS_GROUP | DETACHED_PROCESS` (Windows) или `setsid` (Unix). Родительский процесс печатает run-id и выходит.

**InstanceHandle** сериализуется в `instances.state_json` при provision. Daemon при старте читает его оттуда, реконструирует VastAdapter и запускает поллер.

**Передача handle через БД**: provisioned instance id хранится в `runs.instance_id`, handle JSON — в `instances.state_json`. Daemon открывает две Store connections к одной SQLite (WAL mode).

## Отказы и восстановление

- Прерванный poller — при следующем `xrun ls` / `xrun tui` стартует заново, читает offset из SQLite.
- Упавший инстанс — событие `instance_lost`, run помечается `failed`, артефакты которые успели пулиться остаются.
- Битый манифест — валидация в `launch`, без обращения к вендору.
- Дубль launch — `manifest_hash` + `--allow-duplicate` флаг.

## Что осталось от старого `train-vast`

`train-vast` skill остаётся как был для уже существующих экспериментов. Новые идут только через `xrun`. Когда покрытие фич `xrun` сравняется — `train-vast` депрекейтнем; до тех пор живут параллельно.
