---
worth: yes
where: crates/xrun-cli/src/commands/rerun.rs:103
added: 2026-10-09
---
# `rerun --bump-dataset` пушит датасет без сверки с Kaggle

`xrun dataset push` с 2026-10-09 после `ready` сверяет список файлов на
Kaggle с локальным стейджингом и падает при расхождении (`--verify`).
`bump_dataset` в `rerun` вызывает тот же `cli.dataset_push`, но сверку не
делает и сразу запускает ядро, так что пустая версия на этом пути
по-прежнему проходит молча.

Нужно: вынести `verify_upload` из `commands/dataset.rs` в общий модуль и
вызывать после push в `bump_dataset`; при расхождении не запускать.
