# Roadmap

Ordered scope. Каждая версия — рабочая система целиком, не наполовину.

## v0.1 — CLI core (без TUI)

**Цель**: Скилл уже работает, запуски стандартизованы. TUI пока нет.

### Scope

- [x] Cargo workspace: `xrun-core`, `xrun-vast`, `xrun-cli`.
- [x] Манифест-парсер (serde_yaml + валидатор схемы) — все поля из MANIFEST.md.
- [x] SQLite + миграции — все таблицы из STATE.md, schema_version=1.
- [x] Конфиг (`~/.config/xrun/`) и `xrun config init/set/show`.
- [x] vast-адаптер:
  - [x] Поиск offer по `gpu`/`price` (через `vastai search offers --raw`).
  - [x] Provision (`vastai create instance`).
  - [x] Upload данных (`vastai copy` или `rsync` через ssh).
  - [x] Старт команды через `vastai execute`.
  - [x] Pull через `vastai copy`.
  - [x] Destroy.
- [x] `xrun_hook` Python пакет (`pip install -e .` локально, потом PyPI).
- [x] Poller: тайл `events.jsonl` и `metrics.jsonl`, запись в SQLite.
- [x] CLI команды: `launch / ls / show / logs / events / metrics --ascii / pull / stop / rerun / config / doctor`.
- [x] `--json` для всех read-команд (для скилла).
- [x] Skill-файл `SKILL.md` опубликован в `~/.claude/skills/xrun/`.

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

## v0.2.1 — Vendors screen + auth flow в TUI

**Цель**: убрать первый failure-mode UX («открыл xrun, пусто, что делать?»). Дать настраивать вендоров прямо из TUI.

### Scope

- [x] `VendorAdapter::vendor_status() -> Result<VendorStatus>` (default `NotImplemented`); реализация в `VastAdapter` через `vastai show user --raw`.
- [x] `Credentials::is_empty()` + `import_vast_native()` / `import_kaggle_native()` в xrun-core.
- [x] Новый экран `Vendors` (key `V`, `:goto vendors`): статус подключения, баланс, account; actions `e`=edit, `i`=import, `t`=test, `r`=revoke.
- [x] Masked input форма для ключей; сохранение в `~/.config/xrun/credentials.toml` (0600 на Unix).
- [x] Фоновый `VendorProbeService`: probe раз в 60s + по триггеру.
- [x] Реальный balance в status_bar (вместо `$—`).
- [x] First-run splash overlay при пустых credentials — открывает Vendors по любой клавише.

### Acceptance

1. На чистой машине без credentials: `xrun` показывает splash, любая клавиша → экран Vendors с подсказкой `i` для import.
2. `i` на vast вытягивает ключ из `~/.config/vastai/vast_api_key`, сразу пробит → status `✓ connected`, баланс в status_bar.
3. `e` на vendor → masked input, Enter сохраняет и тригерит probe; revoke стирает ключ.

---

## v0.2.2 — TUI UX polish: dashboard, animations, density

**Цель**: убрать ощущение «дёшево» — убрать пустое место, добавить визуальный контекст и анимации без CPU-overhead.

### Scope

- [x] Расширенная тема: `accent`, `dim_text`, `card_bg`, `success_bg`, `error_bg` + Nord RGB theme.
- [x] Status bar v2: 3 сегмента (`xrun › breadcrumb` | vendor balance + status icon | screen hotkeys).
- [x] Hint-lines внутри экранов удалены (переехали в status bar).
- [x] Empty-states: Runs (no runs / no active), Instances Local & Remote, Vendors detail (unconfigured).
- [x] Adaptive section heights: Active схлопывается до 3 строк когда пуст; (N+3).clamp(5, h/2) иначе.
- [x] Dashboard cards сверху Runs: Vendor (balance + status dot), Active (count + phases), Today (done/failed/$spent).
- [x] Always-on animated splash: 600ms обычный / 1500ms first-run; logo прорастает построчно, потом idle throbber.
- [x] `anim_frame: u64` в AppState, инкремент при каждом render.
- [x] `view/anim`: `pulse` (selection bold toggle ~1s), `count_up` (easing), `reveal_str` (char-by-char).
- [x] Pulse-анимация выделения на Runs, Instances, Vendors.
- [x] Screen breadcrumb в status bar через `screen_stack`.
- [x] 55/55 тестов ✓, clippy clean, fmt check ✓.

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
