---
worth: yes
where: crates/xrun-kaggle/src/snapshot.rs:46
added: 2026-10-09
---
# `dataset push` печатает все имена файлов в одну строку

`SnapshotDiff::render` склеивает каждый список (`added` / `changed` /
`removed`) через запятую. Для стейджинга из 13 865 файлов это одна строка
на 358 КБ в stderr. Агент, читавший вывод, принял список за подтверждение
загрузки, хотя это локальный fingerprint до вызова `kaggle`.

Нужно: сводка `added N · changed M · removed K · unchanged U`, полный
список только по `--verbose`. Источник: отчёт о запуске powerline-seg-v1
на Kaggle, 2026-10-09, §1.5.
