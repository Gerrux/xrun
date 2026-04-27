# Roadmap

Ordered scope. Каждая версия — рабочая система целиком, не наполовину.

## v0.1 — CLI core (без TUI)

**Цель**: Скилл уже работает, запуски стандартизованы. TUI пока нет.

### Scope

- [ ] Cargo workspace: `xrun-core`, `xrun-vast`, `xrun-cli`.
- [ ] Манифест-парсер (serde_yaml + валидатор схемы) — все поля из MANIFEST.md.
- [ ] SQLite + миграции — все таблицы из STATE.md, schema_version=1.
- [ ] Конфиг (`~/.config/xrun/`) и `xrun config init/set/show`.
- [ ] vast-адаптер:
  - [ ] Поиск offer по `gpu`/`price` (через `vastai search offers --raw`).
  - [ ] Provision (`vastai create instance`).
  - [ ] Upload данных (`vastai copy` или `rsync` через ssh).
  - [ ] Старт команды через `vastai execute`.
  - [ ] Pull через `vastai copy`.
  - [ ] Destroy.
- [ ] `xrun_hook` Python пакет (`pip install -e .` локально, потом PyPI).
- [ ] Poller: тайл `events.jsonl` и `metrics.jsonl`, запись в SQLite.
- [ ] CLI команды: `launch / ls / show / logs / events / metrics --ascii / pull / stop / rerun / config / doctor`.
- [ ] `--json` для всех read-команд (для скилла).
- [ ] Skill-файл `SKILL.md` опубликован в `~/.claude/skills/xrun/`.

### Acceptance

1. Запустить существующий `train_v5_multichannel.py` на vast через `xrun launch exp/arborust_v7_C.yaml --detach`.
2. `xrun ls --status running` показывает его.
3. После завершения `xrun pull <id> --ckpt best` достаёт правильный файл.
4. `xrun metrics <id> --ascii` рисует val_f1.
5. Claude через скилл может всё перечисленное без шелл-фолбэков.

### Дельта против старого `train-vast`

Старые эксперименты остаются как есть. Новые манифесты пишутся под `xrun`. `train-vast` skill не трогаем.

---

## v0.2 — TUI

**Цель**: «руками тыкать» интерфейс поверх той же БД.

### Scope

- [ ] Crate `xrun-tui` (ratatui + crossterm + tokio).
- [ ] Экраны: Runs / Run detail (Stages, Logs, Manifest) / Instances / Settings.
- [ ] Биндинги по TUI.md, command palette.
- [ ] Live update через канал из poller'а (без двойного поллинга).
- [ ] `xrun` без аргументов открывает TUI.
- [ ] Throbber, базовая тема, color-eyre.

### Acceptance

1. `xrun` показывает все активные runs с обновляющейся стадией.
2. Из TUI можно: запустить новый run (Launch picker по `exp/`), застопить, спулить best ckpt.
3. Logs tab корректно tail'ит и фильтрует.

### Не входит

- Metrics tab с графиками — в v0.3 (как раз когда есть MLflow).
- Артефакты viewer (только список без preview — отложен).

---

## v0.3 — MLflow + Kaggle + чарты

**Цель**: метрики красиво и шарябельно, второй вендор.

### Scope

- [ ] Crate `xrun-mlflow` (REST клиент).
- [ ] Зеркалирование метрик в MLflow при поллинге.
- [ ] `xrun metrics --png` через MLflow figure API + локальный fallback (plotters).
- [ ] TUI Run detail → Metrics tab с ratatui Chart + multi-series toggle, `s` save PNG, `o` open MLflow.
- [ ] Crate `xrun-kaggle`:
  - [ ] `kaggle kernels push/status/output`.
  - [ ] Адаптация манифеста (нет live tail, post-completion ingest).
- [ ] `xrun launch` с `vendor: kaggle` работает end-to-end.

### Acceptance

1. После завершения ранa MLflow UI показывает все метрики и артефакты, ссылка из TUI открывается.
2. PNG-экспорт даёт картинку, которую можно сразу скинуть в чат.
3. Тренировка через Kaggle kernel логируется в ту же БД, отображается в `xrun ls` рядом с vast.

---

## v0.4+ (backlog)

- `xrun sweep` (декартово произведение по grid).
- `xrun diff <run-a> <run-b>` — манифесты + метрики side-by-side.
- Anomaly detection в poller (loss взлетел → notification).
- Cost forecasting (по средней стоимости похожих ранов).
- Native vast.ai REST вместо CLI subprocess (стабильнее на ошибках).
- Web UI рядом с TUI (тот же state, для шаринга по сети).
- Скилл-плагин в формате Claude Code marketplace.

## Что НЕ в roadmap

- Multi-user / role-based access.
- Distributed training оркестрация.
- Datasets versioning (DVC и пр.) — пользуемся внешними тулзами, в манифесте лишь src путь.
- Своя реализация tracking server вместо MLflow.

## Definition of Done (общая для всех версий)

- Проходит `cargo clippy --workspace -- -D warnings`.
- Проходит `cargo test --workspace`.
- Документация в `docs/` синхронизирована с поведением.
- README отмечает текущий уровень готовности.
- На реальной задаче (один из существующих arborust-экспериментов) пройден end-to-end сценарий из «Acceptance».
