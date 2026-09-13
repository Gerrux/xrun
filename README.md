<p align="center">
  <img src="docs/brand/mark.png" width="88" alt="">
</p>

<h1 align="center">xrun</h1>

<p align="center">Один YAML-манифест — от арендованной GPU до лучшего чекпоинта.</p>

<p align="center"><a href="https://gerrux.github.io/xrun/">Сайт</a> · <a href="https://github.com/Gerrux/xrun/releases">Скачать</a> · <a href="docs/">Документация</a> · <a href="CHANGELOG.md">Изменения</a></p>

<p align="center"><b>Русский</b> · <a href="README.en.md">English</a></p>
<p align="center">
  <a href="https://github.com/Gerrux/xrun/releases/latest"><img alt="" src="https://img.shields.io/github/v/release/Gerrux/xrun?style=flat-square&labelColor=1A1B26&color=7AA2F7"></a>
  <a href="https://github.com/Gerrux/xrun/actions/workflows/ci.yml"><img alt="" src="https://img.shields.io/github/actions/workflow/status/Gerrux/xrun/ci.yml?branch=master&style=flat-square&labelColor=1A1B26&label=ci"></a>
  <a href="LICENSE"><img alt="" src="https://img.shields.io/github/license/Gerrux/xrun?style=flat-square&labelColor=1A1B26&color=9ECE6A"></a>
  <img alt="" src="https://img.shields.io/badge/Windows%20%7C%20macOS%20%7C%20Linux-1A1B26?style=flat-square">
  <img alt="" src="https://img.shields.io/badge/vast.ai%20%7C%20Kaggle%20%7C%20SSH%20%7C%20local-1A1B26?style=flat-square">
</p>

**Запускатель ML-экспериментов.** Один манифест описывает запуск целиком:
где взять GPU, что залить, чем тренировать и что забрать. `xrun` арендует
инстанс, заливает данные, запускает тренировку, следит за стадиями и метриками
и забирает чекпоинты — а когда всё кончилось или деньги вышли за потолок,
гасит инстанс сам.

Rust-ядро в воркспейс-крейтах и CLI `xrun` над ним; поверх — TUI на Python
Textual. Вендоров четыре: vast.ai, Kaggle, свой сервер по SSH и локальная
машина. Вся история запусков лежит в локальной SQLite — ни стороннего
трекинг-сервиса, ни аккаунта для этого не нужно; MLflow и W&B подключаются
зеркалом, если хочется их графиков.

```bash
xrun launch exp/resnet50.yaml --detach --max-cost 5   # арендовать GPU и уйти
xrun events <id> --follow                             # provision → upload → running → done
xrun metrics <id> --key val_f1 --ascii                # кривая прямо в терминале
xrun pull <id> --ckpt best --into models/             # забрать лучший чекпоинт
```

## Главное обещание

Инстанс, за который капает счёт, никогда не остаётся без присмотра. Всё
остальное в архитектуре — следствие.

`xrun launch --detach` оставляет за собой фоновый поллер. Он тянет события и
метрики, считает потраченное и гасит инстанс, когда запуск закончился, упал,
завис без вывода, вышел на плато метрики или упёрся в `--max-cost` /
`--max-hours`. Не смог погасить — шлёт push «инстанс всё ещё тарифицируется».

Остаётся одна дыра, о которой поллер сообщить не может: его собственная
смерть. Её закрывает `xrun watchdog` — из планировщика раз в пять минут и из
TUI раз в минуту. Поллер пишет пульс на каждом тике; пульса нет, а инстанс
жив — watchdog уведомляет и поднимает поллер заново. Заодно ловит инстансы,
которых нет ни в одной записи.

```
                     done / failed / плато / потолок ──► pull → destroy → push
поллер (тик) ────────┤
                     не смог погасить ─────────────────► push «ещё тарифицируется»

watchdog (5 мин) ────► пульса нет, инстанс жив ─────────► push → поднять поллер
```

Как это устроено внутри — [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).

## Установка

macOS и Linux:

```bash
curl -sSf https://raw.githubusercontent.com/Gerrux/xrun/master/install.sh | sh
```

Windows (PowerShell):

```powershell
irm https://raw.githubusercontent.com/Gerrux/xrun/master/install.ps1 | iex
```

Скрипт кладёт бинарь `xrun` (`~/.local/bin` либо `%LOCALAPPDATA%\xrun\bin`) и
ставит TUI через `pip --user` — для неё нужен Python 3.11+. Флаги `--no-tui` /
`-NoTui` ставят только CLI, `--version v0.8.0` — конкретный выпуск,
`--install-pip` / `-InstallPip` попробует `ensurepip`, если pip нет.

Из исходников:

```bash
cargo install --git https://github.com/Gerrux/xrun --branch master xrun-cli
pip install -e python/xrun_tui
```

Обновляется `xrun` сам: при интерактивном запуске он сверяется с релизами и
спрашивает, прежде чем ставить. `xrun update --check` только проверяет,
`XRUN_NO_UPDATE_CHECK=1` выключает проверку в скриптах.

## Первый запуск

Без кредов, без GPU и без данных — чтобы убедиться, что цепочка вообще живая:

```bash
xrun doctor
xrun launch exp/templates/quickstart.yaml
xrun metrics <id> --ascii
```

Дальше — ключи вендоров. Проще всего визардом: `xrun` без аргументов откроет
TUI, а на первом запуске — мастер настройки (`xrun init`). Он же заводит
уведомления. Готовые заготовки манифестов под классификацию, регрессию и
Kaggle лежат в [exp/templates](exp/templates/README.md).

## Манифест

```yaml
name: resnet50_baseline
vendor: vast                    # vast | kaggle | ssh | local

vast:
  image: pytorch/pytorch:2.4.1-cuda12.1-cudnn9-devel
  gpu: { type: "RTX 4090", count: 1 }
  price: { max_per_hour: 0.55 }

data:
  - src: data/train.h5
    dst: /workspace/data/train.h5

run:
  cmd: python train.py
  args:
    --lr: 5e-4
    --epochs: 30

artifacts:
  patterns: ["checkpoints/best*.pt"]

policy:
  on_idle_minutes: 30           # нет вывода полчаса — гасить
  early_stop:                   # плато метрики: забрать лучший и погасить
    metric: val_f1
    patience: 5
```

После запуска копия манифеста ложится рядом с записью о запуске, а его хеш
становится частью личности запуска: оригинал можно править свободно,
`xrun rerun <id>` воспроизводит ровно то, что бежало. Перебор гиперпараметров —
`xrun sweep exp/base.yaml --grid run.args.--lr=1e-3,5e-4 --launch`, он
материализует по манифесту на вариант, а не шаблонизирует один.

Полная схема — [docs/MANIFEST.md](docs/MANIFEST.md).

## Хук в тренировочном скрипте

`xrun_hook` пишет стадии и метрики в `events.jsonl` / `metrics.jsonl`, откуда их
забирает поллер. На Kaggle он встраивается в kernel сам.

```python
from xrun_hook import stage, metric, metrics, done, notify

with stage("train"):
    for ep in range(epochs):
        loss = train_one_epoch(model, loader)
        metric("train_loss", loss, step=ep)
        metrics({"val_loss": v.loss, "val_f1": v.f1}, step=ep)

notify("обучение", f"лучший val_f1 = {best:.3f}")
done()
```

Необработанное исключение хук запишет событием `error` сам. Протокол целиком —
[docs/EVENTS.md](docs/EVENTS.md).

## TUI

`xrun` без аргументов в терминале открывает TUI. Навигация аккордами от `g`:

| | |
| --- | --- |
| `g d` | Dashboard — расход в час, активные запуски, на сколько хватит баланса |
| `g r` | Runs — список с живым статусом; `Enter` — стадии, логи, метрики, артефакты, манифест |
| `g i` | Instances — что сейчас арендовано у вендоров |
| `g v` | Vendors — ключи и баланс |
| `g l` | Launch — выбрать манифест и запустить |
| `g n` | Notifications — каналы push, проверка, регистрация watchdog |
| `g h` | Doctor — проверка окружения |
| `?` · `Ctrl+P` | помощь · палитра команд |

Экраны и биндинги — [docs/TUI.md](docs/TUI.md).

## Уведомления

ntfy, Telegram, вебхук (Slack, Discord) и системные тосты. Поллер шлёт: запуск
кончился, упал или завис; потрачено 50 % и 80 % от `--max-cost`; инстанс
погашен по потолку или не погасился; NaN или взрыв loss; остановка по плато — и
всё, что скрипт отправил через `xrun_hook.notify(...)`. Боту в Telegram можно
ответить `/stop <id>`.

```bash
xrun config set notify.channels ntfy,desktop
xrun config set ntfy.topic my-random-topic
xrun notify test                      # код 1 — канал не работает, чинить до запуска
xrun watchdog schedule --install      # раз в 5 минут через schtasks / crontab
```

В TUI то же самое — экран `g n`: топик ntfy генерируется, chat id Telegram
находится одной кнопкой.

## Принципы

- **Состояние локально.** Одна SQLite на машину, общая для всех проектов. Всё,
  что умеет TUI, умеет и CLI, а все читающие команды отдают `--json`: скрипт и
  агент видят то же, что человек.
- **Потолок дешевле счёта.** Бюджет, простой и плато проверяются на каждом тике
  поллера, а не по итогам. Неудачное уничтожение инстанса — не строка в логе, а
  push.
- **Креды не живут в репозитории.** Только в `credentials.toml` в каталоге
  конфигурации пользователя, никогда в манифесте; на инстанс уходит копия
  манифеста без них.
- **Один манифест — один самодостаточный файл.** Без `include`, `extends` и
  шаблонизации: дублирование лучше скрытой иерархии.
- **Агенту — те же команды.** `xrun install skill --claude` / `--codex` учит
  Claude Code и Codex пользоваться `xrun` вместо ручных `vastai` и `ssh`.

## Документация

| | |
| --- | --- |
| [CLI](docs/CLI.md) | все подкоманды, флаги, машинный вывод, коды выхода |
| [Манифест](docs/MANIFEST.md) | полная YAML-схема с примерами под каждого вендора |
| [Архитектура](docs/ARCHITECTURE.md) | компоненты, поток данных запуска, модель поллера, отказы |
| [События и метрики](docs/EVENTS.md) | протокол `events.jsonl` / `metrics.jsonl` и `xrun_hook` |
| [Состояние](docs/STATE.md) | схема SQLite, миграции, резервная копия |
| [TUI](docs/TUI.md) | экраны, биндинги, темы |
| [Скилл для агентов](docs/SKILL.md) | что скилл делает и чего не делает |
| [Дорожная карта](docs/ROADMAP.md) | история версий и что дальше |

## Как помочь

Дороже всего — отчёт «запустил на живом вендоре, вот что вышло»: с каждым из
них API ведёт себя по-своему, и половина исправлений в истории пришла именно
так. Остальное — в [CONTRIBUTING.md](CONTRIBUTING.md). Утечку кредов или способ
оставить инстанс тарифицироваться незаметно не заводите публичным issue:
[SECURITY.md](SECURITY.md).

## Лицензия

[MIT](LICENSE).
