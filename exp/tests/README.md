# exp/tests — тестовые манифесты для прокликивания TUI

Каждый файл отрабатывает один кусок функционала. Все «local-*» — самодостаточны
(не требуют сети и кредов), запускаются за секунды. «ssh-*», «vast-*»,
«kaggle-*» — для проверки конкретного вендора.

## Local (без сети, без кредов)

| Манифест | Что проверяет в TUI |
|---|---|
| [`local_metrics_rich.yaml`](local_metrics_rich.yaml) | Run detail → Metrics tab: 6 ключей × 5 эпох, `xrun_hook.epoch()`, несколько stage'ов |
| [`local_artifacts.yaml`](local_artifacts.yaml) | Run detail → Artifacts tab: `checkpoints/best*.pt`, `output/*.png`, `metrics.json` |
| [`local_fail.yaml`](local_fail.yaml) | Runs list → красный статус FAILED; Run detail → Stages tab → failed stage; Error detail screen |
| [`local_mlflow.yaml`](local_mlflow.yaml) | Mirror sink: метрики уезжают в MLflow (нужен `xrun config set mlflow.url …`) |
| [`../local_sleep.yaml`](../local_sleep.yaml) | `xrun stop <id>` → graceful kill; Run detail → status STOPPED |
| [`../local_smoke.yaml`](../local_smoke.yaml) | Минимальный happy-path для doctor / poller-daemon |

## SSH (требует настроенный alias в `credentials.toml`)

| Манифест | Что проверяет |
|---|---|
| [`ssh_smoke.yaml`](ssh_smoke.yaml) | `vendor: ssh` + `host_alias` resolve; rsync upload + tail |

## Vast (платно — только `--dry-run`)

| Манифест | Что проверяет |
|---|---|
| [`vast_dryrun.yaml`](vast_dryrun.yaml) | `xrun launch exp/tests/vast_dryrun.yaml --dry-run` — поиск офферов, оценка стоимости, без `create instance` |

## Kaggle

| Манифест | Что проверяет |
|---|---|
| [`../kaggle_livelog_smoke.yaml`](../kaggle_livelog_smoke.yaml) | Mirror via MLflow → live-tail метрик в TUI пока ноут крутится на Kaggle |

## Как запускать

```bash
# Самый быстрый smoke
xrun launch exp/tests/local_metrics_rich.yaml --detach
xrun ls
xrun events <id> --follow
# в TUI: g r → Enter на ране → 4 (Metrics)

# Артефакты
xrun launch exp/tests/local_artifacts.yaml --detach
xrun pull <id> --ckpt best --into /tmp/xrun-pull-test/

# Failure
xrun launch exp/tests/local_fail.yaml --detach
xrun show <id>          # должен быть status=failed, exit_code=1

# MLflow mirror (после `xrun init` с mirror+mlflow url)
xrun launch exp/tests/local_mlflow.yaml --detach
# открой http://<mlflow-url> → experiment `xrun-mlflow-test`

# Vast dry-run
xrun launch exp/tests/vast_dryrun.yaml --dry-run
```

## Чек-лист TUI после запусков

1. **Dashboard** — счётчики running/done/failed, последние 5 ранов.
2. **Runs list** — фильтр по статусу, цветные бейджи.
3. **Run detail → Stages** — таймлайн стадий, длительности.
4. **Run detail → Logs** — `--follow`-tail, поиск.
5. **Run detail → Metrics** — таблица final-значений + grid с sparklines по каждому ключу.
6. **Run detail → Manifest** — отображение исходного YAML.
7. **Artifacts** — список подтянутых файлов с размерами.
8. **Doctor** — все `OK`, MLflow строчка показывает URL после визарда.
