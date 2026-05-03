# TUI

Python Textual. Single-window app с chord-навигацией, command palette и общим status bar.

**Требования**: `pip install -e python/xrun_tui` (Python ≥ 3.11, Textual ≥ 0.70).

**Запуск**: `xrun` без аргументов (TTY) или `xrun-tui`.

## Экраны

### 0. First-run wizard (auto / `xrun init`)

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

Список vast/kaggle инстансов из адаптера. Показывает orphan-инстансы (без привязанного run) — `D` для destroy.

### 5. Vendors (g v / V)

Менеджер вендоров и кредов:

```
┌─ Vendors ───────────────────────────────────────────────────────────────┐
│ Vendor   Status              Account              Balance   Last checked │
│ vast     ✓ connected         user@example.com     $12.34    32s ago      │
│ kaggle   ✗ not configured                         —         —            │
│ mlflow   ⚠ unauthorized                           —         15s ago      │
└─────────────────────────────────────────────────────────────────────────┘
  e/Enter:edit  i:import  t:test  r:revoke  Esc:back
```

- **`e`** — masked-input форма для ввода ключей. Tab/Shift+Tab между полями. Enter — сохранить и запустить probe.
- **`i`** — импортировать существующий ключ: `~/.config/vastai/vast_api_key` для vast, `~/.kaggle/kaggle.json` для kaggle.
- **`t`** — принудительный probe.
- **`r`** — revoke (стирает ключ после confirm).

Фоновый probe запускается каждые 60s и по триггеру (после save / `t`). Баланс vast появляется в status bar после первого успешного probe.

**First-run splash**: если credentials пустые — ASCII-сплеш при старте. Любая клавиша открывает экран Vendors.

### 6. Settings (g s)

Тема, poll interval (active/idle), default vendor, MLflow URL, exclude-countries. Секция Database: размер файла, очистка завершённых runs.

### 7. Dashboard (g d)

Сводка: активные runs, spend today, burn rate, баланс.

### 8. Doctor (g h)

Проверки окружения: `xrun doctor` в TUI-форме. CLI-эквивалент: `xrun doctor`.

### 9. Compare

Сравнение метрик двух runs side-by-side. Открывается из Runs: выбрать первый (`c`), выбрать второй (`c`).

### 10. Artifacts

Браузер артефактов по всем runs (не только текущего). `P` — pull.

## Биндинги

### Глобальные

| Key | Action |
|-----|--------|
| `q` / `Esc` | Назад / выход |
| `?` | Help overlay |
| `:` | Command palette |

### Chord-навигация (лидер `g`)

| Chord | Экран |
|-------|-------|
| `g d` | Dashboard |
| `g r` | Runs |
| `g i` | Instances |
| `g v` | Vendors |
| `g s` | Settings |
| `g l` | Launch |
| `g h` | Doctor |

### Прямые клавиши (из Runs)

| Key | Action |
|-----|--------|
| `V` | Vendors |
| `Enter` | Открыть run detail |
| `L` | Launch picker |
| `s` | Stop выбранного run |
| `S` | Sync — `xrun fix-status` для stale-runs |
| `P` | Pull чекпоинт |
| `R` | Rerun |
| `/` | Фильтр |

### Run detail

| Key | Action |
|-----|--------|
| `tab` / `shift-tab` | Переключение вкладок |
| `s` | Stop run |
| `S` | Sync — `xrun fix-status <id>` для stale runs |
| `r` | Rerun |
| `p` | Pull последний чекпоинт |
| `a` | Открыть Artifacts |
| `o` | Открыть MLflow run в браузере |
| `e` | Открыть manifest в $EDITOR |
| `1`–`4` | Stages / Logs / Manifest / Metrics |

## Command palette

`:goto <screen>` — навигация по имени.  
`:launch <manifest>` — запустить манифест.  
`:stop <id>` — остановить run.

## Status bar

Три сегмента:
1. `xrun › <breadcrumb>` — текущий экран
2. `<vendor> <status-icon> $<balance>` — состояние дефолтного вендора
3. Screen hotkeys

Предупреждения: `⚠ <Nh runway` (красный) если `balance/burn < N часов`.

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
