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
┌──────────────────────────────────────────────────────────────────┐
│                     Локальная машина (контроллер)                │
│                                                                  │
│   ┌─────────────┐     ┌──────────────────┐     ┌──────────────┐  │
│   │ xrun (CLI)  │────▶│   xrun-core      │◀────│ xrun-tui     │  │
│   │ subcommands │     │  (manifest, db,  │     │ (ratatui)    │  │
│   └─────────────┘     │   poller, sync)  │     └──────────────┘  │
│         │             └──────────────────┘            │          │
│         │                  │      │                   │          │
│         │                  ▼      ▼                   ▼          │
│         │           ┌──────────┐ ┌─────────┐    ┌───────────┐    │
│         │           │ SQLite   │ │ MLflow  │◀───│ Browser   │    │
│         │           │ runs.db  │ │  REST   │    │ (share)   │    │
│         │           └──────────┘ └─────────┘    └───────────┘    │
│         │                                                        │
│         ▼                                                        │
│   ┌─────────────┐  ┌─────────────┐                               │
│   │ vastai CLI  │  │ kaggle CLI  │                               │
│   └─────────────┘  └─────────────┘                               │
└─────────│─────────────────│──────────────────────────────────────┘
          │                 │
          ▼                 ▼
   ┌─────────────┐    ┌─────────────┐
   │ vast.ai GPU │    │ Kaggle      │
   │             │    │ Kernel      │
   │ /workspace/ │    │ /kaggle/    │
   │   run/      │    │   working/  │
   │   ├ events  │    │             │
   │   │  .jsonl │    │ stdout +    │
   │   ├ metrics │    │  output/    │
   │   │  .jsonl │    │             │
   │   └ ckpts/  │    │             │
   └─────────────┘    └─────────────┘
```

## Crates

```
xrun-core      — manifest types, sqlite, event/metric model, poller engine
xrun-vast      — vast.ai адаптер (provision, upload, exec, tail, pull)
xrun-kaggle    — kaggle адаптер (kernels push/status/output)
xrun-mlflow    — REST клиент для tracking server
xrun-cli       — clap-парсер, subcommands; этот же бинарь умеет запускать TUI (`xrun tui`)
xrun-tui       — ratatui frontend, читает только из xrun-core, действия — через те же функции, что CLI
```

Один бинарь (`xrun`) — `xrun-cli` подключает `xrun-tui` как фичу. По умолчанию `xrun` без аргументов открывает TUI.

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

## Отказы и восстановление

- Прерванный poller — при следующем `xrun ls` / `xrun tui` стартует заново, читает offset из SQLite.
- Упавший инстанс — событие `instance_lost`, run помечается `failed`, артефакты которые успели пулиться остаются.
- Битый манифест — валидация в `launch`, без обращения к вендору.
- Дубль launch — `manifest_hash` + `--allow-duplicate` флаг.

## Что осталось от старого `train-vast`

`train-vast` skill остаётся как был для уже существующих экспериментов. Новые идут только через `xrun`. Когда покрытие фич `xrun` сравняется — `train-vast` депрекейтнем; до тех пор живут параллельно.
