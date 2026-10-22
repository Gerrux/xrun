---
worth: yes
where: crates/xrun-cli/src/commands/doctor.rs
added: 2026-10-22
---
# `doctor` не ограничивает время внешних проверок

Полевой отчёт (xrun 0.10.0, Windows, проект с многими манифестами в `exp/`,
вендоры kaggle + local): `xrun doctor --json` и `doctor --manifest <m>`
висели больше двух минут. Объём работы сократили: `--manifest` больше не
сканирует `exp/`, авторизация Kaggle и статус датасета идут один раз за
запуск. Но каждый отдельный вызов по-прежнему может зависнуть навсегда.

Вызовы без таймаута, все через голый `.output()` / `.status()`:

- `kaggle datasets status` и `authenticate_via_python` в
  `crates/xrun-kaggle/src/cli.rs` (~379-396 и ~457-589); готовый
  `run_with_timeout` там же (~209), но эти два пути им не пользуются;
- `vastai show user --raw` (`check_vastai_ssh_key`) и
  `python3 -c "import xrun_hook"` (`check_python_hook`) в doctor;
- пинг мостов Lightning и Colab (`PyLightningBridge::ping`,
  `PyColabBridge::ping`): на машине с настроенным Colab одна эта проверка
  занимает ~40 с, и запускается она для любого запуска doctor, потому что
  вендор берётся из кредов.

TUI вызывает `doctor --json` с лимитом 25 с
(`python/xrun_tui/src/xrun_tui/services.py` ~325-334), так что любое из
этих зависаний превращается в пустой экран Doctor.

Предложение: таймаут на каждую проверку (по умолчанию 10-15 с, настраивается
переменной окружения), по истечении — строка `WARN` с `timed out after Ns`
вместо зависания; опционально потолок на число сканируемых манифестов из
`exp/` (с пометкой в выводе, что остальные пропущены). Править придётся
`xrun-kaggle/src/cli.rs`, поэтому отдельной задачей.

Воспроизведение: проект с ~20 Kaggle-манифестами в `exp/` и рабочими
Kaggle-кредами, `xrun doctor --json` под `time`; для пинга Colab —
машина с токеном `colab-cli`, `doctor --manifest exp/<local>.yaml`.
