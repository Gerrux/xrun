# Event & Metric Protocol

Как тренировочный код на инстансе общается с локальным `xrun` без серверной части.

## Файлы на инстансе

```
/workspace/run/
├── events.jsonl       # стадии и состояние, append-only
├── metrics.jsonl      # числовые ряды, append-only
├── stdout.log         # сырые stdout+stderr команды
├── manifest.yaml      # копия манифеста
└── output/            # артефакты, чекпоинты
    ├── ep001.pt
    ├── metrics.json
    └── pr_curve.png
```

`events.jsonl` и `metrics.jsonl` — append-only JSON-Lines. Локальный poller хранит в SQLite per-file byte offset и читает дельты через `vastai execute <id> "tail -c+OFFSET <file>"`.

## events.jsonl

Одна строка — один event, JSON-объект:

```json
{"ts":"2026-04-27T12:01:33.481Z","stage":"unpack","status":"start","msg":"cache.tar","extra":{"size_gb":3.2}}
{"ts":"2026-04-27T12:02:10.001Z","stage":"unpack","status":"ok","extra":{"duration_s":36.5}}
{"ts":"2026-04-27T12:02:12.130Z","stage":"train","status":"start"}
{"ts":"2026-04-27T12:08:02.901Z","stage":"epoch","status":"ok","extra":{"epoch":1,"val_f1":0.812}}
{"ts":"2026-04-27T13:05:11.002Z","stage":"epoch","status":"ok","extra":{"epoch":2,"val_f1":0.831}}
{"ts":"2026-04-27T19:44:00.000Z","stage":"train","status":"ok"}
{"ts":"2026-04-27T19:44:12.000Z","stage":"done","status":"ok"}
```

### Стандартные стадии

```
provision → upload → unpack → env_ready → train_start
   ↓
   epoch (повторяется)
   ↓
train_end → artifacts_ready → done
```

`provision`, `upload`, `unpack`, `env_ready` пишет **сам адаптер** (xrun-vast/kaggle) — для них тренировочный скрипт уже не запущен.

`train_*`, `epoch`, `done` пишет **тренировочный скрипт** через xrun_hook.

Кастомные стадии (произвольная строка) допустимы: `validation`, `export_onnx`, etc.

### Поля

| Поле | Тип | Описание |
|------|-----|----------|
| `ts` | RFC3339 string | UTC время на инстансе |
| `stage` | string | Имя стадии (см. выше или кастом) |
| `status` | enum | `start` \| `ok` \| `fail` \| `progress` |
| `msg` | string? | Свободный коммент |
| `extra` | object? | Произвольный JSON (попадает в SQLite `events.payload_json`) |

## metrics.jsonl

```json
{"ts":"...","step":1,"key":"train_loss","value":0.81}
{"ts":"...","step":1,"key":"val_loss","value":0.74}
{"ts":"...","step":1,"key":"val_f1","value":0.812}
```

`step` обычно epoch, но может быть и iteration — у тренировочного скрипта на усмотрение. Один key — один временной ряд.

## Python hook (`xrun_hook`)

Pip-пакет, ставится на инстансе через `pip install xrun_hook`.

```python
from xrun_hook import stage, metric, epoch, fail, done

stage("unpack")                      # автоматически start; контекстный менеджер закроет ok
with stage("validation"):
    ...

# в обучающем цикле:
for ep in range(epochs):
    train_one_epoch(...)
    val = validate(...)
    metric("train_loss", train_loss, step=ep)
    metric("val_loss", val.loss, step=ep)
    metric("val_f1", val.f1, step=ep)
    epoch(ep, {"val_f1": val.f1})    # сахар = stage("epoch", status="ok", extra={...})

done()                                # пишет stage="done" и закрывает файлы
```

API минимальный, потому что чем больше — тем больше адаптация существующих скриптов. Для исключения автоматический хук:

```python
import xrun_hook   # на импорте устанавливает sys.excepthook → fail(...) если упало
```

### Файловые пути

`xrun_hook` определяет run dir в порядке:
1. `$XRUN_RUN_DIR` (адаптер ставит)
2. `/workspace/run/` (default на vast)
3. `/kaggle/working/run/` (default на kaggle)
4. `./run/` (для локального dev)

Если ни один не доступен — пишет в stdout как fallback (структурный JSON с маркером `[xrun-event] {...}`), poller умеет это парсить тоже (для Kaggle, где SSH нет).

## Как poller читает на vast

```
local: known offset = 12480
remote: vastai execute <id> "wc -c < /workspace/run/events.jsonl"
        → 18992
remote: vastai execute <id> "tail -c +12481 /workspace/run/events.jsonl"
        → новые байты
local: распарсить, записать в SQLite, обновить offset = 18992
```

Period: 5s для активных, 30s для idle (нет stdout активности). Подавляется когда run = `done`/`failed`.

## Как poller читает на Kaggle

Live-tail невозможен. Стратегия:
1. Poll `kaggle kernels status <slug>` каждые 30s.
2. На статусе `complete`: `kaggle kernels output <slug> -p <local-tmp>`, парсим `events.jsonl` целиком, восстанавливаем хронологию событий и метрик.
3. До completion в TUI отображается «running, no live data» — это известный компромисс.

Альтернатива (опционально, v0.4): `xrun_hook` пишет stdout в формате `[xrun-event] {json}`, мы парсим Kaggle-логи через REST. Не для MVP.

## DDP-safe append и rank guard

`xrun_hook` безопасен для запуска в PyTorch DDP (multi-GPU distributed training):

- **Rank guard**: если `RANK` env != `0` и `XRUN_HOOK_ALL_RANKS != "1"` — все вызовы silent no-op. Только rank 0 пишет события.
- **File lock**: каждый `append` берёт `fcntl.flock(LOCK_EX)` (Unix) / `msvcrt.locking` (Windows) перед записью. Защищает от interleaving при параллельных процессах.
- **fsync**: опционально через `XRUN_HOOK_FSYNC=1`. По умолчанию только flush.

## Kaggle fallback (stdout marker)

Когда ни один из стандартных путей не доступен для записи, `xrun_hook` пишет в `stdout`:

```
[xrun-event] {"ts":"...","stage":"epoch","status":"ok","extra":{"epoch":1}}
```

Poller на Kaggle парсит stdout с этим маркером как часть финального output-полла.

## Безопасность

- `events.jsonl` НЕ содержит секреты. Hook валидирует, что в `extra` нет ключей, начинающихся на `_secret`.
- `manifest.yaml` на инстансе — копия с уже отрезанными секциями `credentials.*` (если такие были, но их и не должно быть).
