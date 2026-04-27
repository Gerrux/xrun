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
| `vendor` | enum | да | `vast` \| `kaggle` |
| `vast` / `kaggle` | object | да | По одному из них в зависимости от `vendor` |
| `data` | [object] | нет | Что предзалить |
| `run` | object | да | Команда тренировки |
| `checkpoints` | object | нет | Watch + pull policy |
| `artifacts` | object | нет | Дополнительные файлы |
| `mlflow` | object | нет | Если отсутствует — метрики только в SQLite |
| `policy` | object | нет | Поведение при ошибках/idle |

### `data[]`

| Поле | Описание |
|------|----------|
| `src` | Локальный путь (файл или директория) |
| `dst` | Путь на инстансе |
| `mode` | `copy` (default) \| `rsync` |
| `unpack` | `{ format: tar\|zip\|tar.gz, into: <path> }` после копирования |

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

`manifest_hash = sha256(canonical_yaml(manifest))`. После запуска копия пишется в `runs/{run_id}/manifest.yaml` — оригинал можно править свободно, run всегда воспроизводится по своей копии.

## Что мы СОЗНАТЕЛЬНО не делаем

- **Не jinja-шаблонизация манифеста.** Если нужна развёртка по гиперпараметрам — отдельная команда `xrun sweep <manifest> --grid lr=1e-3,1e-4 batch=4,8`, она генерит N материализованных манифестов.
- **Не include / extends.** Один манифест — один self-contained файл. Дублирование лучше скрытой иерархии.
- **Не secrets в манифесте.** Ключи vast.ai/Kaggle/MLflow — только в `~/.config/xrun/credentials.toml`.
