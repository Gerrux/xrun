# TUI

ratatui + crossterm. Single-window app с стеком экранов и общим status bar.

## Экраны

### 1. Runs (главный)

```
┌── xrun ─────────────────────────────────── balance: $34.12  │  q quit  ─┐
│                                                                          │
│  ┌─ Active (2) ──────────────────────────────────────────────────────┐  │
│  │ ▶ arborust_v7_C        vast 4090   2h 14m   epoch 18/30   loss 0.41│  │
│  │   classifier_eb0       kgl  T4     0h 47m   uploading                │  │
│  └──────────────────────────────────────────────────────────────────┘  │
│                                                                          │
│  ┌─ Recent ─────────────────────────────────────────────────────────┐  │
│  │ ✓ arborust_v6γ_ep25    vast        14h 02m   F1 0.885             │  │
│  │ ✓ ablation_dropout03   vast         3h 51m   F1 0.879             │  │
│  │ ✗ tuba_winter          vast         0h 12m   FAILED: oom            │  │
│  └──────────────────────────────────────────────────────────────────┘  │
│                                                                          │
│  enter:open  L:launch  S:stop  P:pull  R:rerun  /:filter  T:tags        │
└──────────────────────────────────────────────────────────────────────┘
```

### 2. Run detail (Enter)

Tab'ы: **Stages** | **Metrics** | **Logs** | **Artifacts** | **Manifest**.

- **Stages**: вертикальный таймлайн с throbber на текущей. Цвета: pending grey, running yellow, ok green, failed red.
- **Metrics**: ratatui Chart, выбор серий через мульти-чекбокс справа, X-axis = step или time. `s` — сохранить PNG (через MLflow API → возвращает локальный путь). `o` — открыть MLflow run в браузере.
- **Logs**: tail с автоскроллом, поиск (`/`), пауза (`p`).
- **Artifacts**: дерево, `enter` — открыть в системе, `space` — пометить, `P` — pull выбранных.
- **Manifest**: read-only YAML, `e` — открыть в `$EDITOR` для последующего `rerun --patch`.

### 3. Launch (L на главном)

Picker по `exp/`. Превью манифеста справа. На enter — диф со схемой, валидация, preview плана (что зальётся, оценка стоимости vast). Confirm Y/N.

### 4. Instances (I)

Сырой список vast/kaggle инстансов из адаптера, не привязанных к runs (legacy/manual). Чтобы можно было погасить забытые.

```
GPU         price/h   uptime    run-id            status
RTX 4090    $0.48     2h 14m    arborust_v7_C     running
RTX 3090    $0.31     8h 03m    (orphan)          running   <- D=destroy
```

### 5. Settings (,)

Тема, override poll interval (active/idle), default vendor. Креды редактируются на экране Vendors.

### 6. Vendors (V)

Менеджер вендоров и кредов: статус подключения, баланс, ввод/импорт ключей.

```
┌─ Vendors ────────────────────────────────────────────────────────────────┐
│ Vendor   Status              Account              Balance   Last checked │
│ vast     ✓ connected         tester@example.com   $12.34    32s ago      │
│ kaggle   ✗ not configured                         —         —            │
│ mlflow   ⚠ unauthorized                           —         15s ago      │
└──────────────────────────────────────────────────────────────────────────┘
┌─ Detail ─────────────────────────────────────────────────────────────────┐
│    vendor: vast                                                          │
│ connected: yes                                                           │
│   account: tester@example.com                                            │
│   balance: USD 12.34                                                     │
└──────────────────────────────────────────────────────────────────────────┘
e/Enter:edit  i:import-native  t:test  r:revoke  Esc/q:back
```

- **`e` / Enter** — открывает masked-input форму для ввода/правки ключей.
  Для vast: `api_key`. Для kaggle: `username` + `key`. Для mlflow: `url` + `token`.
  Все секретные поля рендерятся как `••••`. Tab/Shift+Tab между полями. Enter — сохранить (`credentials.toml` 0600 на Unix) и сразу запросить probe.
- **`i`** — import существующего native-ключа: `~/.config/vastai/vast_api_key`
  для vast, `~/.kaggle/kaggle.json` для kaggle. Если файл есть — забирает оттуда и сохраняет в `credentials.toml`.
- **`t`** — внеочередной probe соединения (`vastai show user --raw` для vast).
- **`r`** — revoke (стирает ключ в credentials.toml после confirm).

Probe запускается автоматически фоном (раз в 60s) для каждого настроенного вендора, плюс по триггеру после сохранения / `t`.

**First-run splash**: если все credentials пустые, при старте `xrun` короткий ASCII-сплеш с логотипом и подсказкой; любая клавиша сразу открывает экран Vendors.

## Биндинги (глобальные)

| Key | Action |
|-----|--------|
| `q` / `Esc` | Закрыть текущий экран / выход |
| `?` | Help overlay |
| `:` | Command palette (vim-style: `:launch exp/foo.yaml`, `:goto vendors`) |
| `V` | Vendors screen |
| `,` | Settings |
| `tab` / `shift-tab` | Переключение tab'ов в run detail |
| `g g` / `G` | Top / bottom |
| `/` | Filter / search |

## Виджеты и крейты

Базовый стек: `ratatui`, `crossterm`, `tokio`, `tracing`.

Дополнительно (готовое из экосистемы):
- `throbber-widgets-tui` — спиннеры для running-стадий.
- `tui-logger` — встроенный лог-пейн (DEBUG в stderr пайпится сюда).
- `tui-input` — ввод для filter/command palette.
- `ratatui::widgets::Chart` — нативный line chart, хватит для метрик.
- `color-eyre` — error reporting.

Не используем: `tachyonfx` (overhead без очевидной пользы), `tui-realm` (overhead, наш state простой), сторонние chart-библиотеки (Chart хватит).

Metrics tab (графики метрик) — реализован в v0.3. В v0.2 вкладка Metrics отсутствует в Run detail; доступны Stages, Logs, Manifest.

## Архитектура TUI

```
fn main_loop() {
    let (tx_app, rx_app) = mpsc::channel();   // user events
    let (tx_data, rx_data) = mpsc::channel(); // poller pushes data updates

    spawn(input_handler → tx_app);
    spawn(poller(db, vendors) → tx_data);     // та же функция, что и в CLI

    loop {
        select! {
            ev = rx_app.recv() => state.apply(ev),
            data = rx_data.recv() => state.apply_data(data),
        }
        terminal.draw(|f| ui::render(f, &state));
    }
}
```

State целиком в памяти TUI, источник правды — SQLite. Запись в SQLite только через `xrun-core` API; TUI никогда не пишет в БД мимо него.

## Цвета и темы

Default theme — низкоконтрастный (greys + accent). `,` → Settings → theme переключает на high-contrast. Цвета стадий и статусов прибиты:

```
pending: dim grey
running: yellow + throbber
ok:      green
failed:  red bold
warn:    magenta
```
