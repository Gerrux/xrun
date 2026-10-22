---
worth: maybe
where: python/xrun_hook/src/xrun_hook/__init__.py, crates/xrun-core/src/store/metrics.rs
added: 2026-10-22
---
# Итог прогона с интервалом — отдельной сущностью, а не точкой ряда

Модель метрик — ряд «шаг → значение» с ключом `(run_id, key, step)`. У
LOGO-кросс-валидации ArboForge главная цифра (pooled OOF macro-F1 и его
бутстреп-интервал) появляется один раз в конце. Сейчас её пишут
`metrics({"pooled_f1": v, "pooled_f1_lo": lo, "pooled_f1_hi": hi},
step=<последний шаг>)`, а `xrun show` печатает последнее значение каждого
ключа (`last=… (step N)`). Для чтения и сравнения прогонов этого хватает.

Чего так нет: интервал — три несвязанных ключа; `xrun metrics --ascii` и
`--png` рисуют для них ряд из одной точки; MLflow получает их как обычные
метрики со step, а не как итог run. Если это начнёт мешать — `summary({...})`
в хуке (отдельный `summary.jsonl` или событие `stage="summary"`), таблица
`run_summary` новой миграцией, блок «Summary» в `xrun show` и колонка в
`xrun ls`/compare. Решать, когда понадобится сравнивать итог между
прогонами в TUI или проставлять его в MLflow как run-level значение.
