# CLI

Один бинарь `xrun` с подкомандами. Без аргументов — открывает TUI.

## Команды

### `xrun init [flags]`
First-run wizard. Без флагов на TTY — спавнит TUI-визард (`xrun-tui --wizard`),
который проводит через 4 шага: локальные мощности → вендоры → режим
логирования → recap. Для скилла и CI — флаговый non-interactive режим.

```
--non-interactive         не запускать TUI; пишет конфиг по флагам
--sink <name>             включить mirror-sink (mlflow). wandb/comet — v0.8
--mark-completed          выставить [ui] wizard_completed = true
--probe-local             только probe локальных мощностей; не пишет конфиг
--json                    машинно-читаемый вывод (для --probe-local и summary)

# credential-флаги (требуют --non-interactive). Значение `-` читает одну
# строку из stdin; только один `-` за вызов.
--vast-key <KEY|->        записать vast.api_key в credentials.toml
--kaggle-token <TOK|->    записать kaggle.token (JWT, предпочтительно)
--kaggle-username <USR>   legacy-аутентификация (парный --kaggle-key)
--kaggle-key <KEY|->      legacy-аутентификация (парный --kaggle-username)
--lightning-key <KEY|->   записать lightning.api_key (парный --lightning-user-id)
--lightning-user-id <ID>  записать lightning.user_id
--lightning-teamspace <owner/name>   записать lightning.teamspace (необязательно)
```

Colab отдельных флагов не имеет: токен OAuth создаётся командой
`xrun config login colab`.

Примеры:
```bash
xrun init                                      # интерактивный TUI-визард
xrun init --probe-local --json                 # детект GPU/OS для скилла
xrun init --non-interactive --mark-completed --sink mlflow

# скриптовая запись ключа без следов в shell-history:
echo "$VAST_KEY" | xrun init --non-interactive --mark-completed --vast-key -
```

### `xrun launch <manifest> [flags]`
Создаёт run, валидирует манифест, провижинит инстанс, заливает данные, стартует команду.

```
--detach            возврат в shell сразу после старта (по умолчанию — печатает live-стадии)
--allow-duplicate   разрешить запуск манифеста с уже существующим хешем
--dry-run           распарсить + показать план, ничего не делать
--name <override>   переопределить name (не меняет hash)
```

Exit 0 при `status=done`, 1 при `failed`, 2 при cancellation, 130 при Ctrl-C (если не --detach).

### `xrun ls [flags]`
Список runs — по умолчанию активные + последние 10 завершённых.

```
--all                       показать всё
--vendor vast|kaggle
--status running|done|failed
--tag <tag>                 фильтр по tag
--manifests                 показать манифесты в exp/, помеченные «не запускались»
--json                      машинно-читаемо
```

### `<run-id>` — как указать ран

Везде, где команда принимает `<run-id>` (`show`, `logs`, `events`, `metrics`, `pull`, `stop`, `rerun`, `diff`, `resume`, `fix-status`, `shell`), подходит полный ULID или его уникальный префикс либо суффикс длиной от 4 символов, без учёта регистра. Если совпадений несколько, команда перечислит кандидатов (id и имя) и попросит добавить символов; если ни одного — `run not found: <id> (see xrun ls)`. Исключение — `shell`: аргумент из одних цифр считается id инстанса vast, поэтому цифровой хвост ULID там не сработает — дайте префикс или больше символов.

### `xrun show <run-id>`
Полная карточка run: манифест, события, последние метрики, артефакты, ссылки.

### `xrun logs <run-id> [flags]`
stdout/stderr.

```
--follow / -f      live tail
--since 10m        только последние 10 минут
--grep <pat>       фильтр
```

### `xrun events <run-id> [flags]`
Поток событий стадий (download/unpack/train/epoch/...). По умолчанию — таблица, `--follow` — live.

### `xrun metrics <run-id> [flags]`
Метрики.

```
--key val_f1,val_loss        выбрать конкретные
--ascii                      ASCII-график «значение от шага» по каждому ключу
                             (без --key — первые 4 ключа; с --json не действует;
                             «no data yet» — только если точек нет вовсе)
--png <out>                  дамп PNG (через MLflow или локально через plotters)
--per-key                    в комбинации с --png: один subplot на ключ в auto-grid
                             (рекомендуется когда шкалы метрик различаются на порядки)
--mlflow-url                 распечатать URL run в MLflow UI
```

### `xrun pull <run-id> [flags]`
Синхронизация артефактов и чекпоинтов на локальный диск.

```
--ckpt latest|best|all|<glob>
--artifacts                  забрать всё, что в manifest.artifacts.patterns
--into <local-dir>           default: runs/<id>/output/
```

### `xrun stop <run-id>`
Корректный stop: посылает SIGTERM в команду, ждёт N сек, забирает финальные артефакты, гасит инстанс.

```
--force          сразу destroy, без graceful
--keep-instance  временно отклоняется: безопасная остановка процесса с сохранением инстанса не реализована; статус run не меняется
```

### `xrun diff <run-a> <run-b> [flags]`
Сравнение двух запусков side-by-side: различающиеся поля манифеста и метрики
(last + best per key). Best-направление (min/max) для каждого ключа выбирается
по порядку:

1. `--direction KEY=min|max` — явное указание пользователя;
2. `policy.early_stop` манифеста (сначала run a, затем run b), если его `metric`
   равен ключу: берётся `mode` (по умолчанию max);
3. имя ключа: min, если в нём есть токен `loss` (кроме `lossless`), `err*`,
   `perplexity*`, `mae`, `mse`, `rmse`, `ppl`, `wer`, `cer`, `fid`, `nll`, `eer`,
   `bpb`, `bpc` (ключ режется по не-буквенно-цифровым символам и по границе
   camelCase — `valError` как `val_error`, `FIDScore` как `fid_score`; короткие
   слова — только целым токеном), иначе max.

```
--keys k1,k2,...    отфильтровать ключи метрик (по умолчанию объединение)
--direction K=min|max  направление для ключа; повторяемый или через запятую
                       (--direction mae=min,score=max); иное значение или
                       противоречивый повтор ключа (a=min,a=max) — ошибка;
                       ключ, которого нет ни у одного из runs, игнорируется
                       с предупреждением в stderr (stdout/JSON и exit code
                       не меняются)
--manifest-only     только манифест-секция
--metrics-only      только метрики-секция
--json              машинно-читаемый вывод
```

Пример:
```
$ xrun diff 01HX...A 01HX...B
a: 01HX1234 (lr-baseline)
   vendor=vast status=done cost=$0.5821 duration=12m34s
b: 01HX5678 (lr-tweak)
   vendor=vast status=done cost=$0.6102 duration=13m02s

Manifest diff (1 differing paths):
  path                  a       b
  run.args.--lr         1e-3    5e-4

Metrics diff (2 keys):
  key       dir  a (last/best)        b (last/best)        Δ best
  val_f1    max  0.8210 / 0.8340      0.8470 / 0.8510      +0.0170
  val_loss  min  0.4120 / 0.3980      0.3890 / 0.3710      -0.0270
```

### `xrun rerun <run-id> [--patch key=val ...]`
Повтор запуска. Без --patch — точная копия. С --patch — модифицирует args/гиперпараметры (значение лезет внутрь run.args, обозначается через jq-style путь: `--patch run.args.--lr=5e-4`).

### `xrun sweep <manifest> --grid <spec>...`
Декартово произведение гиперпараметров. Каждый `--grid` — отдельная ось,
повторяемый. Материализует N манифестов в `exp/sweep_<stem>_<ts>/` и
опционально лончит каждый.

```
--grid PATH=v1,v2,...        ось перебора (повторяемый)
--out <dir>                  переопределить выходную директорию
--launch                     сразу запустить каждый
--detach                     детачить запуски (только с --launch)
-y / --yes                   skip billable confirm (только с --launch)
--dry-run                    показать план, ничего не писать
--json                       машинно-читаемый план
```

Примеры:

```bash
# 6 манифестов: 3×2 (lr × batch), просто записать
xrun sweep exp/base.yaml \
  --grid run.args.--lr=1e-3,5e-4,1e-4 \
  --grid run.args.--batch-size=4,8

# Запустить всю сетку детачнутыми ранами
xrun sweep exp/base.yaml \
  --grid run.args.--lr=1e-3,5e-4,1e-4 \
  --launch --detach --yes
```

Имя каждого варианта — `<base.name>_<leaf>-<value>_...`. Patch-семантика
такая же как у `xrun launch --override` / `xrun rerun --patch`.

### `xrun fix-status [<run-id>]`
Сверяет «застрявшие» в `running` записи с реальным статусом у вендора и
выравнивает БД. Нужно когда поллер умер посередине (Windows: нельзя заменить
открытый `xrun.exe`, поллер умирает молча).

```
<run-id>      проверить только этот run; без аргумента — все running
--dry-run     показать что бы изменилось, без записи
```

Для Kaggle делает один-shot `kaggle kernels status` и переводит в
`done`/`failed`. Для vast.ai проверяет, что инстанс ещё «жив» в
`vastai show instances`; если исчез — помечает run как `failed`.

### `xrun dataset push|status|list`
Управление Kaggle-датасетами: push (create или новая версия), polling
готовности, list собственных датасетов.

```bash
xrun dataset push <local-dir> --slug <owner>/<name> [-m "msg"] [--wait] [--verify]
xrun dataset status <owner>/<name> [--json]
xrun dataset list [--json]
```

Используется для подготовки данных перед `xrun launch` с
`vendor: kaggle` + `kaggle.datasets: [<slug>]`. `xrun doctor --manifest`
проверит что слаг существует и `ready` ещё до запуска; сам `launch`
отказывается стартовать, если статус датасета не удалось получить
(слага нет, 403, нет kaggle CLI).

`push` загружает подпапки архивами (`--dir-mode tar`), Kaggle распаковывает
их на своей стороне. Список файлов, который `push` печатает до загрузки,
это локальный fingerprint стейджинга (diff с прошлым push), а не
подтверждение загрузки. Подтверждение даёт `--verify` (по умолчанию
включён, требует `--wait`): после `ready` xrun запрашивает список файлов
датасета и сверяет с локальным. Расхождение (пустая версия, нераспакованный
`train.tar` вместо `train/`) печатает недостающие и лишние пути и
завершает команду с exit 1; `--verify=false` отключает. Если Kaggle не
отдал список, печатается предупреждение «could not verify», команда
завершается успешно.

`status` в текстовом режиме добавляет строку `files: N  size: M MiB` из
того же списка; `ready` сам по себе пустую версию не отличает.

### `xrun doctor [--manifest <path>...]`
Проверки окружения. `--manifest` валидирует один или несколько yaml-файлов
до запуска (схема + Kaggle: kernel slug, креды, существование датасетов).
Падает с exit 1 если хоть одна проверка-required зафейлилась.

```bash
xrun doctor                                # быстрый health-check
xrun doctor --manifest exp/foo.yaml        # pre-flight перед launch
xrun doctor --manifest exp/a.yaml --manifest exp/b.yaml --json
```

### `xrun init-manifest --vendor <X> --sink <Y>`
Генерирует скелет манифеста. Вендоры: `vast`, `kaggle`, `local`, `ssh`,
`lightning`, `colab`. Каждое место для правки помечено `TODO_<field>`.

### `xrun tui`
Открывает Python Textual TUI (`xrun-tui`). `xrun` без аргументов делает то же самое, если stdout — TTY; в противном случае выводит help и завершается с кодом 0.

Требует: `pip install -e python/xrun_tui`. Экраны: Dashboard, Runs, Run detail (Stages/Logs/Metrics/Manifest), Instances, Vendors, Launch, Artifacts, Settings, Doctor. Chord-навигация: `g→r`, `g→v`, `g→s` и др. Биндинги: `?` help, `:` command palette, `q`/`Esc` — назад/выход.

### `xrun doctor`
Проверки сгруппированы по категориям: `core`, `vendor:vast`, `vendor:kaggle`,
`vendor:ssh`, `vendor:local`, `vendor:lightning`, `vendor:colab`, `sink:mlflow`,
`manifest:<path>`. По умолчанию условные проверки скипаются, если
соответствующий вендор/sink не сконфигурирован.

Строки новых вендоров:

| Проверка | Категория | Что проверяет |
|----------|-----------|---------------|
| `lightning_sdk` | `vendor:lightning` | Python-мост отвечает на ping, `lightning-sdk` импортируется (`xrun install sdk lightning`) |
| `lightning_credentials` | `vendor:lightning` | заданы `lightning.api_key` + `lightning.user_id` либо есть файл от `lightning login` |
| `colab_sdk` | `vendor:colab` | Python-мост отвечает на ping, `colab_cli` импортируется (`xrun install sdk colab`) |
| `colab_login` | `vendor:colab` | есть OAuth-токен (`xrun config login colab`) |

```
--manifest <path>          добавить pre-flight для конкретного манифеста (повторяемый)
--all                      запустить все проверки, даже если вендор не настроен
--json                     машинно-читаемый вывод (для skill / TUI)
```

### `xrun config`
Управление `~/.config/xrun/`.

```
xrun config init                    создать дефолтные файлы
xrun config set vast.api_key ...    точечный set; пути: <section>.<field>,
                                    ssh.<alias>.<field>, vendors.<name>.<field>
xrun config set <key> --stdin       значение читается из stdin (один хвостовой
                                    \n / \r\n срезается), а не из argv — так
                                    секреты не попадают в список процессов;
                                    нужно ровно одно: VALUE или --stdin
xrun config unset <key>             сбросить ключ; печатает `<key>: <unset>`,
                                    повторный вызов — не ошибка
xrun config show                    текущая конфигурация (без секретов)
xrun config probe --vendor <name>   валидация переданных через
                                    XRUN_PROBE_* env vars кредов без записи на
                                    диск; используется визардом
xrun config login colab             первый вход в Google Colab (OAuth,
                                    copy-paste); нужен TTY
```

`probe` для новых вендоров:

- `--vendor lightning` читает `XRUN_PROBE_LIGHTNING_API_KEY`,
  `XRUN_PROBE_LIGHTNING_USER_ID` и необязательный
  `XRUN_PROBE_LIGHTNING_TEAMSPACE`; если env пуст — берёт файл
  `lightning login`.
- `--vendor colab` env не читает: проверяет токен colab-cli и запрашивает
  сводку потребления аккаунта (compute units); без токена — `ok: false`
  без запроса в сеть.

`xrun config login colab` запускает bridge-скрипт Colab в интерактивном режиме
с унаследованным stdio. Без TTY завершается ошибкой «requires a TTY» (как
`xrun init`), поэтому из Claude Code его нужно выполнять в отдельном терминале.
Консольный `colab login` под Windows не работает (`termios`), эта команда — работает.

Ключи кредов Lightning: `xrun config set|unset lightning.api_key`,
`lightning.user_id`, `lightning.teamspace` (`show` маскирует секрет).

Что делает `unset`:

- ключи кредов (`vast.api_key`, `kaggle.*`, `lightning.*`, `mlflow.*`, `wandb.api_key`,
  `ntfy.*`, `telegram.*`, `webhook.url`) — поле очищается в `credentials.toml`;
- `ssh.<alias>` — удаляет хост целиком, `ssh.<alias>.<field>` — очищает одно поле;
- `vendors.<name>.<field>` — возвращает значение по умолчанию
  (`vendors.<name>.extra.<k>` — удаляет запись из `extra`);
- любой другой ключ `config.toml` — возвращается к значению по умолчанию;
  неизвестный ключ — ошибка `unknown config key`; секция целиком
  (`notify`, `budget`) не сбрасывается — только по полям.

Пустое значение для ключа кредов (`config set vast.api_key ""` или пустой
stdin при `--stdin`) отклоняется: очистка — только через `unset`.

```bash
printf '%s' "$KEY" | xrun config set vast.api_key --stdin
xrun config unset vast.api_key
```

Per-vendor дефолты живут в `[vendors.<name>]` секции `config.toml`:

```toml
[vendors.vast]
default_gpu = "RTX_4090"
default_image = "pytorch/pytorch:2.3.0-cuda12.1-cudnn8-runtime"
max_per_hour_usd = 0.6
```

Поля: `default_gpu`, `default_image`, `default_region`, `default_disk_gb`,
`max_per_hour_usd`, плюс свободный `extra` (string→string) для адаптеров,
которые понимают свои кастомные ключи.

## Глобальные флаги

```
-v / --verbose       DEBUG логи в stderr
-q / --quiet         только ошибки
--db <path>          override SQLite location
--no-color
```

## Машинный вывод и восстановление

- `launch --json` возвращает один объект с `run_id`, `instance_id`, `status`
  и `poller_pid` (null для foreground/upload-only). Ошибка подготовки запуска
  возвращает объект `error` с `code: launch_failed` и `message`, exit 1.
  Терминальный failed/cancelled возвращает результат run и exit 1.
- `events --json` возвращает массив; `events --follow --json` — JSONL,
  один объект события на строку, без табличных заголовков.
- `sweep --launch --json` возвращает один объект: `manifests` и `runs`.
  В каждой записи `runs`: `name`, `success`, `result`, `error`.
  Ошибка одного запуска не мешает остальным, но итоговый exit code — 1.
- `--detach` по-прежнему возвращается после provision/upload/execute.
  При таймауте сначала проверьте существующий run; повтор launch не идемпотентен.
- Daemon получает выбранный `--config-dir`, диагностика сохраняется в
  `<runs-dir>/<run-id>/poller.log`. Успешный spawn ещё не гарантирует готовность daemon.
- Poller повторяет неудачное удаление до трёх раз. Если все попытки неуспешны,
  записывает `instance.cleanup_failed` и возвращает ошибку, сохраняя run активным.
  Проверьте ресурс и повторите `stop <id>`; это не бессрочный supervisor cleanup.

## `xrun install skill`

Устанавливает project-local skill/instructions для agent harness в текущий репозиторий:

```bash
xrun install skill --codex
xrun install skill --claude
```

Codex target пишет `.agents/skills/xrun/SKILL.md` и добавляет обновляемый блок в `AGENTS.md`.
Claude target пишет `.claude/skills/xrun/SKILL.md` и добавляет pointer в `CLAUDE.md`.

Флаги:

```
--repo <DIR>    установить в другой репозиторий вместо текущего каталога
--force         перезаписать существующий SKILL.md
```

## `xrun install sdk`

Ставит Python-SDK вендора (`pip install`) ровно в тот интерпретатор, который
использует Python-мост xrun: `XRUN_PYTHON`, иначе `python`, `python3`, `py -3`.

```bash
xrun install sdk lightning            # lightning-sdk
xrun install sdk colab                # google-colab-cli
xrun install sdk all --upgrade        # оба, с обновлением
xrun install sdk lightning --dry-run  # только показать команду pip
```

Перед запуском печатается выбранный интерпретатор и точная команда
`<python> [-3] -m pip install [--upgrade] <пакет>`; вывод pip идёт напрямую в
терминал. После успешной установки xrun делает `ping` моста и печатает версию
SDK (или ошибку). Интерпретатор не найден — ошибка `python interpreter not
found (set XRUN_PYTHON or install Python 3)`.

Флаги:

```
--dry-run       напечатать команду и выйти (код 0), pip не запускается
--upgrade       добавить --upgrade к pip install
```

## `xrun update`

Проверяет GitHub Releases и устанавливает свежий `xrun` через официальный installer.

```bash
xrun update --check
xrun update
xrun update --yes
```

При интерактивном запуске `xrun` без аргументов и `xrun tui` проверка выполняется
до открытия TUI. Если доступна новая версия, CLI показывает подтверждение
`Install update? [y/N]`. После подтверждения запускается installer того же
тега, что и ставящийся релиз (не с `master`), и процесс `xrun` завершается,
чтобы новая версия стартовала чисто. На Windows updater запускается отдельным
PowerShell-процессом, потому что запущенный `xrun.exe` нельзя заменить на месте.

Installer сверяет скачанный архив с `SHA256SUMS` из того же релиза и при
несовпадении ничего не ставит. У релизов v0.9.0 и раньше этого файла нет — тогда
печатается предупреждение и проверка пропускается. Для более новых релизов
отсутствие `SHA256SUMS` — ошибка: без проверки они не ставятся. Релиз
публикуется только после загрузки всех файлов (до этого он черновик).

Фоновая проверка: `xrun watchdog` (планировщик раз в 5 мин, TUI раз в 60 с)
не чаще раза в сутки сверяется с последним релизом (после сбоя сети —
повтор через час; результат хранится в `update_check.json` рядом с БД) и
шлёт `update.available` в каналы `[notify]` — один раз на релиз. Ничего
не устанавливает.

```toml
[update]
auto = "notify"   # "off" — без фоновой проверки и без сетевого запроса
```

Флаги:

```
--check         только проверить наличие обновления
-y, --yes      установить без подтверждения
--no-tui       обновить только CLI, не трогать Python TUI
```

Отключить startup-check и фоновую проверку для CI/скриптов:

```bash
XRUN_NO_UPDATE_CHECK=1 xrun
```

## Идиомы для skill

```bash
# Скилл всегда пишет:
xrun launch exp/arborust_v7_C.yaml --detach
xrun ls --status running --json
xrun pull <run-id> --ckpt best
xrun metrics <run-id> --key val_f1 --ascii

# Скилл НИКОГДА не пишет:
vastai create instance ...
ssh root@... "tar xf ..."
kaggle kernels push -k ...
```

См. [SKILL.md](SKILL.md).

### `xrun notify test|send|log|kinds`
Push-уведомления. Poll-daemon шлёт их сам (run done/failed, budget
thresholds, NaN loss, auto-destroy, cleanup failed); `xrun watchdog` — про
мёртвый поллер и orphan-инстансы. Эта команда — проверить канал до того,
как оставить инстанс без присмотра, и прочитать журнал.

```
xrun notify test [--channel <name>] [--json]   тестовое сообщение во все каналы; exit 1 если
                                              каналов нет или хоть один упал
xrun notify send <title> [-b body] [--run id] [--priority low|default|high|urgent]
xrun notify log [--run id] [--limit n] [--json] журнал доставок (notify_log в SQLite)
xrun notify kinds [--json]                     список kinds для [notify].events
```

Самый простой путь — TUI: `xrun` → `g n` (или визард при первом запуске,
шаг 4). Карточки каналов, топик ntfy генерируется сам, chat id Telegram
определяется кнопкой Detect, `t` шлёт тест. CLI-эквивалент (каналы в
`config.toml`, секреты в `credentials.toml`):

```bash
xrun config set notify.channels ntfy,telegram,desktop
xrun config set ntfy.topic <random-topic>          # https://ntfy.sh, приложение на телефон
xrun config set ntfy.url https://ntfy.example.com  # self-hosted (опционально)
xrun config set ntfy.token tk_...                  # защищённый topic (опционально)
xrun config set telegram.bot_token 123:ABC         # @BotFather
xrun config set telegram.chat_id 42                # getUpdates после первого сообщения боту
xrun config set webhook.url https://hooks.slack.com/...   # Slack/Discord/свой JSON endpoint
xrun notify test
```

`[notify]` в `config.toml`:

```toml
[notify]
channels = ["ntfy"]
events = ["*"]              # или ["run.failed", "budget.*", "poller.dead"]
cost_warn_pct = [50, 80]    # budget.warn на этих % от --max-cost, по одному разу
heartbeat_stale_min = 5     # для xrun watchdog
dedupe_min = 60             # один и тот же dedupe_key не чаще раза в N минут
```

Настройки применяются **без перезапуска**: poll-daemon следит за mtime
`config.toml` / `credentials.toml` и пересобирает каналы в течение одного
тика (не чаще раза в 5 с). Канал, включённый в TUI посреди обучения,
получит `run.done` этого же рана. Уже отправленные пороги `budget.warn`
не повторяются.

Kinds: `run.done`, `run.failed`, `run.idle`, `run.early_stopped`
(`policy.early_stop`), `budget.warn`, `budget.auto_destroyed`,
`budget.daily`, `budget.monthly`, `instance.cleanup_failed`,
`instance.orphan`, `metric.anomaly` (NaN/inf или loss > 10× running-min
после 10 точек), `poller.dead`, `update.available` (новый релиз xrun,
`[update].auto`), `user` (`xrun_hook.notify`, фильтру не
подчиняется). Каждый канал best-effort: падение одного не
блокирует остальные и не ломает поллер.

### `xrun watchdog [flags]`
Проверка, которую поллер не может сделать сам: жив ли он. Для каждого
`running`-рана смотрит PID и `poller_heartbeat_at` (поллер штампует его
каждый тик). Мёртвый → `poller.dead` уведомление + respawn (тот же путь,
что `xrun resume`). Живой PID, но heartbeat старше `heartbeat_stale_min`
→ `HUNG`, уведомление без respawn. Плюс инстансы с `price_per_hour`, не
уничтоженные и без живого рана → `instance.orphan` (ничего не удаляет —
это `xrun gc`). Раз в сутки — проверка нового релиза → `update.available`
(см. `xrun update`).

```
--dry-run           только отчёт: без respawn, уведомлений и команд
--no-respawn        уведомить, но не поднимать поллер
--stale-min <MIN>   override [notify].heartbeat_stale_min
--no-vendor         не ходить в API вендора за списком инстансов
--no-commands       не обрабатывать Telegram-команды
--json
```

Плюс кросс-проверка с вендором (vast, если есть ключ): инстанс, который
жив у вендора, но отсутствует в БД или уже помечен там уничтоженным →
`instance.orphan` с `source: vendor`. Это самый дорогой сценарий
(инстанс создан, а запись в SQLite не успела) — локально его не увидеть.

#### Команды из Telegram

Если настроен канал `telegram`, watchdog на каждом проходе читает
сообщения боту (`getUpdates`) и выполняет команды из **того же чата**,
что в `telegram.chat_id`; остальные чаты игнорируются и считаются.

```
/status          running-раны: имя, id, время, стоимость, heartbeat
/stop <id>       xrun stop — graceful stop + destroy (id — последние 8 символов)
/pull <id>       xrun pull --ckpt best
/help
```

Курсор `update_id` хранится в `<data_dir>/telegram.offset` и
сдвигается до выполнения, чтобы `/stop` не повторился при падении или
двух параллельных watchdog'ах (TUI + планировщик). Задержка = период
watchdog: до 5 мин из планировщика, до 60 с при открытом TUI.

Запускать раз в 5 минут из планировщика. TUI делает то же самое каждые
60 с, пока открыт; дедуп по `notify_log` не даёт пингануть дважды.

```
xrun watchdog schedule [--json]                статус записи в планировщике (read-only)
xrun watchdog schedule --install [--every-min 5]   зарегистрировать (schtasks на Windows,
                                               crontab на Linux/macOS); путь к бинарю абсолютный
xrun watchdog schedule --remove                удалить запись
```

То же самое одной клавишей в TUI: `g n` → карточка Watchdog → Enter.
Ручной вариант, если планировщик нестандартный:

```bash
# Windows (Task Scheduler)
schtasks /Create /SC MINUTE /MO 5 /TN xrun-watchdog /TR "xrun watchdog" /F
# Linux / macOS (crontab -e)
*/5 * * * * xrun watchdog >/dev/null 2>&1
```

### `xrun __poll-daemon <run-id>` (hidden)

Внутренняя команда, запускаемая автоматически при `--detach`. Запускает поллер событий/метрик в фоне для уже запущенного run.

```
--runs-dir <path>   путь к runs/ каталогу (передаётся лаунчером)
```

Для отладки зависшего поллера:
```bash
xrun __poll-daemon <run-id>   # вручную из терминала, foreground
```

## Статус команд (v0.3)

| Команда | Статус | Заметки |
|---------|--------|---------|
| `xrun launch <manifest>` | ✅ | Полная цепочка: provision → upload → exec → poller |
| `xrun launch --dry-run` | ✅ | Парсит манифест, показывает DryRunPlan |
| `xrun launch --detach` | ✅ | Спавнит фоновый поллер, сразу выходит |
| `xrun launch --max-cost/--max-hours/--idle-timeout` | ✅ | Budget caps; `--yes` для скриптов |
| `xrun ls` | ✅ | Фильтры: `--status`, `--tag`, `--vendor`, `--json` |
| `xrun show <id>` | ✅ | Карточка run из БД |
| `xrun logs <id>` | ✅ | Читает локальный stdout.log |
| `xrun logs <id> --follow` | ✅ | SSH tail -F на удалённый инстанс |
| `xrun events <id>` | ✅ | Таблица стадий из SQLite |
| `xrun events <id> --follow` | ✅ | Polling SQLite каждые 1s до terminal status |
| `xrun metrics <id>` | ✅ | `--ascii`, `--json`, `--png`, `--mlflow-url` |
| `xrun pull <id>` | ✅ | `--ckpt best/latest/all`, `--artifacts`, `--into` |
| `xrun stop <id>` | ✅ | Graceful: SIGTERM → wait → pull → destroy |
| `xrun rerun <id>` | ✅ | `--patch run.args.--lr=5e-4` |
| `xrun balance` | ✅ | Баланс vast.ai |
| `xrun gc` | ✅ | Удалить orphan-инстансы |
| `xrun cp` | ✅ | Streaming tar transfer между инстансами |
| `xrun shell <id>` | ✅ | SSH-сессия на инстанс |
| `xrun doctor` | ✅ | Проверяет конфиг, vastai/kaggle в PATH, ssh key, xrun_hook |
| `xrun config init/show/set` | ✅ | |
| `xrun tui` | ✅ | Запускает Python Textual TUI (`xrun-tui`) |
| `xrun sweep` | ✅ | Декартово произведение гиперпараметров; `--launch` опционально |
| `xrun fix-status [id]` | ✅ | Сверка БД с вендором для зависших running-ранов |
| `xrun dataset push/status/list` | ✅ | Kaggle datasets через xrun-креды |
| `xrun doctor --manifest` | ✅ | Pre-flight: схема + Kaggle dataset readiness |
| `xrun doctor --all` | ✅ | Запускает все проверки даже для не сконфигурированных вендоров |
| `xrun config probe` | ✅ | Probe вендора без записи на диск; вход через `XRUN_PROBE_*` env vars |
| `xrun metrics --per-key --png` | ✅ | Auto-grid PNG, один subplot на ключ |
| `xrun notify test/send/log/kinds` | ✅ | Push: ntfy / Telegram / webhook / desktop; журнал в SQLite |
| `xrun watchdog` | ✅ | Мёртвый/зависший поллер + orphan-инстансы → уведомление + respawn |
| `xrun watchdog schedule` | ✅ | `--install/--remove/--status`: schtasks (Windows) / crontab |

### TUI

`xrun` без аргументов (если stdout — TTY) запускает Python Textual TUI через
`xrun-tui`. Требует отдельной установки:

```bash
pip install -e python/xrun_tui
```

Экраны: Dashboard, Runs, Run detail (Stages/Logs/Metrics/Manifest), Instances,
Vendors, Launch, Artifacts, Compare, Settings, Doctor.  
Навигация: chord `g→X`, `?` help, `:` command palette.

## Exit codes

| Код | Значение |
|-----|----------|
| 0 | Успех / `status=done` |
| 1 | Ошибка стадии / failed run |
| 2 | Cancelled (graceful stop) |
| 64 | Ошибка манифеста / валидации |
| 65 | Конфигурация (нет кредов, неверный API key) |
| 66 | Вендор недоступен (нет offers, kaggle 503) |
| 130 | Ctrl-C |
