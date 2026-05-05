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

## v0.4 — Recovery, sweeps, dataset workflow ✅ done

**Цель**: убрать ручную возню при падении поллера на Windows, дать
hyperparameter sweep одной командой, привести Kaggle dataset workflow внутрь
xrun. И всё это видно из TUI, не только из CLI.

### Scope

- [x] `xrun fix-status [<id>] [--dry-run]` — сверяет stale-running записи с
      вендором (Kaggle: `poll_completion`, vast: `vendor_instances`) и
      выравнивает store. Закрывает Issue 2 #13b.
- [x] `xrun doctor --manifest <path>...` — pre-flight: парсинг + схема +
      Kaggle (kernel slug, dataset readiness, креды). Закрывает Issue 2 #12.
- [x] `xrun dataset push|status|list` — обёртка над `kaggle datasets`,
      использует xrun-креды. Закрывает Issue 2 #7.
- [x] `xrun sweep <manifest> --grid PATH=v1,v2 [--launch] [--detach]` —
      декартово произведение, материализация N манифестов в
      `exp/sweep_<stem>_<ts>/`, опциональный батч-лонч.
- [x] xrun_hook wheel embed в `xrun-kaggle/build.rs`: best-effort find →
      опциональный auto-build (`XRUN_KAGGLE_AUTO_BUILD_WHEEL=1`) → strict
      mode (`XRUN_KAGGLE_EMBED_WHEEL=strict`). Закрывает Issue 2 #3b.
- [x] TUI surfacing stale runs: `last_event_ts` через subquery в
      `runs()`/`run()`, ⚠ маркер в Runs/Dashboard, `S` биндинг на
      `Runs`/`RunDetail` дёргает `xrun fix-status`.

### Acceptance

1. ✓ `cargo test --workspace` зелёный (включая 7 новых sweep-тестов).
2. ✓ `cargo clippy --workspace -- -D warnings` чистый.
3. ✓ `xrun sweep exp/base.yaml --grid lr=1e-3,5e-4 --grid batch=4,8 --dry-run`
   печатает 4 комбинации, JSON через `--json`.
4. ✓ После убитого поллера: `xrun fix-status` или `S` в TUI приводит run в
   терминальный статус без ручного редактирования SQLite.
5. ✓ Wheel автоматически вшивается, когда лежит под
   `python/xrun_hook/dist/`; компиляция без wheel'а не падает, только
   warn'ит и Kaggle-runs работают без live-метрик.

---

## v0.5 — Vendor phase 0: `xrun-local` ✅ done

**Цель**: запускать манифесты прямо на хосте — отладка без оплаты cloud-времени,
паритет с vast по lifecycle (provision → upload → execute → tail → pull →
destroy).

### Scope

- [x] Crate `crates/xrun-local/` с `LocalAdapter`.
- [x] `Vendor::Local` и `LocalSpec { gpu: Option<String> }` в `xrun-core`;
      валидация запрещает `vast`/`kaggle` блоки в local-манифесте, разрешает
      нативные пути в `data[].dst` (Windows: `C:\...`).
- [x] Shell resolver: bash → sh на Unix, pwsh → powershell.exe на Windows
      (`-NoProfile -NonInteractive -Command`).
- [x] `provision` (no-op + insert instance row), `execute` (sync setup,
      detached main subprocess, stdout/stderr → `<run-dir>/stdout.log`,
      env: `XRUN_RUN_DIR`/`XRUN_RUN_ID`/`CUDA_VISIBLE_DEVICES`, PID в
      `<run-dir>/run.pid`).
- [x] `tail` — прямое чтение локального файла с offset, missing-file = empty.
- [x] `upload` — `fs::copy` для файлов, рекурсивная копия для директорий.
      `mode: rsync`, `unpack`, `exclude`, `compress` — warn и skip.
- [x] `pull` — glob по `artifacts.patterns` относительно workdir, fs::copy в
      `--into` директорию, sha256, `record_artifact` в DB.
- [x] `destroy` — `kill -TERM` → wait → `kill -KILL` (Unix) / `taskkill /F /T
      /PID` (Windows). Идемпотентно. Удаляет `run.pid`, помечает `destroyed_at`.
- [x] `vendor_status` — best-effort `nvidia-smi --query-gpu=name,memory.free`,
      hostname в `account`. `connected=true`, balance=0.
- [x] `vendor_instances` — DB-запрос `vendor='local' AND destroyed_at IS NULL`
      + проверка PID alive через `kill -0` / `tasklist`.
- [x] PollerConfig override: для local пути `events_file`/`metrics_file`/
      `stdout_file` берутся в `<runs_dir>/<run-id>/`.
- [x] Dispatch `Vendor::Local` в `launch.rs`, `poll_daemon.rs`,
      `fix_status.rs`, `stop.rs`. `gc.rs` отфильтровывает не-vast рекорды
      (local cleanup делает `xrun stop`).
- [x] `xrun_hook` использует `XRUN_RUN_DIR` env и пишет events.jsonl там же,
      где их тейлит поллер (без изменений в самом хуке — он уже
      кросс-платформенный).
- [x] Тесты: 29 unit + integration в `xrun-local`, включая e2e (manifest →
      provision → execute → events.jsonl через `XRUN_RUN_DIR` → tail →
      destroy убивает PID).

### Acceptance

1. ✓ `cargo test --workspace` зелёный (271/277 тестов; 6 ignored — не
   связанные с local).
2. ✓ `cargo clippy --workspace -- -D warnings` чистый.
3. (нужно проверить на живом манифесте) `xrun launch exp/local-smoke.yaml`
   запускает `python train.py` локально, события и метрики через
   `xrun_hook` попадают в SQLite, `xrun pull <id> --ckpt best` копирует
   артефакт в `--into`.
4. ✓ `xrun stop <id>` убивает локальный subprocess через PID-файл,
   идемпотентно.

### Не входит в v0.5

- `vendor: ssh` (свой сервер / NAS / VPS) — попадает в v0.6 как продолжение
  vendor phase 0 (memory `project_vendor_roadmap.md`).
- RunPod / Lambda Labs / Lightning AI — в v0.6+ соответственно.

## v0.6 — Vendor phase 0 cont'd: `xrun-ssh`

**Цель**: запускать манифест на постоянно включенной машине через SSH.
Свой сервер, NAS, VPS. Always-on железо: provision/destroy не аллоцируют
аппаратуру, только per-run state.

### Scope

- [x] Crate `crates/xrun-ssh/` с `SshAdapter`.
- [x] `Vendor::Ssh` + `SshSpec { host_alias, workdir, gpu }` в `xrun-core`.
- [x] `[vendors.ssh.<alias>]` секция в credentials.toml: `host`/`user`/
      `port`/`key`/`default_workdir`. `BatchMode=yes` — ключи only, без
      пассвордов.
- [x] `cmd.rs` — pure command builders: `ssh_argv`, `rsync_upload_argv`,
      `rsync_download_argv`, `remote_launch_script`, `remote_size_script`,
      `remote_tail_script`, `shell_quote`. Все с unit-тестами.
- [x] `ssh.rs` — subprocess wrappers `ssh_exec`/`rsync`/`remote_file_size`/
      `remote_tail` с `BatchMode=yes`/`StrictHostKeyChecking=no` и
      `CREATE_NO_WINDOW` на Windows.
- [x] `provision`: `mkdir -p <workdir>/<run-id>` через ssh, insert instance row.
      `destroy`: `kill -TERM` → `kill -KILL` PID из `<run_dir>/run.pid`.
      `vendor_status`: nvidia-smi over ssh + `||` hostname fallback.
      `vendor_instances`: DB filter + `kill -0 PID` per row.
- [x] `upload`: rsync per DataSource. `pull`: rsync from glob with sha256
      + `record_artifact`. `tail`: `wc -c` size probe + `tail -c +N`.
- [x] Dispatch `Vendor::Ssh` в launch / poll_daemon (через стейкджед
      `manifest.yaml` копию) / fix_status (через тот же путь) / stop (через
      `XRUN_SSH_ALIAS` env override либо первый ssh-хост в creds).
- [x] Docs: `docs/MANIFEST.md` секция ssh + поля `[vendors.ssh.<alias>]`.

### Acceptance

1. ✓ `cargo test --workspace` зелёный (286 passed, 6 ignored).
2. ✓ `cargo clippy --workspace -- -D warnings` чистый.
3. (требует живой ssh-машины) `xrun launch exp/ssh_smoke.yaml` запускает
   тренировку на удалённой машине, события через `XRUN_RUN_DIR/events.jsonl`
   тейлятся через ssh, `xrun stop <id>` убивает PID.

### Не входит в v0.6

- ssh-agent integration (сейчас только `key=`-файл, BatchMode=yes).
- Password auth (out of scope — ключи only).
- Поддержка Windows-серверов (использует bash/tail/rsync).

## v0.7 — Pluggable metric sinks: WandB ✅ done

**Цель**: дать пользователю выбор, куда зеркалить метрики и логи помимо
MLflow. Та же модель, что у `xrun-mlflow` сегодня — отдельный crate-зеркало,
включаемое из конфига; hook на стороне training-скрипта остаётся прежним.

Skip 0.6.x — field-feedback ramp ушёл в 0.5.4, следующий milestone
заслуживает чистый minor bump.

### Scope

- [x] `MetricSink` trait в `xrun-core::metric_sink` (async via `async-trait`):
      `name` / `open_run` / `log_metrics_batch` / `log_artifact` / `finalize`.
- [x] Рефактор `xrun-mlflow` под trait (`MlflowSink`); поведение не изменилось.
- [x] `crates/xrun-wandb/` — `WandbClient` (graphql viewer + upsertBucket +
      file_stream) + `WandbSink: MetricSink` + 11 wiremock-тестов.
- [x] Poller fan-out: `MetricFanOut` держит `Vec<Box<dyn MetricSink>>`,
      `log_metrics` через `tokio::join!`-style sequence.
- [x] Schema migration 005: `mlflow_run_url`, `wandb_run_id`,
      `wandb_run_url` в `runs`. `Store::set_wandb_run` setter.
- [x] Credentials: `[wandb] api_key`, `xrun init --wandb-key -`.
- [x] `xrun config probe --vendor wandb` — auth probe через
      `viewer { entity }` GraphQL.
- [x] TUI Sinks screen (Python Textual `screens/sinks.py`): cards MLflow /
      WandB / Comet[v0.8 disabled], 5 status states (включая PAUSED для
      «key set but not in metrics.sinks list» mistake), edit/test/revoke/
      toggle-default. Routing: `g m` chord + palette.
- [x] `xrun init-manifest --vendor X --sink Y` — generator с
      `TODO_<field>` маркерами в каждом editable spot. Closes the loop:
      Claude Code продуцирует working manifest без знания схемы.
- [x] Wizard catalog: wandb `available_now` flipped to `True`. Wandb-
      специфичная форма в wizard остаётся todo (юзер вводит ключ через
      Sinks screen).

### Acceptance

1. ✓ `cargo test --workspace` зелёный — 404 теста, 0 failures.
2. ✓ Live smoke: `quickstart.yaml` с `metrics.sinks = ["wandb"]` →
   wandb run create, finalize, dashboard URL HTTP 200, DB row populated.
3. ✓ `xrun init-manifest --vendor local --sink mlflow` парсится через
   `xrun doctor --manifest`.
4. ✓ Multi-sink integration test (mlflow + wandb параллельно) зелёный.

### Не входит в v0.7

- WandB artifacts API (`log_artifact` возвращает `Disabled`) — нужен
  отдельный manifests + S3-presigned PUT surface. Отложено в v0.7.x
  patch или v0.8.
- Comet ML — слот в TUI catalog без impl. v0.8.
- Wizard form для wandb (api_key input) — checkbox enable'ится, но без
  inline формы. Юзер ставит ключ через Sinks screen после wizard'а.

## v0.8 — Comet ML + WandB artifacts

**Цель**: закрыть оставшиеся gaps из v0.7. Comet как третий sink + WandB
artifacts API для checkpoint-uploads. TensorBoard sink — backlog.

### Scope

- [ ] `crates/xrun-comet/` — `CometClient` + `CometSink: MetricSink`. REST
      `www.comet.com/api/rest/v2/`; auth через `[comet] api_key=...`.
      Wiremock-тесты по образцу xrun-wandb.
- [ ] `WandbSink::log_artifact` — manifests + S3-presigned PUT API.
      Проверять size cap (wandb free tier — 100 GB/team), graceful fallback
      на `Disabled` при превышении.
- [ ] Schema migration 006: `comet_run_id`, `comet_run_url` в `runs`.
- [ ] TUI Sinks screen: enable comet card (flip `[v0.8]` badge → READY/
      EMPTY), edit form.
- [ ] `xrun init-manifest --sink comet` поддержка.
- [ ] Wizard form для wandb api_key (был отложен из v0.7).
- [ ] `docs/METRICS.md` — модель fan-out, как добавить свой sink.

### Не входит в v0.8

- Auto-import существующих WandB/Comet runs (только новые с момента launch).
- Sweep-интеграция с WandB Sweeps API (отложено в v0.9).
- TensorBoard sink (минорный спрос; можно отдельно после v0.8).

## v0.9+ (backlog)
- RunPod (`crates/xrun-runpod/`): REST + SSH, копия `xrun-vast` с другим API.
- Lambda Labs (`crates/xrun-lambda/`): REST + SSH; стабильные цены, проще
  для `--max-cost`.
- Lightning AI (`crates/xrun-lightning/`): poll-стиль (как Kaggle), 80
  GPU-ч/мес бесплатно — нужна проверка REST.
- `xrun diff <run-a> <run-b>` — манифесты + метрики side-by-side.
- Anomaly detection в poller (loss взлетел → notification).
- Cost forecasting (по средней стоимости похожих ранов).
- Native vast.ai REST вместо CLI subprocess (стабильнее на ошибках).
- Web UI рядом с TUI (тот же state, для шаринга по сети).
- Kaggle live-tail workaround через `[xrun-event]` stdout-маркер (best-effort).
- Скилл-плагин в формате Claude Code marketplace.
- Sweep aggregations: общий `sweep_id` в DB, агрегированные метрики
  (best run, parallel coordinates) в Compare-экране.

## Field feedback (2026-05-04, arborust offset_v1 kaggle launch)

Накопилось во время попытки прогнать `treetop3d-offset-v1-full` на Kaggle. Все
пункты приоритизированы по «сколько раз сегодня бы пригодилось».

### High value

- [x] **Pre-flight RAM/disk constraints в манифесте.** Поле
  `requires.{ram_gb,disk_gb}` добавлено в schema (`xrun-core` `Requires`).
  `xrun doctor --manifest` зовёт `requires_checks` с lookup-таблицей
  vendor-лимитов (Kaggle ≈ 13 GB RAM / 73 GB working disk; vast/local/ssh
  → warn-only без статических caps). Превышение → fail с понятной
  подсказкой («pick a vendor with more RAM or reduce batch size»).
  Документация в `docs/MANIFEST.md`.

- [x] **Live tail логов Kaggle до завершения kernel.** Закрыто через
  xrun_hook → MLflow chunked artifacts: poll-daemon тейлит chunked artifact
  с kernel-side, обновляет локальный `stdout.log`, `xrun logs --follow`
  стримит его. Public Kaggle API не отдаёт стрим до завершения kernel'а
  (это ограничение их платформы, не xrun) — без MLflow path будем зависеть
  от `[xrun-event]` stdout-маркера, оставленного в v0.7+ backlog. Сегодняшние
  Kaggle-раны пишут в MLflow, наблюдение работает.

- [x] **`xrun dataset push` запоминает snapshot, `xrun show` показывает
  dataset summary.** Auto-retry на transient errors (timeout 1800s + 2
  retry с exp backoff на 502/503/504, connection reset, EOF) — override:
  `XRUN_KAGGLE_DATASET_RETRIES`/`XRUN_KAGGLE_DATASET_TIMEOUT_SECS`. Покрыто
  unit-тестами `test_dataset_push_retries_transient_create_failure` /
  `test_dataset_push_does_not_retry_permanent_failure`. После push'а
  `<data_dir>/dataset_snapshots/<owner>__<name>.json` хранит file list +
  size/mtime + captured_at; `xrun show` для манифестов с `kaggle.dataset`
  печатает `dataset: <slug> (<N> files, <X> MiB; last push <ts>)` в text-
  и JSON-выводах. БД не трогали — sidecar дешевле миграции и идёт в одном
  flow со snapshot-diff'ом.

### Medium value

- [x] **Точный health-check для Kaggle CLI.** `xrun doctor` теперь
  аутентифицируется через python-модуль (`KaggleApi().authenticate()`),
  не парсит stdout `kaggle config view` regex-ом. `KaggleProcessReal::
  authenticate_via_python` пробует `python`/`python3`/`py`, при отсутствии
  модуля падает обратно на legacy stdout-парсинг. `kaggle datasets status -m`
  уже без `-m` (фиксили раньше).

- [x] **`xrun rerun --bump-dataset <DIR>`.** При rerun сравнивает
  staging-dir со снапшотом последнего push (size+mtime sidecar в
  `<data_dir>/dataset_snapshots/<owner>__<name>.json`). Если изменилось —
  `kaggle datasets version`, потом launch; иначе skip.

- [x] **Kaggle GPU quota в `xrun balance` + warn в `xrun launch`.** Public
  Kaggle API без quota-эндпоинта → ввели env-override
  `KAGGLE_GPU_QUOTA_REMAINING_HOURS`; balance его подхватывает, launch
  warn'ит когда `--max-hours > remaining`.

### Low value / quality of life

- [x] **Auto-bump kernel slug через placeholders.** `kernel_slug` теперь
  понимает `{run_id}` и `{date}` (UTC `YYYYMMDD`); `expand_kernel_slug()`
  разворачивает их в `provision()`. Doctor warn-only check скипается
  при наличии placeholder. Auto-bump суффикса на коллизии не нужен —
  `{run_id}` всегда уникален.

- [x] **`xrun init --non-interactive --validate-creds`.** Пробивает vast
  (`show_user`) и kaggle (`KaggleApi().authenticate()`) после записи
  кредов и при успехе implicitly выставляет `wizard_completed=true`.
  При фейле — non-zero exit и флаг не трогается. JSON-summary включает
  `probes.<vendor>.{ok, detail|error}`.

### Field feedback addendum (2026-05-04 evening, same arborust session)

Запустили `treetop3d-offset-v1-full` (run `01KQRXZ3ZQB2CEH2MK0S4RR38X`),
training идёт стабильно (epoch=4 loss=1.52 val_F1=0.69), но **из xrun
наблюдать ничего нельзя**. Всё через ручной `Read` файла артефактов и сторонний
Kaggle TUI/web. Подытожу, что реально болит, чтобы не повторять.

#### Critical (видимость прогона = ноль)

- [x] **Stdout metrics parser подключён в poll loop.** `parse_stdout_metrics`
  (был с v0.3, но висел без вызовов) теперь дёргается на каждый chunk
  stdout в `loop_runner.rs`: line-buffered, `INSERT OR REPLACE` на
  `(run_id, key, step)` безопасно перекрывается с `metrics.jsonl`-путём,
  MLflow-mirror получает те же точки. Покрыто
  `test_poller_extracts_metrics_from_stdout`.
- [x] **`xrun logs --follow` для Kaggle/local теперь реально стримит.**
  Раньше делал one-shot read и выходил; теперь тейлит локальный
  `stdout.log` (его обновляет poll-daemon из MLflow chunked artifacts для
  Kaggle / напрямую для local) до достижения терминального статуса.
  Если файл пуст — печатает тот же diagnostic, что и не-follow вариант.
- [x] **Stale-status false positive снят.** `is_stale()` в Python TUI
  больше не использует event-silence как fallback — только живость
  `poller_pid`. Длинные Kaggle-раны (не пишут events между
  `running:start` и `done:ok`) больше не маркируются ⚠ stale.

_(Три исходные Critical-формулировки — «MLflow/metrics tracking не
подключен», «Live log fetch для активного run», «Stale-status false
positive» — закрыты ✅ выше: stdout parser wired в poll loop,
`xrun logs --follow` тейлит локальный stdout.log, `is_stale()` смотрит
только на живость poller_pid. Нативная MLflow-интеграция как опция уже
была подключена в v0.3 — здесь не нужна, stdout-канала достаточно.)_

#### High

- [x] **`xrun launch --detach` блокирует** ⇒ Watchdog поверх
  `kaggle kernels push`: 600s default, override через
  `XRUN_KAGGLE_PUSH_TIMEOUT_SECS`. Wedged subprocess теперь убивается с
  понятной ошибкой вместо silent multi-min hang. Корневую причину
  невозможно репро без живого окружения, но видимый симптом (бесконечный
  hang) теперь невозможен.

- [x] **`kaggle datasets version` silent skip detection.** Перед push'ем
  `xrun dataset push` (и `xrun rerun --bump-dataset`) фингерпринтит
  staging-dir (size+mtime per file, без `dataset-metadata.json`),
  сравнивает с предыдущим снапшотом (sidecar `<data_dir>/
  dataset_snapshots/<owner>__<name>.json`) и печатает diff
  added/changed/removed/unchanged. После успеха снапшот перезаписывается.

- [x] **Pre-baked cache idempotence pitfall — `xrun dataset verify`.**
  `xrun dataset verify <staging-dir> [--marker meta.json]` обходит
  first-level subdirs и репортит те, в которых нет marker-файла. JSON-вывод
  через `--json`. Exit 1 при наличии битых subdir'ов. Локальный smoke-тест
  до push'а — ловит «пустой `<plot>/`, упал в voxelize, оставил каталог»
  раньше, чем Kaggle FS откажет на повторе.

#### Medium

- [x] **`numpy<->torch` compatibility probe.** После `setup` блока в Kaggle
  main.py (между setup и cmd) дёргается `python -c "import torch, numpy
  as np; torch.from_numpy(np.zeros(1))"`. Если torch не установлен — skip
  silently; если ABI-mismatch — fail loud с подсказкой про numpy<2 до
  старта тренировки (а не через 4 минуты в DataLoader).

## Field feedback (2026-05-05, arborust v9-skipalpha kaggle launch) → v0.5.4

Запустили `treetop3d-v9-skipalpha-*` сериал на Kaggle через `run.notebook`,
training шёл нормально, но **из xrun наблюдать ничего нельзя** — все 4
ранов показывали 0 events / 0 metrics в `xrun events` / `xrun metrics`.
Корневая причина и три bonus-наблюдения собраны в
`issue_2026_05_05_notebook_mode_no_livestream.md` (удалён в этом релизе,
содержимое перешло в commit message + CHANGELOG).

### Critical

- [x] **Notebook-mode now ships live telemetry.** `crates/xrun-kaggle/src/
  notebook_inject.rs` прибавляет одну bootstrap-ячейку (тег
  `xrun-bootstrap`) в начало пользовательского `.ipynb`: base64-decode
  wheel → `pip install --no-deps --quiet` → `os.environ['MLFLOW_*'] =
  …`. До 0.5.4 это работало только для `run.cmd` (script-mode) — wheel
  base64-эмбеддился в `main.py`, а notebook оставался без MLflow env
  vars и без `xrun_hook` install. Симптом был «silent» — kernel
  отрабатывал успешно, но `xrun events` видел только host-side
  `queued:start` / `running:start`.

- [x] **Streamed terminal promotion.** `ingest_telemetry_chunks` теперь
  возвращает `Option<RunStatus>`, и `poll_completion` промоутит run в
  `Failed`/`Done` + cancellит kernel, если ingestнутый event сигналит
  `status=fail` или `stage=done`. Без этого CUDA-OOM продолжал есть
  compute пока Kaggle-state не доедет до `Error/Complete` (минуты —
  десятки минут).

### Medium / quality of life

- [x] **`xrun resume` parses kaggle `kernels list` correctly.**
  Переключились на `--csv` с quote-aware splitting; JSON-путь
  сохранён для существующих моков. Раньше fall-through на
  `vendor_instances` ронял resume с `Expecting value: line 1
  column 1 (char 0)`.

- [x] **Stdout phantom metrics.** `parse_stdout_metrics` ловил
  `numpy>` (из `numpy>=2.0`), `dropout`, `w[0]`, `tobler` etc. как
  `count: 1` keys. Теперь требует strict Python identifier на key +
  явный `epoch=`/`step=` anchor для `=`-формы. `:`-форма (стандартный
  PyTorch/HF print) не тронута.

- [x] **Kaggle CLI 1.8.x outdated-version banner.** Литерал-template
  warning из `kaggle` ломал наш JSON parser. Новые
  `strip_kaggle_cli_noise` / `annotate_kaggle_cli_failure` дропают
  warning-line до парсера и приклеивают actionable hint
  (`pip install --upgrade kaggle`) к финальной ошибке.

- [x] **Pip-eager template caveat.** `exp/templates/README.md`
  получил секцию «что НЕ переустанавливать»: Kaggle container уже
  несёт scipy/sklearn/pandas/numpy/torch (cu128, sm_60+),
  `pip install torch` стоит 15–20 минут runtime, старые
  «P100 detected → reinstall torch 2.2.2+cu118» рецепты устарели.

### TUI

- [x] **Polyline overlay в metrics chart.** `render_chart_multi`
  принимает `lines: bool`, рисует `─ ╱ ╲ │` между точками. `C` в
  Run-detail Metrics tab переключает; toolbar пишет `lines: on/off`.



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
