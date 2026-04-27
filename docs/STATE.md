# State

Локальный state живёт в `~/.local/share/xrun/` (Linux/macOS) или `%LOCALAPPDATA%\xrun\` (Windows). Не в проектной директории — несколько проектов делят одну БД.

```
xrun/
├── runs.db                    # SQLite — primary state
├── runs/<run-id>/             # per-run файлы
│   ├── manifest.yaml          # frozen копия
│   ├── stdout.log             # тянется при pull
│   ├── events.jsonl           # зеркало с инстанса
│   ├── metrics.jsonl
│   └── output/                # pulled артефакты
└── cache/                     # временные файлы (vastai/kaggle CLI cache)
```

Конфиг отдельно: `~/.config/xrun/{config.toml, credentials.toml}`.

## SQLite-схема

```sql
-- Запуски
CREATE TABLE runs (
    id              TEXT PRIMARY KEY,           -- ulid
    name            TEXT NOT NULL,
    manifest_hash   TEXT NOT NULL,
    manifest_path   TEXT NOT NULL,              -- runs/<id>/manifest.yaml
    vendor          TEXT NOT NULL,              -- 'vast' | 'kaggle'
    instance_id     TEXT,                       -- vast contract_id или kaggle kernel slug
    status          TEXT NOT NULL,              -- provisioning|uploading|running|done|failed|cancelled
    created_at      DATETIME NOT NULL,
    started_at      DATETIME,
    ended_at        DATETIME,
    cost_usd        REAL,                       -- финальная стоимость, NULL пока running
    mlflow_run_id   TEXT,
    notes           TEXT
);
CREATE INDEX idx_runs_status ON runs(status);
CREATE INDEX idx_runs_name ON runs(name);
CREATE INDEX idx_runs_manifest_hash ON runs(manifest_hash);

-- События / стадии
CREATE TABLE events (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    run_id          TEXT NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
    ts              DATETIME NOT NULL,
    stage           TEXT NOT NULL,
    status          TEXT NOT NULL,              -- start|ok|fail|progress
    msg             TEXT,
    payload_json    TEXT
);
CREATE INDEX idx_events_run ON events(run_id, ts);

-- Числовые ряды (зеркало MLflow для быстрых TUI-чартов)
CREATE TABLE metrics (
    run_id          TEXT NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
    step            INTEGER NOT NULL,
    key             TEXT NOT NULL,
    value           REAL NOT NULL,
    ts              DATETIME NOT NULL,
    PRIMARY KEY (run_id, key, step)
);

-- Артефакты, выкачанные локально
CREATE TABLE artifacts (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    run_id          TEXT NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
    kind            TEXT NOT NULL,              -- checkpoint|figure|json|log|other
    remote_path     TEXT NOT NULL,
    local_path      TEXT,                       -- NULL пока не выкачан
    size_bytes      INTEGER,
    sha256          TEXT,
    pulled_at       DATETIME,
    is_best         INTEGER NOT NULL DEFAULT 0  -- 1 если матчит keep_best
);
CREATE INDEX idx_artifacts_run ON artifacts(run_id);

-- Состояние poller per-file
CREATE TABLE poll_offsets (
    run_id          TEXT NOT NULL REFERENCES runs(id) ON DELETE CASCADE,
    file            TEXT NOT NULL,              -- 'events.jsonl' | 'metrics.jsonl' | 'stdout.log'
    offset_bytes    INTEGER NOT NULL DEFAULT 0,
    last_polled_at  DATETIME,
    PRIMARY KEY (run_id, file)
);

-- Активные/недавние инстансы вендоров (включая orphan'ов без run)
CREATE TABLE instances (
    id              TEXT PRIMARY KEY,           -- vendor-prefixed: 'vast:1234' | 'kaggle:slug'
    vendor          TEXT NOT NULL,
    run_id          TEXT REFERENCES runs(id),
    gpu_type        TEXT,
    price_per_hour  REAL,
    created_at      DATETIME,
    destroyed_at    DATETIME,
    state_json      TEXT                        -- последний снимок vendor metadata
);

-- Версия схемы для миграций
CREATE TABLE schema_version (version INTEGER NOT NULL);
INSERT INTO schema_version VALUES (1);
```

## Граница ответственности

| Данные | Хранилище | Почему |
|--------|-----------|--------|
| Lifecycle (создан, запущен, упал) | SQLite | Нужен offline, primary truth |
| События стадий | SQLite | TUI читает много раз, низкая латентность |
| Метрики (числовые ряды) | SQLite + MLflow | SQLite — быстрый TUI чарт; MLflow — UI шаринга |
| Артефакты (PNG, ckpt) | Local FS + MLflow | FS — для pull; MLflow — для ссылок «открой в браузере» |
| Manifest (frozen) | Local FS | Один файл на run |
| Креды | `~/.config/xrun/credentials.toml`, `keyring` | Никогда в БД и не в манифесте |

## Миграции

`xrun migrate` — последовательное применение `migrations/00X_*.sql`. На старте CLI/TUI делает `SELECT version FROM schema_version`; если меньше актуальной — предлагает запустить миграцию.

## Резервная копия

`runs.db` — SQLite WAL. `xrun backup --to <dir>` делает `VACUUM INTO`. Per-run папки можно бэкапить отдельно. MLflow имеет свой backend (sqlite или postgres) — бекапится сам.

## Опциональный MLflow

Если MLflow не поднят (`mlflow.experiment` отсутствует в манифесте либо REST недоступен) — xrun работает полностью на SQLite. Метрики только в TUI/CLI, шаринг через ручной экспорт PNG.
