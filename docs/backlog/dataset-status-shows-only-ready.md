---
worth: yes
where: crates/xrun-cli/src/commands/dataset.rs:215
added: 2026-10-09
---
# `dataset status` не показывает версию и время обновления

Текстовый вывод содержит только `ready`. Для проверки «та ли версия
подхватится ядром» нужны `currentVersionNumber` и `lastUpdated` из
`datasets/view` (клиент `KaggleApiClient::dataset_current_version` уже
ходит на этот endpoint). Число файлов и размер добавлены 2026-10-09
вместе со сверкой после push; версия и время обновления остались.
Источник: отчёт powerline-seg-v1, 2026-10-09, §1.3.
