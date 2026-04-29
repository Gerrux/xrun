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
  - [x] Поиск offer по `gpu`/`price` (через REST `POST /bundles/`; раньше — `vastai search offers --raw`).
  - [x] Provision (REST `PUT /asks/{id}/`; раньше — `vastai create instance`).
  - [x] Upload данных (`vastai copy` или `rsync` через ssh).
  - [x] Старт команды через `vastai execute`.
  - [x] Pull через `vastai copy`.
  - [x] Destroy (REST `DELETE /instances/{id}/`; раньше — `vastai destroy instance`).
  - Provision-путь больше не зависит от `vastai` Python CLI — регрессии в этом пакете (например `400: owner: Extra inputs are not permitted`) перестали блокировать запуски.
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

## v0.2.3 — Budget guards

**Цель**: защитить пользователя от перерасхода на vast — hard-cap на инстанс,
billable-confirm перед launch, видимость трат.

### Scope

- [x] Schema: миграция `003_budget.sql` с полями `max_lifetime_secs`,
      `max_cost_usd`, `idle_timeout_secs`, `accumulated_cost`,
      `last_active_at`, `auto_destroyed_reason`.
- [x] `BudgetConfig` в `~/.config/xrun/config.toml` (дефолты 8h / $10 / 30min idle).
- [x] Pure budget core (`xrun-core/src/budget.rs`): `evaluate_caps`,
      `accumulate_cost`, `daily_spend`, `active_hourly_burn`, `caps_from_config`.
- [x] CLI флаги `xrun launch --max-cost --max-hours --idle-timeout --yes`.
- [x] Confirm-flow в CLI: tier-классификация (Free / y/N / TypedConfirm),
      fail-loud при non-TTY без `--yes`.
- [x] Poll-daemon enforcement: каждый тик обновляет `accumulated_cost`,
      вызывает `evaluate_caps` и при срабатывании пишет
      `auto_destroyed_reason` → destroy → событие `instance.auto_destroyed` →
      run.status=failed. Идемпотентен при рестарте.
- [x] VastAdapter сохраняет caps в БД при provision.
- [x] TUI launch confirm: enriched message с vendor/$/hr/projected max в
      существующем `Modal::Confirm` (без отдельной модалки — хватает строки).
- [x] TUI dashboard cards: `active` показывает `$X.XX/hr · cap-left $Y.YY`
      (red при <$1); `today` карточка считает live accrual + completed runs.
- [x] Status bar: `⚠ <Nm runway` (red) при `balance/burn < 1h`.
- [x] Soft-alert event `budget.daily_exceeded` в poll-daemon (раз в день).
      Hard-stop опционально через `daily_budget_hard = true`.

### Acceptance

1. ✓ `cargo test -p xrun-core -p xrun-poller -p xrun-tui -p xrun-vast` зелёный
   (xrun-cli не компилится из-за WIP в `stop.rs` на параллельной ветке —
   не относится к budget guards).
2. ✓ Миграция применяется поверх v2 без потерь.
3. ✓ Запуск `xrun launch ...` без `--yes` в pipe-режиме — exit 1 с подсказкой
   про `--yes` (если выше Free tier).
4. (нужно проверить на живом инстансе) `--max-cost 0.05` приводит к
   auto-destroy через 1 тик poll-daemon с событием `instance.auto_destroyed`.
5. ✓ TUI dashboard в реальном времени показывает `$/hr · cap-left $X.XX`
   и накапливающийся `today $spent` за счёт live-accrual.

---

## v0.3 — MLflow + Kaggle + чарты ✅ done

**Цель**: метрики красиво и шарябельно, второй вендор.

### Scope

- [x] Crate `xrun-mlflow` (REST клиент, auth, batch metrics, retry, wiremock tests).
- [x] Зеркалирование метрик в MLflow при поллинге (`mlflow_mirror.rs`, degrade-silent).
- [x] `xrun metrics --png` через `plotters` BitMapBackend 1200×600, Tokyo Night palette.
- [x] `xrun metrics --mlflow-url` печатает ссылку на MLflow run.
- [x] Crate `xrun-kaggle`:
  - [x] `kaggle kernels push/status/output` subprocess wrapper.
  - [x] `KaggleAdapter` имплементирует `VendorAdapter`.
  - [x] Post-completion ingest из `events.jsonl`/`metrics.jsonl`.
  - [x] Embedded `xrun_hook` wheel + `_xrun_kaggle_entry.py` wrapper.
- [x] `xrun launch` с `vendor: kaggle` работает end-to-end.
- [x] Poll-daemon MLflow wiring для detached runs.
- [x] `manifest.policy.on_idle_minutes` wired to budget caps.
- [x] New vast manifest fields: `inet_down_min_mbps`, `cuda_min`, `reliability_min`, `direct_port_count_min`, `regions`.
- [x] Per-source upload timeout (`policy.upload_timeout_secs`).
- [x] Stdout auto-capture metrics (`parse_stdout_metrics`).
- [x] `xrun balance` command for vast.ai balance.
- [x] `docs/MANIFEST.md` — vast fields, Kaggle section, exclude semantics.

### Acceptance

1. После завершения рана MLflow UI показывает все метрики, ссылка `xrun metrics <id> --mlflow-url` открывается.
2. PNG-экспорт даёт картинку, которую можно сразу скинуть в чат.
3. Тренировка через Kaggle kernel логируется в ту же БД, отображается в `xrun ls` рядом с vast.

---

## v0.4+ (backlog)

- `xrun sweep <manifest> --grid lr=1e-3,1e-4 batch=4,8` — декартово произведение; генерит N материализованных манифестов.
- `xrun diff <run-a> <run-b>` — манифесты + метрики side-by-side.
- Anomaly detection в poller (loss взлетел → notification).
- Cost forecasting (по средней стоимости похожих ранов).
- Native vast.ai REST вместо CLI subprocess (стабильнее на ошибках).
- Web UI рядом с TUI (тот же state, для шаринга по сети).
- Kaggle live-tail workaround через `[xrun-event]` stdout-маркер (best-effort).
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
