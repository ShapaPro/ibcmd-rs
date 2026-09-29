# `ibcmd infobase config apply` in the drop-in mode (milestone 0.4, issue #343)

`ibcmd infobase config apply [--force] [--dynamic=...] [--session-terminate=...]`, in the
platform's syntax, served by the own exclusive apply (`ibcmd-rs mssql-config-apply`, #337).
This page records what the platform's command does (measured on 8.3.27.2214, every case in
[`evidence/dropin-apply/native-cases.md`](evidence/dropin-apply/native-cases.md)), which of it
this version serves and how, and the words and exit codes it answers with.

Contents: [the platform's command](#the-platforms-command) -
[what is served](#what-is-served) - [what the drop-in prints](#what-the-drop-in-prints) -
[differences from the platform](#differences-from-the-platform) -
[the seam](#the-seam) - [tests and lab evidence](#tests-and-lab-evidence) -
[open points](#open-points).

## The platform's command

### Options (from `ibcmd help infobase`, `config` > `apply`)

| Option | Meaning | Default |
|---|---|---|
| `--extension=<name>`, `-e` | the configuration extension to update | the main configuration |
| `--force`, `-F` | "confirm the operation when there are warnings" | off |
| `--dynamic=<auto\|disable\|prompt\|force>` | use of the dynamic (online) update: `auto` decides, `disable` forbids it, `prompt` asks the user, `force` uses the dynamic update only | `auto` |
| `--session-terminate=<disable\|prompt\|force>` | end the active sessions when the exclusive lock of the infobase is needed: forbidden, ask, end them | `disable` |
| `--session-terminate-message=<message>` | the text a terminated session shows | none |
| `--user`, `-u`, `--password`, `-P` (of `config`) | the infobase user | none |

What `--force` confirms: the warnings of an apply that restructures data, such as
`[WARN] Код справочника стал неуникальным: _ДемоКонтрагенты (0)` or `[WARN] Номер документа стал
неуникальным в заданном периоде ЭлектронноеПисьмоИсходящее (00000000001)` (printed and gone past
in the lab's structural applies with `--force`, `restructure-check/runs/probe_auto_a`). A clean
module apply prints the same with and without it (n10 against n30). What the platform does
*without* `--force` when such a warning comes was not measured; the own apply never restructures,
so it has nothing to confirm.

The words are case-sensitive; a wrong or missing word is `Некорректное значение параметра: dynamic`
(or `session-terminate`) on stderr and exit 2 (cases p01-p03, p08, p10, p11). A flag given a value
(`--force=yes`) is `Ошибка разбора параметра: force` (exit 2, the name without dashes; the same for
`export --sync=1`), an unknown option `Ошибка разбора параметра: --bogus`. A stray argument is
ignored. `ibcmd infobase config apply --help` prints the top-level help, not a help of the command.

### Two ways to run it

* **Direct** (`--dbms=MSSQLServer --db-server ... --db-name ...`, the way every script of the lab
  and the drop-in's export and import run): the platform opens the database itself. It does
  **not look for other sessions at all**: with a 1C cluster session connected to the database
  (a thin client of a cluster infobase, `rac session list` shows it) the apply runs as if nobody
  were there (n20). Only a running stand-alone server on the same `--data` stops it
  (`Ошибка блокировки каталога данных сервера.` / `Рабочий каталог заблокирован процессом: <pid>`,
  exit -2, n21).
* **Server** (`--pid=<pid>` or `--remote=<url>` of a stand-alone server, `ibsrv`): the command
  runs inside the server, which knows its sessions. Its lines start `[INFO ]` (a space before the
  bracket). The server keeps its own picture of the infobase: rows written to `ConfigSave` behind
  its back are not seen (n22: "не требуется" with six rows staged).

### What it printed (direct mode, БСП 8.3.27 clone)

| Case | stdout | stderr | Exit | Time |
|---|---|---|---|---|
| Nothing staged (n01, n02) | `[INFO] Обновление конфигурации базы данных...` / `[INFO] Проверка корректности метаданных...` / `[INFO] Обновление конфигурации базы данных не требуется` | empty | 0 | 5-38 s (the check) |
| A module changed, any of `auto` (default), `disable`, `prompt` (n10, n11, n31) | `[INFO] Обновление конфигурации базы данных...` / `Проверка корректности метаданных...` / `Принятие изменений...` / `Обработка данных` / `Обработка данных Регистрация изменений в планах обмена` / `Создано поколение конфигурации: <32 hex><00000000>` / `Обработка данных Регистрация изменений в планах обмена` / `[INFO] Обновление конфигурации базы данных успешно завершено` (the last two lines swap places from run to run) | empty | 0 | 8-34 s |
| The same with `--dynamic=force` (n12) | the same lines | empty | 0 | 6 s |
| `--session-terminate=force` or `prompt`, a message (n30, n31) | as a plain apply: nobody to end | empty | 0 | |
| A staged descriptor breaks the metadata check (n46) | the first two INFO lines | `[ERROR] ОбщийМодуль._ДемоЛокализация: Дублирование имени объекта метаданных: ` / `[ERROR] Операция невозможна: при выполнении проверки корректности метаданных обнаружены ошибки` | **1** | 5 s |
| A staged descriptor is not readable (n45) | the first two INFO lines | `Ошибка формата потока` | -1 | 10 s |
| The database does not exist (n42) | empty | `База данных отсутствует в сервере баз данных` / `Не найдена база данных '<db>' в SQL-сервере 'localhost'` | -1 | 1 s |
| The SQL server cannot be reached (n43) | empty | `Соединение с сервером баз данных разорвано администратором` / the OLE DB text | -1 | 17 s |
| Unknown infobase user (n40) | `Для выполнения операции требуется аутентификация в информационной базе` / `Пароль для '<user>': ` | `Идентификация пользователя не выполнена` | -1 | 5 s |
| No `--user` on a database that has users, stdin closed (n41) | the same first line, then `Имя пользователя: ` forever (12 MB a minute) | empty | never ends | |
| A command-line error (p01-p12) | empty | see above | 2 | 0.3 s |

What the apply changes in the tables: the staged rows replace the `Config` rows in place,
`ConfigSave` is emptied, `versions` names a new generation. `auto` leaves exactly the state
`disable` leaves (n10 against n11: no `_dynupdate_` row, no `DynamicallyUpdated` marker);
`force` writes the dynamic overlay (n12: four alias rows, the markers in `Config` and `Params`).
`Создано поколение конфигурации:` prints the head of the new `versions` row: the 16 bytes of the
GUID as stored (little-endian fields) in hex, then `00000000` (`40cfe0ac-6f3b-4851-85ba-295e568663f6`
is printed `ace0cf403b6f514885ba295e568663f600000000`).

### Sessions connected (server mode: the only mode that sees them)

* Default options, stdin closed (n23): the platform prints
  `Ошибка исключительной блокировки информационной базы.` / `Активные сеансы и соединения:` / one
  line per session (`компьютер: <host>, сеанс начат: <time>, приложение: <application>`) and a
  three-way prompt (1 Отмена, 2 Повторить, 3 Обновить динамически, default `[2]`). With no input
  the prompt takes its default and asks again, without end: 95 372 rounds in eight minutes.
* Answer `1` (n24): `[WARN ] Обновление конфигурации базы данных отменено` on stderr, **exit 0**.
* `--dynamic=disable`, stdin closed (n26): two INFO lines, the same WARN, exit 0, in two seconds.
  This is the non-interactive refusal because sessions are connected. Nothing was applied.
* `--dynamic=disable --session-terminate=force` (n27): the session is ended and the stage applied.

## What is served

The drop-in serves the exclusive apply only. The default of the platform, `--dynamic=auto`, is
served because it *is* the exclusive apply whenever the exclusive lock can be taken (n10 against
n11), and the platform run against a database always takes it, since it sees no session.

| Platform | ibcmd-rs |
|---|---|
| default options | exclusive apply |
| `--dynamic=auto`, `disable`, `prompt` | exclusive apply (the platform, run against a database, never asks and never updates dynamically when nobody blocks the lock) |
| `--dynamic=force` | `Параметр `--dynamic=force` команды `infobase config apply` не поддерживается в этой версии ibcmd-rs (планируется в следующих)`, exit 1 (the result differs: overlay rows) |
| `--force`, `-F` | accepted; the own apply raises no warning that needs confirming |
| `--session-terminate=disable` (default) | with other sessions connected: the refusal below; else the apply |
| `--session-terminate=force`, `prompt` | with nobody connected: accepted, nothing to end. With sessions connected: `Параметр `--session-terminate=force` команды ... не поддерживается в этой версии ibcmd-rs`, the list of sessions, exit 1 (this version ends no session) |
| `--session-terminate-message` | accepted, unused |
| `--extension`, `-e` | not supported (exit 1), as for export and import |
| `--pid`, `--remote` (server mode) | not supported (exit 1), as for export and import |
| `--user`, `--password` | accepted, not needed |
| a stray argument | ignored, as the platform ignores it |

Own options (the platform has none of them): `--report=<file>` (the JSON report, also for a
refusal), `--platform=<version>`, `--settings`, `--db-pwd-env`, `--exclusivity=<sql|assumed>`
(`sql`, the default: look at the sessions SQL Server shows, needs `VIEW SERVER STATE`; `assumed`:
the operator answers for it). `--sqlcmd` is refused: the apply runs on the built-in client.

The platform of the database is `--platform`, else the settings, else 8.3.27 (a release stands
for the build the apply was measured on, `8.3.27.2214`). The storage layout is verified against
the database by the apply. 8.5 is refused (exit 1): the apply is measured on 8.3.27 only.

### Refusals

* **A stage that needs a restructuring**, or anything else the own apply does not do (a stage of
  the platform's own `config import`, removals, a dynamic overlay in `Params`, an interrupted
  operation, a new object of a kind the apply does not create): nothing is written, exit 1,
  stderr `[ERROR] требуется штатный config apply: <row or object>: <reason>; ...` (eight reasons
  at most, then `и ещё N`). The reasons are the gate's; today the apply's conservative gate,
  and `apply_check::Verdict::refusal()` (#338) once `ApplyCheckGate` is its default.
* **Other sessions connected** (SQL Server shows another user process on the database): exit -1,
  `Ошибка исключительной блокировки информационной базы.` / `Активные сеансы и соединения:` / one
  line per session as SQL Server knows it (`компьютер: <host>, приложение: <program>, соединение
  с СУБД: <id> (<login>, <status>)`; the 1C session's own start time is not known here) and the
  advice to close them, then the closing `[ERROR]` line. The platform's server mode cancels with
  exit 0 (n26); a script must not take "cancelled" for "applied", so this is a failure.

## What the drop-in prints

Success:

```
[INFO] Обновление конфигурации базы данных...
[INFO] Создано поколение конфигурации: <hex>
[INFO] Обновление конфигурации базы данных успешно завершено
```

Nothing staged (exit 0):

```
[INFO] Обновление конфигурации базы данных...
[INFO] Обновление конфигурации базы данных не требуется
```

Failure (exit -1): every line of the message as `[ERROR] ...`, then
`[ERROR] Обновление конфигурации базы данных завершено с ошибкой`. Refusal of a stage (exit 1):
`[ERROR] требуется штатный config apply: ...`, nothing else. A command line refused before
anything runs: one bare line on stderr, stdout empty (exit 2 for what the platform refuses too,
exit 1 for what this version does not serve).

| Exit | When |
|---|---|
| 0 | applied; nothing to apply |
| 2 | a command-line error, the platform's words |
| 1 | `требуется штатный config apply`; an option or platform this version does not serve; sessions connected together with `--session-terminate=force\|prompt` |
| -1 | a failed operation: no database, no connection, no password, other sessions connected, the apply's own failure (255 in a POSIX shell) |

`--report=<file>` receives `{"operation": "infobase config apply", "ok": ..., "nothing_to_apply": ...,
"apply": <the apply's report>}` on success and `{"operation", "ok": false, "error"}` otherwise.
The recovery artifact of the apply (the bytes it overwrote) goes to its default folder,
`%TEMP%\ibcmd-rs\config-apply-recovery\<db>-<token>`; the path is in the report.

## Differences from the platform

1. **No metadata check.** The platform checks the whole configuration first
   (`Проверка корректности метаданных...`, 5-38 s, exit 1 on errors). The own apply does not;
   the stage it takes is that of `infobase config import`, whose input was checked when it was
   staged, and its gate refuses whatever changes the structure. The drop-in prints no line for
   the steps it does not do.
2. **Sessions are looked for.** The platform run against a database is blind to them; this apply
   is not (see above). It is stricter than the platform on purpose: it moves rows in one
   transaction, but a working process holds the configuration in memory.
3. **Only stages of this repository's importers**; the stage of the platform's own `config import`
   (a `deleted` row, records in another format, a `{68}` Configuration row) is exit 1.
4. **LF line ends** (the platform writes CRLF), like the drop-in's export and import.
5. **Exit 1** is the platform's code for a failed metadata check and this version's code for what
   it does not serve. A script that tells them apart reads stderr.
6. The `Params` `.ui` rows, the help index, the extension CAS garbage collection and other
   derived state the platform rewrites on every apply are not written (see
   `own-apply.md`, "What it does not write").

## The seam

`src/dropin/apply.rs` has one call into the apply, `call_apply`, which is
`crate::mssql_config_apply::apply_staged_configuration(sql, &options)`. Everything above it is the
platform's command line (`src/dropin/parse.rs`: options, words, exit 2) and the mapping of the
options (`apply_options`, `profile_of`); everything below it is the mapping of what comes back
(`classify`, `structural_text`, `sessions_text`, `native_generation`). The gate is the apply's
default: its conservative gate today, `ApplyCheckGate` (a `StructuralGate` around
`apply_check::check_staged`) after track apply's swap, with no change here.

`classify` sorts an error by type where the apply gives one (`StructuralRefusal`) and by the words
of its message where it does not (`exclusive access is not established`, `... cannot be proven`,
`run the native `ibcmd infobase config ...``). Typed errors for the last two would replace the
text matching (open point 2).

## Tests and lab evidence

* Unit tests (`src/dropin/parse.rs`, `apply.rs`, `mod.rs`, `help.rs`; `cargo test --lib dropin`,
  34 tests): the platform's line of the lab scripts, the defaults, every word, the errors as
  measured, what is not served, the mapping of options and platform, the messages (sessions,
  structural reasons), the generation as the platform prints it, the report file.
* `tests/dropin_cli.rs` (9 tests): the process, without a database: every native command served
  or refused by name, the words and refusals before anything runs (exit codes and streams), the
  accepted spellings, the failure shape and the `--report`.
* Lab, [`evidence/dropin-apply/lab-runs.md`](evidence/dropin-apply/lab-runs.md): the drop-in on
  БСП 8.3.27 clones with the platform's command line. Nothing staged (0.9 s, "не требуется");
  a module change applied (exit 0), and the platform's own apply right after finds nothing
  to update; a descriptor change refused by the conservative gate (exit 1, nothing written);
  a connected cluster session refused with the list (exit -1), `--session-terminate=force`
  refused by name (exit 1), `--exclusivity=assumed` applied; a stage written by the platform's
  own import refused read-only (exit 1).
* The platform's help of the command: [`evidence/dropin-apply/native-help-apply.txt`](evidence/dropin-apply/native-help-apply.txt).

## Open points

1. **Exit code of "sessions connected"**: -1 here; the platform's server mode exits 0 after
   cancelling. Its direct mode has no such case.
2. **Typed refusals in `mssql_config_apply`**: a `NeedsNativeApply(reason)` and an
   `ExclusiveAccessRefused { sessions }` error next to `StructuralRefusal` would replace the
   text matching of `classify`.
3. **`VIEW SERVER STATE`**: without the right the apply cannot see sessions and refuses;
   `--exclusivity=assumed` is the way out. A database login of an ordinary installation is not
   a sysadmin.
4. **Sessions of the 1C cluster** hold SQL connections that a pooled working process keeps
   after the user left; the refusal then names the working process (`1CV8`), and stopping it
   is the operator's.
5. **8.5**: the apply is measured on 8.3.27 only; the drop-in refuses 8.5 with exit 1.
6. **Dynamic update (`--dynamic=force`)** and **ending sessions** are the natural next steps
   (0.5, "смена поколения").
