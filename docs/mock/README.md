# Макет для скриншотов

Картинки для README, сайта и статей двух видов: терминал и social preview
рисует `index.html`, TUI снимает `tui.py` (раздел «TUI» ниже).

`index.html` рисует картинки для README и статей из демо-данных в оформлении
самого xrun: терминальный блок и подсветка YAML — как на сайте
(`docs/index.html`), палитра — из `docs/brand.md`. Вывод CLI собирается по тем
же ширинам колонок и правилам, что в Rust-коде (`ls.rs`, `events_cmd.rs`,
`launch.rs`, `ascii_chart.rs`, `pull.rs`), и, как у самого CLI, без цвета.
Цветные только приглашение `$` и YAML. Демо-данные — один объект `demo`:
никаких реальных проектов, путей и хостов.

Сменился формат вывода — поправь порт форматтера в `index.html` и переснимай.

| Сцена | Файл | Окно | Масштаб |
| --- | --- | --- | --- |
| `launch` — `xrun launch --detach`, потом `xrun events --follow` посреди тренировки | `docs/screenshots/launch.png` | 760×737 | 2 |
| `result` — манифест рядом с `xrun ls`, `xrun metrics --ascii` и `xrun pull` | `docs/screenshots/result.png` | 1320×758 | 2 |
| `social` — social preview для GitHub, X, Reddit (до 1 МБ) | `docs/screenshots/social.png` | 1280×640 | 1 |

## Пересъёмка (Windows, Git Bash)

Из корня репозитория. Chrome запускается из `$TEMP`: длинный путь текущего
каталога ломает нативные программы в Git Bash.

```bash
R="$(pwd -W)"; cd "$TEMP"
C="/c/Program Files/Google/Chrome/Application/chrome.exe"
"$C" --headless=new --hide-scrollbars --force-device-scale-factor=2 --window-size=760,737 \
  --screenshot="$R/docs/screenshots/launch.png" "file:///$R/docs/mock/index.html?scene=launch"
"$C" --headless=new --hide-scrollbars --force-device-scale-factor=2 --window-size=1320,758 \
  --screenshot="$R/docs/screenshots/result.png" "file:///$R/docs/mock/index.html?scene=result"
"$C" --headless=new --hide-scrollbars --force-device-scale-factor=1 --window-size=1280,640 \
  --screenshot="$R/docs/screenshots/social.png" "file:///$R/docs/mock/index.html?scene=social"
cd - >/dev/null
```

Запасной браузер: `"/c/Program Files (x86)/Microsoft/Edge/Application/msedge.exe"`
с теми же флагами. На macOS: `"/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"`.

Высота окна должна совпасть с высотой сцены: изменил содержимое — открой
`index.html?scene=…` в браузере, посмотри высоту `.scene` и поправь
`--window-size` здесь и в таблице. После съёмки открой каждый PNG и проверь
обрезку снизу, полосы прокрутки и перенос строк; `social.png` — до 1 МБ.

Social preview в GitHub загружается руками: Settings → General → Social preview.

## TUI

Снимки TUI не нарисованы: `tui.py` запускает настоящий TUI из
`python/xrun_tui/src` без терминала (`App.run_test`) на демо-базе и сохраняет
экраны через экспорт Textual в SVG, а Chrome переводит SVG в PNG (масштаб 2,
прозрачный фон за скруглёнными углами окна). База собирается из миграций
`crates/xrun-core/src/store/migrations`, так что схема всегда текущая.

Конфиг, данные и домашняя папка уходят во временный каталог: без этого TUI
нашёл бы ключи человека (`~/.config/vastai/vast_api_key`, `~/.kaggle`) и
сходил бы с ними в настоящие API. Аккаунт в строке статуса — `demo`, карточка
Doctor — «all green»: пустой временный конфиг иначе валит проверку кредов.

| Экран | Файл | Окно, ячеек |
| --- | --- | --- |
| Dashboard | `docs/screenshots/tui-dashboard.png` | 132×30 |
| Runs | `docs/screenshots/tui-runs.png` | 132×23 |
| Run detail, вкладка Metrics | `docs/screenshots/tui-run-detail.png` | 132×36 |

```bash
python docs/mock/tui.py
```

Нужны `textual` и зависимости TUI (`pip install -e python/xrun_tui`) и Chrome
по стандартному пути, иначе путь к нему — в переменной `CHROME`. Время на
снимках относительное («37m ago»), часы в шапке — момент съёмки.
