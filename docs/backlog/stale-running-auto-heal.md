---
worth: yes
where: crates/xrun-cli/src/commands/{ls,show,watchdog,resume}.rs, crates/xrun-poller/src/lock.rs
added: 2026-10-22
---
# Зависшие `running` не лечатся сами, пока пользователь не вызовет `fix-status`

Второй полевой случай (после local-после-перезагрузки, закрытого
`fix-status`/`resume`): Kaggle-ран с мёртвым poll-daemon остался
`running` на 8 часов после того, как kernel завершился; закрылся только
ручным `xrun fix-status`. Автоматических лекарей два: `xrun watchdog`
(работает, только если пользователь сам выполнил
`xrun watchdog schedule --install`) и открытый TUI (раз в 60 с). Без них
`xrun ls` / `xrun show` честно показывают `running`, и человек узнаёт
правду от агента или из письма вендора.

## Идея: дешёвая локальная проверка при `ls` / `show`

Для каждой строки `running` без сети:

1. Свежий heartbeat (`poller_heartbeat_at` моложе `heartbeat_stale_min`) —
   ничего не делать.
2. Иначе попробовать взять `<runs_dir>/<run_id>/poller.lock`
   (эксклюзивный fs2-замок, который держит живой поллер,
   `crates/xrun-poller/src/lock.rs`). Замок не зависит от переиспользования
   PID: ОС снимает его при смерти процесса.
3. Замок свободен и heartbeat протух — поллера нет. В stderr подсказка:
   `poller for <id> is not running (last heartbeat 8h ago) — run xrun resume <id>`;
   в `--json` аддитивные поля `poller_alive` и `heartbeat_age_secs`
   (существующие поля не меняются).
4. Для `vendor: local` можно сразу закрыть ран синхронно тем же
   `reconcile_dead_local`, что используют `fix-status`/`resume`: это
   локальные файлы, сети нет.
5. Для удалённых вендоров (kaggle/vast/ssh) — максимум фоновый
   `xrun resume <id>` detached, с троттлингом (один раз в N минут на ран,
   метка в `runs/<id>/`).

Компромиссы:

- Читающие команды начинают менять БД. Для варианта «только подсказка»
  этого нет; синхронное лечение local и фоновый resume — осознанный шаг,
  лучше за флагом/настройкой `[ui].auto_heal`.
- Гонка со стартом поллера: между `launch --detach` и первым heartbeat
  замок ещё свободен. Нужна грация от `started_at`/`created_at`
  (как в `watchdog::classify` для варианта «neither»).
- Ноутбук спал: heartbeat протух у всех ранов разом, а поллеры живы и
  проснутся. Замок это отличает (держится живым процессом), одного
  heartbeat — нет.

## Дыра в watchdog: переиспользованный PID поллера

`watchdog::classify` (`watchdog.rs` ~137-164) считает поллер живым по
`process_alive(poller_pid)`. После перезагрузки (или просто спустя время)
PID поллера может достаться постороннему процессу; тогда при протухшем
heartbeat состояние `Hung`, и respawn не делается. Тот же недостаток в
`resume_one` (~134-145): `AlreadyRunning` возвращается по одному PID.
Исправление: перед `Hung`/`AlreadyRunning` пробовать `poller.lock`;
свободный замок = поллера нет, независимо от PID (и тогда `Dead`, respawn).

## Как воспроизвести

Случай 1 (мёртвый поллер, живой/завершённый kernel):
`xrun launch exp/kaggle_smoke.yaml --detach`, завершить процесс
`__poll-daemon` (`Stop-Process`), дождаться конца kernel'а. `xrun ls`
показывает `running` бесконечно; `xrun fix-status` закрывает.

Случай 2 (watchdog и переиспользованный PID): запустить любой ран с
`--detach`, убить поллер, в БД подставить `poller_pid` чужого живого
процесса (например, PID оболочки) и состарить `poller_heartbeat_at`
больше чем на `heartbeat_stale_min`. `xrun watchdog --json` даёт `Hung`
и не респавнит; `xrun resume <id>` отвечает `already_running`.
