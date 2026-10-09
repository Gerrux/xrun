# TUI

Python Textual. Single-window app с chord-навигацией, command palette и общим status bar.

**Требования**: `pip install -e python/xrun_tui` (Python ≥ 3.11, Textual ≥ 0.70).

**Запуск**: `xrun` без аргументов (TTY) или `xrun-tui`.

**Заставка** показывает знак xrun и чек-лист старта (БД, креды, вендоры,
манифесты). Знак анимирован: плитка проявляется, кривая потерь прорезается
сверху вниз, последней появляется точка лучшего чекпоинта (~0,6 с). Старт
анимация не задерживает: как только чек-лист готов, знак сразу встаёт в
последний кадр. При `TEXTUAL_ANIMATIONS=none` или `basic` знак рисуется
сразу целым. Размер знака — по высоте терминала: от 29 строк — 32 px
(16 строк), от 25 — 24 px (12 строк), ниже знак не показывается. `xrun init`
заставку пропускает.

## Экраны

### 0. First-run wizard (auto / `xrun init`)

Пять шагов: Local → Vendors → Logging → **Notify** → Done. Шаг Notify
предлагает ntfy с уже сгенерированным топиком и desktop-toast; «Next,
Next, Finish» даёт рабочий push без ввода текста.

Запускается автоматически при первом старте TUI (когда
`[ui] wizard_completed = false` в `config.toml`) или явно через `xrun init`.
Один экран, четыре шага: **Local** → **Vendors** → **Logging** → **Done**.

```
┌── xrun — Setup ────────────────────────────────────────────────────────┐
│  ✓ Local  →  ● Vendors  →  ○ Logging  →  ○ Done                       │
│                                                                        │
│  Step 2 — Vendors                                                      │
│  Toggle vendors. Press [o] on a card to open its API-key page.         │
│                                                                        │
│  ●  vast.ai          GPU spot marketplace          key set             │
│     [paste vast.ai API key……………………………………………………]                        │
│  ○  Kaggle           Free notebooks (mlflow live)  no key              │
│  ○  SSH machine      Your own server / NAS / VPS   no key              │
│  ○  RunPod          [v0.7+]                        no key              │
│  ○  Lambda Labs     [v0.7+]                        no key              │
│                                                                        │
│  [Back  Ctrl+B]  [Skip wizard  Esc]  [Next  Ctrl+N]                    │
└────────────────────────────────────────────────────────────────────────┘
```

| Шаг | Что делает |
|------|-----------|
| Local    | Запускает `xrun init --probe-local --json`; показывает OS/GPU. Спиннер пока probe не вернулся. |
| Vendors  | `Checkbox` per vendor — Tab/Space навигация. Vast/Kaggle открывают password-Input при выборе. `o` открывает API-key страницу focused-карточки (работает ДО выбора). |
| Logging  | Radio: `off` / `polling` (default) / `polling+mirror`; для mirror — Checkbox-список sinks (mlflow ✓; wandb/comet `[v0.8]` disabled). При выбранном Kaggle подсветка-подсказка про mirror. |
| Done     | Recap + live `xrun doctor --json` (✓/⚠/✗ по чекам) + `Finish` пишет конфиг через `xrun init --non-interactive --mark-completed --sink ...` (ключи — через `xrun config set`). |

`Esc`/`Skip wizard` показывает confirm-modal (Y/N) — случайный Esc больше не
сбрасывает прогресс. После подтверждения ставит `wizard_completed = true`
без записи выбранных вендоров/sinks; вернуться можно через `xrun init`.

### 1. Runs (g r)

```
┌── xrun › runs ───────────── vast ✓ $12.34  │  g:goto  ?:help  ::cmd ─┐
│                                                                        │
│  Active (2)         Vendor   Run: $0.42/hr · cap-left $4.21            │
│  ▶ arborust_v7_C    vast     2h 14m   epoch 18/30   loss 0.41          │
│    classifier_eb0   kaggle   0h 47m   uploading                        │
│                                                                        │
│  Recent                                                                │
│  ✓ arborust_v6γ     vast     14h 02m   F1 0.885                        │
│  ✓ ablation_drop    vast      3h 51m   F1 0.879                        │
│  ✗ tuba_winter      vast      0h 12m   FAILED: oom                     │
│                                                                        │
│  enter:open  L:launch  S:stop  P:pull  R:rerun  /:filter               │
└────────────────────────────────────────────────────────────────────────┘
```

Dashboard cards сверху: текущий burn `$/hr`, `cap-left $X.XX`, `today $spent`.

**Stale runs** — `running`-записи без событий ≥30 минут получают `⚠ stale` в
колонке Status и в счётчике дашборда. `S` зовёт `xrun fix-status <id>` (или
все running-runs если ни один stale не выбран) и обновляет статус в БД из
ответа вендора. Лечит ситуацию когда поллер умер посередине (Windows: после
`cargo install --force` исполняемый файл подменён, дочерний процесс
поллера упал молча).

### 2. Run detail (Enter из Runs)

Вкладки: **Stages** | **Logs** | **Metrics** | **Artifacts** | **Manifest**

- **Stages**: таймлайн с throbber на текущей стадии. Цвета: grey pending, yellow running, green ok, red failed.
- **Logs**: читает локальный снапшот `stdout.log` (поллер обновляет каждые ~5s). Для live-стриминга: `xrun logs <id> --follow` в терминале.
- **Metrics**: левая палитра ключей с спарклайнами (`MetricsPalette`),
  справа `MetricsView` — таблица final-значений + grid с одним subplot на ключ.
  `o` — открыть MLflow run в браузере; `g` (в --png export) включает
  per-key grid; см. `xrun metrics --per-key --png`.
- **Artifacts**: дерево артефактов. `P` — pull выбранных. `Enter` на
  PNG/JPG открывает встроенный `ImageView` (ASCII-preview через chafa-style).
- **Manifest**: read-only YAML. `e` — открыть в `$EDITOR`.
- **Report**: `ReportView` рендерит `report.md`/`report.html` артефакт run-а
  (если есть) — markdown в Textual-нативном виде.

### 3. Launch (g l)

Picker по `exp/`. Превью манифеста справа. Enter → confirm с оценкой стоимости.

### 4. Instances (g i)

Две вкладки. **All vendors** (по умолчанию) — инстансы всех вендоров из локальной БД, с колонкой Vendor. **vast.ai (live)** — живой список с vast.ai, включая orphan-инстансы (без привязанного run); `x` — destroy, работает только на этой вкладке; подтверждение по умолчанию на «No». Раны остальных вендоров останавливаются через `s` на экране Runs.

Строка-сводка под заголовком относится к активной вкладке: на All vendors — активные инстансы по вендорам и число уничтоженных, на vast.ai — запущенные, суммарный $/hr и uptime. Без vast-ключа вкладка vast.ai не опрашивается по таймеру (ключ перепроверяется при `Ctrl+R` и при открытии вкладки).

### 5. Vendors (g v / V)

Менеджер вендоров и кредов. Шесть карточек, все вендоры равноправны:

| Карточка | Что настраивается |
|---|---|
| Local machine | ничего — готов всегда; `t` проверяет окружение |
| SSH hosts | свои серверы; `Enter` открывает список хостов |
| vast.ai | API-ключ, SSH-ключи, исключение стран |
| Kaggle | токен или legacy username+key |
| Lightning AI | user_id, API-ключ, teamspace (необязательно); `t` — проверка через `xrun config probe --vendor lightning` |
| Google Colab | формы нет: готов, когда есть токен; вход — `xrun config login colab`, `t` — проверка |

**SSH hosts** (`Enter` на карточке SSH): `a` — добавить, `Enter`/`e` — изменить, `t` — проверить подключение, `r` — удалить (с подтверждением). Поля хоста: alias, host, user, port, путь к ключу, рабочая папка по умолчанию.

Клавиши ниже относятся к карточкам vast.ai и Kaggle; на Local, SSH и Colab импорт и revoke не применяются.

- **`e`** — masked-input форма для ввода ключей. Сохранённый ключ в поле не подставляется: виден только его хвост в placeholder, пустое поле при сохранении значит «оставить как есть». Удаление ключа — только через `r`.
- **`i`** — импортировать существующий ключ: `~/.config/vastai/vast_api_key` для vast, `~/.kaggle/kaggle.json` для kaggle.
- **`t`** — принудительный probe.
- **`r`** — revoke (стирает ключ после confirm).

Фоновый probe запускается каждые 60s и по триггеру (после save / `t`). Баланс vast появляется в status bar после первого успешного probe.

**First-run splash**: если credentials пустые — ASCII-сплеш при старте. Любая клавиша открывает экран Vendors.

### 6. Settings (g s)

Тема, лимит истории, poll interval (active/idle), default vendor и exp dir, бюджетные лимиты. Вкладка Updates: фоновая проверка релизов `update.auto` (Notify — пуш `update.available` раз на релиз через каналы `g n`, Off — без проверки и сетевых запросов; поле пустое, если бинарник xrun — v0.9.0 или раньше). Секция Storage: размер файла, очистка завершённых runs (с подтверждением). Сохраняются только изменённые поля; очищенное поле возвращается к значению по умолчанию.

У каждой настройки один редактор. Ключи вендоров и exclude-countries — в Vendors (`g v`), MLflow / WandB и список sinks — в Sinks (`g m`), каналы уведомлений — в Notifications (`g n`). Все записи идут через `xrun config set` / `xrun config unset`; секреты передаются через stdin (`--stdin`), TUI сам `credentials.toml` не пишет.

### 7. Dashboard (g d)

Сводка: активные runs, spend today, burn rate, баланс.

### 8. Doctor (g h)

Проверки окружения: `xrun doctor` в TUI-форме. CLI-эквивалент: `xrun doctor`.

### 9. Compare

Сравнение метрик двух runs side-by-side. Открывается из Runs: выбрать первый (`c`), выбрать второй (`c`).

### 10. Artifacts

Браузер артефактов по всем runs (не только текущего). `P` — pull.

### 10a. Sweep (g x)

Раны, сгруппированные по папке манифеста; в заголовке группы — лучший ран по основной метрике со стрелкой направления (`best ↓ val_loss`, `best ↑ val_f1`). «Лучший» — лучшее значение за всю историю рана по ключу группы (не последнее), как `best` в `xrun diff`; значение в строке рана — тоже оно. Основной ключ рана выбирается так же, как для спарклайна на Dashboard (loss, val_loss, accuracy, … по приоритету), NaN-точки пропускаются. Направление определяется по имени метрики (loss, error, mae, mse, rmse, perplexity, ppl, wer, cer, fid, nll, eer, bpb, bpc — меньше лучше; остальное — больше лучше). Экран Sweep не читает `policy.early_stop` из манифестов (это делает только `xrun diff`), поэтому если имя метрики вводит в заблуждение, нажмите `m`: он переворачивает направление для группы под курсором до конца сессии. Переопределение в `xrun diff` — `--direction KEY=min|max`.

### 11. Notifications setup (`g n`)

Карточки каналов: **ntfy** (push на телефон, топик генерируется сам),
**Telegram** (токен от @BotFather, chat id определяется кнопкой Detect
после первого сообщения боту), **Webhook** (Slack/Discord/свой JSON),
**Desktop** (toast, без настройки). Плюс карточка **Rules** (пресеты:
всё / только проблемы / только деньги; проценты budget.warn; порог
heartbeat) и **Watchdog** (Enter регистрирует/снимает запись в Task
Scheduler / crontab через `xrun watchdog schedule`).

| Key | Action |
|-----|--------|
| `Enter` / `e` | Форма канала (Save & test включает канал и сразу шлёт тест) |
| `Space` / `d` | Включить / выключить канал |
| `t` | Тестовое уведомление через этот канал |
| `r` | Забыть креды канала |
| `h` | История уведомлений |

Секреты пишутся в `credentials.toml`, список каналов и правила — через
`xrun config set`, так что CLI и TUI видят одно и то же. Работающие
поллеры подхватывают изменения сами в течение ~5 с — перезапускать
обучение не нужно.

### 12. Notifications history (`n`)

История in-app toast'ов плюс журнал push-уведомлений (`xrun notify log`):
что poll-daemon / watchdog отправили на телефон, в какой канал и дошло ли
(`push/error` с текстом ошибки, если канал сломан). `c` — очистить in-app
историю (журнал в SQLite остаётся).

Каждые 60 с TUI вызывает `xrun watchdog --json`: мёртвые поллеры
поднимаются, orphan-инстансы всплывают toast'ом (один раз за сессию) и
уходят в push-каналы.

## Биндинги

### Глобальные

| Key | Action |
|-----|--------|
| `Esc` | Назад (на формах с несохранёнными правками — вопрос «Discard unsaved changes?») |
| `q` | Назад; на Runs и Dashboard — выход |
| `?` | Help overlay (повторное нажатие закрывает) |
| `ctrl+p` | Command palette |
| `ctrl+o` | Jump |
| `n` | История уведомлений |

Актуальный список клавиш каждого экрана — в Help (`?`): раздел навигации там строится из того же реестра экранов, что хорды и палитра (`screens/registry.py`).

### Chord-навигация (лидер `g`)

| Chord | Экран |
|-------|-------|
| `g d` | Dashboard |
| `g r` | Runs |
| `g w` | Watch |
| `g b` | Budget |
| `g x` | Sweep |
| `g i` | Instances |
| `g v` | Vendors |
| `g m` | Sinks |
| `g h` | Doctor |
| `g l` | Launch |
| `g s` | Settings |
| `g n` | Notifications setup |

Переход на уже открытый экран возвращает к нему, а не открывает второй экземпляр. Если по пути закрывается форма с несохранёнными правками, сначала будет вопрос; во время сохранения переход отклоняется.

### Runs

| Key | Action |
|-----|--------|
| `Enter` | Открыть run detail |
| `l` | Launch picker |
| `s` | Stop выбранного run (подтверждение, по умолчанию «No») |
| `S` | Sync — `xrun fix-status` для stale-runs |
| `p` | Pull последнего чекпоинта |
| `r` | Rerun (подтверждение, по умолчанию «No») |
| `R` | Rerun с правкой аргументов |
| `space` | Отметить ран для массового действия |
| `ctrl+s` / `P` | Stop / Pull отмеченных (в подтверждении перечислены раны) |
| `c`, затем `C` | Отметить два рана и открыть Compare |
| `G` | Группировка по манифесту |
| `f` или `/` | Фильтр |

Подтверждённые действия выполняются в фоне: экран остаётся отзывчивым, результат приходит уведомлением. Пока действие над раном не завершилось, повторно запустить его же нельзя.

### Run detail

| Key | Action |
|-----|--------|
| `s` | Stop run |
| `S` | Sync — `xrun fix-status <id>` для stale runs |
| `r` | Rerun |
| `p` | Pull последний чекпоинт |
| `a` | Открыть Artifacts |
| `o` | Открыть MLflow run в браузере |
| `e` | Открыть manifest в $EDITOR |
| `1`–`5` | Stages / Logs / Manifest / Metrics / Report |
| `ctrl+f` | Поиск по логам |

## Command palette

`ctrl+p` открывает список команд с фильтром по подстроке: переход на любой экран из реестра, Help, Refresh, Quit. Аргументов команды не принимают — запуск манифеста и остановка рана делаются на экранах Launch и Runs.

## Status bar

Слева направо: число активных ранов и их разбивка по вендорам (`● 3 active  local 1 · ssh 2`), затем — если настроены — аккаунт vast.ai с балансом и аккаунт Kaggle, справа часы. Для local и ssh аккаунтов нет, поэтому они видны только в разбивке активных ранов.

## Темы

Доступные: `tokyo-night` (default), `catppuccin-mocha`, `gruvbox-dark`.  
Переключение: Settings → Theme → Ctrl+S. Полный эффект после перезапуска.

## Архитектура

```
xrun (Rust CLI)
  └─ при запуске без аргументов: spawn xrun-tui (Python Textual binary)

xrun-tui (Python)
  ├─ читает SQLite напрямую (aiosqlite, тот же runs.db)
  ├─ вызывает xrun CLI через asyncio subprocess (stop, pull, launch, config)
  └─ не пишет в SQLite напрямую — только через CLI
```

Python TUI и Rust CLI используют одну БД (WAL mode — concurrent read-write безопасен).
