# Online, live and worker activation of one main-configuration change: what each mode writes, who sees it and when

Issue [#344](https://github.com/Untru/ibcmd-rs/issues/344) ("Довести direct-mssql-online-activation: свидетельства,
ревью, документация"), track "trace", milestone 0.5. OpenSpec changes `direct-mssql-online-activation` (tasks 1 and 7)
and `add-mssql-live-generation-switch`. Companion of `native-apply-trace.md` (#336: what the *native* apply writes).

Scope: `ibcmd-rs mssql-apply-source-change` and `mssql-activate-staged-main` with `--mode exclusive|online|live|worker`,
one existing module body (`CommonModules/<name>/Ext/Module.bsl`) of the **main** configuration, platform 1C:Enterprise
8.3.27.2214, Microsoft SQL Server 2025 (17.0.1135.8), the БСП 3.1.11 demo configuration with its four extensions (2 234
tables), two disposable clones of the lab corpus, 2026-09-29. The binary was built from this branch
(`feat/0.5-online-evidence`, base `3dd0b4db`). Extensions (online/exclusive only) are **not** covered here; their
evidence is `evidence/extension-live-validation.md` and `evidence/extension/protocol.md`.

Every number is **measured** unless the text says **hypothesis**. The workstation was shared with six other tracks during
every run: total CPU 98 % on average (minimum 65 %), free RAM between 3.3 and 26 GB
(`evidence/online-live-8327-20260929/timelines/*-load.tsv.gz`). Timings of client start-up and of every SQL step are
therefore upper bounds of what an idle machine gives; where the load matters for the *outcome* (section 4.3) it is said.
**WORKER was not run in this issue** (rule: it restarts a working process of the shared 8.3.27 cluster); section 4.4 is
the 2026-08-30 evidence, re-read, nothing new.

The evidence (journals of the observer sessions, timelines, SQL scripts as rendered, snapshot diffs, traces) is in
`openspec/changes/direct-mssql-online-activation/evidence/online-live-8327-20260929/` (index there); the scripts that
made it are `scripts/apply-trace/lab/online-live/`.

## 1. Summary

1. **ONLINE works as designed, and is invisible to running sessions.** One `mssql-apply-source-change --mode online`
   wrote 4 new `Config` rows (module owner and body under `<uuid>_dynupdate_<generation>`, `versions_dynupdate_<generation>`,
   the `DynamicallyUpdated` marker), 1 new `Params` row (`DynamicallyUpdated`) and replaced `root`/`version` with the same
   bytes; the ordinary module rows were not touched. The database stayed `ONLINE`/`MULTI_USER` in all 703 samples of the run.
   An already-open session (used **or not yet used** the module) kept the old code on both the client and the server side for
   as long as it was watched (34 minutes after the first generation, 11 minutes after the second); a session opened after
   the commit ran the new code on both sides at its first call. A 60-second server transaction that spanned the commit
   committed normally. Details and the matrix: section 4.1/4.2.
2. **LIVE did not complete once in five attempts** (section 4.3). Every attempt was refused by the readiness gate
   (`57234`, `src/mssql_main_activation.rs:607-609`) after the **first** recovery cycle, with the ordinary promotion already
   committed. What the sessions then see depends on the attempt: no error and a half-switched configuration (client and
   server disagree), or a modal DB error that needs "Перезапустить"/"Завершить" and loses the client's unsaved data; an
   in-flight server transaction is rolled back and its client gets an unrecoverable error. **A retry of the same command is a
   no-op** (`executed=false`, 2.0 s) and never runs the second cycle. The historical success
   (`add-mssql-live-generation-switch/evidence/live-stable-v7.md`, 2026-08-30) predates the gate and was one idle client.
3. **Two defects block the second apply on a database that has an online generation** (section 6): a re-apply of the *same*
   object fails after 250-300 s under load with "selected storage closure did not emit required source body"
   (`mssql_dump/mod.rs:3762-3765`, `44820-44834`); an apply of a *different* object stages and is then refused with "Params
   dynamic ordinary generation disagrees with versions" because the activation reads `Config` through a process-global
   dynamic overlay (`mssql_dump/dynamic_generation.rs:139-157`, `mssql.rs:957`). The workaround for the second one is to run
   `mssql-activate-staged-main` in a fresh process.
4. **`exclusive`, `live` and `worker` silently discard earlier online generations** (measured for `live`; the three modes
   share `render_ordinary_transition`). Their ordinary promotion replaces
   only the staged rows and deletes both markers; the `_dynupdate_` alias rows stay in `Config` as orphans, so what the
   earlier online changes published is gone from the effective configuration. Measured on a session (section 6, F-4): after a
   `live` promotion of an unrelated module, a new session sees the original text of the module changed by the first online
   generation and no longer finds the function that the second online generation added. Native `exclusive` apply *merges* the
   overlay (`native-apply-trace.md`, section 5); ours does not. **Mitigated fail-closed by #408:** these modes now
   refuse before any write when a marker or a `_dynupdate_` row exists (`mssql_main_activation.rs`, `online_history_refusal`,
   SQL code `57208`) and point to `mssql-config-apply` or the native apply; the fold itself is still to be built.
5. **`exclusive` cannot run against an infobase that has users** (F-3; fixed in 0.5, section 6.2): the tool's own RAS
   verification (`rac infobase info --infobase-user=...`, `mssql_platform_profile.rs:211-227`) opens two `1CV83 Server` SQL
   sessions that the gate (`57209`) then refused.
6. **The `live` preflight does not protect the commit** (F-9): with a broken log chain the promotion is committed and only
   then `BACKUP LOG` fails (`4214`); the error does not say that the new generation is already in the ordinary rows.
7. **Timing (ONLINE, measured):** 10.4 s in the tool (active export 2.0, classification 0.07, staging 1.0, activation
   2.7 s), 11.8 s wall; the activation transaction itself is 190.8 ms; activation alone in a fresh process 2.1 s.

## 2. The four modes on one page

| | `exclusive` | `online` | `live` | `worker` |
|---|---|---|---|---|
| ordinary `Config` rows of the staged names | replaced (`DELETE`+`INSERT`) | **kept**; `root`, `version` replaced (same bytes) | replaced | replaced |
| what is written instead | | `<name>_dynupdate_<gen>` alias rows, `versions_dynupdate_<gen>` | | |
| `Config`/`Params` `DynamicallyUpdated` markers | both deleted (exactly one each if present) | both written/extended | both deleted | both deleted |
| `_dynupdate_` alias rows of earlier generations | **left as orphans** | kept, listed in the marker | **left as orphans** | **left as orphans** |
| gate before writing | no other user session in the database (`57209`) | none | recovery model FULL/BULK_LOGGED, database ONLINE, tail-log file absent | dedicated worker process (exactly one process serves the infobase, no foreign infobase on it) |
| after the commit | nothing | nothing | two `SINGLE_USER` -> `BACKUP LOG ... NORECOVERY` -> `RESTORE ... WITH RECOVERY` -> `MULTI_USER` cycles | `rac process turn-off` of the old working process, wait for a replacement (10 s) |
| open sessions | none may exist | keep their generation (measured) | expected to switch without reconnecting (design); see 4.3 | expected to reattach through the new process (2026-08-30 evidence) |
| extensions | yes (CAS path) | yes (CAS path) | **rejected** | **rejected** |
| status in this issue | cannot run on a base with users (F-3) | measured | measured, 0/5 completed | not run |

All modes share the classifier and the staging: only an existing module body (`CommonModules/*/Ext/Module.bsl`) or an
existing common-form body is admitted (`mssql_apply.rs:591-619`), the change must not touch any other source path, the
staged rows are `<owner uuid>`, `<owner uuid>.0`, `root`, `version`, `versions` (5 rows; `versions` is 344 KB on the
БСП), and the whole publication is one `SERIALIZABLE` transaction under `sp_getapplock 'ibcmd-rs:main-activation'`.

## 3. What each mode writes

### 3.1 The transaction (all modes)

`render_main_activation_sql` (`mssql_main_activation.rs:316-419`), rendered script of the measured ONLINE run:
`evidence/online-live-8327-20260929/online/online-v1.sql`. In order:

1. `SET TRANSACTION ISOLATION LEVEL SERIALIZABLE; BEGIN TRANSACTION; sp_getapplock(... Exclusive, LockTimeout=0)` (`57200`
   when busy).
2. Two expected tables: the staged rows (`ExpectedStage`) and the **current** ordinary rows of the same names
   (`ExpectedActive`), each with `DataSize` and `SHA2_256`. `ConfigSave` must equal `ExpectedStage` exactly (`57201`); every
   `ExpectedActive` row must still be in `Config` with the same size and hash (`57202`); the two markers must be exactly as
   read (`57203`/`57204`: present with the same bytes, or absent).
3. The mode's transition (3.2 - 3.4), each row checked with `@@ROWCOUNT` (`57210`, `57212`, `57215`, `57216`).
4. `DELETE FROM dbo.ConfigSave` (`57220`), then the postconditions: the published rows read back with size and hash
   (`57222`), the markers as expected (`57223`-`57226`), `ConfigSave` empty (`57221`); `COMMIT`. Any error rolls back
   (`XACT_ABORT`, `CATCH`).

The staged rows come from `ConfigSave`, written earlier by the staging step in its own transaction (368.8 ms in the
measured run); a failure after that leaves the stage in `ConfigSave` (F-2 left exactly that; since 0.5 the checks that
need no staged row run before the stage, section 6.1).

### 3.2 `online` (`render_online_transition`, `mssql_main_activation.rs:448-478`)

Measured on a database without dynamic history (`online/online-v1.diff.md`, `online/online-v1.transactions.tsv`):

| `Config` | change |
|---|---|
| `313d9858-...b4_dynupdate_4832dd20-...` | **inserted** (165 B): the owner descriptor |
| `313d9858-...b4_dynupdate_4832dd20-....0` | **inserted** (704 B): the module body |
| `versions_dynupdate_4832dd20-...` | **inserted** (344 212 B): the staged `versions` |
| `DynamicallyUpdated` | **inserted** (45 B) `{1,1,4832dd20-...}` (with the UTF-8 BOM) |
| `root`, `version` | deleted and inserted again, **same content**, new `Creation`/`Modified` |
| `313d9858-...b4`, `.0`, `versions` | **untouched** (the previous generation stays) |

| `Params` | change |
|---|---|
| `DynamicallyUpdated` | **inserted** (82 B) `{0,2,848a0a59-...,4832dd20-...}` |

`<gen>` is the header GUID of the staged `versions` row (`4832dd20-...` here); `848a0a59-...` is the header GUID of the
ordinary `versions` (the "ordinary generation"). The alias name keeps the storage suffix last (`<uuid>_dynupdate_<gen>.0`).
After a second generation (measured, `online/online-v2b.diff.md`) the markers grow: `Config` `{1,2,<g1>,<g2>}` (82 B),
`Params` `{0,3,<ordinary>,<g1>,<g2>}` (119 B); every generation adds its aliases and never removes the earlier ones.

What native `--dynamic=force` writes *besides* this (16 `Params <guid>_dynupdate_<gen>.si` overlay rows, `dbStruFinal`,
`dynamicCommit`, `_ConfigChngR._MessageNo = NULL`, see `native-apply-trace.md` section 5) the direct mode does **not**
write; for a non-structural module change new sessions got the new code without them (sections 4.1, 4.2), and the
2026-08-29 native control kept the old generation in the open session the same way. Whether data exchange or other consumers
of the change register need those rows is **not measured**.

### 3.3 `exclusive`, `live`, `worker` (`render_ordinary_transition`, `mssql_main_activation.rs:421-446`)

For each staged name: `DELETE FROM Config WHERE FileName=... AND PartNo=...`, then `INSERT ... SELECT FROM ConfigSave`
(the module owner and body, `root`, `version`, `versions`); then `DELETE FROM Config WHERE FileName='DynamicallyUpdated'`
and the same on `Params`, each expecting exactly one row if the marker was read and none if not
(`render_marker_delete`, `mssql_main_activation.rs:525-537`). Measured for `live`
(`live/live-v1.diff.md`): `Config` -1 (`DynamicallyUpdated`), 3 rows updated (module owner, body, `versions`), `root`/`version`
same content with new dates; `Params` -1; `ConfigSave` 5 -> 0. `exclusive` adds before this, inside the transaction, the gate
`IF EXISTS (SELECT 1 FROM sys.dm_exec_sessions WHERE is_user_process=1 AND session_id<>@@SPID AND database_id=DB_ID()) THROW
57209` (`mssql_main_activation.rs:427`).

**Not written by any mode:** rows of `Params`/`Files` other than the marker, the change register, the schema tables, data
tables. The diffs of the runs show no other service table changed by the tool; the only other differences are the user-settings
registers of the observer sessions (`_CommonSettings`, `_UsersWorkHistory`) and the constant the transaction probe wrote.

### 3.4 `live` after the commit (`render_live_recovery`, `mssql_main_activation.rs:580-628`)

Preflight, before the transaction (rendered lines 3-7 of `live/live-v5.sql`): the database exists (`57230`), recovery model
FULL or BULK_LOGGED (`57231`), state `ONLINE` (`57232`), the tail-log file does not exist and is not a directory
(`57233`, through `sys.dm_os_file_exists`, i.e. on the SQL Server host).

After the `COMMIT`: `@LiveExpected1cConnections` = number of SQL sessions of program `1CV83 Server` bound to the database
(line 587); `CHECKPOINT`; then, from `master`: **cycle 1** `ALTER DATABASE SET SINGLE_USER WITH ROLLBACK IMMEDIATE` ->
`BACKUP LOG ... TO DISK=<tail> WITH NORECOVERY, INIT, COMPRESSION, CHECKSUM` -> `RESTORE DATABASE ... WITH RECOVERY` ->
`SET MULTI_USER`; the **readiness gate** (a loop of 100 ms until the number of `1CV83 Server` sessions is at least the
expected one and has stayed so for 500 ms, at most 4 000 ms after `MULTI_USER`; otherwise `THROW 57234`, lines 607-609);
**cycle 2** (`NOINIT`, appended to the same file). The catch block restores `MULTI_USER` when the database is not
`RESTORING` and otherwise throws `57250` with the mandatory `RESTORE DATABASE ... WITH RECOVERY` (lines 623-627).

### 3.5 `worker` (`mssql_worker_switch.rs`, `mssql.rs:1053-1065`)

Ordinary promotion as in 3.3 (no recovery cycle), then `rac process turn-off --cluster=... --process=<old>`, polling
`rac connection list` every 100 ms until the infobase is served by another process (timeout 10 s,
`mssql.rs:1025`, `mssql_apply.rs:111`). Before staging and again before the SQL commit exactly one process must serve the
infobase and no foreign infobase (`infobase != 0000...`) may be on it (`prepare_dedicated_worker`, `mssql_worker_switch.rs:37-55`,
`validate_process_is_dedicated`, `130-150`); the assignment is checked once more after the commit (`57-77`). Killing the
process was tried and removed (2026-08-30). Not run in this issue.

## 4. When do sessions see the change

The instrument: an external data processor (`observer/IbcmdRsObserver.epf`, sources in
`scripts/apply-trace/lab/online-live/observer/`) opened with `/Execute` in a thin client (`1cv8c.exe`) on the registered
clone. Once a second it reads `ОбсужденияСлужебныйКлиентСервер.ТипыВнешнихСистем().Telegram` **on the client** and through an
`&НаСервереБезКонтекста` function **on the server** (the module is a client-server module, so each side runs its own compiled
copy), and probes a second module, `РаботаСКлассификаторамиКлиентСервер.IbcmdRsMarkerB()`, which exists only in the second
online generation. Every line is `ms-since-0001-01-01 UTC | label | event | 1C session number | client value | server
value | details` (details: the value of the client-side "unsaved draft" form field and its modified flag). Modes: `poll`;
`lazy` (session opened but the module untouched until a flag file appears); `txn` (on a flag file, opens a server
transaction, writes a constant, holds it for 60 s). The change is the string stored under the key "Telegram" (original
value `Telegram`). The thin client polls every 1.02 s (median); it has a periodic stall of up to 9.4 s every ~30 s, so
gaps below 10 s in the journals are baseline (`analysis/poll-gaps.txt`).

"WARM" = a session opened before the change and kept open; "NEW" = opened after the commit; times are relative to the
commit of the activation transaction (from the Extended Events trace).

### 4.1 ONLINE, first generation (commit 17:08:35.690 UTC, clone `ibcmd_rs_05_online_a1`, no dynamic history)

| session | opened | what it saw | until |
|---|---|---|---|
| WARM-A (`poll`, session 1) | 17:00:47 | old on client and server, 1 320 polls in all, no error, no gap beyond baseline | its client process ended on its own at 17:28:56 (+20 min 20 s), not caused by the change (no crash event was found) |
| WARM-B (`txn`, session 4) | 17:01:37 | server transaction opened 20 s **before** the commit, held 60 s: `ЗАФИКСИРОВАНО за 60023 мс`; the write is in the database (`_ConstChngR9527` +3 rows) | after it: old code, until stopped at 17:29 |
| LAZY (`lazy`, session 5) | 17:01:44 | never touched the module before the change; armed 1.06 s after the commit, first read at **+1.43 s: old** on client and server | old code until stopped at 17:43:18 (**+34 min 43 s**, 1 639 polls) |
| NEW-1 (`poll`, session 8) | launched 17:08:36.4 (+0.7 s) | first poll at +29.5 s (start-up under load): `ibcmd-online-v1` on client and server | v1 to the end (+35 min) |
| NEW-2 (`poll`, session 9) | launched 17:11:14 | first poll 8 s later: v1 on both sides | v1 to the end |

So: **the generation is bound when the session is created, not when the module is first used** (LAZY). An idle or busy
session is not disturbed: the largest gap of every session in the ±30 s around the commit is inside the baseline stall
(7.4 s at -21.7 s for LAZY; 1.2 s after the commit for NEW-1). The `SQL` timeline (`timelines/online-timeline-sql.tsv.gz`,
100 ms) never left `ONLINE`/`MULTI_USER`. The ordinary module rows are unchanged, so `mssql-dump-config`-style readers that do
not know the overlay see the old text (F-1, F-15).

### 4.2 ONLINE, second generation (another module; commit 17:32:07.480 UTC)

The second generation could not be made for the same module (F-1) and, for another module, needed the two-step path (F-2);
both are fixed in 0.5 (section 6.1).
The change added an exported function `IbcmdRsMarkerB` to `РаботаСКлассификаторамиКлиентСервер`.

| session | opened | first module A / module B values |
|---|---|---|
| WARM-C (`poll`, session 10) | 17:29:28 (between the generations) | A = v1 (generation 1); B = "нет" (function absent); **B stayed absent for the 11 min 13 s watched after the commit** |
| NEW-3 (`poll`, session 16) | launched 17:32:08 (+0.6 s) | A = v1; B = `ibcmd-online-B-v2` on client **and** server |
| LAZY, NEW-1, NEW-2 | earlier | unchanged (A only) |

Markers after it: `Config` `{1,2,4832dd20-...,96eee589-...}`, `Params` `{0,3,848a0a59-...,4832dd20-...,96eee589-...}`.

### 4.3 LIVE (clone `ibcmd_rs_05_online_b1`, FULL recovery, base backup taken; native dynamic history present)

Five attempts, all ended by the gate `57234` ("1C SQL connections did not recover before the live activation deadline;
database is online and the second recovery was not started"). The promotion was committed each time; the tail file holds
cycle 1 only. Table (times UTC; window = `SINGLE_USER` until `MULTI_USER`, from `timelines/*-timeline-sql.tsv.gz`):

| # | run | sessions | window | first 1C SQL connection back | what the sessions saw |
|---|---|---|---|---|---|
| 1 | `live-v1`, capture kit | 3 WARM (poll, txn, lazy), then NEW-1, NEW-2 | 9.72 s (17:56:07.456 - 17:56:17.180); `RESTORING` 1.05 s | none within 5 s | all three WARM clients: modal **"Ошибка СУБД ... пользователя sa ... native=18456"** (poll, lazy) and **"Невосстановимая ошибка ... POST /e1cib/logForm"** (txn client; the transaction was rolled back, no constant written); NEW-1/NEW-2: client `ibcmd-live-v1` (new), server `Telegram` (old), for 9 minutes |
| 2 | `live-v2`, capture kit | 1 WARM (poll, session 1), NEW-1 | 0.87 s (18:08:22.356 - 23.222) | +3.5 s | WARM: **no error, same session, draft kept**; from +7.5 s server = v2, **client still v1**; NEW (opened +12.9 s): client v1, server v2 |
| 3 | `live-v3`, lean | 1 WARM | 2.41 s (18:13:39.054 - 41.460) | none in 25 s | WARM stopped polling 14 s earlier (before the window) and shows the DB error dialog |
| 4 | `h1-live-c1` (clone `a1`) | 1 WARM, NEW | 4.95 s (18:21:14.245 - 19.195) | n/a | unchanged views for 65 s (see F-4) |
| 5 | `live-v5`, lean (only the SQL sampler) | 1 WARM | 2.09 s (18:25:00.272 - 02.356) | +10.2 s | WARM: no error, same session, **no change of view** (client v1, server v3; v5 expected) |

Manual completion of the missing second cycle (the tool cannot do it, see F-5): after attempt 1 (window 2.05 s at 18:03:05)
`BACKUP LOG` failed with `924` ("database is already open and can only have one user at a time", i.e. a 1C connection took
the single-user slot between `ALTER` and `BACKUP`); after attempt 2 (window 3.21 s at 18:10:37.6) it succeeded, and **both**
open sessions then showed the DB error dialog. Pressing the default button ("Перезапустить") of such a dialog makes the client
process start **another client process** (`1cv8c.exe MNG_ENTERPRISE`) and exit: a new session, no external processing, the
draft is lost.

What can be said from these five runs (**measured**): a session survives a cycle when it makes no call that needs the
database during the window (attempts 2 and 5: 0.9 s and 2.1 s); it gets a modal DB error when it does (attempts 1, 3 and the
3.2 s manual cycle); the platform does not retry silently for longer than about a second or two. The 1C SQL connections
came back 3.5 s, 10.2 s and never after the first cycle, against a gate that allows 4 s, so under this load the gate
aborts. **Hypothesis:** with an idle machine and one polling client the gate passes (as on 2026-08-30); the constants
4 000 ms/500 ms and the expected count (which includes the two sessions that the tool's own RAS verification opens, F-3) are
not robust against a busy machine or several sessions.

After an abort, the state is a *mixed generation*: attempt 1 left new sessions with new client code and old server code until
every session had ended (a fresh session at 18:06:27 saw both new); attempt 2 the other way round (client old, server
new). Which side lags was not the same in the two attempts (**not explained**).

### 4.4 WORKER (2026-08-30 evidence, not repeated)

`evidence`: `add-mssql-live-generation-switch/evidence/worker-watch-20260830.md`. Same client PID and 1C session UUID; the open
client saw the new client and server code 3.3 s after an already staged promotion (promotion + signal 323 ms); the complete
save path (export 400-734 ms, classification 16-32 ms, staging 1.2-1.7 s, activation 5.8-7.3 s of which the worker hand-off
4.3-6.2 s) took 7.5-9.0 s; a shared process was refused in 108 ms with `ConfigSave` untouched. Under the load of this issue
each `rac` call took 1.7 s (`infobase info`) to 5.4 s (`session list`), against a 100 ms poll and a 10 s handoff timeout
(`mssql_worker_switch.rs:91-116`): on a busy machine the timeout is tight (**hypothesis**, not run).

## 5. Timing

ONLINE, clone without history, report of the tool (`online/online-v1.json`): active export 2 021 ms, classification 66 ms,
staging 1 049 ms, activation 2 737 ms (includes the second RAS verification and the three reads of the snapshot), total 10 419 ms;
wall of the process 11 838 ms; Extended Events: staging transaction 368.8 ms, activation transaction 190.8 ms
(`online/online-v1.transactions.tsv`). Activation alone in a fresh process (`online/online-v2b.json`): 2 142 ms wall. Dry-run
of a no-op on a database that has dynamic history but not for the object: 10.6 s wall, export 2.0 s. The same export for an
object that *has* an alias: 250-300 s (F-1/F-15). LIVE: the tool ran 13.8-27.8 s before the gate aborted; the window of
cycle 1 was 0.87-9.72 s (section 4.3). `rac`: `infobase info --infobase-user` 1.7 s, `session list` 5.4 s (load).

## 6. Safety: what fails closed and what does not

Fails closed (all measured or read from the SQL): stale stage or drifted `Config`/marker (`57201`-`57204`), busy lock
(`57200`), alias that already exists (`57211`), any row count mismatch, unsupported target/extension mode, missing
`--allow-non-lab`/`--sqlcmd-trust-cert`, platform-profile mismatch (RAS agent build, registered infobase, live column
layout), unreadable markers ("Config and Params dynamic markers must both be present or both absent"), a stage that reuses
the active generation. Recovery artifacts are written **before** the SQL runs (`mssql.rs:1031-1051`).

Review findings (2026-09-29, "report, do not fix"; severity = effect on a user of the mode; **file:line** are of this branch):

| id | severity | finding | where | evidence |
|---|---|---|---|---|
| F-4 | **critical** | An ordinary promotion (`exclusive`, `live`, `worker`) replaces only the staged rows and deletes both markers. The `_dynupdate_` rows of the earlier online generations stay as orphans, so **what earlier online generations published is silently gone**. Native `exclusive` merges the overlay. **Mitigation (#408): the three modes refuse before any write when the database holds markers or `_dynupdate_` rows; the fold is not built.** Measured (before the mitigation): after a `live` promotion of an unrelated module on a base with two online generations and once every old session had ended, a new session saw the original `Telegram` (generation 1 gone) and no `IbcmdRsMarkerB` (generation 2 gone); on the corpus base the storage shows 5 alias rows, no marker, ordinary bodies of 1 522/2 244 B against 1 533/2 262 B in the aliases. | `mssql_main_activation.rs:421-446`, `525-537` | `analysis/h1-sessions.txt`, `live/h1-storage-b1.txt` |
| F-1 | high | **Fixed in 0.5 (#409), section 6.1.** The second apply of an object that already has an online alias fails ("selected storage closure did not emit required source body") after 250-300 s. By the code (the symptom is measured, the mechanism was not tested separately): the headers are read for the plain names, then the overlay hides the plain rows and the aliases were never selected, so nothing remains. The dev loop "edit, apply online, edit, apply online" cannot be repeated. | `mssql_dump/mod.rs:3762-3765`, `44820-44834`; `mssql_apply.rs:1020-1030` | `online/online-v2-single.err`, `online-v2-dry.err` |
| F-2 | high | **Fixed in 0.5 (#409), section 6.1.** The dynamic overlay is process-global (`STORAGE_GENERATION_OVERLAYS`), installed by the active export and cleared only by the next `dump_config`. `activate_staged_main`, run in the same process, reads the ordinary `Config` rows **through the overlay** (`qualified_storage_table`), takes the alias generation as the ordinary one, and is refused with "Params dynamic ordinary generation disagrees with versions" after staging, leaving a dirty `ConfigSave` (the same staged rows on the same database pass in a fresh process: `mssql-activate-staged-main --dry-run` on the corpus clone, and the real `online` run of generation 2). Any real apply on a base that has markers (native leftover of the corpus, or an earlier online generation) hits it. Workaround: `mssql-activate-staged-main` in a fresh process (measured). | `mssql_dump/dynamic_generation.rs:139-157`, `mssql_dump/mod.rs:2129`, `44845-44851`, `1008-1027`; `mssql.rs:957-976`; refusal `mssql_main_activation.rs:826-829` | `online/online-v2b-single.err`, `online/baseline-exclusive.err`, `live/live-v1-single.err` |
| F-3 | high | **Fixed in 0.5 (#409), section 6.2.** `exclusive` is refused on every registered infobase that has users: the RAS verification with `--infobase-user` makes the cluster open two `1CV83 Server` SQL sessions (the RAS connection that holds them stayed for the whole observation, 56+ minutes; `rac connection disconnect` with the infobase user just creates another RAS connection, without it is refused) and the gate counts them. | `mssql_platform_profile.rs:211-227`; `mssql_main_activation.rs:427` | `online/baseline-act2.err`, `.sql` |
| F-5 | high | `live` aborts half-way when the 1C connections do not come back within 4 s (0 of 5 completed here). The promotion and markers are already committed, sessions may be left in a mixed generation or in a modal DB error, and **no command can run the second cycle**: a retry is a no-op (`executed=false`, 2.0 s). The manual second cycle is racy (`924`). The error text does not tell the operator any of this. | `mssql_main_activation.rs:587`, `607-609`; no-op `mssql_apply.rs:246-274`; `mssql.rs:1053-1065` | section 4.3, `live/live-v*.err`, `live/live-v2-retry.json` |
| F-6 | low | The platform-profile verification (two `rac` calls, RAS authentication, SQL schema probe) runs **twice** in one high-level apply and also for `--dry-run` and before the `--allow-non-lab` check; each `rac infobase info --infobase-user` opens a cluster connection and SQL sessions on the infobase (1.7 s each under load). | `mssql_apply.rs:70-95`; `mssql.rs:910-931` | `online/online-v1.trace-summary.md` (statements of `1CV83 Server` sessions before the staging) |
| F-9 | medium-high | The `live` preflight checks the recovery model, the state and the tail-file name, not the log chain or the destination directory. With `FULL` but no full backup the transaction commits and `BACKUP LOG` then fails with `4214`; the database is online with the new generation in the ordinary rows and no session switch. The design says a failure before a successful tail backup "leaves the database online and returns the bounded row recovery artifact"; the artifact path is not in the error. | `mssql_main_activation.rs:342-348`, `402-406` | `live/live-v6-nochain.err`, `live/break-log-chain.sql` |
| F-10 | medium | `live` kills every connection of the database with `ROLLBACK IMMEDIATE`: no session preflight, no warning. In-flight server transactions are rolled back and their clients get an unrecoverable error; a client that touches the database in the window shows a modal error and loses unsaved data on restart. It is the documented design, but the command has no `--force`-style acknowledgement beyond `--allow-non-lab`. | `mssql_main_activation.rs:595-599`, `610-614` | section 4.3 |
| F-15 | medium | With an active generation every read of `Config` is a derived table over the **whole** table (`CASE` over the file name), so a bounded read costs a full scan and a memory grant: 4 762 logical reads and 34.5 s elapsed under load against 3 reads and 1 ms for the plain read (`RESOURCE_SEMAPHORE` wait); the bounded export of one object took 250-300 s instead of 2 s. | `mssql_dump/dynamic_generation.rs:167-190` | `online/overlay-query.sql`, `online/online-v2-dry.meta.txt` |
| F-8 | low-medium | The recovery artifact does not keep `Creation`/`Modified`/`Attributes` of the overwritten rows (empty strings, `0`) and stores bytes as a JSON array of numbers; it is written non-atomically with a read-then-write check, so a crash leaves a truncated file that a repeat refuses to overwrite. For `online` it lists the ordinary rows that were *not* overwritten and has no script that removes the aliases and markers. | `mssql_dump/mod.rs:1016-1021`; `mssql.rs:1123-1136` | `online/online-v1.recovery-summary.txt` |
| F-7 | low | Rows the tool writes carry `2026-...` timestamps (the staging copy, the markers via `SYSUTCDATETIME()`), native rows `4026-...` (year offset 2000). Sessions accepted the rows, so it is not shown to matter. | `mssql_main_activation.rs:515-523`; staging | `online/online-v1.diff.md` |
| F-11 | low | Worker: "dedicated" is decided from `rac connection list` only; an idle infobase loaded by the process but without a connection is invisible. `rac process turn-off` inherits stdout/stderr, so its output can enter the JSON stream. 10 s timeout, 100 ms poll of a call that takes 1.7-5.4 s under load. | `mssql_worker_switch.rs:130-150`, `79-88`, `91-116` | code, section 4.4 |
| F-12 | low | `--infobase-pwd` goes to `rac` on the command line (visible to other local users); reads always trust the server certificate (`trust_server_certificate: true`), so `--sqlcmd-trust-cert` is a ceremony for the main path. | `mssql_platform_profile.rs:217-222`; `mssql_apply.rs:664`, `1140`; `mssql.rs:7375`; `mssql_dump/mod.rs:2152` | code |
| F-13 | low | `tables_changed` is built by a branch with two identical arms and does not list the alias rows or the markers of an online run. | `mssql_apply.rs:450-475` | code |
| F-14 | low | Two path comparisons disagree (ASCII case fold against Unicode lower-case). The watch loop hashes the whole closure every 100 ms. | `mssql_source_change.rs:994-1005`; `mssql_apply.rs:558-566` | code |
| F-16 | info | `overlay_active_dynamic_module` (the older alias reader) is unreachable for an aliased object since the export overlay was added: the export fails first (F-1). Two mechanisms for one job. | `mssql_apply.rs:1116-1215` | code |
| F-17 | info | OpenSpec `add-mssql-live-generation-switch` task 7 (readiness gate) is checked but has no evidence with the gate; the recorded live run is the fixed five-second delay it replaced. | `add-mssql-live-generation-switch/tasks.md` | section 4.3 |

### 6.1 Fixed in 0.5 (#409): F-2 and F-1

Both were reproduced first, by tests on the unfixed code (an export leaves its overlay behind; another database reads the
overlay; a selected aliased object lists no row) and on a corpus clone with the unfixed binary, then fixed and proven on the
same clone with the fixed one (`ibcmd_rs_05_ui_rf_a`, restored from the БСП 8.3.27 corpus backup, which already holds a native
online generation of two objects: the common form `_ДемоПримечание` and the common module `_ДемоЗаметки`).

**F-2, what was wrong and what is now.** `STORAGE_GENERATION_OVERLAYS` was keyed by table name only and cleared only by the
next `dump_config`. The activation that follows the active export in the same process read its snapshot (`fetch_main_activation_rows`)
through it, took the alias generation for the ordinary one and was refused after the stage. Now:

- the overlay is kept per (database, table) and lives in a `StorageViewScope`; `dump_config` and `export_staged_state` open
  one, so the overlay ends with the export, and another database or a step that opens its own scope never reads it;
- `fetch_main_activation_rows` (the activation's reads: staged rows, the rows they replace, both markers) always reads the rows
  as stored, whatever scope is open: the transaction compares them with `dbo.Config`, not with a view;
- the checks that need no staged row are made **before** anything is staged, dry runs included: an ordinary mode (`exclusive`,
  `live`, `worker`) on a database that holds markers (the #408 refusal), markers that are not one pair or do not agree with the
  ordinary `versions` row, and the `--tail-log-output` argument (required for a real `live` run, absolute, no control
  characters; refused for the other modes). The plan of the activation makes the same checks through the same function
  (`check_publication_state`), so the two cannot disagree; a test runs both on the same input. The state preflight runs after the
  no-op decision (a no-op is not refused) and before the compile tree and the stage; the tail-log check runs before the first `rac` call;
- an `online` apply on a base with markers is now one step in one process: no `mssql-activate-staged-main` in a fresh process.

`overlay_active_dynamic_module` (F-16) no longer rewrites the exported file from the alias row of the selected body: the export's
view already publishes that row, and the function would have come back to life now that the view does not outlive the export.
It is `active_dynamic_generation` and only names the generation the change applies to.

**F-1, what was wrong and what is now.** A run that selects names lists the headers of the *stored* rows with those names. For an
object with an alias that is the plain row, which the overlay hides, and none of the alias rows, so nothing was left to export
("selected storage closure did not emit required source body"). The published headers of a selected run now come from the
inventory of the table (already read to resolve the overlay), filtered by the published names.

Measured on the clone (this machine, no other load on the database; times of the tool's own report, wall of the process):

| step | unfixed binary | fixed binary |
|---|---|---|
| `online`, common module `ОбсужденияСлужебныйКлиентСервер` (no alias), base with the native marker | real run: refused after the stage, `Params dynamic ordinary generation disagrees with versions`, wall 2.4 s, `ConfigSave` holds 5 rows | real run: applied, generation 2 written, wall 2.8 s (export 1.1 s, stage 0.2 s, activation 0.7 s), `ConfigSave` empty |
| `exclusive`, same change, real run | refused after the stage (the #408 refusal, raised by the plan), wall 4.4 s, `ConfigSave` holds 5 rows | refused **before** the stage, wall 1.5 s; row counts and checksums of `Config`, `ConfigSave` and `Params` identical to the state before |
| `worker`, `live`, `exclusive`, dry run | not run | refused, wall 1.1-1.7 s, nothing written |
| `online`, dry run, module with an alias | fails, `did not emit required source body`, wall 40.6 s | passes, wall 4.2 s (export 3.0 s) |
| `online`, real run, module with an alias | (fails as above) | generation 3: wall 2.4 s (export 0.7 s, activation 0.9 s); the alias body holds the new text |
| the same module again with another edit | (fails) | generation 4: wall 1.9 s; history `{1,4,...}` / `{0,5,...}` chains the four generations |
| the same source once more | (fails) | `no_op`, wall 1.1 s: the active export of the module is the text just applied |

The header rows of the new generations are byte-identical (inflated) to the native alias header of the same object. The 250-300 s
of F-1 were measured under the load of the other runs; the unfixed binary needed 40.6 s here, on an idle database. F-15 is
untouched: a bounded read of an object with an alias is still a scan of the table under a derived table (the export took 3.0 s
cold and 0.7 s warm here), and would take longer under load.

**Found while proving F-1, not changed here.** The stage reads its base rows and the `versions` blob with its own SQL on the
ordinary `Config` rows (`fetch_config_blob`, `fetch_config_blobs_for_files`), never through the overlay. On a base with earlier
generations the new `versions_dynupdate_<g>` is therefore built on the ordinary `versions`, not on the active generation's:
in the clone, `versions_dynupdate_<native g1>` lists the form `a627e390-...` at `226957a7-...` and its body at `9fab40a2-...`,
while the three generations written by this tool list `9947107e-...` and `f2422a32-...` (the ordinary values). Bodies are read from
the aliases whatever `versions` says, and a session has not been opened on such a state, so the effect on a session (a reload of
the objects whose stamp went back) is **unverified**.

### 6.2 Fixed in 0.5 (#409): F-3

**What was wrong, measured.** `rac infobase info` without the infobase user is refused ("Недостаточно прав пользователя на
информационную базу"); with it the cluster's worker process loads the infobase and keeps **two idle `1CV83 Server` SQL sessions**
(`sleeping`, `open_transaction_count = 0`, `host_process_id` = the pid `rac process list` gives for the worker) for as long as its
RAS connection lives. `rac connection list` shows that connection: application `RAS`, session number 0. The gate `57209`
(`57212` for an extension) counts every session, so `exclusive` was refused on every infobase that has users. On the marker-free
clone `ibcmd_rs_05_ui_rf_b` the unfixed binary stops with `57209` after the stage, with exactly those two sessions on the database.

**What is now.**

- `read_infobase_clients` asks the cluster (`rac connection list`, `rac session list`, `rac process list`, no infobase login,
  which would open another RAS connection). Every connection whose application is not `RAS` (or has none) and every session is a
  client; a worker process that carries RAS connections of the infobase and no client is a RAS-only process (host, pid).
- An `exclusive` apply is refused **before the stage** when the cluster lists a client, and the activation asks again before it
  renders its script (a dry run included).
- `exclusive_session_gate` builds the one statement both gates use. It leaves out the sessions that are `1CV83 Server`, `sleeping`,
  without an open transaction, on a RAS-only process (host and pid match). A session of another program, another process, a running
  one or one that holds a transaction still refuses. With no process named the text is the old one, and the dry-run report of the
  activation lists the processes it left out (`own_ras_processes`).
- The extension activation uses the same helper for its gate (`57212`). Unit tests cover it; it was not run on an extension in the lab.

**What it does not cover.** A user who signs in through the same worker process in the seconds between the cluster query and the
transaction, while the sessions stay idle, is not seen: the exemption is by process, not by session. The window is the time from the
last `rac` call to the transaction (a few seconds in the runs below); the cluster's own list and the running/transaction condition
narrow it, an infobase lock (`sessions-deny`) would close it and is not done here.

Measured on the marker-free clone (the corpus backup with its markers and aliases deleted, so that the #408 refusal does not come
first; the worker process 22608 held the RAS connection):

| run | unfixed binary | fixed binary |
|---|---|---|
| `exclusive`, no users | refused `57209` after the stage, 6.9 s; `Config` unchanged, `ConfigSave` 5 rows; two idle `1CV83 Server` sessions of pid 22608 | applied, 13.4 s (activation 4.1 s); the report names `DESKTOP-SMI5N4O` / 22608 as left out; `ConfigSave` empty |
| the same, with an idle non-1C SQL session on the database | - | refused `57209` after the stage, 3.1 s; `Config` unchanged (the gate still counts it) |
| the same, with a thin client (`1cv8c`) connected | - | refused before the stage on the cluster's word, 2.0 s, five connections and sessions listed; `Config` and `ConfigSave` unchanged; the dry run is refused as well; `online` (dry run) passes |
| the client killed, its session left in the cluster | - | refused the same way until the session was ended with `rac session terminate` (the cluster keeps the session of a killed client) |
| after that | - | applied, 5.7 s, three idle sessions of the worker left out |

A connected client also writes `Params` on its own (a row went away while the tool was refusing before any write), so a fingerprint
of the whole database is only comparable without one.

### 6.3 The export path after the scope guard (#409)

`StorageViewScope` wraps `dump_config` and `export_staged_state`, the export every user runs and the guard of the import. Checked
against the native platform, with the fixed binary (merged with feat/0.4 5e146f00):

- `ibcmd_rs_04_rcheck_bsp_a` (active generations): the drop-in `infobase config export` (37 s) against the native `ibcmd infobase
  config export` (40 s) of the same database: **12 198 of 12 198 files identical**.
- `ibcmd_rs_05_ui_rf_a` after the three generations this tool wrote (F-1, F-2): ours 55 s, native 20 s, **12 198 of 12 198 identical**;
  the native platform reads the text of the last generation (both markers are in `_ДемоЗаметки`).
- БСП 8.3.27 from rows (`mssql-dump-config --rows-dir`), the binary before (feat/0.4 e672908c) against the binary after:
  **12 199 of 12 199 identical**.
- Tests: the whole `cargo test --locked -p ibcmd-rs --no-default-features` (lib and the integration tests, 46 binaries): 3 643 passed, 0 failed, 12 ignored; the lib alone 3 435, the import guard's `mssql::stage_guard` tests among them.

## 7. Recovery

**ONLINE** (nothing is deleted by the tool). To go back to the state before generation N: in one transaction delete from
`Config` the rows `<uuid>_dynupdate_<gN>` and `<uuid>_dynupdate_<gN>.0` of the module named in the run's `recovery.json`
(`staged_rows`, the two GUID rows) and `versions_dynupdate_<gN>`, then set the two markers back to the previous payload
(`prior_config_dynamically_updated`, `prior_params_dynamically_updated` in the artifact; delete them when the artifact says
`null`). New sessions then load the previous generation; open ones keep whatever they loaded. There is no script for it
(F-8); the transaction has to be written by hand and has not been run.

**A refused apply leaves `ConfigSave` staged** (F-3, any failure after staging; the refusals that need no staged row
- a marker base with an ordinary mode, disagreeing markers, the tail-log argument - come before the stage since 0.5, F-2).
Run `mssql-activate-staged-main` with the same mode (for exclusive see F-3), or empty the stage by staging the next change (the
staging replaces `ConfigSave`).

**LIVE after `57234`** (database `ONLINE`, promotion committed, tail file holds cycle 1): the sessions are in a mixed
state until every old session ends, or until the second cycle is run by hand, from `master`, the statements of
`live/manual-second-cycle.sql` (same tail file, `NOINIT`); it can fail with `924` (repeat) and shows the sessions a
DB error dialog. **After `57250`** (database `RESTORING`): `RESTORE DATABASE [<db>] WITH RECOVERY;`. The `.trn` is part of the
log chain: keep it (62 MB for the first cycle after a full backup in the measured runs, 3-6 MB for later ones).

**Split or stale generations after any aborted live/worker:** end all sessions of the infobase (a fresh session then sees
one generation, measured), or restart the working process (worker: the tool's own signal).

## 8. Limits

- One existing module body or common-form body of the **main** configuration per call; everything else is rejected before
  any write (`mssql_apply.rs:591-619`); extensions: online/exclusive only.
- Row limits: 128 staged rows, 16 MiB per row, 32 MiB per plan, 16 MiB inflated `versions` (`mssql_main_activation.rs:15-18`);
  `PartNo` must be 0 (multi-part rows are refused).
- A base with a `DynamicallyUpdated` marker: `online` applies in one step, also to an object that already has an alias (F-1,
  F-2, fixed in 0.5); `exclusive`/`live`/`worker` on it are refused before the stage, because they would discard the earlier
  generations (F-4, #408).
- `exclusive` needs an infobase on which the cluster lists no client connection or session; the idle SQL sessions of the tool's
  own RAS verification are left out of its gate (F-3, fixed in 0.5). A session of another program, or a running one, still refuses.
- `live` needs FULL/BULK_LOGGED recovery, a full backup taken after that, a tail-log path writable by the SQL Server account,
  and a machine on which the 1C SQL connections return within 4 s (F-5, F-9); it interrupts every database connection
  (F-10).
- `worker` needs exactly one dedicated `rphost` for the infobase, RAS and `rac` on the same host.
- The observed BSP client shows a modal message after a lost database connection; nothing here changes that.

## 9. Evidence and how to repeat

Index and file list: `openspec/changes/direct-mssql-online-activation/evidence/online-live-8327-20260929/README.md`. The lab kit,
`scripts/apply-trace/lab/online-live/` (paths are the lab's `F:\ibcmd\lab\05\online`; `lab-env.ps1` holds them):
`apply.ps1`/`act.ps1` run one command with wall-clock bookends, `cap-apply.ps1` wraps them in the capture kit (snapshots,
trace, diff) with hooks that arm the transaction observer before the command and start NEW/lazy observers after it,
`obs-start.ps1`/`obs-stop.ps1` start and stop observer clients, `sql-timeline.ps1` (database state from `master`, never
connecting into the database), `rac-timeline.ps1`, `load-sampler.ps1`, `timelines.ps1`, `gen-state.ps1` (generations and
markers), `sessions-clean.ps1`, `make_versions.py` (source trees), `analyze_obs.py` (segments, gaps, errors relative to a
commit time), `build-observer.ps1` (Designer batch on a throw-away file infobase). The first client start on a restored
БСП clone shows "Информационная база была перемещена или восстановлена из резервной копии" and blocks the session; on `a1`
the dialog was simply not shown again after the first client was closed, on `b1` an Enter posted to it (`uia-click.ps1`)
closed it and the windows kept the "[КОПИЯ]" title. `winshot.ps1` and `uia-click.ps1` touch only the windows of the lab's
own client process.

Before repeating: register the clone with `register-ib.ps1` (8.3.27), for `live` make it FULL and take a full backup, and
unregister it afterwards. Do not run `worker`.
