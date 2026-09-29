# The own `config apply` (milestone 0.4, issues #337 and #339)

`ibcmd-rs mssql-config-apply` moves what `ibcmd infobase config import`
(native or ibcmd-rs) staged in `ConfigSave` into `Config`, the way the
platform's exclusive `ibcmd infobase config apply --force --dynamic=disable`
does, without the platform. It applies a configuration that needs **no
restructuring**: changed modules, forms, templates, pictures and help pages of
any object. Anything that changes the database's structure is refused with the
list of the rows that would (use the native apply for it).

Platform: 8.3.27 (profile `platform-8.3.27.2214`), Microsoft SQL Server. 8.5
stays closed until the `.ui` rows of the ui track are written (issue #340).

Contents: [what the native apply does](#what-the-native-apply-does-measured) -
[what this apply does](#what-this-apply-does) -
[what it does not write](#what-it-does-not-write) -
[safety](#safety) - [the structural gate](#the-structural-gate) -
[command line](#command-line) - [verification](#verification) -
[limits and open points](#limits-and-open-points).

## What the native apply does (measured)

Measured on 8.3.27.2214 with SQL Server Extended Events (`rpc_completed`,
`sql_batch_completed`, `sql_transaction`) and full before/after row-level
snapshots of every service table, on twins of one БСП clone that differ only in
who applied (`ibcmd_rs_04_apply_bsp8327_*`, lab `F:\ibcmd\lab\04\apply`).
Scenarios: S1 a base-free stage with a new form and a new template; S2 a
patch-mode stage of 227 module/form/template edits (9 517 rows); S3 a delta
stage of three objects (30 rows); S2b a second apply with nothing staged.

Order of the native apply (S2/S3, session numbers omitted):

1. Reads: `commit`, `DynamicallyUpdated`, `convertPhase`, `deleted`,
   `erase_save`, `versions_dynupdate_*`, `root`, `version`, the whole
   `ConfigSave` listing, the user list. Nothing else: **no application lock,
   no session check, no single-user switch** (only `SET LOCK_TIMEOUT 20000`).
   A standalone `ibcmd` does not enforce exclusive access at the SQL level.
2. `Params.<uuid>.ui` (two rows): `DELETE ... PartNo <> 0`, `UPDATE ... BinaryData`
   in one user transaction. The value is a base64 text (encrypted); the same
   size, all bytes different.
3. `DELETE FROM ConfigSave/Config WHERE FileName LIKE '%.new'`, then for every
   staged row `F`, two autocommit statements: `DELETE FROM Config WHERE
   FileName = 'F.new' AND EXISTS (SELECT 1 FROM ConfigSave WHERE FileName = 'F')`
   and `INSERT Config SELECT 'F.new', Creation, Modified, Attributes, DataSize,
   BinaryData, PartNo FROM ConfigSave WHERE FileName = 'F'` (all parts). The
   data never leaves the server. ~20 000 statements for 9 517 rows.
4. `MobileVersions.datNEW` in `Files`; `DBNames.New` / `DBNames-Ext-*.New` in
   `Params` (rewritten with unchanged content).
5. The restructuring framework, always, even when nothing is structural: it
   rebuilds `_ConfigChngR` and `_ConfigChngR_ExtProps` as `..NG` tables (new
   `_IDRRef` for every row, the staged objects' `_MessageNo` set to NULL), fills
   `_ExtensionsRestructNGS` when extensions exist, and only in S1 restructured
   the route-point table of a business process. Around it `SchemaStorage`
   (SchemaID 0) walks `Status` 100 -> 200 (`UPDATE ... SET NewGenCreated = <1 KB
   of table text>, Status = 200`) -> 400 -> 500 -> 100 (`UPDATE ... SET Status = 100,
   CurrentSchema = @P1, NewGenCreated = @P2, NewGenDropped = @P3`, 977 KB) and
   `UPDATE DBSchema SET SerializedData = @P1` writes the schema blob again: **both
   are written in every apply, but with the bytes they already had** (S2, S3:
   `DBSchema` and `SchemaStorage` hashes equal before and after; they change only when
   a table does, as in S1).
6. The extension CAS garbage collection (`ConfigCAS` 12 797 -> 636 rows,
   `Files.CAS_GC_Info`, `extd_props_cached/gc.mrk`) and the help index
   (`Files.userDocs_ru*`, `userPostings_ru*`, `userVocabulary_ru*`).
7. Recovery marker: one row `commit` in `Config`.
8. `Params.*.si` service-information rows (16 on БСП) written as `.sinew` and
   renamed, `siVersions` with new version guids.
9. `DELETE FROM Params WHERE FileName = 'DynamicallyUpdated'`; a dynamic
   generation from an earlier dynamic update is **folded into the ordinary
   rows**: for every alias `X_dynupdate_G[.n]`, the ordinary `X[.n]` is
   deleted and the alias renamed to it; `versions_dynupdate_G` and
   `deleted_dynupdate_G` are deleted.
10. For every staged row `F`: delete `Config.F`, rename `F.new` to `F`
    (autocommit again, ~10 000 statements, the crash-safe part is the `commit`
    marker). `root`, `version`, `versions` last. `Config.DynamicallyUpdated`
    deleted.
11. `DELETE FROM ConfigSave` (all rows), `MobileVersions.datNEW` renamed over
    `MobileVersions.dat`, the markers `commit`, `dynamicCommit`, `dbStruFinal`
    deleted (the last two are never written by an exclusive apply), the help
    index files renamed.

Consequences measured on the end state:

- a multi-part `Config` row (10 MB parts) becomes one `PartNo = 0` row: the
  staged row is copied as it is;
- the staged rows' `Creation`/`Modified` are kept, `Attributes` too;
- a `ConfigSave` that is empty makes the native apply print "Обновление
  конфигурации базы данных не требуется" and change nothing (5.6 s);
- the new generation the native apply prints is the header GUID of the staged
  `versions` row (byte-swapped) followed by `00000000`.

### The state an interrupted native apply leaves

The native apply is ~30 000 autocommit statements around the `commit` marker. When it dies
part-way (ddl track, 2026-09-29: killed at «Принятие изменений» while several native runs
competed for the CPU) the database answers every `ibcmd config` command with «Обнаружена
незавершенная операция сохранения конфигурации». By the trace that state is: `SchemaStorage`
(SchemaID 0) at `Status` 200, 400 or 500 (never 100), a non-empty `NewGenCreated`, possibly
`DBSchema` already rewritten, `*.new` rows in `Config` (and a `commit` row once the renames
began), `ConfigSave` still full. The own apply **refuses** that state before it writes anything
(the `*.new` and marker names, and `SchemaStorage.Status <> 100`, are checked in the plan and
again under the locks) and points at the native `config repair`. It never produces the state
itself: the whole move is one transaction.

### What the importer stages (findings for the import track)

- Our import stages the **whole** configuration (patch mode 9 517 rows, base-free 9 842), not a
  delta, so a native apply of it moves every row through `.new`, unchanged ones too.
- ddl track: patch mode silently drops descriptor changes (a new catalog attribute never reached
  `ConfigSave`; every staged row equalled `Config` but `versions`). The apply can only judge what
  is staged: **a change that is missing from `ConfigSave` is invisible to every gate here**.
- Patch mode cannot stage a new form or template (`Config row not found`); base-free can, but it
  recompiles a business process's route-point flowchart differently, which makes the native apply
  restructure that table (S1).
- The help index (`Files.userDocs_ru*`) built by the native apply from our staged help rows is
  nearly empty (2 KB and 2 bytes against 47 KB in the base), so our packed help pages are not
  what the platform's indexer reads.

## What this apply does

One serializable transaction, data moves inside the server only:

1. application lock `ibcmd-rs:config-apply`, exclusive table locks on
   `Config`, `ConfigSave`, `Params`, `Files` (and `_ConfigChngR`);
2. exclusive access: no other user session on the database (`sys.dm_exec_sessions`,
   needs `VIEW SERVER STATE`; our own process is excluded by pid), no unfinished
   operation (`commit`, `dynamicCommit`, `dbStruFinal`, `convertPhase`,
   `erase_save`, `deleted`, `*.new`);
3. the plan's view must still hold: aggregate fingerprints of `ConfigSave`, of
   the `Config` rows it replaces, of the dynamic-update rows and of the `Params`
   marker (row count, byte total and three sums of SHA-256 slices, computed by
   the server) equal what the read-only planning saw;
4. the dynamic generations are folded oldest first (see step 9 above), then
   both `DynamicallyUpdated` rows are deleted;
5. every `Config` row named by a staged row is deleted (all parts) and the staged
   rows are inserted as they are;
6. `_ConfigChngR._MessageNo := NULL` for every staged descriptor's object, all nodes
   (measured: 4 703 staged descriptors = 4 703 objects x nodes reset in S2, 13 objects
   x 3 nodes in S3; unstaged objects keep their value);
7. `Files.MobileVersions.dat` gets a fresh random GUID at the head (list capped at
   1 000), timestamps in the platform's form (local time plus the year offset);
8. postconditions inside the transaction: every staged row is in `Config`
   byte for byte (name, part, size, attributes, timestamps, `BinaryData`), no other
   part of it is, the moved row count equals the staged count, no alias or marker is
   left, no change registration of a staged object keeps a message number;
9. `ConfigSave` is emptied and the transaction commits. A failed step, a lost
   connection or a crash rolls everything back: nothing is left for
   `ibcmd infobase config repair` to finish.

A rehearsal (`--rehearse`) runs the same script and ends it with `ROLLBACK`.

## What it does not write

Derived state the native apply rewrites and this one leaves alone (each is
listed in the report as `not_written`):

| What | Why it is safe to skip |
|---|---|
| `Params.<uuid>.ui` (two rows) | Encrypted generation-selection rows. 8.3.27 sessions read the ordinary generation; the earlier exclusive/live modes of ibcmd-rs never wrote them and fresh sessions saw the new code. On 8.5 they are required, hence the hook for the ui track. |
| `Params.*.si`, `siVersions` | Service-information caches. Their content is a function of the object set and names (same GUID multiset before and after S2; unchanged bytes in S3). Only a new object, a rename or a synonym change alters them. |
| Help index (`Files.userDocs_ru*` ...) | Rebuilt by the native apply from help pages; a body-only change leaves the pages, so the old index stays valid. |
| ConfigCAS garbage collection and its bookkeeping | Housekeeping of extension content, independent of the main configuration. |
| `_ExtensionsRestructNGS`, `_IDRRef` renewal in `_ConfigChngR` | Scratch of the restructuring framework; the registrations keep their ids, so `_ConfigChngR_ExtProps` stays consistent. |

## Safety

- **Fail closed**: an unknown storage layout (table fingerprint of the profile), an
  unsupported platform profile, a `deleted_dynupdate_*` row, an unfinished operation,
  a reused generation, an unlisted staged row (warning), any structural blocker: no
  write.
- **Plan without locks, verify under locks**: the plan reads metadata and server-side
  fingerprints only; the transaction re-checks them after taking the locks.
- **Exclusive access** is proven by SQL Server's session list, not assumed. A working
  process that still holds a pooled connection makes the apply refuse (stop the
  process or the infobase's sessions first). `--exclusivity assumed` is for the operator
  who has proved it with `rac session list`.
- **Recovery artifact** (before the transaction): the bytes of every row the apply
  changes (`--recovery-blobs changed`, default) or a hash manifest (`none`), the special
  rows, `MobileVersions.dat` and the `_MessageNo` values.
- **Dry run** writes nothing at all; **rehearsal** writes nothing that survives.

## The structural gate

`ConservativeGate` (module `mssql_config_apply::gate`, one call site in `plan`) admits:

- `root` and `version` unchanged, `versions` replaced with a new generation;
- a descriptor row that exists in `Config` and inflates to the same text;
- a body row whose owner kind and suffix the source-asset registry names as a module,
  form, template, picture or help page; other body roles pass only when the inflated
  text is unchanged.

It refuses new objects, descriptors whose text differs, predefined data, rights, interface,
package and unknown bodies, and unknown row names. The restructure-check track's
`check_staged` replaces it behind the `StructuralGate` trait.

## Command line

```
ibcmd-rs mssql-config-apply --platform-profile platform-8.3.27.2214 --database <db>
    [--server localhost] [--sql-user U --sql-pwd P | --sql-pwd-env IBCMD_DB_PSW]
    [--dry-run | --rehearse] --allow-non-lab
    [--exclusivity sql|assumed] [--recovery-dir DIR] [--recovery-blobs changed|none]
    [--script-output FILE] [--report FILE]
```

## Verification

Lab: `F:\ibcmd\lab\04\apply` (scripts in `tools\`, snapshots in `snap\`, traces in `xe\`, logs in
`logs\`). Twins of one БСП 8.3.27.2214 clone (`ibcmd_rs_04_apply_bsp8327_*`) carry byte-identical
`ConfigSave` rows; one is applied natively, the other by `mssql-config-apply`; a row-level snapshot
(`tools\snapshot.sql`) of every service table is taken before and after (`tools\explain_diff.py`
sorts every difference into a class).

| Scenario | Stage | Native apply | Own apply |
|---|---|---|---|
| S2 patch stage of 227 module, form and template edits, one earlier dynamic generation in the base | 9 517 rows, 81.3 MB | 298 s (under load; 110 s in S1) | 22 s in all: 4 s inventory, 1 s fingerprints, 9 s recovery artifact, **5.6 s transaction** |
| S3 delta stage of three objects (catalog, document, common module) | 30 rows, 0.4 MB | 87.5 s | **1.2 s** in all, 0.6 s transaction |

End state, own vs native, on the same stage:

- `Config`: **9 838 of 9 838 rows identical** (S2), **9 841 of 9 841** (S3) -- names, parts, sizes,
  attributes, `Creation`/`Modified` and bytes, including the aliases folded into the ordinary rows
  and the removed `DynamicallyUpdated` rows.
- `_ConfigChngR`: the message number of **all 20 685 rows** equal (S2 and S3, from a state where
  every row was set to 0 first, so that a reset shows).
- `Params`: the dynamic marker gone; 18 (S2) rows identical, 16 rewritten by the native apply with
  unchanged content; only the two `.ui` rows, `siVersions` and (S2) one `.si` row differ.
- `Files`: `MobileVersions.dat` differs in its random head GUID only; the help-index files and
  chunk rows and the CAS bookkeeping are the native apply's own housekeeping.
- `DBSchema`, `SchemaStorage`, `IBVersion`: identical.
- `ConfigCAS`: 636 rows identical, 12 161 garbage-collected by the native apply only.

The platform on the own result (`ibcmd_rs_04_apply_bsp8327_ours2_20260929`):

- `ibcmd infobase config apply --force --dynamic=disable` afterwards: "Обновление конфигурации базы
  данных не требуется" in 4 s, zero row changes;
- `ibcmd infobase config check --force`: "Проверка корректности метаданных успешно завершена";
- `ibcmd infobase config generation-id`: `b904aa5eecc9ad469d3ff335dab4f27b00000000`, the value the
  native apply printed for the same stage;
- `ibcmd infobase config export`: **all 12 198 files byte-identical** to the native export of the
  natively applied twin (`ConfigDumpInfo.xml` included), 12 180 identical to the edited tree; the
  other 18 are `ConfigDumpInfo.xml` (`configVersion`) and 17 modules that were empty in the reference
  tree, where the test edit appended a comment with LF and the export writes CRLF.

Also exercised: a rehearsal (`--rehearse`) leaves the database identical to its before-snapshot;
another session on the database makes the apply refuse with the session named; the recovery
artifacts verify against the before-snapshots (`tools\verify_recovery.py`: every manifest row equals
the snapshot row, every saved file hashes to its manifest entry).

## Limits and open points

- **New rows** (a new form or template on an existing object) are refused by the gate: a new
  descriptor needs `_ConfigChngR` rows for every node, an entry in the `.si` service information
  and a check that the owner's descriptor differs only by the reference. Measured, not yet
  written (`docs/apply/own-apply.md` will gain it with the implementation).
- **Sessions**: that a fresh session sees the change is not yet observed here; the 1C server
  (LocalSystem) has no SQL login on this machine. The 8.3.27 evidence for ordinary-row publication
  (`live`, `exclusive` modes) applies; `tools\session_probe.ps1` runs the check once a login exists.
- **8.5** stays closed: its `.ui` rows are required (hook: `ConfigApplyOptions`).
- The help index and the extension CAS garbage are left as they are; they are caches.
- A working process that keeps a pooled connection makes the SQL exclusivity check refuse; the
  native standalone `ibcmd` does not check at all.
