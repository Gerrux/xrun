# exp/templates — стартовые шаблоны для типовых ML-задач

Скопируй файл, переименуй, замени тело `train.py` на свою тренировку. Структура,
которую ждёт xrun (events.jsonl, metrics.jsonl, артефакты), уже на месте.

| Шаблон | Файлы | Задача |
|--------|-------|--------|
| `quickstart` | `quickstart.yaml` + `quickstart_train.py` | Zero-config smoke-test (vendor=local, без данных и кредов) — для проверки что xrun вообще работает |
| `classification` | `classification.yaml` + `classification_train.py` | Многоклассовая классификация с метриками `loss`, `acc`, `f1_macro`, `precision`, `recall` |
| `regression` | `regression.yaml` + `regression_train.py` | Регрессия с метриками `loss`, `mae`, `rmse`, `r2` |
| `kaggle_smoke` | `kaggle_smoke.yaml` (inline cmd) | Минимальный live-telemetry smoke на Kaggle (~30 секунд, 15 ticks). Проверяет что xrun_hook → MLflow → xrun ingest работает end-to-end. Требует настроенный `mlflow.url`. |
| `kaggle_classification` | `kaggle_classification.yaml` (inline cmd) | Тот же classification, но vendor=kaggle. Live events/stages/metrics в `xrun show <id>` мид-ран через MLflow. |

## Самая короткая первая проверка

```bash
xrun launch exp/templates/quickstart.yaml      # ничего не нужно настраивать
xrun events <id>
xrun metrics <id> --ascii
```

## Как пользоваться

```bash
# Скопируй шаблон в свой эксперимент
cp exp/templates/classification.yaml exp/my_classifier.yaml
cp exp/templates/classification_train.py exp/my_classifier.py

# Отредактируй cmd, args, data, artifacts под себя
# Запусти
xrun launch exp/my_classifier.yaml --detach
xrun events <id> --follow
xrun metrics <id> --key val_f1_macro --ascii
```

## Что задано шаблоном

1. **Stages**: `data_load`, `model_init`, `train`, `eval` — через `with xrun_hook.stage(...):`
2. **Metrics**: батч-логирование `xrun_hook.metrics({...}, step=epoch)` — все числа за эпоху одним вызовом, одним timestamp
3. **Epoch markers**: `xrun_hook.epoch(idx, {...})` — для прогресса в TUI
4. **Artifacts**: чекпоинты в `checkpoints/`, опциональные графики в `output/`
5. **Done**: `xrun_hook.done()` в конце — даёт корректный финальный event

## Универсальность

Метрики — произвольные строковые ключи, любые float-значения. Сегментация
(`iou_class_3`), детекция (`mAP_50`), NLP (`bleu`, `rouge_l`), RL
(`episode_reward`), генерация (`fid`, `inception_score`) — всё пишется тем же
`xrun_hook.metric(key, value, step)` или `xrun_hook.metrics({...}, step=...)`.
Никаких task-specific схем в xrun нет.

## Live-телеметрия на Kaggle

С 0.5.3 события / метрики / логи стримятся **во время** работы kernel'а
через MLflow side-channel. Перед первым Kaggle-запуском один раз:

```bash
xrun config set mlflow.url http://<your-mlflow-host>:5000
xrun config set mlflow.username <user>
xrun config set mlflow.password <password>
```

После этого `xrun events <id>` / `xrun metrics <id>` отдают данные мид-ран,
а в MLflow UI вкладка Metrics автоматически подхватывает train_loss/val_*
(зеркалится через `log-batch`). `xrun_hook` встроен в kernel автоматически —
ничего ставить в notebook deps не надо.
