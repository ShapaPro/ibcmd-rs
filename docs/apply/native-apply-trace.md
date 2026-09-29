# What native `config apply` writes: traces on the БСП demo, platform 8.3.27

Issue [#336](https://github.com/Untru/ibcmd-rs/issues/336) ("Трасса штатного `config apply`: что пишется кроме
Config"), track "trace", 0.4 «Своё применение конфигурации». Companion of `native-infobase-create.md` (#342) and
of `restructuring.md` (#341, track "ddl": the DDL and the `SchemaStorage` protocol, **not repeated here**).

Scope of the measurements: platform `ibcmd` 8.3.27.2214, Microsoft SQL Server 2025 (17.0.1135.8), the БСП demo
configuration with its extensions (2 234 tables in the database) restored from the lab corpus, 2026-09-29. Every number is
**measured** unless the text says **hypothesis**. Not measured yet (planned as the next phase of the track): ERP УХ,
platform 8.5, multi-part rows (> 10 MB), extensions being changed by the apply, `--dynamic=auto`.

The traces were made with the capture kit `scripts/apply-trace/` (commits `b3f2bb11` .. `41dc713e` of this branch):
an Extended Events session on the database plus a row-by-row snapshot of every service table before and after.
The full captures (traces of 17 000 - 110 000 statements, snapshots, diffs) are in the lab folder
`F:\ibcmd\lab\04\trace\captures\` (not in the repository); the condensed evidence is in
`docs/apply/evidence/native-apply-8.3.27/` (section 10).

## 1. Summary

1. **An apply writes far more than `Config`.** For a change of five module texts (38 staged rows) the first apply
   of a clone made 12 833 writes to service tables, of which 12 160 are the garbage collection of `ConfigCAS`
   (first apply of a lineage only); a repeated apply of the same kind makes 351. All writes go through the file-row
   protocol of section 3.1; there is no bulk load except the change register.
2. **Fixed skeleton.** Every apply we traced but one (2n, the short path of section 6.4) runs the same five phases
   (section 3.2): prepare (copy of
   the staged rows to `Config` under `<name>.new`, name tables, mobile-client ring), bookkeeping and
   rebuild of the change register through "new generation" tables, the `commit` marker and the switch,
   promotion (`.new` -> final name, `.sinew` -> `.si`), tail (`.ui`, help index). What changes with the change
   is only the *content* of a few rows (section 4) and, for structure changes, the DDL of `restructuring.md`.
3. **Six groups of rows are rewritten on every apply of an existing infobase, whatever the change**: `Params
   <guid>.ui` (3 rows), `Files MobileVersions.dat` (a 1000-GUID ring, one new random GUID), the 16 `Params
   <guid>.si` rows and `siVersions` (new random guids), `Files extd_props_cached/gc.mrk`, the help index
   `userDocs_ru.bin` / `userVocabulary_ru.bin` / `userPostings_ru.bin`, and the change register `_ConfigChngR`
   (+ `_ExtProps`) with **new row ids** (rebuilt through `..NG` tables even when nothing changed). `DBSchema` and
   `SchemaStorage` are rewritten too (with identical content when the structure is unchanged).
4. **The crash protocol is a marker row**: `Config` row `commit` (empty) is written after everything is prepared and
   deleted after everything is promoted; the promotion of the staged rows runs in autocommit, row by row, **outside
   any transaction** (19 230 statements, 45 s for 9 615 staged rows). `dynamicCommit` (exclusive: absent)
   holds the list of names in a dynamic apply.
5. **Exclusive and dynamic (`--dynamic=force`) differ only in naming and two rows** (section 5): staged rows go to
   `<name>_dynupdate_<generation>` instead of replacing `<name>`; a new overlay of 16 `.si` rows is added; the
   change register gets `_MessageNo = NULL` for the changed objects; `dynamicCommit`, `dbStruFinal` and
   `DynamicallyUpdated` are written. The structure phase and the register rebuild are the same in both.
6. **Case 3 works end to end.** Native `infobase create` -> our import of the whole native export -> native `apply`
   -> native `export` gives an export identical to the reference (12 197 files identical, `ConfigDumpInfo.xml`
   differs only in the 9 835 `configVersion` values). The first apply of such an infobase ends with exit code
   -1 and «Использование быстрой вставки недопустимо без удаления индексов» on the last statements, after
   everything was committed; a second apply is a no-op with exit 0 (section 6.3).
7. **Dates.** Native writes `Creation`/`Modified` with the `_YearOffset` (+2000 years: `4026-09-29`); the rows our
   import stages carry `2026-...`. `infobase create` itself writes real years. (Section 8.)
8. Findings for other tracks and the open questions are in sections 8 and 9.

## 2. Method and cases

Every case = a clone (`ibcmd_rs_04_trace_<suffix>`, the suffix is in the table) restored from the БСП 8.3.27
corpus; staging by **our** tool or by
**native** `ibcmd infobase config import files --partial`; one native `config apply --force` under the kit
(`capture.ps1`: snapshot, XE trace, snapshot, diff). The native apply always ran with `--user=Администратор` except
on the created infobase (no users). The wrappers took the lab locks (`heavy`, and since 11:30 `native`).

| id | case | staged by | apply | database | wall | statements | capture (lab) |
|---|---|---|---|---|---|---|---|
| E01 | baseline (pilot): no-op import of the whole tree (patch mode, 9 517 rows) | ours | `--dynamic=disable` | `_noop` | 122.6 s (trace window; exit not recorded, the log ends with success) | 60 172 | `20260929-101302-e01-noop-import-apply` |
| 1x | **case 1**, five module texts (common module, catalog object module, document manager module, managed form module, common form module), first apply | ours (`mssql-stage-source-objects`, 38 rows) | `--dynamic=disable` | `_c1_disable` | 108.8 s | 17 126 | `20260929-105417-c1-disable-exclusive` |
| 1d | case 1, the twin restored from the same backup | ours, byte-identical rows | `--dynamic=force` | `_c1_force` | 73.1 s | 17 163 | `20260929-110125-c1-force-dynamic` |
| 1x' | case 1, second edit of the same modules on the database after 1x (steady state) | ours | `--dynamic=disable` | `_c1_disable` | 30.2 s | 4 627 | `20260929-115633-c1b-second-exclusive` |
| 1d' | the same on the twin after 1d | ours | `--dynamic=force` | `_c1_force` | 33.0 s | 4 700 | `20260929-115935-c1b-second-dynamic` |
| 2s | **case 2**, String(25) attribute in `Catalog._ДемоВидыНоменклатуры`: staging | native, one file (9 rows) | - | `_c2_attr` | 6.0 s | 477 | `20260929-111517-c2a1-native-stage-attribute` |
| 2a | case 2, apply of that stage | native | `--dynamic=disable` | `_c2_attr` | 110.2 s | 16 912 | `20260929-111719-c2a2-native-apply-attribute` |
| 2n | case 2, a stage from our patch-mode import that carried **no** attribute (it drops it) | ours (9 rows) | `--dynamic=disable` | `_c2_attr` | 45.1 s | 2 322 | `20260929-110735-c2a-attribute-added` |
| 2ns | case 2, new catalog `ТрассаНовыйСправочник` (no attributes) + `Configuration.xml`: staging | native (9 615 rows) | - | `_c2_attr` | 362.6 s | 89 712 | `20260929-112108-c2b1-native-stage-new-catalog` |
| 2c | case 2, apply of that stage | native | `--dynamic=disable` | `_c2_attr` | 157.3 s | 52 077 | `20260929-113311-c2b2-native-apply-new-catalog` |
| 3 | **case 3**, the whole tree into a created infobase: our import 37.2 s, apply, retry, export | ours (base-free by itself) | `--dynamic=disable` | `_c3_new` | 123.5 s (exit -1) + 6.7 s | 87 128 + 416 | `20260929-114021-c3-ours-import`, `-114151-c3-native-apply`, `-115512-c3-native-apply-retry` |

Staging mode of case 2: the attribute and the new catalog were staged with the **native**
partial import, because our **patch mode drops a new attribute silently** (2n: all 9 staged rows identical to
`Config` but for `versions`; the apply saw no structure change and took the short path of section 6.4). The
`--base-free` mode of our import was not run for case 2 (track ddl staged its structural cases that way and native
accepted it); case 3 is the base-free path by construction (an empty `Config`).

Two side effects of the lab worth knowing when reading the diffs:

* **First apply on a clone of the corpus** (1x, 1d, 2a): the clones were last touched by an older platform build. The
  apply also upgrades service tables (`_DbCopies` +`_StorageVariant`, `_DbCopiesUpdates` +3 columns), sets
  `ALLOW_PAGE_LOCKS = OFF` on the primary keys of eight extension tables (`ALTER INDEX ... _Reference10877X1`, ...)
  and collects the garbage of `ConfigCAS`/`Files` (12 797 -> 636 rows, 353 -> 44 rows). Later applies (1x', 1d', 2c)
  do none of this.
* **2c re-imported the whole tree**: with `Configuration.xml` in `--partial`, native staged 9 615 rows (every
  object gets a new version guid; the 517 role-rights rows gained an entry for the new object) and, because the tree
  `c2b` has no attribute, the apply **dropped** the `_Fld11034` column that 2a had added (`_Reference12` rebuilt). That
  is noise for the new-catalog case; the new catalog itself is the creation of `_Reference11035`.

## 3. The exclusive apply (`--dynamic=disable`), phase by phase

### 3.1 The file-row write protocol

Every row of `Config`, `ConfigSave`, `Params`, `Files` (and `ConfigCAS`) is written the same way (verbatim in the
create document, section 5):

```
SELECT COUNT(*) FROM <T> WHERE FileName = @P1                                   -- exists?
INSERT INTO <T> (FileName,Creation,Modified,Attributes,DataSize,BinaryData,PartNo) VALUES (@P1..@P7)   -- empty part 0, autocommit
BEGIN TRANSACTION
DELETE FROM <T> WHERE FileName = @P1 AND PartNo <> 0                            -- drop old extra parts
UPDATE <T> SET Modified=@P1, DataSize=@P2, BinaryData=@P3 WHERE FileName=@P4 AND PartNo=0   -- data (UPDATE ... PartNo=n for more parts)
COMMIT
```

`Modified` (and `Creation` of new rows) is `now + _YearOffset` (2000 years) with ten fractional digits in the
parameter (`4026-09-29 10:57:21.1640765552`). Parts hold at most 10 000 000 bytes. The help-index writer (E2) adds
three autocommit statements after the data transaction, `UPDATE Files SET Attributes = @P1 WHERE FileName = @P2`,
then the same for `Creation` and for `Modified` (explicit file info); the other writers do not. Renames are
`UPDATE <T> SET FileName = @new WHERE FileName = @old`; a promotion is `DELETE FROM <T> WHERE FileName = @P1
AND EXISTS(SELECT 1 FROM <T> WHERE FileName = @P2)` followed by that `UPDATE` (both autocommit).
Tables are read with `SELECT Creation,Modified,Attributes,DataSize,BinaryData FROM <T> WHERE FileName = @P1 ORDER BY PartNo`.

### 3.2 The phases

Times are from 1x (108.8 s) with the steady state 1x' (30.2 s) in brackets. The command opens about a dozen
connections, most of which only run `SET` statements; the writes of phase A come from one connection, everything
from B on from another (the session id changes at 11.4 s).
`write-phases.md` of each capture (in the evidence) lists the same blocks in order.

| # | phase | writes (table: row name, size, format) | tx |
|---|---|---|---|
| A0 | **check** (0 - 9.4 s; 0 - 9.5): "Проверка корректности метаданных", thousands of single-row `SELECT ... FROM Config/ConfigSave WHERE FileName = @P1` | none | - |
| A1 | first row | `Params <guid>.ui` (`54ba7662-...ui`, 9 bytes, unchanged content: only `Modified` moves) | 1 tx |
| A2 | clean-up | `DELETE FROM ConfigSave WHERE FileName LIKE '%.new'`, `DELETE FROM Config WHERE FileName LIKE '%.new'` (leftovers of an interrupted apply) | autocommit |
| A3 | **copy of the staged rows** (9.56 - 9.70 s) | for every `ConfigSave` name: `DELETE Config '<n>.new' (if it exists)` + `INSERT Config SELECT '<n>.new', Creation, Modified, Attributes, DataSize, BinaryData, PartNo FROM ConfigSave WHERE FileName = '<n>'`; in FileName order, each object as `<guid>` then `<guid>.<n>` ascending, then `root`, `version`, `versions` (38 rows: 14 `<guid>`, 21 `<guid>.<n>`, 3) | autocommit |
| A4 | mobile-client ring | `Files MobileVersions.datNEW`: INSERT empty, then UPDATE 37 009 B (section 4.3) | 1 tx |
| A5 | name tables, pass 1 | `Params DBNames.New` (143 619 B stored), `DBNames-Ext-1.New` (20), `DBNames-Ext-347eea02-....New` (954), `DBNames-Ext-b3fa0ef0-....New` (17 817), each INSERT + DELETE + UPDATE; then `DELETE FROM Params WHERE FileName = 'DBNames.New' AND FileName <> 'Params/DBNamesVersion-DBNames'` (and the three others): pass 1 is thrown away | 4 tx |
| B0 | connection 2: `SELECT SchemaID, Status FROM SchemaStorage WHERE SchemaID <> 0`, `_DbCopies` read, `_ExtensionsRestructNGS` DELETE (0 - 3 rows) | | |
| B1 | **rebuild of the change register** (11.5 - 18.0 s [14.2 - 16.1]) | `CREATE TABLE _ConfigChngRNG`, `_ConfigChngR_ExtPropsNG` (+ 3 indexes each, dropped and created again around the load), `UPDATE SchemaStorage SET NewGenCreated = @P1, Status = 200`, `insert bulk` of all rows (20 685 + 21 357 rows in batches of 10 000; the source is read through 1C temp tables `#tt`), `_ExtensionsRestruct` DELETE + INSERT ×2 | 2 tx |
| B2 | extension bookkeeping (18.0 - 22.6 s) | `Files MobileVersions.datNEW` rewritten twice (new head GUID each time), `_ExtensionsRestructNGS` DELETE + INSERT (2 tx) twice, `Files extd_props_cached/gc.mrk` (9 B, section 4.3) | 1 tx per write |
| B3 | **ConfigCAS garbage collection** (22.9 - 96.8 s; **first apply of a lineage only**) | `DELETE FROM ConfigCAS WHERE FileName = @P1` ×12 160 (sha1 names), `DELETE dbStruFinal<guid>` (1); then in **one** transaction 309 `DELETE FROM Files` (`userDocs_ru_<sha1>.bin`, `userPostings_ru_<sha1>.bin`, `userVocabulary_ru_<sha1>.bin`, 103 each) and `Files CAS_GC_Info` rewritten (`{0,20260929105718}`) | 1 tx |
| B4 | name tables, pass 2, and the `.sinew` cache (98.2 - 103.5 s [18.5 - 25.1]) | `Params DBNames*.New` again (same protocol), then `Params <guid>.sinew` ×16 (16 names, 443 B .. 2.9 MB, 4.06 MB in all), one tx per row; `DBNames*.New` deleted if equal to the current row (they are, for module edits) | 20 tx |
| C1 | **commit marker** (103.7 s [25.3]) | `Config commit` INSERT empty, DELETE, UPDATE (0 bytes) | 1 tx |
| C2 | switch of the register (103.7 - 105.5 s [25.3 - 26.5]) | `UPDATE SchemaStorage SET Status = ...`, `drop table _ConfigChngR`, `drop table _ConfigChngR_ExtProps`, `UPDATE SchemaStorage`, 5 × `exec sp_rename '_ConfigChngRNG', '_ConfigChngR', 'object'` (2 tables, 3 indexes; each via `sp_prepexec` in its own tx), `ALTER INDEX <pk> ON _ConfigChngR SET(ALLOW_PAGE_LOCKS = OFF, ALLOW_ROW_LOCKS = ON)` (first apply of a clone: + 8 on the `X1` tables), `UPDATE SchemaStorage SET Status = 100, CurrentSchema = @P1, NewGenCreated = @P2, NewGenDropped = @P3` (`@P1` = the 977 476-byte schema text, the trace keeps the first 524 186), `UPDATE DBSchema SET SerializedData = @P1` (same text) | 5 tx |
| D1 | **promotion in `Params`** (105.6 - 106.2 s) | `DELETE <guid>.si` ×16 + `UPDATE <guid>.sinew SET FileName = '<guid>.si'` ×16; `siVersions` DELETE + UPDATE (1 273 B); `DELETE DynamicallyUpdated`; when the names changed: `DELETE DBNames` + `UPDATE DBNames.New -> DBNames`, `DBNamesVersion-DBNames` DELETE + UPDATE (43 B, `{0,<guid>}`) | 1 tx |
| D2 | **promotion in `Config`** (106.2 - 106.5 s; 45 s for 9 615 rows) | (1) first the rows of pending overlays of earlier dynamic updates: `DELETE '<n>'` + `UPDATE '<n>_dynupdate_<g>' -> '<n>'` (the merge), `DELETE versions_dynupdate_<g>`, `DELETE deleted_dynupdate_<g>`; (2) for every staged object, the `<guid>.<n>` rows ascending and the `<guid>` descriptor **last**: `DELETE FROM Config WHERE FileName = '<n>' AND EXISTS(<n>.new)` then `UPDATE Config SET FileName = '<n>' WHERE FileName = '<n>.new'`; (3) `root`, `version`, `versions`; (4) `DELETE Config DynamicallyUpdated`. `deleted.new` is deleted without being promoted | **none** (autocommit) |
| D3 | clean-up | `DELETE FROM ConfigSave WHERE FileName LIKE '%'`; `Files MobileVersions.dat` DELETE + `UPDATE ... datNEW -> dat`; `DELETE Config commit`, `dynamicCommit`, `dbStruFinal` (three autocommit deletes, absent rows included) | autocommit |
| E1 | user rows (106.6 s) | `Params 789702c6-....ui` (24 301 B) and `97f2c291-....ui` (94 B) rewritten, one tx each | 2 tx |
| E2 | **help index** (107.0 - 107.4 s) | `Files userDocs_ru.new` (46 KB), `userVocabulary_ru.new` (103 KB), `userPostings_ru.new` (512 KB), each: `DELETE FROM Files WHERE FileName = 'userX_ru.new'`, INSERT empty, (tx) DELETE + UPDATE data, then the three file-info UPDATEs of 3.1; afterwards, for the three: `DELETE userX_ru.bin` + `UPDATE userX_ru.new SET FileName = 'userX_ru.bin'` | 3 tx |

The whole run has 48 user transactions (43 `BEGIN TRANSACTION` batches and the 5 `sp_rename` calls); every
explicit transaction wraps one file-row write (the `DELETE ... PartNo <> 0` + `UPDATE` pair), the `.si` promotion,
one `sp_rename` or the CAS clean-up. Nothing spans phases: an interrupted run is resumed from the markers, not
rolled back (section 3.3).

### 3.3 Markers and states

| row / value | meaning (measured) |
|---|---|
| `Config` `<n>.new` | a staged row copied to `Config`, waiting for the switch |
| `Config` `commit` (0 bytes) | everything is prepared and the register is being switched; present from C1 to D3 |
| `Config` `dynamicCommit` | dynamic apply only: `{N,"<name>",...}` all N staged names (38 in 1d), present C1 to D3 |
| `Config` `dbStruFinal` (0 bytes) | dynamic apply only, present between the `.si` overlay and D3; deleted at the end of every apply |
| `Config` / `Params` `DynamicallyUpdated` | the list of overlay generations of dynamic updates (section 5) |
| `SchemaStorage.Status` 100 -> 200 -> 400 -> 500 -> 100 | the structure protocol of `restructuring.md` section 3.2 |
| `ConfigSave` rows | the input; the last statement of D3 empties the table |

Track ddl measured that a run interrupted after the switch of the schema and before the promotion leaves a database
that refuses every `ibcmd config` command (their section 10): the `commit` marker and the `.new` rows are what the
platform resumes from, and native does not resume when they are inconsistent.

## 4. What is written, table by table

Case columns: **fixed** = the same in every apply of an existing infobase; **content** = depends on the change.
The complete matrix (statements / rows / bytes per family and case) is `write-families.md` in the evidence.

### 4.1 `Config`, `ConfigSave`, `ConfigCAS`

* **Copy and promotion** (A3, D2): every staged row, changed or not; 38 rows in case 1, 9 615 in 2c. New content in
  `Config` = the staged bytes (`INSERT ... SELECT`, `RENAME`): the apply does not touch the payload. Rows of
  `Config` keep their `Creation`/`Modified` of the staging (native: `4026-...`, ours: `2026-...`).
* **`versions`**: staged; the header uuid (`{1,<count>,"",<generation>,...}`) is the "Создано поколение
  конфигурации" line of the log. The apply does not modify it (1x: `848a0a59-...` -> `bd7d351c-...`, the value the
  staging wrote).
* **Deleted rows**: none in a plain apply. 1x deleted six rows only because the clone carried a pending overlay of an
  earlier dynamic update (`<guid>_dynupdate_06cb0442-...` ×4, `versions_dynupdate_...`, `DynamicallyUpdated`).
* **`ConfigSave`** ends empty (D3). An apply after a *partial* native import (track ddl saw a `ConfigSave` of 9 597 -
  9 635 rows and a native apply that died at «Принятие изменений») is a real risk, but **the row count alone does
  not tell**: the complete native stage of 2ns has 9 618 rows against 9 841 in `Config` (a stage holds only the rows
  whose version guid changed, and here 9 615 of the 9 840 entries of `versions` did). The check used here, all
  measured on 2ns: no `commit` row and no `%.new` rows in `ConfigSave`, `root`, `version` and `versions` present,
  and every entry of the staged `versions` whose guid differs from `Config`'s has a `ConfigSave` row (9 615 of 9 615).
* **`ConfigCAS`** (B3): garbage collection of the entries no extension version refers to (12 797 -> 636 rows, sha1
  names, one statement per row, 74 s). Not repeated on a collected database. What "unreferenced" means is not decoded.
* **`Config` `deleted`** (`ConfigSave deleted`, 45 B text): written by the native import when objects disappear from
  the tree; `1,"4e84d26f-8ca9-45dd-b1f3-8037eb78f6bd",1` in 2ns is the attribute that 2a had added and the tree
  `c2b` lacks. It is copied as `deleted.new` (A3) and then **deleted, not promoted** at the start of D2; it drives
  the removal of the column (`_Reference12` rebuilt without `_Fld11034`), and `DBNames` keeps the entry.

### 4.2 `Params`

| row | fixed / content | size | format and derivation |
|---|---|---|---|
| `<guid>.ui` ×3 | fixed | 9 B, 24 301 B, 94 B | `54ba7662-...` is rewritten with the same 9 bytes (`Modified` only). `789702c6-...` and `97f2c291-...` are Base64 text (lines of 64 characters) whose content changes on every apply with the same size (**hypothesis**: re-encrypted with a fresh IV); decoding belongs to track ui |
| `DBNames`, `DBNames-Ext-1`, `DBNames-Ext-<guid>` ×2 | content | stored 143 619 B, 20 B, 954 B, 17 817 B (inflated 347 167 B, 18 B, 2 309 B, 40 882 B) | raw deflate of `{<max>,{<count>,{<guid>,"<kind>",<number>},...}}`: the numbering of metadata objects and fields (`Fld`) and of platform tables (nil guid: `WebSocketClients`, `DbCopies...`). **Append-only**; the number counter is global over the main and all extension `DBNames`: 2a: `{4e84d26f-...,"Fld",11034}` (11034 = max over `DBNames-Ext-b3fa0ef0-...` (11033) + 1; the main header said 10824), 2c: `a51bda0b-...` "Reference" 11035 (the new table `_Reference11035`) and the three new platform tables 11036 - 11038 (header 11039). Rewritten only when an entry was added (module edits: identical, `.New` thrown away) |
| `DBNamesVersion-DBNames` | content | 43 B | `{0,<guid>}`, a new random (v4) guid whenever `DBNames` changed; the `-Ext-` rows never changed |
| `<guid>.sinew` -> `<guid>.si` ×16 | fixed | 295 B .. 3.0 MB | raw deflate of `{2,{1,<guid>,5,...}}` / `{0,{27,...}}`: indexes of the metadata (type patterns `{"Pattern",{"#",<type guid>}}`; `1a621f0f-....si` is the list of all metadata objects, entries `<guid> <parent guid> <kind code> "<name>" {1,1,{"ru","<synonym>"}} 0 0` with the entry count in the header: the added attribute made `10807 -> 10808` and one entry `4e84d26f-... eeaa0c4f-... 36 "ТрассаРеквизит" ...`), 16 rows with fixed names (the same 16 in the created infobase). **Content is identical to the previous apply when only modules change** (1x: 16 of 16 "same content, other metadata"); 2 of 16 changed for the attribute (2a; `1a621f0f-....si` grew by 41 B) and 9 of 16 for the new catalog with the removed attribute (2c). The rows are rebuilt and written on every apply |
| `siVersions` | fixed | 1 273 B | `{0,16,"<guid>.si",<guid>,...}`: **a new random (v4) guid for every one of the 16 entries on every apply**, entries reordered |
| `DynamicallyUpdated` | fixed (deleted; written in a dynamic apply) | 82 B .. 119 B | section 5 |
| `evlogparams.inf`, `ibparams.inf`, `locale.inf`, `log.inf` | not touched | | written by `infobase create` only |

### 4.3 `Files`

| row | fixed / content | size | format and derivation |
|---|---|---|---|
| `MobileVersions.dat` (`.datNEW` -> `.dat`) | fixed | 37 009 B (1000 guids) | `{<n>,<guid>,<guid>,...}` a **ring of at most 1000 version guids**; each apply puts one **fresh random (v4) guid** in front and drops the last (`after[1:] == before[:-1]` in 4 of 4 applies). `.datNEW` is written three times (a new head guid each time; the third is the result). The created infobase starts with `{1,<guid>}` (43 B) |
| `extd_props_cached/gc.mrk` | fixed | 9 B | `0x11` + a little-endian 64-bit **time in 1/10 000 s since 0001-01-01**, real local time of the apply (`11 20 12 d3 f5 67 45 02 00` = 2026-09-29 10:56:02). Decoded from eight samples of five applies, all equal to the `Modified` of the row minus 2000 years |
| `CAS_GC_Info` | with the CAS GC | 21 B | `{0,20260929105718}` local time `YYYYMMDDhhmmss` of the GC |
| `userDocs_ru.bin` / `userVocabulary_ru.bin` / `userPostings_ru.bin` | fixed (content: see below) | 46 KB / 103 KB / 512 KB stored (229 KB / 516 KB / 1.0 MB inflated) | raw deflate of UTF-16LE text: documents (`244192<TAB>22<TAB>6ba07682...`), a vocabulary (`0 187 0`, `00 70 187`, ...), postings. The log line is «Построение индекса справки». Stored size 46.2 / 103.1 / 511.9 KB in 1x, 1d, 1x', 1d' (46.4 KB for the first file in 2n and 2a); **2.2 / 8.6 / 4.9 KB in E01 and 2 bytes each in 2c** (the two applies that moved ~9 600 staged rows), and absent after the failed first apply of the created infobase. **Derivation open** (section 9) |
| `userDocs_ru_<sha1>.bin` ... | with the CAS GC | 2 B each | the per-content parts of the same index, deleted by B3 (103 of each) |
| `dbcopiesparams` | not touched | | written by create |

### 4.4 Data tables and structure

* **`_ConfigChngR` (+ `_ConfigChngR_ExtProps`)**: rebuilt **on every apply**, also with no change and also in a
  dynamic apply (B1, C2). `_IDRRef` of all rows is new (a sequential run from a fresh base value, e.g.
  `8F5B00E04C68009311F1BBDB32A05000`, `...05001`, ...); `(_NodeTRef, _NodeRRef, _MDObjID)` is the key and does not
  change; the `..NG` DDL is `restructuring.md` section 3.3. **`_MessageNo` after the rebuild is NULL exactly for the
  objects whose descriptor row `<guid>` was in the staged set of this apply, and 0 for all others** (measured in
  six applies, the counts match to the row: the register has five nodes; per large node 1x has 14 NULL = the 14
  staged descriptors, 2a 2 NULL, 2c 4 930 NULL = the 4 930 staged descriptors; the two small nodes hold the subset
  of those objects they register: 3, 0, 1 270; in the steady state 1x'/1d' nothing flips because the same objects
  are staged again). The clone's registers came with NULL for 17 279 objects, so the first apply shows `NULL -> 0`
  for them (1x, 1d, 2a: 17 279 / 17 279 / 17 321 rows) and 2c, which staged nearly everything, `0 -> NULL`
  (17 321 rows). A **new object** is registered by the apply at three of the five nodes with `_MessageNo = NULL`
  (2c: `0BDA1BA5-1C1B-3149-80D4-6AB10522DE81` = `a51bda0b-...`, three new rows in `_ConfigChngR`, none in
  `_ExtProps`); which objects and nodes a register holds beyond that is not decoded. The dynamic apply also
  runs `UPDATE T2 SET _MessageNo = CAST(NULL AS NUMERIC(38,8)) FROM _ConfigChngR T2 WHERE _MDObjID IN (<the 14
  changed objects>) AND _IDRRef IN (SELECT RS_FIELD FROM #tt6)` (49 rows), which the rebuild rule already implies.
* **`_ExtensionsRestruct`, `_ExtensionsRestructNGS`**: bookkeeping of the extensions' restructure state: DELETE and
  INSERT of 1 - 3 rows per step (`_ExtDataID` = the extension, `_RestructData` a blob of 444 - 13 890 B); after the
  apply `_ExtensionsRestructNGS` holds three rows.
* **`SchemaStorage`, `DBSchema`**: rewritten with the same schema text when the structure did not change (1x, 1d:
  the rows hash equal); with the new text when it did (2a: 977 751 B, 2c: 979 610 B). `restructuring.md` section 3.2.
* **New platform tables** on the first apply of an older clone: `_DbCopiesInfoBaseUse`, `_DbCopiesUpdateStat`,
  `_DbCopiesUpdateTableStat`, `_WebSocketClients` (2c), `_DbCopies`/`_DbCopiesUpdates` altered (2a). In a created
  infobase `_DbCopiesInfoBaseUse` gets one data row `{_Id = random guid, _Description = "<computer> : DefAlias"}`.

## 5. The dynamic apply (`--dynamic=force`)

Same phases A - E with these differences (1d against 1x, 1d' against 1x'):

| step | exclusive | dynamic |
|---|---|---|
| A3 copy | `<n>.new` | the same |
| C1 marker | `commit` | `commit` and `dynamicCommit` (`{38,"<n>",...}`) |
| D1 `.si` | `.sinew` -> `<guid>.si` (old rows replaced) | `.sinew` -> `<guid>_dynupdate_<gen>.si`: **16 more rows** (Params 39 -> 55); the old `.si` and `siVersions` stay valid; `siVersions` is still rewritten |
| after D1 | | `Config dbStruFinal` (empty), `Params DynamicallyUpdated` `{0,3,<base gen>,<previous overlay>,<gen>}` (119 B), `UPDATE _ConfigChngR SET _MessageNo = NULL` for the changed objects |
| D2 promotion | `<n>.new` -> `<n>` | `<n>.new` -> `<n>_dynupdate_<gen>`; `versions.new` -> `versions_dynupdate_<gen>`; `root`, `version` as in exclusive; `Config DynamicallyUpdated` `{1,2,<previous overlay>,<gen>}` (82 B) |
| result | 9 844 -> 9 838 rows | 9 844 -> 9 880 rows (+36 overlay rows: 21 + 14 + `versions`), then +36 again per further dynamic apply (9 916 after 1d'); Params +16 per apply |
| the register rebuild, `sp_rename`, help index, `.ui`, ring | as exclusive | as exclusive |

`<gen>` is the header guid of the staged `versions` row (the same in 1x and 1d, since the twin restored the same
staged rows). Overlay rows accumulate until an **exclusive apply merges them** (rename to the base name, or delete
when a newer staged row replaces them; 1x did that for the clone's old overlay). Timing: 73.1 s against 108.8 s
(first apply; both include the CAS GC, the load of the machine differed between the runs), 33.0 s against 30.2 s
(steady state): no measurable difference in the work itself.

## 6. The cases

### 6.1 Case 1: modules only (1x, 1d, 1x', 1d')

* Staged: 38 `ConfigSave` rows for five modules (14 descriptors `<guid>`, 21 `<guid>.<n>`, `root`, `version`,
  `versions`). The five modules: common module `06427cd3-....0`, document manager module `08c4ceb3-....2`,
  catalog object module `28e59c50-....0`, managed form module `a12cfe30-....0`, common form `323997ca-....0`. In the
  steady state (1x') **exactly** these five containers (`container element text changed`) and `versions` differ from
  `Config` (6 rows); the other 32 staged rows are identical.
* On the first apply (1x) 11 staged rows differed in content from `Config`: the five module rows, `versions`, and
  five managed-form rows (`04332cbc-....0`, `1f671d47-....0`, `3b52d3f8-....0`, `73338f66-....0`, `81b9c9c4-....0`)
  where **our writer produces other properties than the platform stored** (e.g. added `15 {"U"}` and `19 {"S" ""}`,
  counters `11 -> 13`); a further 16 rows differed only in the deflate stream and 8 only in line breaks. Once
  these rows are in the database (1x') the diff is clean: findings for the import track, section 8.
* What is written: everything of section 3.2; 12 833 service writes in 1x (12 160 of them CAS GC, 309 `Files` GC),
  351 in 1x'. Statements: 17 126 (4 627), user transactions 48 (47), DDL statements 27 (19) of which 5 renames:
  the register only, plus in 1x the eight primary-key lock options.
* Time, from the blocks of `write-phases.md`: 108.8 s = 9.4 s check + 2.0 s prepare + 11.2 s register and
  bookkeeping + 73.9 s CAS GC + 1.4 s `Files` clean-up + 5.3 s names and `.sinew` + 3.6 s switch, promotion and
  tail. Steady state 30.2 s = 9.5 s check + 1.3 s prepare + 4.4 s register and bookkeeping (behind a 3.3 s gap
  while the second connection starts) + 6.6 s names and `.sinew` + 3.5 s switch, promotion and tail.
* Twin comparison (1x against 1d, restored from one backup, measured on the `after` snapshots): the row sets
  written are the same except section 5; all 36 overlay rows of 1d, its `versions_dynupdate_...` (344 223 B) and its
  16 overlay `.si` rows equal, decoded, the rows 1x wrote under the base names.

### 6.2 Case 2: an attribute (2s, 2a), a new catalog (2ns, 2c)

**Staging by native import** (for reference; the import track will want it): one file, 9 rows in `ConfigSave`
(2s, 6.0 s): `<guid>.<n>.new` ×4, `<guid>.new` ×2, `root.new`, `version.new`, `versions.new` (340 595 B, the whole
row), `commit`, each through INSERT + DELETE + UPDATE in its own tx, then renamed to the final names and `commit`
deleted; 10 tx. Our patch-mode stage of the same change has the same 9 rows but the catalog descriptor lacks the
attribute (2n).

**Apply of the attribute (2a)**: everything of 3.2, plus the DDL of `restructuring.md` for `_Reference12`
(`_Reference12NG` rebuilt with the new column `_Fld11034 nvarchar(25)`: 9 rows copied, 4 indexes, 4 renames, the
old table dropped), `_DbCopies` and `_DbCopiesUpdates` rebuilt for their new columns, `DBNames.New` promoted to
`DBNames` (one entry), `DBNamesVersion-DBNames` new, two of the 16 `.si` rows with new content, `DBSchema`
977 476 -> 977 751 B. Log: «Объект изменен: Справочник._ДемоВидыНоменклатуры»,
«Изменена структура таблиц базы данных». 110.2 s, 56 DDL statements (of which 14 renames).

**Apply of the new catalog (2c)**: the same plus `_Reference11035` (+ 3 indexes) and the three platform tables
created through `..NG`, 19 `sp_rename`, `DBNames` +4 entries, `_ConfigChngR` +3 rows; the staged 9 615 rows make the
copy (53 s, 19 231 statements) and the promotion (45 s, 19 230 statements) the dominant costs. Log: «Новый объект:
Справочник.ТрассаНовыйСправочник». 157.3 s.

### 6.3 Case 3: a new infobase

See `native-infobase-create.md` section 7 for the numbers. The apply of the whole tree into the created infobase:
the file-row protocol as above; the structure of 1 753 tables is created through `..NG` (5 507 `sp_rename`),
27 of the 72 tables of the create are rebuilt for the data-separation fields; **no CAS GC, no `.ui` rows, no
`DBNames-Ext`**; `MobileVersions.dat` is created as `{1,<guid>}`; `CAS_GC_Info` is written. The error at the end:

```
SELECT T1._Fld3051, T1._RecordKey FROM dbo._Const3050 T1 WITH(NOLOCK)                      -- 1 row
SELECT T1._Fld3051 FROM dbo._Const3050 T1 WITH(TABLOCK) WHERE DATALENGTH(T1._Fld3051) < 128
DELETE FROM T1 FROM dbo._Const3050 T1 WITH(TABLOCK)                                          -- 1 row deleted
-> «Ошибка SDBL: Использование быстрой вставки недопустимо без удаления индексов.»  exit code -1
```

(`_Const3050` = the constant `АккредитованныеУдостоверяющиеЦентры`, `varbinary(max)`, the platform re-inserts its
value with a fast insert into a table that has an index.) All promotions, the emptying of `ConfigSave` and the
deletion of the markers had been executed before. The retry logged 416 events and changed no table (exit 0, 6.7 s).
The export of the result matches the reference (section 1, item 6).

### 6.4 The short path (2n)

The staged rows of 2n (9 rows, no semantic change but `versions`) took **another route**: no «Обработка
структуры базы данных», no `..NG` tables, no register rebuild, no CAS GC, no `DBNames`; the log says «Сбор служебной
информации», «Принятие изменений», «Обработка данных Регистрация изменений в планах обмена» (twice) and the write
sequence is: `.ui`, `.new` copy, `MobileVersions.datNEW` once, `.sinew` ×16, `commit`, `.si` promotion, `dbStruFinal`,
`UPDATE _ConfigChngR SET _MessageNo = NULL` (7 rows), promotion (with the merge of a pending overlay), clean-up,
`.ui`, help index. 45.1 s, 2 322 statements. Both variants were called with `--dynamic=disable`. **Why this
path was taken and the module cases took the long one is not established** (section 9, question 1).

## 7. Which of it is required (classification, mostly hypothesis)

| kind | rows | evidence / hypothesis |
|---|---|---|
| the configuration | `Config` promotion (`root`, `version`, `versions`, staged rows), `DBSchema`/`SchemaStorage`, `DBNames` (+ version) | measured: what the platform reads back; the export of case 3 depends on it |
| protocol | `commit`, `dynamicCommit`, `dbStruFinal`, `.new` names, `Status` | measured: resume markers; without them a stopped run cannot be continued (track ddl) |
| derived caches | 16 `.si` + `siVersions`, the help index, `_ConfigChngR` ids | **hypothesis**: rebuilt by the platform when absent or stale; content unchanged for module edits. Not tested |
| bookkeeping | `MobileVersions.dat`, `gc.mrk`, `CAS_GC_Info`, `_ExtensionsRestruct*`, `.ui` | **hypothesis**: not needed for a server to open the configuration; the first three carry only times and random guids |
| garbage collection | `ConfigCAS`, `Files` clean-up | measured: unrelated to the change; first apply of a lineage |

The proposal in section 9 turns the hypotheses into a test: apply with the minimal subset and open the result with
native `config export` and `infobase config check`.

## 8. Findings other tracks need

* **Import.** (a) The rows our tools stage carry `Creation`/`Modified` in `2026`, the platform's `4026`
  (`_YearOffset`); after the apply `Config` holds a mix. (b) Patch mode stages 9 517 rows for a no-op (E01: 6 100
  differ only in compression, 2 661 only in formatting, **399 in content**) where native stages 9 rows for one
  attribute (2s); 1x showed five form rows with extra properties. (c) Native staging of `Configuration.xml` is a
  full re-import: 9 615 rows, 362.6 s with 50 sessions, 225 rollbacks, 9 841 transactions (2ns). (d) Adding a
  metadata object adds an entry to the rights rows of every role (517 rows in 2ns).
* **Apply.** Everything in section 3; the promotion must run outside a transaction row by row like native, or in
  a transaction if we accept a different recovery story; the `commit` marker before the switch is the contract with
  native tools.
* **Everyone who checks a native stage** (lab rule of 2026-09-29 11:30, "check the `ConfigSave` row count"): a complete stage can hold fewer rows than
  `Config` (2ns: 9 618 of 9 841, complete by the `versions` test of section 4.1), so a count near 9.6 thousand, as
  track ddl reported for its partial stages, is not proof of a partial stage by itself; use the `versions` test.
* **Track ddl.** (a) The number counter of `DBNames` is global over the main and the extensions' `DBNames`;
  the main header may lag (10824 while the extension counter is at 11033); the header after 2c (11039) exceeds the
  largest entry (11038) by one, cause unknown. (b) `sp_rename` runs through `sp_prepexec` (the kit reads it now).
  (c) The register is rebuilt in all four kinds of apply. (d) A created infobase needs 27 service tables
  rebuilt for the data-separation fields.
* **Track ui.** Three `.ui` rows per apply: one touched, two re-encrypted with the same size (section 4.2).
* **Create (#342).** The first apply on a created infobase fails on `_Const3050` after committing everything
  (section 6.3); anyone replaying `infobase create` + our apply must not copy that failing statement.

## 9. Open questions

1. **Why did 2n take the short path?** Hypotheses: (a) no staged row differs semantically from `Config` ("nothing to
   restructure"), (b) no module text changed. 1x' differs from 2n by five module edits (long path), 2a by an
   attribute (long). Proposed test: on a fresh clone stage (i) one module edit only, (ii) a descriptor edit with
   no structure effect, (iii) nothing; apply each natively and read the log lines.
2. **`_ConfigChngR`**: the `_MessageNo` rule of section 4.4 is exact on six applies (1x, 1d, 1x', 1d', 2a, 2c); open: which objects and nodes a
   register holds at all (20 685 rows, five nodes; a new object went to three nodes), and the generator of the
   row ids.
3. **`DBNames` header** after 2c (11039 with entries up to 11038); the allocation order of the three platform tables.
4. **`.si`**: which metadata property changes which of the 16 rows; whether a stale `.si` set is tolerated.
5. **Help index**: what feeds it. Its size is the same in four applies that moved 38 rows, but tiny (E01) or empty (2c)
   in the two that moved ~9 600 rows, although the help pages are in `Config` in all of them. Whether the
   created infobase gets it at its next apply.
6. **`ConfigCAS` GC**: the reference rule; does it run when nothing is unreferenced?
7. **The failed first apply** (6.3): is the `_Const3050` row needed (the constant's default value)? Does a second
   apply with a real change write the missing `.ui`/help index rows?
8. **Multi-part rows** and **extension changes** in an apply, `--dynamic=auto`, **8.5**, **ERP УХ**: not traced.

**Proposed next step for the apply track:** implement the exclusive apply for the "modules only" and "no structure
change" cases first (phases A1 - E2 with `commit`, promotion, the ring, the register rebuild), because the write set
is fully known (section 3.2, 1x'); use `compare_traces.py` to check our apply against a native oracle on a twin (the
diff of both `after` snapshots must show only random guids, timestamps and the ids of `_ConfigChngR`); test the
hypotheses of section 7 by dropping the cache writes one at a time.

## 10. Evidence and how to repeat

* Condensed evidence: `docs/apply/evidence/native-apply-8.3.27/write-families.md` (the matrix: every family of
  writes, per case; the `Config` copies `<n>.new` are left out with `--skip-config-copies`; columns E01,
  modules-excl = 1x, modules-dyn = 1d, modules-excl-2 = 1x', modules-dyn-2 = 1d', no-change = 2n, attribute = 2a,
  new-catalog = 2c, new-infobase = the first apply of 3; a cell is "statements / rows affected / payload bytes"),
  `write-phases-*.md` (the blocks of consecutive writes per case, in order).
* Captures (lab, not in the repository): `F:\ibcmd\lab\04\trace\captures\<id>\` with `trace\{summary,write-phases,
  timeline,groups}.md`, `service-writes.tsv` (every write in order), `payloads.jsonl`, `ddl.sql`, `diff\diff.md`,
  `before\`, `after\`; blobs in `F:\ibcmd\lab\04\trace\blobs`.
* Repeat one case: restore a clone (`F:\ibcmd\lab\04\tools\restore-clone.ps1`), stage, then
  `pwsh -NoProfile -File F:\ibcmd\lab\04\trace\scripts\run_native_apply.ps1 -Database <db> -Tag <tag> -Dynamic disable`
  (heavy and native locks, kit capture); compare runs with
  `python scripts/apply-trace/compare_traces.py --capture <label>=<capture dir> ...`.
* The case scripts: `run_native_import_files.ps1` (native staging with the row count check), `run_native_apply.ps1`,
  `run_case3.ps1`, `run_queue2.ps1` in `F:\ibcmd\lab\04\trace\scripts\`.
