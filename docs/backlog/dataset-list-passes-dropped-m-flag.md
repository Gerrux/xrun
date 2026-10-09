---
worth: yes
where: crates/xrun-kaggle/src/cli.rs:416
added: 2026-10-09
---
# `dataset list` передаёт `-m` и ждёт JSON, а kaggle 1.8.x печатает таблицу

`datasets_list_mine` вызывает `kaggle datasets list --mine -m`. Комментарий
рядом с `datasets_status` в том же файле говорит, что `-m` как
machine-readable убран в kaggle CLI 1.7.x; в 1.8.3 `-m` означает `--mine`,
а JSON-вывода нет вовсе. `parse_dataset_list` падает с `failed to parse
dataset list JSON` и вкладывает в текст ошибки всю таблицу, поэтому
пользователь видит и ошибку, и таблицу.

Нужно: вызывать `--mine --csv` и парсить CSV, как уже сделано для
`kernels list` (`parse_kernel_list_csv` там же). Источник:
отчёт powerline-seg-v1, 2026-10-09, §1.4.
