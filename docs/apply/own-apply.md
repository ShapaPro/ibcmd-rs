# The own `config apply` (milestone 0.4, issues #337 and #339)

`ibcmd-rs mssql-config-apply` moves what `ibcmd infobase config import`
(native or ibcmd-rs) staged in `ConfigSave` into `Config`, the way the
platform's exclusive `ibcmd infobase config apply --force --dynamic=disable`
does, without the platform. It applies a configuration that needs **no
restructuring**: changed modules, forms, templates, pictures and help pages of
any object, and a **new form or template of an existing object**, from a full
stage or a delta stage (a few objects and `versions`). Anything that changes the
database's structure is refused with the list of the rows that would (use the
native apply for it).

Platform: 8.3.27 (profile `platform-8.3.27.2214`), Microsoft SQL Server. The 8.5
profile is refused: the apply is measured on 8.3.27 only. The `Params` `.ui` rows
(the platform's configuration-licensing records, track ui #340) are **never
written** by this apply; see [known differences](#known-differences-from-the-native-apply).

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
stage of three objects (30 rows); S2b a second apply with nothing staged; S4 a
delta stage without `root` and `version` (16 rows: a new form, a new template
and their owner, ten module edits, `versions`); S5 delta stages of the other
shapes of new objects (see below).

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
5. The restructuring framework, on the **long path** (S1, S2, S3; the trace track's cases 1x, 2a, 2c): even
   when nothing is structural it
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

### The short path (S4; the trace track's case 2n)

Not every stage takes the long path above. The S4 stage (traced with the kit of track trace, 195 write
statements, 24 user transactions, 75 s, `docs/apply/evidence/own-apply/s4-write-families.md`) ran no
structure phase, no `..NG` tables (no DDL at all), no `DBNames` passes, no ConfigCAS garbage collection:
`.ui` (3 rows), the `.new` copy of the staged rows, `MobileVersions.datNEW`, the 16 `.sinew` rows, the `commit`
marker, the promotion of `.si`, `dbStruFinal`, `UPDATE _ConfigChngR SET _MessageNo = NULL` (40 rows) with six
`INSERT`s into `_ConfigChngR` and six into `_ConfigChngR_ExtProps` (the two new objects, three nodes each),
the promotion of the staged rows, the clean-up and the help index. `_IDRRef`, `DBSchema` and `SchemaStorage`
kept their bytes. What decides the path is open (trace track, question 1; its case 2n took the short path
too, with `root`, `version` and `versions` staged). S4 adds two data points: it staged the descriptor of
one data processor (a table-less kind) and ten body rows of objects of mixed kinds -- a constant and a report
among them -- **without** their descriptors, and stayed on the short path; S3 staged the descriptors of a
catalog and a document and took the long one. This apply does not care: it never renumbers ids and writes
the same end state for both paths (S3 on the long path, S4 on the short one, both equal).

A **third path** was seen for a stage of `versions` and one body row (S5C, a module added to a data
processor, 25 s): no «Сбор служебной информации», no help-index build, `Params.DynamicallyUpdated` and the
`.si` rows left alone; `.ui` (3 rows), `MobileVersions.dat`, `Config` (with the fold of the dynamic
overlay) and the change registrations are written. This apply reproduces it by writing `.si` only for new
objects and the `Params` marker deletion only for stages with a descriptor row. The same paths, by owner
kind, in S5: a data processor's or a form's descriptor (short path), a catalog's (S5B: the long path, the
register rebuilt).

### What a new form or template adds (measured: S1, S4, S5)

Besides the staged rows themselves (the two descriptors, their `.0` bodies and the
owner's descriptor, which lists the new uuid in its forms or templates group with
the group's count raised):

- **`_ConfigChngR`**: one row per ordinary exchange-plan node and new object, `_MDObjID` =
  the uuid in the platform's byte order, `_MessageNo` NULL, a fresh `_IDRRef` (the platform
  takes them from a clock-based sequence; any unique 16 bytes do). The nodes are those of the
  exchange plans the table already holds rows for, **except the plan's own node**
  (`_PredefinedID <> 0` in the plan's node table `_Node<n>`, `n` = the `_NodeTRef` as a
  number): in the БСП demo three of five registered nodes (two DIB nodes and a DIB node with
  a filter); the plan's own node and the offline-work plan's own node get no row.
- **`_ConfigChngR_ExtProps`**: the new object's file list, one row per node row:
  `(_KeyField 0, '<uuid>.0')` (a form's help page `.1` follows as key 1). The list of an
  *existing* object is historical, not derived: one form in the demo lists `.1` before `.0`
  although only `.0` exists. A body row an existing object gains is therefore *appended*
  (key = last key + 1).
- **`_MessageNo` reset**: every object that owns a staged row, not only those with a staged
  descriptor: the object whose uuid starts the staged name, or the object whose file list
  names it (the configuration's own module `f389d417-...0` resets the configuration
  object `66193438-...`). S4: 11 objects, 39 rows (two of them the plan's own nodes).
- **`Params` search information** (16 `.si` rows written as `<uuid>.si`, plus `siVersions`):
  every row is rewritten with the same content and a new version guid; the **main row**
  (`1a621f0f-...si`, 2 MB inflated, raw deflate, BOM, CRLF) lists every metadata object as a
  record `uuid, parent, kind, "name", {1,n, {"lang","synonym"}...}, flag, flag`, depth
  first: the owner, then the groups of its attributes, tabular sections, forms, templates and
  commands. *kind* is the index of the object's class id in the row's own first list
  (a catalog's forms are `fdf816d2-...` = 37, a data processor's `d5b0e5ed-...` = 60, every
  template `3daea016-...` = 16), so it is the descriptor's group class id. The new object gets
  a record next to its sibling of the same group (behind the previous one in the owner's
  list, in the descriptor's order); the count in `{10809,` rises. The two flags: the first is
  the form descriptor's flag behind its header (0 for a managed form, 1 for an ordinary one:
  383 of 383 forms agree), the second 0; templates 0/0. Names and synonyms come from the
  descriptor's header (`{1,"ru","Форма"}` becomes `{1,1,{"ru","Форма"}}`; several languages
  sorted by code, one per line). `siVersions` is `{0,16,"<name>.si",<guid>,...}`: only the
  edited row's guid needs to change.
- **What does not change**: `DBSchema` and `SchemaStorage` (a form or a template has no
  table), `IBVersion`, `Config` rows other than the staged ones and the folded aliases.

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
   the `Config` `DynamicallyUpdated` row is deleted, and the `Params` one too when the
   stage carries a descriptor row (native leaves it after a stage of body rows alone, S5C);
5. every `Config` row named by a staged row is deleted (all parts) and the staged
   rows are inserted as they are;
6. `_ConfigChngR._MessageNo := NULL` for every object that owns a staged row, all
   nodes: by the uuid a staged name starts with, or through `_ConfigChngR_ExtProps` (a file
   list that names a staged row). Measured: 4 703 objects x nodes in S2, 13 objects x 3
   nodes in S3, 11 objects / 39 rows in S4; unstaged objects keep their value;
7. new forms and templates: registered in `_ConfigChngR` for every ordinary node
   (ids continue the table's sequence: the greatest `_IDRRef` plus one), their files listed in
   `_ConfigChngR_ExtProps`; a body row an existing object gains is appended to its list;
8. `Params`: the main search-information row gets a record per new object and its
   `siVersions` entry a new version (both guarded by the digest the plan saw); `Creation`
   and `Modified` in the platform's form;
9. `Files.MobileVersions.dat` gets a fresh random GUID at the head (list capped at
   1 000), timestamps in the platform's form (local time plus the year offset);
10. postconditions inside the transaction: every staged row is in `Config`
    byte for byte (name, part, size, attributes, timestamps, `BinaryData`), no other
    part of it is, the moved row count equals the staged count, no alias or marker is
    left, no change registration of a staged object keeps a message number, every new
    object is registered for every node with its files;
11. `ConfigSave` is emptied and the transaction commits. A failed step, a lost
    connection or a crash rolls everything back: nothing is left for
    `ibcmd infobase config repair` to finish.

A rehearsal (`--rehearse`) runs the same script and ends it with `ROLLBACK`.

## What it does not write

Derived state the native apply rewrites and this one leaves alone (each is
listed in the report as `not_written`):

| What | Why it is safe to skip |
|---|---|
| `Params.<uuid>.ui` (two rows) | The platform's configuration-licensing records (track ui, #340), re-encrypted by every native apply. This apply never creates or alters licensing data. See [known differences](#known-differences-from-the-native-apply). |
| `Params.*.si`, `siVersions` (except for new objects) | Service-information caches. Their content is a function of the object set and names (same text before and after S2/S3; only a new object, a rename or a synonym change alters it). The native apply rewrites all 16 rows with unchanged content and new version guids; this one rewrites the main row (and its `siVersions` entry) only when a new form or template adds records. |
| Help index (`Files.userDocs_ru*` ...) | Rebuilt by the native apply from help pages; a body-only change leaves the pages, so the old index stays valid. |
| ConfigCAS garbage collection and its bookkeeping | Housekeeping of extension content, independent of the main configuration. |
| `_ExtensionsRestructNGS`, `_IDRRef` renewal in `_ConfigChngR` | Scratch of the restructuring framework (three constant rows in every native apply that takes the long path, none on the short one); the registrations keep their ids, so `_ConfigChngR_ExtProps` stays consistent. |

### Known differences from the native apply

Everything that differs between our end state and the native one on the same stage, and why
the platform does not mind (S2, S3, S4, and the probe below):

- **`Params` `.ui`**: the native apply re-encrypts two rows on every apply (the large one
  is the configuration-licensing request, the small one its answer; the ui track decrypts
  them, `docs/apply/params-ui.md` when it lands). We leave them as they were. Evidence
  that the platform opens the database without warnings: `ibcmd infobase config check`
  succeeds on the result (S2, S4, probe); the native `config apply` afterwards answers
  "Обновление конфигурации базы данных не требуется" (S2, S4); a **standalone server
  (`ibsrv`) started cold on the probe database** (a new process, new sessions; the 1C server
  cannot be used here, see limits) answers an HTTP web-service call with status 200 and the
  marker of the module change the apply moved, with an empty error stream. Track ui measures
  the same for 8.5 and licensing; its result replaces this paragraph when it lands.
- **All `.si` rows and their versions** are rewritten by the native apply even when their
  content is unchanged; we write only what changes (S4: the rows are identical in text).
- **`_IDRRef`** of `_ConfigChngR` is renumbered by the native apply on the long path (S1-S3);
  we keep the ids (the short path, S4, does not renumber either). Ids of new registrations
  differ by construction.
- **Help index, ConfigCAS garbage collection, `_ExtensionsRestructNGS`**: caches and
  scratch of the native apply, see the table above.
- **`Creation`/`Modified` of `Files.MobileVersions.dat`** and the random head guid differ.

### Which per-apply writes are required

The trace track's list of what every native apply writes (`docs/apply/native-apply-trace.md`
sections 3.2 and 7) against what this apply does, with the verdict the evidence supports.
The verdicts come from applying with the minimal set at once (S2, S3, S4, the probe) and
opening the result with the platform's own tools; a single omitted group was not dropped
alone from a full native set, because nothing in the results asks for it:

| Native write | This apply | Verdict and evidence |
|---|---|---|
| `Config` rows: `.new` copy, `commit` marker, promotion row by row outside a transaction | one `INSERT ... SELECT` in one transaction; `commit`, `dbStruFinal`, `dynamicCommit` are never written | **required**: the rows (identical to native in S2, S3, S4). The markers exist to resume an interrupted promotion; a transaction has nothing to resume. |
| Fold of dynamic overlays (`X_dynupdate_G` over `X`, `versions_dynupdate_G`, `DynamicallyUpdated`) | written, oldest generation first | **required** when the base has an overlay (S1-S4 all had one); rows identical. |
| `_ConfigChngR._MessageNo := NULL` | written for every owner of a staged row | **required** (it is what the exchange plans read); identical in S2-S4. |
| New objects: rows in `_ConfigChngR`, `_ConfigChngR_ExtProps` | written | **required**; identical to native but for the ids. |
| `_ConfigChngR` and `_ExtProps` rebuilt through `..NG` tables with new ids (long path) | not written | **not required**: the platform runs on the old ids (check, apply afterwards, cold server); the short path does not renumber. |
| 16 `.si` rows and `siVersions` rewritten | only the main `.si` row and its version, when a new form or template adds records | **required for new objects** (the row lists them; text identical to native in S4); **not required otherwise** (content unchanged, S2-S4). |
| `Files.MobileVersions.dat` (ring of 1 000 guids) | written, one new guid in front | kept: it is what mobile clients compare, and it is cheap. Not shown to be needed by anything measured here. |
| `Files.extd_props_cached/gc.mrk`, `CAS_GC_Info`, ConfigCAS garbage collection | not written | **not required**: bookkeeping of the extension content store, independent of the change (first apply of a lineage only for the collection). |
| Help index `userDocs_ru`, `userVocabulary_ru`, `userPostings_ru` | not written | **not required to open the database** (not a session or export input); a stale index only affects searching help. Rebuilding it needs the platform's indexer. |
| `DBNames*.New` passes, `DBNames`, `DBNamesVersion-DBNames` | not written | **not required**: unchanged for every stage measured (no table is added or renamed by a module, form or template). |
| `DBSchema` and `SchemaStorage` walk 100 -> 200 -> 400 -> 500 -> 100 (long path) | not written; a state other than 100 is refused | **not required** while no table changes: the bytes are identical before and after. |
| `_ExtensionsRestructNGS` clean-up | not written | scratch; identical rows in every native apply. |
| `Params` `.ui` (3 rows) | not written | licensing records (track ui): see known differences. |

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
  rows, `MobileVersions.dat`, the `_MessageNo` values, the `Params` rows rewritten for new
  objects with their old bytes (`params_replaced.tsv`) and the registrations added
  (`new_registrations.tsv`).
- **Dry run** writes nothing at all; **rehearsal** writes nothing that survives.

## The structural gate

`ConservativeGate` (module `mssql_config_apply::gate`, one call site in `plan`) admits:

- `root` and `version` unchanged (or absent: a delta stage), `versions` replaced with a new
  generation;
- a descriptor row that exists in `Config` and inflates to the same text;
- a body row whose owner kind and suffix the source-asset registry names as a module,
  form, template, picture or help page; other body roles pass only when the inflated
  text is unchanged;
- **new rows**, judged by `mssql_config_apply::objects` and passed to the gate as accepted:
  - a *new form or template of an existing object*: the owner's staged descriptor must be
    the active one plus the references (equal as brace trees once the new uuids are taken
    out of every `{class,count,uuid...}` group and the counts are put right); nothing removed,
    nothing else changed; the new descriptor's own header carries its row's id; a form has
    the 0/1 flag behind its header; its bodies are `.0` (and `.1` for a form), nothing else;
  - a *new body row of an existing object* that has a descriptor row and change registrations;
    one per object (the order of several is not known);
  - the search information must list the owner, the class of the group must be in its class
    list, and where the first child of a group goes must follow from the order other owners
    of the kind list their groups in; the exchange-plan nodes must be readable
    (`_Node<n>` tables) and none marked for deletion.

It refuses new objects of every other kind (a catalog, an attribute, a command, a subsystem),
new bodies of nested objects or of the configuration, owners whose descriptor changes more
than the lists, descriptors whose text differs, predefined data, rights, interface, package
and unknown bodies, and unknown row names. The restructure-check track's `check_staged`
replaces the gate behind the `StructuralGate` trait; the new-row analysis is the plan's,
not the gate's, and applies whichever gate is used.

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

S4, a delta stage with a **new form and a new template** (16 rows: the two new objects with their
`.0` bodies, their owner, ten module edits, `versions`; no `root`/`version`), native 39.6 s against
own 9.7 s in all (transaction 1.8 s); traced with the kit of track trace: native 195 write statements
in 24 transactions, own 17 in one (`docs/apply/evidence/own-apply/s4-write-families.md`). Own vs native end state (`tools\verify_new.py`):
`Config` **9 845 of 9 845 rows identical** (the four new rows and the owner's descriptor included);
`_ConfigChngR` 20 691 rows, the same (node, object) pairs and message numbers, ids unique;
`_ConfigChngR_ExtProps` identical for 16 349 objects; **all 16 `.si` rows have identical text**
(the main row with the two new records at the same places, `siVersions` the same entries);
`Params`, `Files`, `DBSchema`, `SchemaStorage`, `ConfigCAS` identical but for the documented
differences. Native `config check` succeeds on the own result, `generation-id` equals the native
twin's (`206601e511d02f4f844aa29ae70f045300000000`), the native `config apply` afterwards
says "Обновление конфигурации базы данных не требуется".

S5, the other shapes of new objects, each applied natively and by the own apply to twins of one stage
(`tools\verify_new.py`; a first attempt that cloned a data processor's form into a catalog was refused by
the native metadata check, «Ошибка формата потока», before it wrote anything -- the clone source matters,
not the apply):

- **S5A** (15 rows): a form with a help page (`.0` and `.1`) appended to a data processor that has forms
  and commands; the **first template** of a data processor that has a form and commands (its record goes
  between them); the **first form** of a data processor that has only attributes; the first template of
  a data processor **with no children at all**; a **help page added to an existing form** (`.1`).
  `Config` 9 851 rows, `_ConfigChngR` 20 697 rows, `_ConfigChngR_ExtProps` of 16 355 objects and all 16 `.si`
  rows (text) identical to the native result.
- **S5B** (4 rows): a **form of a catalog** (cloned from a form of that catalog) on an owner that is
  registered for five nodes: identical; the form is registered for the same three ordinary nodes as
  any other new object, and the native apply took the long path (a catalog's descriptor), so its `_IDRRef` differ.
- **S5C** (2 rows): an **object module added to a data processor** (a container row `.0` next to its
  existing `.1`): `Config`, `_ConfigChngR` and `_ConfigChngR_ExtProps` identical (the new file appended
  behind the existing one, as predicted), `.si` untouched by both; the native apply took the third path
  above and left `Params.DynamicallyUpdated`, which this apply now leaves too for such a stage.

Exclusivity with a real 1C process: an `ibsrv` (standalone server) started on a staged twin holds 23
connections; the apply refuses with the sessions listed, and a second apply started meanwhile is
refused by the in-transaction check (`THROW 57302`) -- both leave the database unchanged.

Session probe (`ibcmd_rs_04_apply_bsp8327_probe_20260929`, a clean БСП clone with a full-tree
stage of 9 517 rows -- the 227 S2 edits and a marker added to `ПоддерживаемыеВерсииПрограммногоИнтерфейса`
in `СтандартныеПодсистемыСервер`): a standalone server (`ibsrv`, integrated SQL login, `tools\srv.ps1`)
started cold on the staged, unapplied database answers the web service
`InterfaceVersion.GetVersions("UiProbe")` with status 200 and no version; after the own apply
(a rehearsal-free run, dry-run gate passed, 9 517 rows) a server started cold again answers 200 with
`ibcmd-rs-apply-probe-v1` and an empty error stream (`tools\probe_http.py`). The own apply ran
under load from other tracks' native runs (transaction 120 s against 5.6 s on an idle machine).

Also exercised: a rehearsal (`--rehearse`) leaves the database identical to its before-snapshot;
another session on the database makes the apply refuse with the session named; the recovery
artifacts verify against the before-snapshots (`tools\verify_recovery.py`: every manifest row equals
the snapshot row, every saved file hashes to its manifest entry).

## Limits and open points

- **New rows**: only a new form or template of an existing object (bodies `.0`, and `.1` for
  a form) and a body row an existing object gains are done; a new catalog, attribute, command,
  subsystem or any other object is structural or needs records this apply does not know, and
  is refused. Several new body rows of one object are refused (the order of their
  registration is not known). The importer's patch mode cannot stage a new form or template
  yet (`Config row not found`); a delta stage made by hand or by the base-free import can.
- **Sessions**: the 1C server (LocalSystem) has no SQL login on this machine, so a clone cannot
  be registered in the cluster and `rac`/COM sessions cannot be tested (creating the login is a
  security change this track does not make: `tools\session_probe.ps1` runs the check once one
  exists). A standalone server (`ibsrv`) started cold on the result is the substitute: it is a
  new process reading the tables, and it sees the applied module (probe above). A **warm**
  server does not notice an external apply within minutes (ui track), which is why the
  apply demands exclusive access.
- **Dynamic-update overlays in `Params`** (a `.si` row under a `_dynupdate_` name, left by a native
  dynamic apply) are refused: this apply folds only `Config` overlays.
- **8.5** is refused: the apply is measured on 8.3.27 only.
- The help index and the extension CAS garbage are left as they are; they are caches.
- A working process that keeps a pooled connection makes the SQL exclusivity check refuse; the
  native standalone `ibcmd` does not check at all.
