---
worth: later
where: crates/xrun-kaggle/src/adapter.rs (KaggleAdapter::pull), crates/xrun-poller/src/loop_runner.rs:391-413
added: 2026-10-22
---
# Kaggle `pull` игнорирует паттерн и качает весь output на каждый паттерн

`kaggle kernels output` не умеет фильтровать файлы, поэтому
`KaggleAdapter::pull` скачивает весь output ядра целиком, а аргумент
`remote` (паттерн) отбрасывает. Poller на `on_done` вызывает `pull` по разу
на каждый элемент `artifacts.patterns` в одну и ту же директорию
(`loop_runner.rs` ~391-413): при трёх паттернах — три полных скачивания
output'а, у больших ядер это многогигабайтный повтор.

Идея: у вендора признак «pull игнорирует паттерн» (capability), и poller при
нём делает один вызов `pull` на финише, а не цикл по паттернам.

## Воспроизведение исходного exit 1 (проверить у пользователя)

Причина падения `kernels output` с пустым stderr на Windows не подтверждена;
гипотеза — кодировка вывода Python в pipe (не-cp125x символы в логе ядра
или в именах файлов). Фикс выставляет `PYTHONUTF8=1` и
`PYTHONIOENCODING=utf-8`; чтобы подтвердить, в PowerShell, по одной команде:

```
kaggle --version
```

```
kaggle kernels output <user/slug> -p C:\tmp\kout 1> out.txt 2> err.txt; echo $LASTEXITCODE
```

Затем то же самое с выставленными переменными (в чистую директорию):

```
$env:PYTHONUTF8=1; $env:PYTHONIOENCODING='utf-8'
```

```
kaggle kernels output <user/slug> -p C:\tmp\kout2 1> out2.txt 2> err2.txt; echo $LASTEXITCODE
```

Собрать: `out.txt`, `err.txt`, код выхода и список файлов в обеих директориях.
Если без переменных код 1, а с ними 0 — гипотеза верна; если 1 в обоих
случаях — в `out.txt` будет настоящая причина.

## Что ещё проверить после подтверждения причины

- Допуск «ненулевой выход, но файлы появились» (`KaggleCli::output`)
  держится на том, что kaggle CLI ставит файлам mtime скачивания, а не дату
  на сервере, — не проверено. Если причина exit 1 подтвердится и
  `PYTHONUTF8` её снимет, допуск, возможно, стоит убрать.
- `KaggleCli::output` не обёрнут в ретрай для временных ошибок (в отличие от
  остальных вызовов, `cli.rs` около `is_transient_kaggle_error`): обрыв
  посреди скачивания сразу даёт `artifacts.pull fail`.
