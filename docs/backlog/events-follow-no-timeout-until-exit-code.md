---
worth: yes
where: crates/xrun-cli/src/commands/events_cmd.rs:55
added: 2026-10-09
---
# `events --follow` не умеет ждать стадию, не имеет таймаута и всегда выходит с 0

Команда уже выходит сама при терминальном статусе (`done` / `failed` /
`cancelled`), так что отдельная `xrun wait` не нужна. Не хватает трёх
вещей для агента без TTY: `--until <stage>` (выйти при первом событии
стадии, например `epoch`), `--timeout 15m` и ненулевой код возврата на
`failed` / `cancelled` / таймаут. Сейчас после `launch --detach` агент
ждёт через `sleep N; xrun logs` в фоне. Источник: отчёт powerline-seg-v1,
2026-10-09, §6.
