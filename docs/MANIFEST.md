# Experiment Manifest

Один YAML файл = один воспроизводимый запуск. Хеш манифеста — first-class identity для дедупа.

## Полный пример (vast.ai)

```yaml
# exp/arborust_v7_C.yaml
name: arborust_v7_C
description: ResUNet3D v7, channels=2, curated apex_top3 GT
tags: [arborust, treetop3d, v7]

vendor: vast

vast:
  image: pytorch/pytorch:2.4.1-cuda12.1-cudnn9-devel
  gpu: { type: "RTX 4090", count: 1, vram_min_gb: 24 }
  disk_gb: 80
  price:
    max_per_hour: 0.55
    bid: false               # spot/interruptible — пока false
  region: any                # eu, us, asia, any
  ssh: true                  # нужно для tail/pull
  ports: [8888]              # пробросы (опционально)

# Что заливаем на инстанс перед стартом
data:
  - src: "C:/Users/gerrux/Desktop/cache_mc_v5_curated_20260421.tar"
    dst: /workspace/data/cache.tar
    unpack: { format: tar, into: /workspace/data/cache }
  - src: "C:/Users/gerrux/garage/arborust/experiments/ml_detector_3d/"
    dst: /workspace/code
    mode: rsync               # вместо tar — синхронизация директории

# Тренировочный код
run:
  workdir: /workspace/code
  setup: |
    pip install -e . xrun_hook torch==2.4.1
  cmd: python train_v5_multichannel.py
  args:
    --cache: /workspace/data/cache
    --output: /workspace/run/output
    --epochs: 30
    --batch-size: 8
    --lr: 1e-4
    --in-channels: 2
    --dropout: 0.2

# Что наблюдать и забирать
checkpoints:
  watch: /workspace/run/output/ep*.pt
  pull:
    on: [epoch_end]            # или: [done] для только финальных
    keep_last: 3
    keep_best:
      metric: val_f1
      mode: max

artifacts:
  patterns:
    - /workspace/run/output/*.png
    - /workspace/run/output/metrics.json
    - /workspace/run/stdout.log
  pull_on: done

# Куда зеркалить метрики
mlflow:
  experiment: arborust-treetop3d
  log_args_as_params: true

# Поведение xrun
policy:
  on_stage_failed: stop_instance     # при status=fail: stop_instance (по умолчанию) гасит инстанс;
                                     # keep — ран всё равно Failed, но инстанс остаётся для отладки;
                                     # reprovision принимается, но пока ведёт себя как stop_instance
  on_idle_minutes: 30                # авто-стоп, если > N мин нет вывода (stdout, events, metrics);
                                     # vast и local, на ssh/kaggle игнорируется (активность не видна);
                                     # `--idle-timeout` в CLI приоритетнее, 0 — выключено
  on_done: stop_instance             # stop_instance (default) | keep
  early_stop:                        # остановить, когда метрика вышла на плато
    metric: val_f1                   # ключ из xrun_hook.metric(...)
    patience: 5                      # столько подряд оценок без улучшения
    mode: max                        # max (default) | min
    min_delta: 0.001                 # улучшение меньше этого не считается
    pull: true                       # забрать чекпоинт до уничтожения инстанса
    pull_pattern: "**/best*"         # что забирать (default)
```

## Минимальный пример (local — отладка на хосте)

Локальный вендор запускает `run.cmd` как subprocess на текущей машине — без
SSH, без сети, без оплаты. Идеально для отладки манифеста перед запуском в
облако.

```yaml
name: smoke_local
vendor: local

local:
  gpu: auto      # или "0", "0,1", "cuda:0", "cpu"; default = auto

data:
  - src: ./datasets/tiny       # путь на хосте
    dst: ./staging/data        # тоже на хосте — fs::copy

run:
  workdir: ./staging           # default = <runs_dir>/<run-id>/work/
  cmd: python train.py --epochs 1
  args: { lr: 5e-4 }

artifacts:
  patterns: [checkpoints/best*.pt, metrics.json]
```

### Local-специфичные нюансы

- **Shell.** На Unix `run.cmd` исполняется через `bash -c` (fallback `sh -c`).
  На Windows — через `pwsh -NoProfile -NonInteractive -Command` (PowerShell 7,
  поддерживает `&&`/`||`); если `pwsh` не установлен, используется
  `powershell.exe` (5.1, без chain operators — пиши `; if ($?) { ... }`).
  Манифесты, использующие bash-идиомы (`&&`, heredoc, `>>`), нуждаются в
  правках под PowerShell на Windows-хосте — либо ставь `pwsh` через
  winget/scoop.
- **GPU.** `gpu: auto` (default) ничего не выставляет — `CUDA_VISIBLE_DEVICES`
  наследуется. `gpu: cpu` обнуляет его. `gpu: 0` или `cuda:0` ставит в
  `CUDA_VISIBLE_DEVICES`. Реальный список GPU виден в TUI Vendors-экране
  через `nvidia-smi` best-effort.
- **`data: dst`** на local интерпретируется как нативный путь хоста: можно
  относительный, можно абсолютный (Windows: `C:\...`), `/` не требуется —
  именно для local-вендора ослаблено.
- **MVP scope upload.** Только `mode: copy` (файл или рекурсивно директория).
  `mode: rsync`, `unpack`, `exclude`, `compress` — пока игнорируются с
  warn-event `upload:progress`. Добавим в следующих релизах при
  необходимости.
- **Завершение и cleanup.** PID живущего процесса лежит в
  `<runs_dir>/<run-id>/run.pid`; `xrun stop <id>` посылает SIGTERM/taskkill,
  ждёт, при необходимости SIGKILL. Идемпотентно.

## Минимальный пример (ssh — свой сервер / NAS / VPS)

`vendor: ssh` — отправляет тренировку на машину, доступную по SSH (always-on).
Provisioning не делает ничего (железо постоянно), `destroy` только убивает
дочерний процесс. Полный паритет lifecycle с vast/local через `ssh` + `rsync`
subprocess.

```yaml
name: ssh_train_v1
vendor: ssh
ssh:
  host_alias: my-workstation     # см. credentials.toml ниже
  workdir: /home/me/xrun-runs    # optional, default /tmp/xrun
  gpu: cuda:0                    # optional CUDA_VISIBLE_DEVICES override

data:
  - src: ./datasets/tiny
    dst: /home/me/xrun-runs/data
run:
  cmd: python train.py --epochs 10
artifacts:
  patterns: [checkpoints/best*.pt]
```

В `~/.config/xrun/credentials.toml`:

```toml
[vendors.ssh.my-workstation]
host = "192.168.1.10"
user = "ubuntu"
port = 22                          # optional, default 22
key = "~/.ssh/id_ed25519"          # optional
default_workdir = "/home/ubuntu/xrun-runs"   # optional fallback
```

### SSH-специфичные нюансы

- **Ключи только.** `ssh -o BatchMode=yes` — пароль/passphrase prompt
  отключён, чтобы запуск не висел в ожидании ввода. Используй ssh-agent
  или unencrypted key. (Можно поправить позже, добавив ssh-agent integration.)
- **Зависимости на хосте.** `rsync`, `bash`, `tail`, `wc`, `nvidia-smi` —
  обычные Unix-инструменты. Windows-серверы пока не поддерживаются.
- **`workdir`.** Дефолт `/tmp/xrun`, перезатирается `ssh.workdir` в манифесте,
  и тот в свою очередь — `default_workdir` из creds. Per-run subdir
  `<workdir>/<run-id>/` создаётся автоматически в `provision()`. `~/…` и
  относительные пути (`ssh.workdir`, `default_workdir`, `run.workdir`)
  отсчитываются от домашнего каталога пользователя на хосте. Относительные
  `artifacts.patterns` ищутся в `run.workdir`, а без него — в `<workdir>/<run-id>/`.
  Путь run-dir запоминается в хэндле инстанса при запуске: смена
  `default_workdir` позже уже идущий ран не сдвигает (у ранов от старых
  версий путь вычисляется заново).
- **xrun_hook на удалёнке.** Установи `pip install xrun-hook` на сервере или
  включи в `data:` как для vast. `XRUN_RUN_DIR=<run-dir>` подставляется в env.
- **destroy только убивает PID,** не машину. Идемпотентно: повторный `xrun
  stop <id>` ничего не сломает.
- **stop без manifest copy.** `xrun stop` использует `XRUN_SSH_ALIAS` env
  override либо первый ssh-хост из creds (best-effort, идемпотентен).
  Когда есть стояла копия манифеста — берётся правильный alias.

## Минимальный пример (lightning — Lightning AI Studio)

`vendor: lightning` запускает тренировку в Studio на Lightning AI. Provision
поднимает (или создаёт) Studio и стартует машину, `run.cmd` уходит туда
detached-процессом, события и метрики тейлятся так же, как на ssh. Работает
через Python SDK `lightning-sdk` (xrun держит один постоянный Python-процесс,
см. [ARCHITECTURE.md](ARCHITECTURE.md)).

```yaml
name: lightning_train_v1
vendor: lightning
lightning:
  machine: T4                    # default T4
  interruptible: true            # default true — дешевле, но машину могут забрать
  studio: my-studio              # optional, default xrun-<name>
  teamspace: owner/name          # optional, перекрывает credentials.toml
  workdir: xrun                  # optional, относительно home Studio
  max_runtime_secs: 10800        # optional

data:
  - src: ./datasets/tiny
    dst: data/tiny               # относительно home Studio, без ведущего /
run:
  cmd: python train.py --epochs 10
artifacts:
  patterns: [checkpoints/best*.pt]
policy:
  on_done: stop_instance         # остановить Studio (файлы остаются)
```

В `~/.config/xrun/credentials.toml`:

```toml
[lightning]
api_key = "..."                  # секрет
user_id = "..."
teamspace = "owner/name"         # optional
```

Тот же результат даёт `xrun init --non-interactive --lightning-key - --lightning-user-id <ID>`
или `xrun config set lightning.api_key|user_id|teamspace`. Если креды в xrun не
заданы, берётся файл, который пишет `lightning login`
(`~/.lightning/credentials.json`).

### Lightning-специфичные нюансы

- **Предварительно.** `xrun install sdk lightning` (или `pip install lightning-sdk`); проверка — `xrun doctor`
  (строки `lightning_sdk`, `lightning_credentials`).
- **Пути home-relative.** `data[].dst` обязан быть относительным к домашнему
  каталогу Studio: ведущий `/` отвергается валидацией, `~/x` допустим (префикс
  `~/` отбрасывается). То же для `lightning.workdir` — без ведущего `/`. Per-run
  каталог `<workdir>/<run-id>/` (`events.jsonl`, `metrics.jsonl`, `stdout.log`,
  `run.pid`) создаётся в `provision()`.
- **`artifacts.patterns`** отсчитываются как на ssh: абсолютный путь — как есть,
  `~/x` — от home, остальные — от `run.workdir` (а без него — от каталога
  запуска).
- **Бесплатный тариф.** 15 кредитов в месяц, одна активная Studio, машина
  перезапускается каждые 4 часа. `interruptible: true` (default) дешевле, но
  ран может быть прерван; для длинных ранов ставь чекпоинты и
  `max_runtime_secs`.
- **`policy.on_done: stop_instance`** (default) останавливает Studio —
  вычислительные ресурсы освобождаются, файловая система сохраняется.
  `xrun launch --reuse-instance` переиспользует ту же Studio; `keep` не
  останавливает её (кредиты продолжают тратиться).
- **Только `run.cmd`.** `run.notebook` не поддерживается. Цену xrun не
  считает (оценка в `--dry-run` — 0), расход смотри в Lightning.
- **`--max-cost` не действует.** Кредиты Lightning xrun не оценивает в деньгах,
  поэтому лимит по стоимости не сработает; ограничивай ран через
  `--max-hours` (или `max_runtime_secs`).
- **Нет неявного pull `**/best*`.** В отличие от vast, при пустом
  `artifacts.patterns` ничего не забирается перед остановкой Studio: если
  чекпоинты важны, всегда задавай `artifacts.patterns`.
- **xrun_hook** на PyPI нет: залей каталог пакета через `data:`
  (`src: python/xrun_hook/src/xrun_hook`, `dst: xrun_hook`) и запускай с
  `PYTHONPATH="$HOME"`, как в `exp/templates/lightning_smoke.yaml`;
  `XRUN_RUN_DIR` подставляется в env абсолютным путём.

## Минимальный пример (colab — Google Colab)

`vendor: colab` берёт бесплатную (или Pro) сессию Google Colab, запускает в ней
`run.cmd` через Jupyter-ядро и тейлит те же файлы, что и ssh. Работает через
библиотеку `google-colab-cli`, которой xrun управляет напрямую.

```yaml
name: colab_train_v1
vendor: colab
colab:
  gpu: T4                        # T4 | L4 | A100 | H100 | G4 | cpu, default T4
  high_mem: false                # default false (только Pro)
  workdir: /content/xrun         # optional, абсолютный путь

data:
  - src: ./datasets/tiny
    dst: /content/xrun/data
run:
  cmd: python train.py --epochs 3
artifacts:
  patterns: [checkpoints/best*.pt]
policy:
  on_done: stop_instance         # освободить сессию
```

### Colab-специфичные нюансы

- **Предварительно.** `xrun install sdk colab` (или `pip install google-colab-cli`), затем один раз
  `xrun config login colab` (интерактивный OAuth copy-paste; нужен TTY, из
  Claude Code не запускается). Токен хранит сам colab-cli
  (`~/.config/colab-cli/token.json`), отдельной секции в `credentials.toml`
  нет. Проверка — `xrun doctor` (строки `colab_sdk`, `colab_login`).
- **Windows.** Консольный бинарь `colab` импортирует `termios` и под Windows не
  работает. xrun вызывает библиотеку напрямую, поэтому сам вендор на Windows
  работает, но логиниться нужно именно через `xrun config login colab`.
- **Раскладка.** Per-run каталог — `/content/xrun/<run-id>` (`events.jsonl`,
  `metrics.jsonl`, `stdout.log`, `run.pid`); `colab.workdir` должен быть
  абсолютным.
- **Нет гарантий квоты.** На бесплатном тарифе GPU выдаётся «когда есть»,
  сессия живёт не дольше 12 часов, после чего диск `/content` пропадает:
  забирай артефакты (`artifacts.patterns`) и ставь чекпоинты. Сессия
  называется `xrun-<run-id>`.
- **Загрузка данных.** `upload` — по одному файлу за вызов, целиком в память и
  через base64; держи `data:` небольшим (десятки МБ), крупное тяни из
  `run.setup` (`gdown`, `wget`, Drive).
- **`policy.on_done: stop_instance`** освобождает сессию (unassign и удаление
  из store colab-cli).
- **Только `run.cmd`;** `run.notebook` не поддерживается. Цену xrun не считает
  (оценка в `--dry-run` — 0).
- **Pull без совпадений.** Как на vast/ssh, pull, под который не подошёл ни один
  файл, оставляет сессию живой (чтобы не потерять данные): поправь паттерн,
  выполни `xrun pull <id>`, затем `xrun stop <id>`.
- **Нет неявного pull `**/best*`.** При пустом `artifacts.patterns` ничего не
  забирается перед освобождением сессии; если чекпоинты важны, всегда задавай
  `artifacts.patterns` (unassign Colab стирает `/content`).
- **`run.workdir`** обязан быть абсолютным (например `/content/proj`):
  относительный отвергается валидацией.

## Минимальный пример (Kaggle)

```yaml
name: classifier_eb0_baseline
vendor: kaggle

kaggle:
  kernel_slug: gerrux/classifier-eb0-baseline
  competition: null
  dataset: gerrux/forest-tiles-v2     # привязанный датасет
  enable_gpu: true
  enable_internet: false

run:
  notebook: notebooks/train_eb0.ipynb # или script.py
  args: { epochs: 10, fold: 0 }

artifacts:
  patterns: [output/*.png, output/metrics.json, output/best.pt]
  pull_on: done

mlflow:
  experiment: classifier-eb0
```

## Поля

### Top-level

| Поле | Тип | Обязательно | Заметки |
|------|-----|-------------|---------|
| `name` | string | да | Slug; используется как experiment name в MLflow |
| `description` | string | нет | Свободный текст |
| `tags` | [string] | нет | Видны в `xrun ls`, фильтруются |
| `vendor` | enum | да | `vast` \| `kaggle` \| `local` \| `ssh` \| `lightning` \| `colab` |
| `vast` / `kaggle` / `local` / `ssh` / `lightning` / `colab` | object | да | По одному в зависимости от `vendor` (блоки `local`, `lightning`, `colab` опциональны) |
| `data` | [object] | нет | Что предзалить |
| `run` | object | да | Команда тренировки |
| `checkpoints` | object | нет | Watch + pull policy |
| `artifacts` | object | нет | Дополнительные файлы |
| `mlflow` | object | нет | Если отсутствует — метрики только в SQLite |
| `policy` | object | нет | Поведение при ошибках/idle; `early_stop` — остановка по плато метрики (см. ниже) |
| `requires` | object | нет | Pre-flight floor: `ram_gb`, `disk_gb`. `xrun doctor --manifest` падает, если `vendor` известен и значения превышают аппаратный лимит (Kaggle ≈ 13 GB RAM / 73 GB working disk). Защита от 6-минутного OOM. |

### `vast`

| Поле | Описание |
|------|----------|
| `image` | Docker image |
| `gpu.type` | GPU модель (e.g. `RTX 4090`) |
| `gpu.count` | Количество GPU (default 1) |
| `gpu.vram_min_gb` | Минимальный VRAM |
| `disk_gb` | Размер диска на инстансе |
| `price.max_per_hour` | Максимальная цена ($/hr) |
| `inet_up_min_mbps` | Минимальный аплинк (Mbps) — критично для больших данных |
| `inet_down_min_mbps` | Минимальный даунлинк (Mbps) |
| `cuda_min` | Минимальная версия CUDA (e.g. `12.1`) |
| `reliability_min` | Минимальный reliability score (`0.0`–`1.0`) |
| `direct_port_count_min` | Минимум прямых TCP-портов |
| `regions` | Список регионов: `[Europe, "North America"]` |

#### Тихие дефолтные фильтры

Каждый поиск автоматически добавляет следующие фильтры (переопределить нельзя через манифест):

| Фильтр | Значение | Причина |
|--------|----------|---------|
| `verified` | `true` | Только верифицированные хосты |
| `rentable` | `true` | Только реально арендуемые |
| `external` | `false` | Не внешние (иные провайдеры через vast) |
| `rented` | `false` | Только свободные |
| `type` | `on-demand` | Не spot/bid |
| `order` | `score-desc` | Сортировка по vast score |

Если вы получаете «no offers available», попробуйте ослабить `price.max_per_hour` или убрать `gpu.type`.

### `local`

| Поле | Описание |
|------|----------|
| `gpu` | `auto` (default), `cpu`, `0`, `0,1`, `cuda:0` — выставляется в `CUDA_VISIBLE_DEVICES` |

Блок опционален. Если опущен, `gpu` берётся как `auto` и `CUDA_VISIBLE_DEVICES` не трогается.

### `ssh`

| Поле | Описание |
|------|----------|
| `host_alias` | Ключ в `[vendors.ssh.<alias>]` credentials.toml (обязательно) |
| `workdir` | Remote workdir root, default `/tmp/xrun` |
| `gpu` | `CUDA_VISIBLE_DEVICES` override (`auto`/`cpu`/`0`/`cuda:0`/...) |

### `lightning`

| Поле | Описание |
|------|----------|
| `machine` | Имя машины из `lightning_sdk.Machine` (`T4`, ...), default `T4` |
| `interruptible` | Прерываемая (дешевле) машина, default `true` |
| `studio` | Имя Studio. Default `xrun-<name>` (только `[a-z0-9-]`, до 40 символов) |
| `teamspace` | `owner/name`; перекрывает `lightning.teamspace` из credentials.toml. Если нигде не задан — первый teamspace пользователя |
| `workdir` | Корень на удалённой стороне, **относительно домашнего каталога Studio** (без ведущего `/`), default `xrun` |
| `gpu` | `CUDA_VISIBLE_DEVICES` override, как у `ssh.gpu` (`auto`/`cpu`/`0`/`cuda:0`/...) |
| `max_runtime_secs` | Потолок времени работы машины; уходит в `Studio.start(max_runtime=)` |

Блок опционален: все поля имеют дефолты.

### `colab`

| Поле | Описание |
|------|----------|
| `gpu` | `T4` \| `L4` \| `A100` \| `H100` \| `G4` \| `cpu` (регистр не важен), default `T4` |
| `high_mem` | Машина с большим объёмом RAM, default `false` (только Colab Pro) |
| `workdir` | Абсолютный корень на удалённой стороне, default `/content/xrun` |

Блок опционален: все поля имеют дефолты.

### `kaggle`

| Поле | Описание |
|------|----------|
| `kernel_slug` | `<username>/<slug>` (обязательно). Поддерживает плейсхолдеры `{user}` (резолвится из kaggle creds — авто-fill для шаблонов), `{run_id}`, `{date}` |
| `competition` | Название соревнования (или `null`) |
| `dataset` | Attached dataset slug (`user/ds`) |
| `enable_gpu` | `true` / `false` |
| `enable_internet` | `false` для большинства соревнований |

#### Kaggle constraints

- `enable_internet=false` → нельзя `pip install` на ходу. xrun автоматически кладёт `xrun_hook` wheel в staging и инжектит `sys.path` — ничего настраивать не нужно.
- `run.notebook` указывает `.ipynb`. xrun автоматически прибавляет одну bootstrap-ячейку в начало notebook'а (тег `xrun-bootstrap`): она base64-декодит `xrun_hook` wheel, `pip install`-ит его и проставляет `MLFLOW_TRACKING_URI` / `MLFLOW_TRACKING_USERNAME` / `MLFLOW_TRACKING_PASSWORD` (если `mlflow.url` настроен) — ровно то же, что в script-mode `main.py`. Пользователю **не нужно** вручную ставить `xrun_hook` или экспортить MLflow env vars.
- `kernel_slug` обязан быть в формате `<username>/<slug>` — `push` упадёт иначе. Шаблоны (`exp/templates/kaggle_*.yaml`) используют `{user}/<slug>`, и xrun сам подставляет твой kaggle username (из `kaggle.username` или, для token-auth, через `kaggle config view`). Запасной плейсхолдер `{run_id}` гарантирует уникальный slug на каждый запуск.
- Live-телеметрия на Kaggle идёт через MLflow side-channel: `xrun_hook` стримит events / metrics / stdout как chunked-артефакты, поллер тянет их каждый тик. Без `mlflow.url` видны только синтетические `queued:start` / `running:start` + post-run `ingest`.

### `data[]`

| Поле | Описание |
|------|----------|
| `src` | Локальный путь (файл или директория) |
| `dst` | Путь на инстансе |
| `mode` | `copy` (default, tar-pipe) \| `rsync` |
| `compress` | `gzip` (default) \| `zstd` — сжатие при tar-pipe; zstd быстрее, gzip универсальнее |
| `exclude` | Список glob-паттернов для исключения (tar `--exclude` семантика) |
| `unpack` | `{ format: tar\|zip\|tar.gz, into: <path> }` после копирования |

#### `exclude` паттерны — важно

Паттерны имеют **`tar --exclude` семантику** (gnu tar). Ключевые правила:

1. **Совпадение против относительного пути от `src`**, без implicit prefix
   wildcard. `cache_*/` *не* матчит `_cache_model/` — нужно `_cache_*/`,
   потому что ведущий `_` входит в имя.
2. `*` не пересекает `/`. Чтобы поймать любую глубину — `**/<pattern>`.
3. Имена с `.` на конце (`output.`) и без — разные паттерны.
4. Регистр чувствителен на Linux/Mac; на Windows tar.exe обычно тоже.

```yaml
exclude:
  - "**/__pycache__"   # любой уровень вложенности
  - "*.pyc"            # в любой директории
  - "_cache_*"         # ДОЛЖЕН включать ведущий символ если он есть
  - "output/**"        # поддерево под src
  - ".git"             # скрытые директории
  - "**/.DS_Store"     # mac-мусор на любой глубине
```

**Часто встречающиеся ошибки**:

| Хочется исключить | Неправильно | Правильно |
|---|---|---|
| `_cache_zmax_exp/`, `_cache_model_cmp/` | `cache_*` | `_cache_*` |
| `data/raw/big.h5`, `notebooks/raw/x.h5` | `raw/*.h5` | `**/raw/*.h5` |
| Все `.pyc` рекурсивно | `*.pyc` (только верхний уровень) | `**/*.pyc` |
| `runs_archive/` (директория с подпапками) | `runs_archive` | `runs_archive/**` |

**Проверка перед заливкой**: на хосте можно прогнать
`tar -cf /dev/null -C <src-parent> <src-name> --exclude=<pattern>` и
посмотреть на размер через `du -sh` на промежуточный staging — это
эквивалент того, что делает xrun перед отправкой на инстанс. Цена
ошибки реальна: один лишний `_cache_*` стоил ~6 GB трафика на
запуске 2026-04-29.

### `run`

| Поле | Описание |
|------|----------|
| `workdir` | cwd на инстансе |
| `setup` | shell-сниппет, выполняется один раз перед `cmd` |
| `cmd` | Основная команда |
| `args` | Map; рендерится как `--key value`. Bool `true` → флаг без значения, `false` → опускается |
| `notebook` (kaggle) | Путь к .ipynb для kernel push |

На local, ssh, lightning, colab и vast xrun экспортирует `PYTHONUNBUFFERED=1`: вывод Python не
буферизуется, `stdout.log` наполняется сразу (от него зависит `idle_timeout`).
Своё значение побеждает: переменная в окружении хоста/инстанса или префикс в
`run.cmd` (`PYTHONUNBUFFERED=0 python train.py`). На Kaggle не выставляется.

### `policy.early_stop`

Поллер считает подряд идущие оценки `metric` без улучшения (по `step`,
повторы и replay одного шага не считаются). Когда их набирается
`patience`, он: пишет событие `early_stop` (best, best_step, patience,
куда забрал артефакты), делает `pull` по `pull_pattern` в
`runs/<id>/artifacts/` (если `pull: true`), уничтожает инстанс, ставит
статус `done` и шлёт push `run.early_stopped`. Это единственный путь, где
ран заканчивается `done`, а не `failed`, без события `done:ok` от скрипта.

Работает для всех вендоров; на Kaggle `pull` тянет весь output. Метрика
берётся из metrics.jsonl и из распарсенного stdout — `xrun_hook` не
обязателен, но надёжнее.

### `policy.on_done` и `artifacts.pull_on`

Что поллер делает, когда ран закончился штатно (`done`):

1. **Забирает артефакты.** Если `artifacts.patterns` не пуст и `pull_on`
   не задан или равен `done` (единственное допустимое значение, по
   умолчанию `done`), каждый паттерн тянется в `runs/<id>/artifacts/`;
   на каждую попытку пишется событие `artifacts.pull` (`ok` / `fail`).
2. **Гасит инстанс.** `policy.on_done: stop_instance` (по умолчанию, если
   поле не задано) уничтожает инстанс; `keep` оставляет его живым — он
   продолжит тарифицироваться, пока вы не сделаете `xrun stop`/`xrun gc`.
   Любое другое значение — ошибка валидации манифеста.
3. Ставит статус `done` и шлёт push `run.done`.

Если хотя бы один `pull` упал, инстанс **не уничтожается** (иначе
артефакты потеряны): пишется событие `instance.kept`, ран всё равно
`done`, а забрать файлы можно повторно через `xrun pull <id>`. Для
платного инстанса (vast) сразу приходит push `instance.orphan`, и пока
инстанс жив, `xrun watchdog` продолжит показывать его как осиротевший. Если же
уничтожение не удалось после трёх попыток, ран остаётся `done`, в
событиях `instance.cleanup_failed`, приходит срочный push — такой
инстанс подберёт `xrun gc`.

На vast относительный паттерн отсчитывается от `run.workdir` (по
умолчанию `/workspace`) — там же, где запускалась команда. На Kaggle
`pull` тянет весь output ядра независимо от паттерна, поэтому
выполняется один раз, сколько бы паттернов ни было.

**Страховка от потери чекпоинтов (vast).** Если инстанс сейчас будет
уничтожен, а `artifacts.patterns` не заданы, поллер сначала забирает то же,
что `xrun pull --ckpt best` (`**/best*` от `run.workdir`, `**` рекурсивно).
Если файлов не нашлось, это ошибка pull: инстанс остаётся (`instance.kept`).
Чтобы страховка не срабатывала, задайте `patterns` или `on_done: keep`.
После успешного pull push `run.done` указывает на локальную папку
артефактов, а не на `xrun pull`.

**`--reuse-instance`.** Запуск на переиспользованном инстансе без
явного `policy.on_done` ведёт себя как `keep` (запуск пишет событие
`instance.reused`, его читает поллер, в том числе после рестарта). Явный
`on_done: stop_instance` инстанс всё же уничтожит.

**local и ssh.** На штатном `done` `vendor.destroy` не вызывается (он убил бы
PID из `run.pid`, который мог быть переиспользован); инстанс лишь
помечается уничтоженным, платить не за что. На local автопулла нет (файлы уже
здесь), на ssh паттерны тянутся как обычно (относительные — от каталога
запуска).

**Ранняя остановка** (`policy.early_stop`) идёт тем же путём: тянет свой
`pull_pattern` и `artifacts.patterns`, и только потом уничтожает инстанс;
упавший pull оставляет инстанс (+ `instance.orphan` на платных). Обучение
в этот момент ещё идёт, поэтому инстанс гасится независимо от `on_done` и
вендора.

### `checkpoints`

| Поле | Описание |
|------|----------|
| `watch` | Glob на инстансе; новые матчи трекаются |
| `pull.on` | Список событий-триггеров: `epoch_end`, `done`, `manual` |
| `pull.keep_last` | Удалять локально всё, кроме последних N |
| `pull.keep_best` | `{ metric: val_f1, mode: max }` — отдельно держим лучший |

## Дискаверабельность

`exp/` (или любой другой) — папка с манифестами. `xrun ls --manifests` обходит её и показывает unrun. `xrun launch` без аргументов — fzf-подобный picker.

## Хеш и иммутабельность

`manifest_hash = sha256(canonical_json(manifest))`. После запуска копия пишется в `runs/{run_id}/manifest.yaml` — оригинал можно править свободно, run всегда воспроизводится по своей копии.

### Canonical hash

Алгоритм реализован в `xrun-core::manifest::canonical_hash`:

1. Десериализовать манифест в `serde_json::Value`.
2. Рекурсивно пройти по Value: объекты переложить в `BTreeMap` (сортировка ключей), `null`-поля удалить, числа нормализовать через `serde_json::Number::from_f64` (убирает -0, NaN → ошибка).
3. Сериализовать в строку без пробелов (`to_string`).
4. Взять SHA-256, вывести как hex lowercase.

Гарантия: порядок ключей в YAML и платформа не влияют на хеш. Хеш стабилен между запусками.

## Что мы СОЗНАТЕЛЬНО не делаем

- **Не jinja-шаблонизация манифеста.** Если нужна развёртка по гиперпараметрам — отдельная команда `xrun sweep <manifest> --grid lr=1e-3,1e-4 batch=4,8`, она генерит N материализованных манифестов.
- **Не include / extends.** Один манифест — один self-contained файл. Дублирование лучше скрытой иерархии.
- **Не secrets в манифесте.** Ключи vast.ai/Kaggle/MLflow — только в `~/.config/xrun/credentials.toml`.
