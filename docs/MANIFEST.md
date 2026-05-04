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
  on_stage_failed: stop_instance     # stop_instance | keep | reprovision
  on_idle_minutes: 30                # auto-stop если нет stdout > N min
  on_done: stop_instance
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
  `<workdir>/<run-id>/` создаётся автоматически в `provision()`.
- **xrun_hook на удалёнке.** Установи `pip install xrun-hook` на сервере или
  включи в `data:` как для vast. `XRUN_RUN_DIR=<run-dir>` подставляется в env.
- **destroy только убивает PID,** не машину. Идемпотентно: повторный `xrun
  stop <id>` ничего не сломает.
- **stop без manifest copy.** `xrun stop` использует `XRUN_SSH_ALIAS` env
  override либо первый ssh-хост из creds (best-effort, идемпотентен).
  Когда есть стояла копия манифеста — берётся правильный alias.

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
| `vendor` | enum | да | `vast` \| `kaggle` \| `local` \| `ssh` |
| `vast` / `kaggle` / `local` / `ssh` | object | да | По одному в зависимости от `vendor` (`local` блок опционален) |
| `data` | [object] | нет | Что предзалить |
| `run` | object | да | Команда тренировки |
| `checkpoints` | object | нет | Watch + pull policy |
| `artifacts` | object | нет | Дополнительные файлы |
| `mlflow` | object | нет | Если отсутствует — метрики только в SQLite |
| `policy` | object | нет | Поведение при ошибках/idle |
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

### `kaggle`

| Поле | Описание |
|------|----------|
| `kernel_slug` | `<username>/<slug>` (обязательно) |
| `competition` | Название соревнования (или `null`) |
| `dataset` | Attached dataset slug (`user/ds`) |
| `enable_gpu` | `true` / `false` |
| `enable_internet` | `false` для большинства соревнований |

#### Kaggle constraints

- `enable_internet=false` → нельзя `pip install` на ходу. xrun автоматически кладёт `xrun_hook` wheel в staging и инжектит `sys.path` — ничего настраивать не нужно.
- `run.notebook` указывает `.ipynb`; первая ячейка должна содержать `import xrun_hook` (или xrun добавит её автоматически через nbformat).
- `kernel_slug` обязан быть в формате `<username>/<slug>` — `push` упадёт иначе.
- Live-tail недоступен (нет SSH на Kaggle). Метрики и события восстанавливаются после завершения через `ingest` (парсинг `events.jsonl` / `metrics.jsonl` из output).

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
