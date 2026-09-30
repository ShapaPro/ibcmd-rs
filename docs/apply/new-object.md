# S1-F: a new catalog or document on the own apply (issue #402): design and the native write plan

Checkpoint 1 of S1-F (track trace, branch `feat/0.4-s1-f` on `feat/0.4` ee651602). Design and the plan of
writes; no code of the operation yet. Part of S1 (#391, `docs/apply/restructuring.md` 12). The caches of a new
object are done (S1-G, `docs/apply/derived-caches.md`); this is what is left: the tables, the names, the schema, the
registration and the wiring into the planner, the gate and the apply.

Evidence used (all measured on БСП 8.3.27): case c, a new catalog `ДемоНовыйСправочник` (native apply of a
`--base-free` stage, with an XE trace: `docs/apply/evidence/new-object/c-structure-statements.sql`), case d, a new
document with an attribute and a tabular section with an attribute (native apply of a native partial import;
snapshots, no trace), and the traces of the `ddl` track for a new tabular section of an existing object (e1, forwarded
by the coordinator): the platform creates a sub-table that has no old table with the same statements as a rebuilt one
minus the copy and the drop.

## 1. What the platform does for a new catalog

### 1.1 The writes, in order (case c, one apply)

| phase (`SchemaStorage.Status`) | statements for the new table `_Reference11036` | for a rebuilt table (compare) |
|---|---|---|
| structure, `200` | `create table dbo._Reference11036NG (...)` then `ALTER TABLE ... SET (LOCK_ESCALATION = DISABLE)`; the indexes in order: non-clustered ones by number (`_1NG`, `_2NG`, `_3NG`), the clustered `_S_HPKNG` last; then `UPDATE SchemaStorage SET NewGenCreated = @P1, Status = 200` | the same, then `INSERT ... SELECT` from the old table |
| `400` | nothing: **no copy, no drop** (the table has no old table) | `drop table dbo._<old>` |
| `500` | `sp_rename '_Reference11036NG' -> '_Reference11036'`, then the indexes `_Reference11036._Reference11036_1NG -> _Reference11036_1`, `_2`, `_3`, `_S_HPK` | the same |
| `100` | `UPDATE SchemaStorage SET Status = 100, CurrentSchema = @P1, NewGenCreated = <empty>, NewGenDropped = <empty>`; `UPDATE DBSchema SET SerializedData = @P1` | the same |

The same shape holds for a sub-table that is new in an existing object (`ddl`'s e1: `create <t>NG`, its indexes, no
`INSERT`, no drop, renamed with the rest). The trace of case c also holds the platform's own work of that run, which is
**not** ours (drift list of 12.6): `_DbCopiesInfoBaseUse`, `_DbCopiesUpdateTableStat`, `_DbCopiesUpdateStat`,
`_WebSocketClients` (new system tables of the build), `_BPrPoints10` and `_ConfigChngR` (rebuilt), the three
`_ExtensionsRestructNGS` scratch rows.

### 1.2 The table entry

`Params.DBSchema` and `SchemaStorage.CurrentSchema` hold one entry per table; the new one stands **before
`ConfigChngR`**, after the tables rebuilt earlier (12.4). For case c, a flat catalog without attributes
(`evidence/new-object/c-dbschema-entry-Reference11036.txt`):

```
{"Reference11036","N",11036,"",
 {7, ID(R Reference11036,2) Version(V) Marked(L) PredefinedID(B,16) Code(S var 9) Description(S var 25) Fld2683(N 7,0)},
 {0},                                                        -- sub-tables
 {3, ByPredefinedIDNotUniq{1,"PredefinedID"}, Code{2,"Code","ID"}, Descr{2,"Description","ID"}},   -- declared indexes
 1,"R", {1,{{1,"Fld2683"}}}, {1,{{1,"Fld2683"}}}, "",0,0}    -- data-separation lists
```

For a document with one attribute and one tabular section (case d, `d-dbschema-entry-Document11034.txt`): the fields
`ID Version Marked Date_Time Number Posted Fld<attr> Fld2683`, one sub-table `{"VT11036","I",0,"Document11034",
{LineNo11037 N(5,0), Fld11038 ...},{0},{0},1,"S",{0},{0},"",0,0}`, the indexes `ByDocNum{2,"Number","ID"}`,
`ByDocDate{3,"Date_Time","ID","Marked"}`. The SQL of the table (columns, the physical indexes `_S_HPK`, `_1`...) is
derived from the entry by the code that exists (`schema::physical_tables`, checked against the stored entries of the
whole БСП: `docs/apply/evidence/restructuring/dbschema-conformance-8327.txt`).

The rules that decide the entry (`docs/apply/restructuring.md` 5, checked on the БСП for the fields of all 139 catalogs
and documents; the index sets below counted on the same corpus, 282 main tables):

| part | rule |
|---|---|
| fields of a catalog | `ID`, `Version`, `Marked`, `PredefinedID`, [`OwnerID`], [`ParentID`, and `Folder` only for folders and items], [`Code` when `CodeLength > 0`, `S` variable or fixed / `N` by `CodeType`], [`Description` when `DescriptionLength > 0`], the attributes in metadata order, then the fields of the **common attributes that apply** in the configuration's order |
| fields of a document | `ID`, `Version`, `Marked`, `Date_Time`, [`NumberPrefix` when the number is periodic], `Number` (`S` or `N`), `Posted`, the attributes, the common attributes |
| declared indexes of a catalog | `ByPredefinedIDNotUniq`, then by the properties: `OwnerCode`, `OwnerDescr`, `ParentCode`, `ParentDescr`, `Code`, `Descr` (11 index sets in 114 catalogs), then per indexed attribute `ByOwnerFieldFld<n>`, `ByParentFieldFld<n>`, `ByFieldFld<n>` |
| declared indexes of a document | `ByDocNumPrefix` (periodic number), `ByDocNum`, `ByDocDate`, then `ByFieldFld<n>` per indexed attribute |
| sub-table | `VT<n>`, `LineNo<m>` `N(5,0)`, the attributes as `Fld<k>`, no common attributes |
| data-separation lists | both `Fld2683` for 112 of 139 (the data separator applies), both empty for 24 catalogs, `Fld3454` / empty for 3: decided by the **content of the common attributes** |

**Which common attributes apply** (settled by the hint of the coordinator and a check on the БСП). The БСП has seven
common attributes and each is a service field of the tables *and* of the XDTO types: `ОбластьДанныхОсновныеДанные`
(`Fld2683`, the data separator), `ОбластьДанныхВспомогательныеДанные` (`Fld3454`), `НаименованиеЯзык1`, `НаименованиеЯзык2`,
`КомментарийЯзык1`, `КомментарийЯзык2`, `ОтредактированныеПредопределенныеРеквизиты`. The descriptor of a common attribute
is `{5,<body>,<Content>,<Indexing>,<FullTextSearch>,<DataSeparation>,<AutoUse>,...}` with `Content =
{3,<n>,<object uuid>,{2,<use>,<condition>},...}`, `use` 0 Auto, 1 Use, 2 DontUse, and `AutoUse` 0 Use, 1 DontUse
(`metadata_model::simple`). What the БСП has (`Content` size, `AutoUse`):

| common attribute | listed objects | `AutoUse` | so an object that is not listed |
|---|---|---|---|
| `ОбластьДанныхОсновныеДанные` | 347, all `DontUse` | **Use** | **has the field**, unless it is one of the 347 |
| `ОбластьДанныхВспомогательныеДанные` | 57, `Use` | DontUse | does not |
| `НаименованиеЯзык1` / `2` | 11 each, `Use` | DontUse | does not |
| `КомментарийЯзык1` / `2` | 3 each, `Use` | DontUse | does not |
| `ОтредактированныеПредопределенныеРеквизиты` | 17, `Use` | DontUse | does not |

Checked on the 114 catalogs: the service properties of a catalog's XDTO type are **exactly** the common attributes that list
it with `Use` (114 of 114); and the 112 data-separated objects are the 139 minus the 24 (`Fld2683` absent) and 3 (`Fld3454`
in the first list). A **new** object is in no `Content`, so it gets only the fields of the common attributes with
`AutoUse = Use`: in the БСП `Fld2683` alone (as in cases c and d) and **no** service property in the XDTO type. The 33
catalogs with service properties are the members of the other lists, and `derived-caches.md`'s "open risk" is closed for
this configuration. A stage that lists the new object in a common attribute changes that common attribute's descriptor
(the classification refuses it: another kind); a configuration where another common attribute has `AutoUse = Use` would
add columns and properties this design does not model: `common.rs` reports it and the new object is **refused**. So: build
`Fld2683`-like fields (the type is the common attribute's own pattern, the number its `Fld` entry in `DBNames`), refuse
everything else. The two lists (`{1,{{1,"Fld2683"}}}` twice) come from the same descriptors (`DataSeparation`,
`UsersSeparation`, `SeparatedDataUse`, ...) and are part of the corpus test.

What is not known yet, and is the first work of S1-F: the nullable / index rules for attributes of a **hierarchical**
catalog, and the rule of the two separation lists. The old-attribute code already maps an attribute's type to its field
(`catalog::type_entries`, primitive types, `ValueStorage`, `UUID`); references and composite types need the map from a
type id to a table name (`R`, `E` fields), which the `a07b62f0` row and `DBNames` hold.

### 1.3 `DBNames`

`Params.DBNames`: `{<header>,{<count>,{<uuid>,"<kind>",<number>},...}}`. The numbers a new object takes come from the
shared counter (the header, extensions included) and are handed out in this order (`evidence/new-object/dbnames-deltas.txt`):

| case | numbers |
|---|---|
| c | `11034 Fld` (a new attribute of a catalog), `11035 Fld` (of a document), **then** `11036 Reference` (the new catalog) |
| d | `11034 Document`, `11035 Fld` (its attribute), `11036 VT`, `11037 LineNo` (its tabular section), `11038 Fld` (the section's attribute) |

Read: the **changed existing objects first** (in the order `restructuring.md` 4.3 has: kind, then the root's order), the **new objects
afterwards** (a new catalog got its number after the attribute of a *document*, so the order is not "kind first" across
old and new); inside a new object the main table, its attributes in order, then for each tabular section `VT`,
`LineNo` and its attributes. A catalog without exchange-plan membership allocates only `Reference` (no `ChngR`, no
`RefSInf`); a document only `Document`. Both native runs also took three numbers for the platform's own new system
tables (`DbCopies*`, after the object's): those are the build's upgrade of a restored base, not the object's; a twin is
made on a base that already had a native apply, or the three entries are tolerated (12.6 drift). `DBNamesVersion-DBNames`
gets a new guid as always. The order across old and new objects rests on **one** trace (c); a stage with a new object
listed before a changed one in the root confirms it (section 5, case N3).

### 1.4 The rows and their registration

The staged rows of the object (its descriptor `<uuid>`, and the body rows it has) are promoted like any staged row.
The registration for exchange plans (measured on the native result of case c): **three rows in `_ConfigChngR`**, one per
ordinary node of the exchange plans (`_MDObjID` = the uuid in the platform's byte order, `_MessageNo` NULL), and **no
`_ConfigChngR_ExtProps` row**, because this catalog has no body row (a body row would be listed as `(0, '<uuid>.0')`).
That is what the own apply already writes for a new form or template (`mssql_config_apply::objects`, `NewRegistration`),
so the apply's writer is reused for the created object; only the finding of the object differs (section 3.4).

### 1.5 The caches

Done: `caches::change::rewrite` gives the eight rows (`derived-caches.md` 7), equal to native's in cases c and d.

## 2. The plan of writes for a new catalog (what our transaction does)

Inside the own apply's transaction (`s1.rs` phase, one `StructurePhase`), after the assertions and before the move of
the staged rows:

1. **Assert** the table `_Reference<N>` and its sub-tables are absent; the schema and the names are those the plan read
   (their SHA-256, as for a rebuild).
2. **Numbers**: the counter from the header of the main `DBNames` and the extensions'; changed objects first, then each
   new object in (kind, root position) order; inside it the order of 1.3. `DBNames` text gets the entries in number
   order, the header the last number, `DBNamesVersion-DBNames` a new guid.
3. **Entry** of every table, built from the staged descriptor (section 3.2), inserted into the schema before
   `ConfigChngR` in the order of creation.
4. **Structure phase**: `TablePlan::created` for the main table and each sub-table: `create ...NG`, its indexes, the
   marker (`Status 200`), no copy, no drop (`400`), the renames (`500`), the final update of `SchemaStorage`
   (`100`) and `DBSchema`.
5. **Caches**: `caches::change::rewrite` over the cache rows the changed objects' updates left; `siVersions` gets the new guids of all
   rewritten rows (`ParamsRewrite`).
6. **Registration**: `_ConfigChngR` rows for the ordinary nodes and the `_ConfigChngR_ExtProps` rows of the body files,
   by the apply's `NewRegistration`; `_MessageNo` reset as for any staged object.
7. **Postconditions** (asserted before commit, as for a rebuild): the new tables exist with the planned columns and
   indexes, and are empty; the names and the schema are the planned text.

The rehearsal, the recovery backup (12.4.4) and the rollback of a failure are the apply's, unchanged: the operation adds
statements to the script, it does not add a phase.

## 3. Design

### 3.1 Where the code goes

New files (so that the shared planner of `ddl` is touched in a few small places):

| file | content |
|---|---|
| `src/restructure/entry.rs` | `main_entry(kind, &NewObjectFacts, &Numbers, &CommonAttributes, &TypeMap) -> Result<Brace>`: the DBSchema entry of a new catalog or document with its sub-tables; `Numbers` = the numbers of section 1.3; `CommonAttributes` = which common attributes apply. Validated against the stored entries of the whole БСП |
| `src/restructure/common.rs` | the `CommonAttribute` descriptors: `AutoUse`, `Content` (`{2,use,cond}` per object), `DataSeparation`; `applies_to(object) -> bool` and the fields they add; a report of the common attributes other than the data separator that have `AutoUse = Use` (then the new object is refused) |
| `src/restructure/create.rs` | `plan_created(stored, running, &NewPrepared) -> Result<(ObjectPlan, Brace)>` (the new object's tables as `TablePlan::created`, the entry), `created_caches(...)` (the call of `caches::change::rewrite`) |

Shared with `ddl` (each a small separate commit, after their creation primitive is merged: `TablePlan::created`,
`Running::allocate`, `schema::subtable_entry`, `push_subtable`):

| hook | change |
|---|---|
| H1 `plan.rs::check_files` | a file that is new is accepted when the object it belongs to is a created object of `create.rs` |
| H2 `plan.rs::plan` | after the loop over the changed objects, a loop over the created ones with the same `Running` counter; their entries join `entries` |
| H3 `plan.rs::ObjectPlan` | `created: bool`, and `changes()` says "new catalog X" |
| H4 `plan.rs` caches | the created objects' caches are computed **after** the attribute updates of `xdto_update` / `registry_update` and chained on their rows (inflate, `change::rewrite` with `changed` = the created objects, deflate); one `siVersions` update covers the union |
| H5 `s1.rs::decide` | `AddObject` (and `AddTabularSection`, ddl's) leaves the "designed, not built" list; the two decoders agree on the set of created objects (names, kinds) as they do on attributes |
| H6 `s1.rs::phase_of` / `gate::StructurePhase` | a list `created: Vec<(uuid, kind, body files)>` for the apply's registration and its report |

And one change in the apply (`mssql_config_apply`, owner: apply track): the rows of a created object are **answered for
by the structure phase** (like the planned objects' descriptors), and the object is registered like a new form:
`objects::analyze` takes `phase.created` as new objects of kind `Catalog` / `Document` instead of finding them.

### 3.2 The entry builder and its proof

`entry.rs` reproduces, from the staged descriptor and the numbers, an entry that is byte for byte what the platform
stored. Its test is offline and needs no native run: for **every catalog and document of the БСП** (139 stored
entries, sub-tables included) build the entry from the stored descriptor and the stored `DBNames` numbers and require
the text to equal the stored one. Objects the builder does not cover (owner-subordinated catalogs, defined types and
characteristics, predefined items, exchange-plan membership, register-like extras) must be refused **before** they are
attempted, by a list checked in the same test: covered + refused = 139. A new object only ever gets what the test proved.

### 3.3 Refusals (fail closed)

An object that lists forms, templates, commands or predefined items (its rows and registrations are not built); an owner
(`OwnerID`) unless the test proves it; a member of an exchange plan (then the plan's descriptor changes too and the
classification refuses it: another kind); an attribute of a defined type or characteristic; more than one new catalog or
document per kind in one stage (the caches, `derived-caches.md` 11); a `Config` row of the object other than the
descriptor and the bodies the registration lists; a table number that is taken; a configuration with a common attribute
other than the data separator that has `AutoUse = Use` (1.2); a stage that changes a common attribute. These are also the refusals of
`caches::change::rewrite`.

### 3.4 How the pieces meet

The gate: `check_staged` names the new catalog by the pair of reasons that `apply_check::s1::classify` already turns into
`S1Operation::AddObject`. `decide` runs `plan`, which now reads the created objects from the staged image: every
descriptor that is new in `ConfigSave` and has a listing in the configuration row. The apply: the structure phase is
the same seam (`take_structure`); it also hands the created objects to the registration of the apply. The database side
of the plan's input (`reader::read_inputs`) gains the staged **new** descriptors and the common-attribute descriptors.

## 4. Verification

The twin protocol of 12.6, per case, with the split of `derived-caches.md` 9:

| check | how, for a new catalog |
|---|---|
| 1 | a test per case: the plan made offline from the staged snapshot equals the native result: the `DBNames` text (on a base that already had a native apply), the entry of every new table, the cache rows (done) |
| 2 - 5 | `snapshot.py` / `snapdiff.py`, `compare_config.ps1`, `dbschema_cmp.py` on the twins; the new tables are empty |
| 6, 7, 9 | `si_diff.py`; native `config apply` afterwards «не требуется»; a session that creates and reads an element of the new catalog (the probe of S1-G) |
| 8 | native export of both; `source-diff` |
| 10 | `--rehearse` changes nothing |
| 11 | the refusals of 3.3 as tests of `decide` and a dry run on a stage that has one change more |
| 12 | a `THROW` before `COMMIT` in the script; the database as it was, no `NG` table, no new name |

## 5. Native cases to trace (checkpoint 2)

Each on a base that already had a native baseline apply (so the platform's upgrade tables are not in the diff), staged by
the native partial import, a COPY_ONLY backup, the native twin traced by XE (recipe 10.8 of `restructuring.md`), ours
compared. The kit of `scripts/apply-trace/lab/s1g-caches/` (case d) makes the stage.

| case | what | why |
|---|---|---|
| N1 | a flat catalog with attributes of string, number, date, boolean, a reference to a catalog, one indexed | fields, the index names, the numbers, the `R` type entry |
| N2 | a hierarchical catalog (folders and items) with a tabular section | `ParentID`, `Folder`, `ByParent...` indexes, the nullable rule |
| N3 | a new catalog that the root lists **before** a catalog with a new attribute, in the same stage | the order of the numbers across old and new objects (1.3) |
| N4 | a new document with a periodic numeric number and an indexed attribute | `NumberPrefix`, `Number N`, `ByDocNumPrefix`, `ByFieldFld` |
| N5 | a new catalog and a new document in one stage | the caches and the numbers across kinds |

## 6. Work packages, in this order

1. `common.rs`, `entry.rs` and their corpus test (3.2): no lab, no shared file. Checkpoint 2.
2. `create.rs` and the hooks H1-H4 in small separate commits, the corpus tests of the plan against the native results of
   c, d and N1-N5. Checkpoint 3.
3. H5, H6 and the apply's change; the twin runs of section 4 per case, the refusals, the failure injection. Checkpoint 4.

## 7. Open points

- **The creation primitive of `ddl`** (`TablePlan::created`, `Running::allocate`, `subtable_entry`, `push_subtable`): used
  as designed. One request: `allocate` should be callable in the order of 1.3 for a whole new object, and it should
  return the SQL name with the number, so that the created table's name (`Reference<N>`) needs no second lookup.
- **The apply's registration of created objects**: owned by the apply track; this design asks for `phase.created` as its
  input (3.1). To settle with the coordinator before work package 3.
- **The service properties** of the XDTO type are the common attributes that list an object (1.2): none for a new object
  of the БСП, so the caches' open risk is closed for it; the refusal of a configuration with another `AutoUse = Use`
  common attribute is `common.rs`'s.
- **The rule for the two separation lists** and the `Fld3454` case: read from the descriptors, decided by the corpus test.
