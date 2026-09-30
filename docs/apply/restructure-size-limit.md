# S1-J: a limit on the tables the own restructuring rebuilds (#406)

Checkpoint 1 of #406, track ui, 2026-09-30: the measurement plan and the place of the check, before code. Part of
[#391](https://github.com/Untru/ibcmd-rs/issues/391); the design it belongs to is
`docs/apply/restructuring.md`, section 12 (S1-J is row 12.7). Nothing here is measured yet unless it says so;
what is quoted is quoted from that document.

## 1. The question

The own restructuring rebuilds an object the platform's way: for every changed table (the main table and all its
sub-tables) it creates `<table>NG`, copies the rows with `INSERT INTO <t>NG WITH(TABLOCK) (...) SELECT ... FROM <t>
WITH(NOLOCK)`, creates the indexes, drops the old table and renames (`plan.rs`, `Plan::statements`). The whole
structure phase runs inside the apply's one SERIALIZABLE transaction (12.4.1), so that a failure takes everything
back and no `Status 200/400/500` is ever visible. That is the design, and it has one cost the design already
names (9.3, 12.9 item 4): the log of the whole copy is held until `COMMIT`, and a transaction also reserves log
for its own rollback. For a catalog of 14 or 716 rows it is kilobytes; for a table of 100 GB it is not
acceptable. The native apply commits in steps; chunking the copy is 0.5.

S1-J puts a limit on the rows and bytes of what is rebuilt and refuses above it, with a message that sends the
user to the native `ibcmd infobase config apply`. The limit has to be justified by measurement, not chosen.

Known now (from the traces of 12.8, БСП 8.3.27, small tables): a stage of 11 tables (716 rows the biggest) copies in
0.4 s and creates its 40 indexes in 1.9 s; the apply's transaction, all of it, is 21 s. Not known: the log per
byte of table, the time per gigabyte, what the recovery model changes, what the indexes and the LOB columns
cost, and how much log a rollback needs. Those are the numbers this plan measures.

## 2. Where the check plugs in

The check needs two things: which tables will be rebuilt, and how big they are. The first is known only after
`plan()` (`Plan::tables()`; a new object's table has no rows). The second is database state, not a fact of the
staged image.

| place | what it means | for | against |
|---|---|---|---|
| **in `plan()`** (`plan.rs`, with `Inputs` and `PlanOptions`) | every caller is covered at once | one place | `Inputs` gets a field and `plan_object` a check: the hot file of S1-D/E (track ddl), the constructors of `Inputs` in the tests of `tests_plan.rs` and `tests_corpus.rs` change, `plan()` becomes impure or `Inputs` grows a database-side field |
| **in `decide()`** (`s1.rs`, the pure decision) | the refusal is one more blocker of the gate | pure, unit tests need no database | the signature of `decide` and all its call sites in `tests_s1.rs` change (ddl's file); sizes must be read before it, before the plan says which tables |
| **in the gate's impure wrapper, `S1Gate::check`, and in the standalone command** (proposed) | after `decide` returned a phase, read the sizes of `phase.tables` and refuse above the limit | additive: a new file, a builder on `S1Gate`, a few lines at two call sites; the check is one pure function that a test feeds with sizes; the gate already reads the database here (`check_staged`, `read_inputs`); the refusal takes the apply's existing path (`verdict.block`, `StructuralRefusal`) | the standalone `mssql-restructure` has to call it too (`command.rs:226`, after `make_plan`) |
| inside the transaction script (`script.rs`) | an `IF ... THROW` on the size just before the copy | closes the window between the gate's read and the transaction | changes the script that 12.6 check 12 and the twin comparisons pin; the apply holds an exclusive lock anyway (12.4.1), so the window is only the gate's seconds; **not proposed** for the first version |

**Proposal: the gate side, in a new file `src/restructure/size_guard.rs`.**

- `SizeLimit { max_rows: u64, max_bytes: u64 }` with the measured defaults; `TableSize { rows, data_bytes,
  index_bytes }`.
- `read_sizes(source: &mut dyn RowSource, tables: &[String]) -> Result<BTreeMap<String, TableSize>>`: one query on
  `sys.dm_db_partition_stats` (rows and pages of the heap or clustered index, and of the other indexes; LOB and
  row-overflow pages included) for the names of the plan's tables. `RowSource` is the existing reader trait, so
  the same function serves the standalone command (`TdsConnection`) and the gate (`ClientSource`).
- `evaluate(sizes, limit) -> Result<(), SizeRefusal>`: pure. The limit is on the **sum over the stage**, because
  one transaction rebuilds every table of the stage and its log is the sum; the refusal names the largest table,
  the totals and the limit, and ends with what to do: the native apply. A table missing from the result (a new
  object) counts as 0.
- Hooks (the only edits to shared files): `S1Gate::check` in `s1.rs` (after `decide` returns a phase: read, evaluate,
  on refusal `verdict.block("", "S1: ...")` and no phase); a builder `S1Gate::size_limit(...)` next to `xml_version`
  (`mssql_config_apply/mod.rs:187` passes it, and `command.rs:318` builds the same gate for `--through-apply`, so
  `mssql-config-apply` and `mssql-restructure --through-apply` are covered by that one hook); `command.rs` after
  `make_plan` (line 226) for the standalone run and for `--trial`, which also fills the log; `mod.rs` two lines for
  the modules.
- Only `Method::Rebuild` is guarded. `Method::AlterAdd` is the research variant the apply never uses.

Not affected: an operation of S1-D/E/F that ddl adds needs no change here. The guard reads what the plan says it
rebuilds, whatever the operation.

Open for the coordinator: whether the limit may be raised on the command line (`--max-rebuild-bytes`,
`--max-rebuild-rows`, for a user who knows his log) or is fixed; I propose a flag that can raise it, printed in the
report, because the refusal points to a slower tool and the measured limit is conservative by a stated factor.

## 3. The measurement plan

### 3.1 What the numbers must answer

1. **Log per byte.** Peak log used and reserved by the rebuild transaction, against the bytes of the rebuilt
   tables (data, and data plus indexes), for a heap load plus index build.
2. **Recovery model.** The same under SIMPLE and FULL. The lab twins are SIMPLE (`sys.databases`, the ddl twins): a heap
   load with `TABLOCK` can be minimally logged there, so SIMPLE alone would flatter the copy. A customer's base is
   usually FULL, where the copy is fully logged. **Hypothesis:** FULL is about the size of the table plus its
   indexes, SIMPLE a fraction of it; the limit is set from FULL.
3. **Time** per phase (create, copy, indexes, drop, rename, publication) against size, and what part of it the
   transaction holds `Sch-M` locks for (all of it; the apply is exclusive).
4. **Rollback.** The log reservation is why one transaction needs about twice its used log. Measure
   `database_transaction_log_bytes_reserved` beside `..._used`, and the time of a rollback (`--trial` ends in one).
5. **Shape.** Row width (narrow rows are index-bound, wide rows copy-bound), LOB columns, and sub-tables
   (a tabular section can be the biggest table of an object).
6. **The summed-stage claim.** Two objects in one stage log the sum of their tables.

### 3.2 Instruments (no new code)

- The tool is what exists: `ibcmd-rs mssql-restructure --database <twin> --trial` (the standalone rebuild in one
  transaction, verified, rolled back) with its JSON report (phases and milliseconds per step); and
  `--through-apply --rehearse` (the whole apply's 9.3 MB script, rolled back) for the full-transaction number.
  Both leave the database as it was (12.6 check 10), so one twin serves a whole ladder of sizes.
- A poller in its own process, connected to **`master`** (not to the twin: the tool refuses when another session
  is connected to the database: `reader::other_sessions`, called by `exec.rs`), every 250 ms:
  `sys.dm_tran_database_transactions` (`database_transaction_log_bytes_used` and `..._reserved` of the twin's
  open transaction), `DBCC SQLPERF(LOGSPACE)` (log size and percent used), `sys.dm_io_virtual_file_stats` (bytes
  written to the data and log files), tempdb `sys.dm_db_file_space_usage` (`SORT_IN_TEMPDB`), and the phase clock
  from the tool's report. Output: one CSV per run, one JSON summary.
- The log file is pre-sized above the expected peak so that autogrowth does not enter the timing (the file size
  after the run is recorded as well: it is what the user's disk sees).

### 3.3 The twins and the synthetic table

- **Base.** A twin restored from the staged backup of the simplest case, `ibcmd_rs_04_ddl_bsp8327_a_a2_staged.bak`
  (case a2: one String attribute in `Catalog._ДемоПартнеры`, main table `_Reference20` and two sub-tables
  `_Reference20_VT155`, `_Reference20_VT159`; 14 rows), with `restore-clone.ps1 -Corpus bak` (name
  `ibcmd_rs_04_ui_s1j_a`, track `ui`, about 10 s). Track ddl's file is only read. The staged image is in the
  backup, so no native import is needed: the plan is the same as on every a2 twin, and the growth below does not
  touch `ConfigSave`.
- **Growth.** A T-SQL script (lab folder) reads the columns of the three tables from `sys.columns`, and inserts N
  rows into each by `INSERT ... SELECT` from an existing row and a number generator, giving `_IDRRef` a new
  `binary(16)` and leaving the rest as the existing row has it. Variants: **narrow** (the row as it is), **wide**
  (the long string columns padded to about 2 KB per row), **LOB** (an `nvarchar(max)` value of about 20 KB per row where the
  table has one, or the value-storage column), **sub-table heavy** (few rows in the main table, many in a section).
  The growth is inserted once per rung on the same twin: the trial after it rolls back, and the next rung adds
  more rows.
- **Ladder** (main-table rows for a narrow row of about 250 bytes; the real width is read first from the twin):
  10^4, 10^5, 10^6, 4x10^6 (about 1 GB) and 1.6x10^7 (about 4 GB). Wide: 10^5 and 10^6 (up to 2 GB). LOB: 10^5. Stop
  when the log or the time of a rung is clearly out of any usable limit; the last rung is the evidence for the
  refusal.
- **Disk.** F: has about 1 TB free now. The largest rung is about 4 GB of data and up to 2x that in log, about
  10-15 GB with tempdb, one twin at a time; I check `(Get-PSDrive F).Free` before each rung and stop below
  25 GB. The twin is dropped at the end of the task with `drop-lab-dbs.ps1 -Track ui`.

### 3.4 The matrix

| run | table | rows | recovery | mode | purpose |
|---|---|---|---|---|---|
| R0 | a2 twin as restored | 14 | SIMPLE | trial | the baseline of the tool and the poller (9.3: trial 33.5 s under load; an apply 9 s, 7.3 s of it in the transaction) |
| R1-R5 | narrow ladder | 10^4 ... 1.6x10^7 | SIMPLE | trial | log and time against size where the load can be minimally logged |
| R6-R10 | narrow ladder | the same | FULL (`ALTER DATABASE` of the own twin; a log backup to `NUL` before each run) | trial | the same where it is fully logged: the number the limit is set from |
| R11-R12 | wide | 10^5, 10^6 | FULL | trial | bytes against rows |
| R13 | LOB | 10^5 | FULL | trial | LOB pages |
| R14 | sub-table heavy | 10^3 in the main table, 10^6 in `_VT155` | FULL | trial | the sum over the object |
| R15 | a mid rung | about 10^6 | FULL | **apply** (`mssql-restructure`, committed) then rollback of the test by restoring the twin | the committed number against the trial's: the commit itself, checkpoint and log backup behaviour |
| R16 | a mid rung | about 10^6 | FULL | `--through-apply --rehearse` | the log of the whole apply transaction (the move of 9 842 staged rows and the caches on top) |
| R17 | two objects grown to the same size in one stage | on the types-case twin (`s1_base_t1_staged.bak`) | FULL | trial | the sum over two objects |
| R18 | rollback | the largest rung that finishes | FULL | trial | time of the rollback, and log that the rollback itself writes |

Each run: the tool's JSON report, the poller CSV, the size of the tables before (`sys.dm_db_partition_stats`), the
machine load (nothing else heavy: every run of more than five minutes is one command inside
`heavy-lock.ps1 acquire ui` ... `release ui`, and the ladder is a loop of such commands, never one long hold).

### 3.5 From the numbers to the limit

- Fit peak log (used, and used plus reserved) against the rebuilt bytes, per recovery model and shape; the predictor
  is the one with the tightest fit that the guard can read cheaply: data bytes of the heap or clustered index, or
  data plus index bytes, both from `sys.dm_db_partition_stats`.
- The limit is a policy on top of a measured coefficient: refuse when the **projected log** (coefficient x bytes,
  worst measured shape, FULL) exceeds a budget, or the **projected time** exceeds a budget. I propose the budgets
  as figures for the coordinator to fix: a log of at most 4 GB (a first figure, to be decided) and a rebuild
  that keeps the apply's exclusive window under about 10 minutes; whichever gives the lower bytes wins, and a safety
  factor of 2 on the coefficient. `max_rows` is the narrow-row cap where indexes rather than bytes bind.
- The result is stated as constants with the table of the measurements that justify them, in
  `docs/apply/restructure-size-limit.md` (this file, extended at checkpoint 2); the refusal text quotes the limit.

### 3.6 The ERP УХ confirmation

УХ only to confirm, under the heavy lock. The УХ base is not staged with a change, and staging one there costs a
native import. Proposal: confirm the coefficient on real data without staging, on the largest catalog or document
table of a УХ clone, in one rolled-back transaction: create `<t>NG` from the table's own `DBSchema` entry, copy with
the plan's statement shape, create its indexes, read the transaction's log, `ROLLBACK`. Needs an ERP УХ clone (about
8 GB) or an existing УХ twin: **the coordinator's OK is required** (README, "Processes and disk"). If a УХ twin is
already in the lab I will reuse it.

### 3.7 Cost

About 19 runs. The narrow ladder is minutes for the top rungs, the LOB and wide runs a few minutes each, the
whole matrix roughly two to three hours of machine time in one-command locks, plus the twin's growth time (a 4 GB
`INSERT ... SELECT` is minutes). The 3.6 confirmation adds an hour if allowed.

## 4. Code at checkpoint 2

New files: `src/restructure/size_guard.rs` (types, `read_sizes`, `evaluate`, the refusal text) and
`src/restructure/tests_size_guard.rs` (below the limit passes; exactly at the limit passes; one row or one byte above
refuses; the sum over two tables; the largest table is named; a missing table counts as 0; the message names the native
apply and the limit; `AlterAdd` unguarded). Edits to shared files, a few lines each: `mod.rs` (two module lines), `s1.rs`
(`S1Gate::check` and the builder), `command.rs` (the standalone call), `mssql_config_apply/mod.rs` (pass the limit).
Measurement kit: `scripts/restructure-lab/size-limit/` (new folder). Doc: this file, plus one pointer line in 12.9
item 4 of `restructuring.md`.

Acceptance, from 12.6: the case is "a synthetic table above the limit is refused, and the limit is measured".
Checks that apply: 3 and 4 and 8 unchanged for a stage below the limit (the guard adds no statement: the twin
comparison of a small case equals native as before); 10 (a rehearsal still changes nothing); 11 (the refusal: a twin
with a table grown above the limit, `--through-apply --dry-run`, refused with the message, nothing written, snapshot
equal); 12 needs no new case (the guard adds no statement to the transaction).

## 5. Coordination

Track ddl is in the same planner (S1-D, S1-E). The plan touches `s1.rs` and `command.rs` in a few lines each and adds
files otherwise; I will rebase on `feat/0.4` before every edit of those two, and keep the hooks small enough to merge by
hand.

## 6. Decisions I ask for

1. The place of the check: the gate side in a new file (section 2), not `plan()`.
2. Whether the limit may be raised on the command line (I propose yes, printed in the report) or is fixed.
3. The two budgets of section 3.5 (log, exclusive window) the limit is derived from, or leave them to me with
   the measured table in front of you.
4. The ceiling of the synthetic table: about 4 GB of data and 10-15 GB of disk at the top rung, one twin at a time.
5. The ERP УХ confirmation of 3.6: yes or no, and a clone or an existing twin.
