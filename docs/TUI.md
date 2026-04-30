# TUI

Python Textual. Single-window app с chord-навигацией, command palette и общим status bar.

**Требования**: `pip install -e python/xrun_tui` (Python ≥ 3.11, Textual ≥ 0.70).

**Запуск**: `xrun` без аргументов (TTY) или `xrun-tui`.

## Экраны

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

### 2. Run detail (Enter из Runs)

Вкладки: **Stages** | **Logs** | **Metrics** | **Artifacts** | **Manifest**

- **Stages**: таймлайн с throbber на текущей стадии. Цвета: grey pending, yellow running, green ok, red failed.
- **Logs**: читает локальный снапшот `stdout.log` (поллер обновляет каждые ~5s). Для live-стриминга: `xrun logs <id> --follow` в терминале.
- **Metrics**: ключи метрик из SQLite, ASCII chart по выбранному ключу. `o` — открыть MLflow run в браузере.
- **Artifacts**: дерево артефактов. `P` — pull выбранных.
- **Manifest**: read-only YAML. `e` — открыть в `$EDITOR`.

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
| `S` | Stop выбранного run |
| `P` | Pull чекпоинт |
| `R` | Rerun |
| `/` | Фильтр |

### Run detail

| Key | Action |
|-----|--------|
| `tab` / `shift-tab` | Переключение вкладок |
| `o` | Открыть MLflow run в браузере |
| `e` | Открыть manifest в $EDITOR |
| `P` | Pull артефакты |

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
