# Claude Skill

Цель: Claude умеет запускать и инспектить эксперименты через `xrun`, не пиша руками bash и не дёргая `vastai`/`kaggle` CLI напрямую.

## Установка скилла

`~/.claude/skills/xrun/SKILL.md` — короткий контекст для модели:
- Что такое xrun и когда использовать
- Полный список команд (см. CLI.md)
- Схема манифеста (см. MANIFEST.md, краткая версия)
- Список anti-patterns
- 3-5 каноничных примеров

`~/.claude/skills/xrun/scripts/` — опционально вспомогательные shell-скрипты, если что-то совсем мономотивное.

## First-run / unconfigured state

Перед запуском любого вендорного экспа скилл:
1. Проверяет `xrun doctor --json` (exit 0 = core healthy).
2. `xrun config show` (без `--secrets`) — что настроено.
3. Если для нужного вендора нет кредов — НЕ пишет `credentials.toml`
   вручную и НЕ пытается `! xrun init` (Claude Code-овая bash без TTY,
   визард упадёт). Инструктирует пользователя открыть **отдельный
   терминал** и запустить `xrun init` там. После выхода визарда —
   `xrun doctor --json` и продолжить. Если ключ уже у пользователя в
   руках, можно записать без TTY: `printf '%s' "$KEY" | xrun init
   --non-interactive --mark-completed --vast-key -`.
4. Для абсолютной проверки «xrun вообще жив» — `xrun launch
   exp/templates/quickstart.yaml` (vendor=local, без кредов и данных).

В non-interactive контекстах (CI/скрипт) скилл может использовать
`xrun init --non-interactive --vast-key -` со stdin-pipe. В обычной сессии
с пользователем — всегда визард, чтобы ключи не попадали в транскрипт.

## Триггеры

Скилл активируется когда пользователь:
- «запусти эксперимент / тренировку / запусти X на vast / Kaggle»
- «pull чекпоинт / забери best / посмотри артефакты»
- «покажи метрики / графики / val_f1 для последнего ранa»
- «список запусков / что сейчас крутится»
- «погаси инстанс / останови run»
- «сравни runs X и Y»
- «повтори запуск с другим lr»

## Что скилл МОЖЕТ делать

- Создавать/редактировать манифесты в `exp/`.
- Звать `xrun launch / ls / show / metrics / pull / stop / rerun / sweep`.
- Парсить `xrun ... --json` вывод и принимать решения (например: «возьми чекпоинт ранa с наибольшим val_f1 за последние сутки»).
- `xrun doctor --manifest exp/foo.yaml` — pre-flight перед запуском (схема + Kaggle dataset readiness).
- `xrun fix-status [<id>]` — починить «зависшие» в `running` записи если поллер умер.
- `xrun dataset push/status/list` — Kaggle datasets без `kaggle` CLI напрямую.
- Запускать `xrun config show` (без секретов) при необходимости.

## Что скилл НЕ ДЕЛАЕТ

- ❌ `vastai create instance ...` напрямую
- ❌ `kaggle kernels push ...` напрямую
- ❌ `ssh root@... "..."`
- ❌ `rsync` руками для заливки данных на инстанс
- ❌ `mlflow ui` как фоновый процесс
- ❌ Чтение `runs.db` напрямую (всегда через `xrun ... --json`)
- ❌ Запись в `events.jsonl` или другие файлы state

Если фича недоступна — добавить её в `xrun`, а не обходить.

## Канонические примеры в SKILL.md

````markdown
### Запустить новый эксперимент

User: «Запусти v8 ResUNet3D с lr=5e-4, batch=4 на vast 4090»

Claude:
1. Открыть существующий близкий манифест, например `exp/arborust_v7_C.yaml`.
2. Скопировать в `exp/arborust_v8.yaml`.
3. Изменить `name`, `args.--lr`, `args.--batch-size`.
4. `xrun launch exp/arborust_v8.yaml --detach`.
5. Запомнить run id из вывода для следующих команд.

### Pull лучшего чекпоинта

User: «Забери best из последнего успешного arborust ранa»

Claude:
1. `xrun ls --status done --tag arborust --json | head` → run_id
2. `xrun pull <run_id> --ckpt best --into models/`
3. Сообщить локальный путь.

### Сравнение

User: «Покажи метрики двух последних ранов arborust»

Claude:
1. `xrun ls --status done --tag arborust --json` — взять два последних.
2. `xrun metrics <id1> --key val_f1,val_loss --json`
3. `xrun metrics <id2> --key val_f1,val_loss --json`
4. Сравнить, дать таблицу + рекомендацию.

### Hyperparameter sweep

User: «Прогони arborust по lr 1e-3, 5e-4, 1e-4 и batch 4, 8»

Claude:
1. `xrun sweep exp/arborust_v8.yaml \
     --grid run.args.--lr=1e-3,5e-4,1e-4 \
     --grid run.args.--batch-size=4,8 \
     --launch --detach --yes --json`
2. Запомнить путь к материализованным манифестам (под `exp/sweep_*/`) и
   `run id` каждого детачнутого ранa из stdout.
3. Доложить таблицу: 6 ранов запущено, ID и параметры.

### Зависший run

User: «Уже час `xrun ls` показывает arborust running, а в Kaggle UI он
давно done».

Claude:
1. `xrun fix-status <id>` (или просто `xrun fix-status` для всех).
2. Сообщить новый статус из вывода.
3. Если стал `done` — `xrun pull <id> --ckpt best`.
````

## Anti-pattern в SKILL.md

````markdown
### НЕ делай так

❌ User: «Запусти train_v5_multichannel.py на vast 4090 с lr 5e-4»
❌ Claude: `vastai create instance ... && vastai execute ... "python train_v5_multichannel.py --lr 5e-4"`

✅ Правильно: создать/обновить манифест в `exp/` и `xrun launch <manifest>`.

### НЕ делай так

❌ User: «Какой val_f1 у последнего ранa?»
❌ Claude: `cat ~/.local/share/xrun/runs.db | sqlite3 ...`

✅ Правильно: `xrun show <id>` или `xrun metrics <id> --key val_f1 --json`.
````

## Версионирование

Skill хранит свою версию (например `xrun-skill-v1`), парная к мажорной версии CLI. При несовместимом изменении CLI — bump skill.

## Доставка

Когда `xrun v0.1` готов:
1. Положить SKILL.md в `~/.claude/skills/xrun/`.
2. (Опционально) пакетировать как Claude Code plugin для распространения.
3. Депрекейтнуть `train-vast` skill в его SKILL.md строкой «для новых экспериментов используй `xrun`».
