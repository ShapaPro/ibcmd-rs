# The own restructure and the extensions (S1-I, #405)

Part of #391 (the minimal own restructuring, S1), sub-issue S1-I, track "ext". Everything below is **measured**
on the БСП 8.3.27 corpus clone (four extensions, platform 8.3.27.2214, SQL Server 2025) unless it says
**inference**. Code: `src/restructure/extensions.rs`, `mssql_dump::extension::adopted_objects`; the hooks in
`plan.rs`, `reader.rs`, `exec.rs`, `command.rs`. Evidence: `docs/apply/evidence/restructuring/s1i-*.txt`.
The design of S1 itself is `docs/apply/restructuring.md`, section 12 (branch `feat/0.4-restructure-s1`).

## 1. Summary

1. **An extension keeps its own storage beside the main one.** `SchemaStorage(1)` holds the schema of the
   extensions (243 table entries on the clone, physical tables with the suffix `X1`), the `Params` rows
   `DBNames-Ext-<registry id>` number their objects, and `_ExtensionsRestruct` / `_ExtensionsRestructNGS` keep
   bookkeeping. The counter of numbers is shared with the main `DBNames` (the allocation rule of the prototype,
   "one more than the largest header of all of them", was right: `Fld11034` after the headers 10824 / 10989 /
   7841 / 11033).
2. **An adopted object is paired with its base by identity, not by uuid** (kind and name): 47 of the 75 adopted
   objects of the four extensions carry the nil uuid where the header would name the base object, and the
   object has a uuid of its own (`Catalog._ДемоПартнеры` is `3014d9c1-...` in `_ДемоРасширение` and
   `5eab8a1b-...` in the configuration). A check by uuid finds none of them.
3. **What a change of an adopted object does natively depends on whether the extension extends it with data**
   (section 3):
   - an object **no extension adopts**: the platform rebuilds the main tables only; every table and row of the
     extensions is as it was (case n);
   - an object **adopted with no table of the extension's own**: the same (case a; also the earlier a2);
   - an object **adopted and extended with data** (the extension keeps `_Reference18X1` for it): the platform
     rebuilds the main table **and** the extension's table, adds the new field to the extension's schema entry
     and rewrites `SchemaStorage(1)` (case c). A rebuild of the main table alone leaves the extension out of step.
4. **What S1-I does**: the plan reads the objects the extensions adopt and **refuses** a change of an object an
   extension adopts (whatever the case above: the acceptance criterion is a refusal, section 4); it refuses when
   the extension schema is not idle or when the objects were not read; and the executor **proves** inside its
   transaction that nothing the extensions keep changed, and rolls back otherwise.
5. **On a twin of the БСП clone with four extensions** (section 5): an object no extension adopts passes and the
   result equals the native apply (tables, data, `Config`, `DBNames`, `DBSchema`, 14 of 16 `.si` rows, native
   export 12 198 / 12 198, native apply afterwards "не требуется", a cluster session), and the four extensions
   are as they were (243 `X1` tables, `SchemaStorage(1)`, `DBNames-Ext-*`, `_ExtensionsInfo`,
   `_ExtensionsRestruct`, and their native export 737 / 737); an adopted object is refused, dry run, trial and
   apply alike, and the database is exactly as it was.

## 2. The storage of the extensions

| what | measured |
|---|---|
| `SchemaStorage` | two rows, `SchemaID` 0 (main) and 1 (extensions), both `Status 100`, empty generations |
| the extension schema | 243 table entries; the tables `_<Name>X1`; a table an extension **owns** has a number from that extension's `DBNames-Ext-<id>`; a table of an **adopted object the extension extends** has the number of the main object (`Reference18` is `_ДемоНоменклатура`; counted by name over 221 of the entries: 142 owned, 78 of adopted objects, mostly registers and their change-registration tables, 1 of neither) |
| `Params` | `DBNames-Ext-<registry id>` and `DBNamesVersion-DBNames-Ext-<registry id>` only for the extensions that own tables (`_ДемоРасширение` header 7841 with 41 entries, `ServiceDesk` header 11033 with 753); `DBNames-Ext-1` is an empty list with the header 10989 |
| the attribute an extension adds to an adopted catalog | `Fld7840` of `_ДемоНоменклатура`: its number is in `DBNames-Ext-347eea02...`, its field sits in the entry `Reference18` of **`SchemaStorage(1)`** (`_Reference18X1`), never in the main entry |
| `_ExtensionsInfo` | four rows (the registry), unchanged by any apply |
| `_ExtensionsRestruct` | 57 rows, unchanged by a native apply |
| `_ExtensionsRestructNGS` | empty on the corpus clone; after the first native apply **three rows** (one per extension that owns tables: 13 890, 444 and 3 864 bytes, keyed by the registry id) which stay; the platform writes them again at every apply. They are derived state, not a marker of a restructure in flight (an earlier version of this check took them for one and would have refused every database that had ever been applied) |

The objects an extension adopts are read off its descriptor rows: the first header of a row named by the header's
own uuid, when its belonging is 1 (`mssql_dump::extension::adopted_objects`): the row, the own uuid, the name, and
the uuid of the base object when the header names one. Children (attributes, sections) sit in the same rows.

## 3. What native does when an adopted object changes

Three twins each, restored from one backup of the clone after a native baseline apply (an unchanged file
staged and applied: it folds the `_dynupdate_` rows a corpus clone carries), one string attribute added to a
catalog by a native `config import files --partial`, then the native `config apply --force --dynamic=disable`.
Full diffs: `s1i-native-adopted.txt`, `s1i-case-n-twins.txt`.

| case | object | adopted by | native rebuilt | extension state after |
|---|---|---|---|---|
| n | `Catalog._ДемоСтавкиНДС` (`Reference23`) | nobody | `_Reference23` | unchanged: `SchemaStorage(1)`, 243 `X1` tables, `DBNames-Ext-*`, `_ExtensionsInfo`, `_ExtensionsRestruct`, `_ExtensionsRestructNGS` |
| a | `Catalog._ДемоПартнеры` (`Reference20`) | `_ДемоРасширение`, no table of its own | `_Reference20` | unchanged (as case n; the earlier case a2 said the same) |
| c | `Catalog._ДемоНоменклатура` (`Reference18`) | `_ДемоРасширение`, which keeps `_Reference18X1` (46 rows) | `_Reference18` **and `_Reference18X1`** (`_Fld11034 nvarchar(50) NULL` in both) | **`SchemaStorage(1)` rewritten**: the entry `Reference18` of the extension schema gained `Fld11034` beside the extension's own `Fld7840`; the rows of `DBNames-Ext-*` unchanged |

So a change of an adopted object that the extension extends is **not** confined to the main tables, and the own
restructure has no model of the extension's entry. Whether an adopted object with no table of the extension's
own is harmless is what cases a and a2 say (for the addition of an attribute; the deletion of an attribute the
extension adopts, a change of the type, and every other operation are **not traced**).

## 4. What S1-I does

- **Refuse** (`extensions::check`, called by `plan` right after the changed object is known): the object is
  adopted when an extension's adopted header has its uuid, names its uuid as the base object, or has its name
  (kind-agnostic: the kind is not read off an extension row, so a same-named object of another kind refuses
  too, which is the safe side). Staged images of the extensions count as well as the active ones. The message
  names the extension, the adopted object and image, and the tables of its own the extension keeps for the object
  (`_Reference18X1`, sub-tables included), because those are what a rebuild would leave out of step.
- **Refuse** when the infobase has extensions and the plan was given none of their objects (a reader that fills
  `Inputs` must fill `extensions.adoptions` too: `read_adoptions`), and when `SchemaStorage(1)` is not idle.
- **Prove** (`exec::run_inside`): before its first statement and after its last, in the same transaction, the
  executor reads `fingerprint`: `SchemaStorage` but the main row, the `Params` rows `DBNames*-Ext-*`, count and
  checksum of `_ExtensionsInfo`, `_ExtensionsRestruct`, `_ExtensionsRestructNGS`, and the row count of every
  table of the extension schema (253 parts on the clone). A difference rolls the transaction back and names
  the parts.
- **The criterion is a refusal for every adopted object**, as the issue states; the measured harmlessness of
  cases n and a is recorded, not used. **Proposal**: an option that lets an object pass when its only adoption
  is one with no table of its own (`extension_tables` empty), for the operations traced (add an attribute), once
  the deletion and the widening are traced on such an object.

Contract for the S1 gate and the apply seam (the code that builds `restructure::plan::Inputs`): call
`extensions::read_state(connection, &storage)` (done by `reader::read_inputs`) and `extensions::read_adoptions(target,
database)` when `registered > 0`; set `adoptions_read`. `plan` returns the `Refusal` as its error, so a gate that
turns a plan error into a blocker (`S1: <reason>`) needs nothing more. The executor's `fingerprint` belongs to
whichever code runs the statements: the seam's `structure_sql` should carry the same before/after assertions
(hashes of the same parts) as `THROW`s.

## 5. The twin protocol (12.6) for the two cases

The twins of case n: `n2_nat` (native apply) and `n2_own` (dry run, trial, apply of `ibcmd-rs mssql-restructure`
of this branch). Case a and c twins: refused, checks 11 and 10.

| # | check | case n | cases a, c |
|---|---|---|---|
| 1 | the plan made offline equals the native result | `DBNames` inflated equal (347 223 bytes both); the `DBSchema` entry `Reference23` equal | the plan is not made |
| 2 | tables, columns, indexes | identical but `_DbCopies*` (build drift) and the auto-named primary key of `_ConfigChngR` | unchanged: 0 tables changed |
| 3 | the data of every rebuilt table, EXCEPT both ways | `_Reference23`, `Config` and the 243 `X1` tables, `_ExtensionsInfo`, `_ExtensionsRestruct`, `_ExtensionsRestructNGS`: 248 tables, 0 differing | - |
| 4 | `Config` rows | equal (9 841 rows; the standalone command does not fold the `_dynupdate_` rows a fresh corpus clone carries, which is why the twins start after a native baseline apply) | unchanged |
| 5 | `DBSchema` entries, `DBNames` | 1 761 entries, equal but `DbCopies`, `DbCopiesUpdates` | unchanged |
| 6 | the 16 `.si` rows | 14 of 16 have the same text; the two others are known to this version of the prototype: the object registry row (`1a621f0f`, left unwritten) and the XDTO row, in which the wave-0 prototype writes `lowerBound="0"` for an attribute that is not nullable (native does not; the S1 branch of the ddl track reports equal XDTO for 23 attributes, 12.8) | unchanged |
| 7 | a native apply afterwards | "Обновление конфигурации базы данных не требуется", 0 tables and rows changed | - |
| 8 | native export | 12 198 / 12 198 identical; the four extensions 737 / 737 identical to the reference exports | - |
| 9 | a session in the cluster | identical output on both twins (metadata, read, XDTO serialization of an item, write, change of an existing item) | - |
| 10 | a rehearsal changes nothing | `--trial`: everything runs, "extensions: 253 parts as they were", rolled back; the real run followed on the same database | dry run, trial and apply: nothing written (0 tables, all service rows equal) |
| 11 | refusals | - | Catalog `_ДемоПартнеры`: "adopted by the extension _ДемоРасширение ... keeps no table of its own"; Catalog `_ДемоНоменклатура`: "... keeps tables of its own for it (_Reference18X1)" (`s1i-refusals.txt`) |
| 12 | a failure inside the transaction takes everything back | **not run** for this command: the standalone executor rolls back on any error (`XACT_ABORT`, rollback on the first failed verification, the same path as the new extension assertion); the injected `THROW` of 12.6 belongs to the seam's script. The assertion itself is unit-tested (`Fingerprint::differences`) | - |

One native export of the extension `_ДемоРасширение` ended with exit code 1 after 172 of 185 files on the own twin
and did not repeat (exit 0, 185 files, in the next attempt and in the next run of all four); the same export on the
native twin never failed. Not reproduced, not explained; recorded.

## 6. Not covered

- other kinds than catalogs (the prototype is catalogs only; the check is by name, so it works for any kind the
  gate will hand it: `ChangedObject::kind`), the deletion of an attribute or a widening of an adopted object;
- the platform's behaviour for an object the extension adopts when the **extension** carries an attribute of the
  same name, or the extension's form uses the deleted attribute (extension consistency checks of the apply);
- 8.5 (the S1 gate refuses it); an extension whose staged image adopts an object its active image does not is
  read (the union), but no such case was traced;
- a live failure injection (check 12) and the seam's script assertions: they wait for S1-A.
