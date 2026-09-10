# Стабилизация xrun и работа с агентными harnesses

> Это исходный аудит до исправлений. Первый набор изменений реализует JSON для
> launch/events/sweep, корректную установку skill, явные ошибки Python CI,
> передачу config-dir и лог daemon, межпроцессный lock, ограниченные повторы
> cleanup и правильный SSH alias/workdir при stop. Небезопасный keep-instance
> теперь явно отклоняется. Атомарный ingest, idempotency, readiness handshake,
> постоянный cleanup supervisor и MCP остаются отдельными задачами.
> Дополнительно тестами выявлено: local-команда без terminal event может
> оставаться running после выхода процесса; пример smoke skill учитывает это.
>
> Проверка первого набора исправлений: fmt и Clippy успешны; Rust — 423 passed,
> 6 ignored (отчёт RTK включает дочерние процессы lock-теста), Python hook —
> 31 passed. Установленный skill прошёл quick_validate в режиме Python UTF-8.
> Live SSH/Vast/Kaggle проверки не выполнялись.

Дата: 2026-09-10. Анализ текущего рабочего дерева, включая существующие незакоммиченные файлы. Реализация не изменялась. Выводы ниже получены чтением кода; удалённые отказы и платные запуски не воспроизводились.

## Направление развития

Сохранить Rust CLI, SQLite и vendor adapters как основу. Выделить общий прикладной слой операций, который возвращает типизированные результаты; CLI форматирует их для человека или машины. Python TUI и будущий MCP используют тот же контракт. Главная инвестиция — восстановление после прерывания, достоверное состояние ресурсов и безопасные повторы операций.

Сильные стороны уже есть: отдельные адаптеры, сохранённый manifest, ULID, WAL, offsets, mock vendors, тесты финального чтения метрик, local vendor без credentials, resume и fix-status. Их следует довести до проверяемых гарантий.

## Подтверждённые проблемы и приоритеты

### P1: остановка и стоимость ресурсов

- `crates/xrun-poller/src/loop_runner.rs`: budget guard записывает `auto_destroyed_reason`, игнорирует ошибку `vendor.destroy`, переводит run в Failed и завершает poller. Неудачный запрос удаления может оставить оплачиваемый инстанс без дальнейшего контроля. Нужны раздельные состояния эксперимента и ресурса: cleanup_pending / cleanup_failed / destroyed, повторные попытки и подтверждение провайдера. Намерение удалить не должно означать успешное удаление.
- `crates/xrun-cli/src/commands/stop.rs`: `--keep-instance` пропускает destroy и только меняет статус на Cancelled; отдельного вызова остановки training process здесь нет. Следует разделить stop_process, collect_artifacts и destroy_resource. Проверять прекращение процесса до сообщения об успехе.
- Там же SSH adapter выбирается через `XRUN_SSH_ALIAS` или первый host из credentials, а не по сохранённому manifest. В `crates/xrun-ssh/src/adapter.rs` destroy использует `self.conn` и игнорирует ошибку SSH. При нескольких хостах возможна попытка остановить run на неверном хосте и ложный успех. Восстанавливать точную connection identity и workdir из контекста run; такой подход уже частично есть в poll_daemon.

### P1: конкуренция и восстановление

- `crates/xrun-poller/src/lock.rs`: HashSet защищает только потоки одного процесса; PID-файл просто перезаписывается. Два процесса могут одновременно мониторить run. Нужна межпроцессная блокировка с освобождением ОС либо lease с owner token, heartbeat и защитой от устаревшего владельца. Тест должен запускать два настоящих процесса.
- `crates/xrun-poller/src/loop_runner.rs`: append_event / append_metric и сохранение offset выполняются отдельно, ошибки записи игнорируются. При ошибке INSERT и успешном обновлении offset данные пропускаются; при сбое после INSERT до offset события повторяются. Нужна транзакция «batch + cursor», продвижение памяти только после commit, стабильный source offset для дедупликации событий. У metrics уже есть INSERT OR REPLACE — это не заменяет атомарность batch.
- `crates/xrun-cli/src/commands/launch.rs::spawn_daemon`: дочернему процессу передаются db и runs-dir, но не config-dir. Запуск с отдельной конфигурацией может получить daemon с другой конфигурацией. Кроме того, успех spawn не подтверждает готовность poller, stderr направлен в null. Передавать весь runtime context, сохранять диагностический лог, ждать ограниченный по времени readiness handshake.
- `--detach` срабатывает только после provision/upload/execute. Таймаут harness может произойти до выдачи ID. `allow_duplicate` объявлен в CLI, но в launch не используется. Нужны durable operation_id и idempotency key до первого внешнего действия; повтор того же ключа возвращает прежнюю операцию, конфликт параметров — явную ошибку. Manifest hash сам по себе не заменяет ключ запроса: одинаковые эксперименты иногда запускают намеренно.

### P1: машинный контракт

- `launch --detach --json` печатает голый ULID; foreground success не возвращает JSON-результат.
- `events --follow --json` переходит на табличный вывод.
- `sweep --launch --json` сначала печатает JSON, затем текст запуска; ошибки отдельных запусков подавляются, итоговый Result остаётся Ok.
- `main.rs` сводит прикладные ошибки к тексту и exit 1. Агенту трудно отличить ошибку входа, временный отказ, отсутствие credentials и частичное выполнение.

Решение: общий сериализуемый результат с schema_version, operation_id, run_id, outcome и error.code/retryable; stdout только JSON либо явно выбранный JSONL, диагностика в stderr. Для sweep — результат каждого элемента и общий partial_failure. Контрактные тесты запускают бинарник, парсят весь stdout и проверяют exit code, в том числе при отказах.

### P1: доставка skill

`install.rs` встраивает `claude/skill.md`. В нём нет YAML frontmatter с name и description; Codex installer пишет `.codex/skills/xrun/SKILL.md`, тогда как актуальная документация описывает `.agents/skills`. Указатель в AGENTS.md помогает прочитать файл вручную, но не заменяет корректное обнаружение skill.

Устанавливается только один файл, а ссылки на `exp/templates/` и `docs/` предполагают наличие репозитория xrun в пользовательском ML-проекте. Нужны bundled references/templates либо команды CLI, выдающие шаблоны. Пример Kaggle также устарел: он безусловно отрицает live telemetry, хотя проект поддерживает путь через MLflow.

Обновить frontmatter, пути и примеры; отделить короткий общий workflow от справочника; добавить версию CLI/skill и проверку установки. `--force` должен обновлять ограниченный маркерами блок инструкций: сейчас наличие начального маркера полностью отключает обновление указателя.

Официальные основания: [OpenAI — Build skills](https://learn.chatgpt.com/docs/build-skills), [Claude Code — Skills](https://code.claude.com/docs/en/skills). Codex документирует `.agents/skills` и обязательные name/description; Claude Code — `.claude/skills` и YAML frontmatter.

### P1/P2: проверка качества и таймауты UI

- `.github/workflows/ci.yml`: `pytest ... || echo "No tests yet"` скрывает реальные падения hook tests; Python job с названием lint фактически не запускает lint и тесты TUI. Удалить подавление ошибок, добавить проверку Python пакетов и ключевых сценариев TUI на Windows и Linux.
- `python/xrun_tui/src/xrun_tui/services.py::_run`: TimeoutError возвращает ошибку без завершения/ожидания subprocess и без восстановления результата операции. Запуск может продолжиться после сообщения timeout. Для read-only процессов — контролируемая отмена и reap; для мутаций — durable operation и последующее reconcile, чтобы таймаут UI не провоцировал повторный платный запуск.

## Предлагаемый интерфейс для Codex и Claude Code

Следующие команды — проектное предложение, их пока нельзя использовать как существующие:

1. `xrun capabilities --json`: версия протокола, команды, vendor capabilities, поддерживаемые modes.
2. `xrun launch manifest.yaml --detach --json --idempotency-key KEY`: быстро зарегистрировать операцию; выполнение provision/upload продолжает worker.
3. `xrun wait RUN_ID --timeout 30 --json`: ограниченное ожидание, отдельные результаты running / terminal / needs_attention. Истечение ожидания не отменяет training.
4. `xrun events RUN_ID --after CURSOR --limit 100 --json`: продолжение после compaction без повторной выдачи всей истории; явный next_cursor.
5. `xrun inspect RUN_ID --json`: компактный snapshot — фаза, состояние процесса и ресурса, свежесть poller, последние метрики, ошибка и доступные действия восстановления.

Сохранить project/repository identity, commit, manifest hash и fingerprint входных данных; предусмотреть явную область проекта при нескольких worktrees. Не использовать «последний run» как неявную цель изменяющих команд. Training logs и артефакты считать данными, а не инструкциями агенту.

MCP имеет смысл после стабилизации контракта: небольшой адаптер к тому же прикладному слою, с ограниченными ответами и теми же operation IDs. Начальный набор: validate, launch, inspect, wait, events, metrics, artifacts, stop. Не дублировать lifecycle в MCP и CLI.

## Порядок реализации и критерии готовности

1. **Контракт и skill:** JSON на всех ветках, корректные exit codes, исправленная упаковка skill, честный Python CI. Каждый пример skill проверяется в пустом временном пользовательском репозитории.
2. **Lifecycle:** межпроцессное владение, транзакционный ingest, точный runtime context daemon, stop/cleanup с подтверждением. После restart нет потерь событий; два poller не получают владение; ошибка destroy остаётся видимой и повторяемой.
3. **Агентные операции:** idempotency, ранний operation ID, bounded wait/cursors, восстановление после kill harness. Два одинаковых запроса создают ровно один внешний ресурс; частичный sweep можно продолжить без повторных запусков.
4. **Общий прикладной слой и MCP:** унифицировать сборку adapters/config и результаты операций; затем подключить MCP и перевести TUI на стабильные snapshots там, где прямые DB-запросы дублируют бизнес-логику.

Регрессионная матрица: Windows/Linux, local/mock vendor, non-TTY, отдельные config/data directories, повтор запроса, два процесса, обрыв после provision, ошибка upload/execute/destroy, занятая DB, частичная JSONL-строка, обрезка файла, падение poller до/после commit, частичный sweep. Платные live tests — отдельный явно запускаемый контур.

## Выполненная проверка

- `rtk cargo fmt --all -- --check`: успешно.
- `rtk proxy cargo clippy --workspace --all-targets -- -D warnings`: успешно. Первый вызов через специализированный `rtk cargo clippy` завершился ошибкой передачи `-D` как входного файла; повтор через proxy прошёл, поэтому первая ошибка не классифицируется как дефект проекта.
- `rtk cargo test --workspace`: 411 passed, 6 ignored, 50 suites по отчёту RTK.
- `rtk python -m pytest tests/ -q` в `python/xrun_hook`: 31 passed.
- Тесты Python TUI отдельно не запускались. Удалённые эксперименты не запускались; реальные credentials и пользовательская база runs не инспектировались.

Прохождение существующих тестов не покрывает перечисленные сценарии отказа. Нужны дополнительные проверки машинного протокола, двух процессов и восстановления после прерывания.
