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
- Запускать `xrun doctor` при подозрении на сломанные креды.
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
