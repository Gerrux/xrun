---
worth: yes
where: crates/xrun-kaggle/src/adapter.rs:1368
added: 2026-10-09
---
# В ядре Kaggle путь монтирования экспортируется только для первого датасета

Обёртка `main.py` пробует `/kaggle/input/datasets/<owner>/<name>` и
`/kaggle/input/<name>` и кладёт первый найденный в `XRUN_INPUT_DIR`. При
двух и более датасетах (код + кеш) второй путь приходится искать через
`find /kaggle/input`. Переменная не описана ни в `docs/`, ни в скилле.

Нужно: `XRUN_DATASET_<NAME>` (имя датасета в верхнем регистре, `-` → `_`)
на каждый датасет плюс `XRUN_INPUT_DIR` для совместимости; описать в
`docs/MANIFEST.md` (`kaggle.datasets`) и в скилле. Источник: отчёт
powerline-seg-v1, 2026-10-09, §3.
