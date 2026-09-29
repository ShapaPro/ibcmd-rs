# Apply-trace capture kit

Answers one question about a native 1C operation on a SQL Server infobase
(`ibcmd infobase config apply`, `infobase create`, `config load`, ...):
**what exactly did it write?**  Every claim is backed by a before/after row
diff and the SQL trace of the run.

Read-only for the inspected database: the kit never writes to it.  It only
creates and drops an Extended Events session on the server.

Needs: PowerShell 7 (or Windows PowerShell 5.1), Python 3.9+, SQL Server with
Windows authentication for the current user (`ALTER ANY EVENT SESSION` for the
trace).  No sqlcmd, no bcp, no pip packages.

| Script | What it does |
|---|---|
| `snapshot.ps1 -Database <db> -Out <dir>` | the state of the infobase: every table (rows, checksum), the schema, one line per row of the service tables |
| `diff.py --before <dir> --after <dir>` | tables created/dropped/altered, rows inserted/updated/deleted per service table, count/checksum changes of data tables |
| `trace.ps1 -Database <db> -Tag <t> -Command { ... }` | runs the command under an XE trace and writes the report |
| `capture.ps1 -Database <db> -Tag <t> -Command { ... }` | snapshot before, trace of the command, snapshot after, diff, one report folder |
| `trace_report.py` | the trace analysis used by `trace.ps1` (also usable on its own on `events.tsv`) |
| `tests/` | offline unit tests: `python -m unittest discover -s scripts/apply-trace/tests` |

## One command, everything

```powershell
$ibcmd = 'C:\Program Files\1cv8\8.3.27.2214\bin\ibcmd.exe'
$db    = 'ibcmd_rs_04_x_bsp_1'                      # a disposable clone
$data  = "F:\lab\ibdata\$db"
pwsh -NoProfile -File scripts\apply-trace\capture.ps1 -Database $db -Tag apply-exclusive `
   -OutRoot F:\lab\04\x\captures -BlobStore F:\lab\04\x\blobs `
   -Command { & $ibcmd infobase config apply --dbms=MSSQLServer --db-server=localhost --db-name=$db `
                --data=$data --user=Администратор --force --dynamic=disable }
```

(`pwsh -File` cannot pass a script block: call the script from a PowerShell
session, `& scripts\apply-trace\capture.ps1 ... -Command { ... }`, or use
`-Exe <path> -ArgumentList <args>` from anywhere.)  The script block runs in
the scope chain of the script: name your variables differently from the
script parameters (`$Database`, `$Tag`, `$Exe`, `$Server`, `$Track`, ...) if
they differ in value.

Result, `<OutRoot>\<yyyyMMdd-HHmmss>-<Tag>\`:

```
report.md           headline and links
before\ after\      snapshots (see below)
trace\              summary.md groups.md timeline.md service-writes.tsv payloads.jsonl
                    data-writes.tsv ddl.sql transactions.tsv slowest.tsv sessions.tsv
                    events.tsv trace-meta.json session.sql command.log
diff\               diff.md diff.json
```

A native ibcmd on a БСП database needs `--user=Администратор` (without it, with
stdin closed, it loops forever printing «Имя пользователя:»).

## snapshot.ps1

```powershell
pwsh -NoProfile -File scripts\apply-trace\snapshot.ps1 -Database <db> -Out <dir> `
     [-BlobStore <dir>] [-Reference <earlier snapshot>] [-DataChecksum full|counts|none]
```

Files: `meta.json`, `tables.tsv` (rows, used_kb, `CHECKSUM_AGG(BINARY_CHECKSUM(*))` and
the sum of the row checksums of every table), `columns.tsv`, `indexes.tsv`,
`constraints.tsv`, `objects.tsv`, `dbsettings.tsv`, `rows\<Table>.tsv`.

*Service tables* (`rows\`) are the file tables (`FileName` + `PartNo` + `BinaryData`:
Config, ConfigSave, ConfigCAS, ConfigCASSave, Params, Files, DepotFiles) and every
table whose name has no digit (DBSchema, SchemaStorage, IBVersion, v8users,
_SystemSettings, _ConfigChngR, ...).  Object tables (`_Reference123`, ...) have a
row count and a checksum only.

File tables: one line per file (parts are joined): size, stored bytes, sha256 of
the stored bytes, creation/modified **as stored (the platform adds a year offset,
`_YearOffset`, normally 2000: 4026-09-29 is 2026-09-29)**, `enc` (`deflate` when
the bytes are exactly one raw-deflate stream, else `raw`), decoded size and sha256,
`kind` (`v8text` for the `{...}` serialization, `text`, `container` for the 1C file
container, `binary`, `empty`), `blob` (Y when the content is kept), a preview.

The content of rows lives in a content-addressed **blob store** (`-BlobStore`,
append-only `pack-*.bin` files plus `index.tsv`; share one store between the
snapshots of a run: identical rows are stored once).  Policy: every row of
Params, Files, ConfigSave and ConfigCASSave; rows of Config/ConfigCAS that are
about to be replaced (their name is in ConfigSave/ConfigCASSave), the service
names of Config (`root`, `version`, `versions*`, `*.new`, `DynamicallyUpdated`,
...) and, with `-Reference`, every row that is new or different from the
reference snapshot.  `capture.ps1` passes the *before* snapshot as the reference of
the *after* snapshot, so both sides of every changed row are available to `diff.py`.
`-ContentMaxKB` (512) bounds one row, `-ContentBudgetMB` (1024) one snapshot.

Other service tables: `key` (primary key), `row_sha256` and a summary of the columns
(large columns are shown as length and sha; DBSchema/SchemaStorage keep up to 16 MB).

Time: about 1.5 minutes on a БСП infobase (2,200 tables, 350 MB) on a busy
machine; the full checksum scan is most of it for big databases (`-DataChecksum
counts` skips it, `-ChecksumMaxMB` skips only the big tables).

## trace.ps1

```powershell
& scripts\apply-trace\trace.ps1 -Database <db> -Tag <t> -OutDir <dir> -Command { ... }
& scripts\apply-trace\trace.ps1 -Database <db> -Tag <t> -Exe <path> -ArgumentList a,b,c
& scripts\apply-trace\trace.ps1 -Database x -Tag x -Cleanup      # drop sessions left by a killed run
```

XE session `ibcmd_rs_04_<Track>_<n>` (default Track `trace`; the first free `n`),
filtered by `sqlserver.database_id`, events `rpc_completed`, `sql_batch_completed`,
`sql_transaction` (user transactions: begin/commit/rollback with the duration),
`begin/commit/rollback_tran_completed`, `error_reported` (severity 11+), optionally
`sql_statement_completed` (`-IncludeStatements`); actions: event sequence, session,
transaction id, client application, database.  Files under
`C:\temp\ibcmd_rs_04\<Track>\`, deleted after the export (`-KeepXel` keeps them).
The session is stopped and dropped even when the command fails.
`-AllDatabases` replaces the database filter by "non-system sessions" (use it when
the command also works in `master`, like `infobase create` may).

Limits worth knowing: `rpc_completed.statement` is cut by SQL Server at 2,000,000
characters, i.e. a binary parameter is visible only up to its first ~1,000,000
bytes (the report says `truncated by SQL Server`); `sql_batch_completed` keeps
the whole text.  Bulk-copy (`INSERT BULK`) row data is not visible in a trace: the
row diff is the evidence there.  Retention is `ALLOW_SINGLE_EVENT_LOSS` (needed for
`error_reported`); `trace-meta.json` and `summary.md` state the number of dropped events.

### Reading the report

* `summary.md` - what was done: statement counts by verb, writes per service table
  by row-name shape (`<guid>.<n>`, `<guid>_dynupdate_<guid>`, `root.new`, ...), DDL
  summary, errors, the transactions, the slowest statements.
* `timeline.md` - the statements in order, consecutive equal statements collapsed
  into runs, with BEGIN / COMMIT / ROLLBACK markers (coarsened to ~400 lines).
* `groups.md` / `groups.tsv` - normalized statements (literals replaced) in order of
  first appearance with counts, total time, rows, the shapes of the string parameters.
* `service-writes.tsv` - **every** INSERT/UPDATE/DELETE on a service table in
  order: transaction, FileName, new FileName (rename), PartNo, rows affected,
  declared size, size and sha256 of the written bytes.  `payloads.jsonl` decodes the
  small written file rows.
* `ddl.sql` - CREATE/ALTER/DROP/TRUNCATE/sp_rename verbatim, in order (temporary
  tables are only counted).
* `transactions.tsv` - user transactions, from `sql_transaction` events.

## diff.py

```powershell
python scripts\apply-trace\diff.py --before <snapshot> --after <snapshot> [--out <dir>] [--blobs <store>]
```

`diff.md`: schema changes (tables created/dropped/altered with columns and indexes,
other objects, database options); per service table the rows inserted/deleted/updated
by name shape and one line each; for updated file rows whose content is in the store,
what changed inside (module text of a container, `versions` entries, `{...}` tokens,
whitespace ignored); for data tables the row count and checksum changes.

## Twins (two runs from identical state)

Stage once on clone X, `BACKUP DATABASE X ... WITH COPY_ONLY, COMPRESSION` to your lab
folder, restore the twin with `F:\ibcmd\lab\04\tools\restore-clone.ps1 -Corpus bak -Bak <file>`
(see the header of that script), take the *before* snapshot on each, run each
variant under `capture.ps1`, and compare the two `after` snapshots with `diff.py`.

## Notes

* Dates in the platform's tables carry a year offset (see `year_offset` in
  `meta.json`).  Files and the text of `tables.tsv` are UTF-8, `\`, tab, CR, LF escaped.
* The kit reads only.  It is not a backup: a snapshot has no data of the object tables.
