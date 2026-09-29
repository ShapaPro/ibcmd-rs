# Restructuring on `config apply`: what native does, and what it takes to do it ourselves

Issue [#341](https://github.com/Untru/ibcmd-rs/issues/341), track "ddl", 0.4 "Своё применение конфигурации".
Research draft of 2026-09-29 (checkpoint 1: traces, formats, mapping). The prototype (section 9) is not
written yet.

Scope of the measurements: platform 8.3.27.2214, Microsoft SQL Server 2025, exclusive apply
(`ibcmd infobase config apply --force --dynamic=disable --user=Администратор`), the БСП demo configuration
(1761 main tables, 4 extensions) restored from the lab corpus. The 8.5 БСП was restored read-only and its
formats compared; nothing was traced on 8.5. Everything below is **measured** unless it says
**hypothesis**. Evidence files are listed in section 11; the scripts that produced them are in
`scripts/restructure-lab/`.

## 1. Summary

1. **Native apply never uses `ALTER TABLE`.** Whenever the structure entry of a table changes (a column
   added, widened or removed, an index added or removed, a sub-table added), the platform rebuilds the
   whole object through "new generation" tables: it creates `_XNG` for the table and each of its
   sub-tables, loads the data with `INSERT ... SELECT` (defaults for new columns), drops the old tables and
   renames the new ones. New objects take the same path (create `NG`, rename). Progress is recorded in
   `SchemaStorage`: `Status` 100 -> 200 -> 400 -> 500 -> 100, and `NewGenCreated` holds the definitions of the
   tables created so far (section 3).
2. **The structure is a text, and it is decoded.** `DBSchema` (== `SchemaStorage.CurrentSchema` of
   `SchemaID = 0`) is 1C brace text; `Params.DBNames` (raw deflate of brace text) numbers the objects. A
   byte-exact writer and a model "DBSchema entry -> `CREATE TABLE` / `CREATE INDEX`" reproduce **all 1937
   tables of the 8.3.27 БСП and all 1922 tables of the 8.5 БСП** (columns, types, nullability, indexes,
   implicit primary keys); no mismatch is left (section 4).
3. **Metadata -> fields is regular.** For 94 of 94 catalogs and documents (owner-subordinate catalogs and
   attributes of defined types excluded: 16 + 29 objects) the field list of the main table is reproduced
   exactly from the XML tree plus `DBNames` (section 5). The rules that had to be found are few: the
   standard fields per object kind, the type table, the nullable rule, common-attribute fields.
4. **Adding an attribute needs four things**: a new number (global counter over the main *and all
   extension* `DBNames`), a new field entry in the right place, the rebuild of the object's tables
   with a type-dependent default for the existing rows, and the promotion of the staged `Config` rows
   (section 6).
5. **Estimate** (section 8): the "add/remove attribute or tabular section, add a plain catalog or document,
   index flag" subset is a matter of weeks; registers with indexes and totals, common attributes,
   subordination, predefined data, type changes with data conversion, extensions and exchange plans make
   the full job a matter of months. A **round-trip gate** (the generator must reproduce the *old* schema of
   an object family from the *old* metadata before it may change it) turns coverage into an automatic
   safety check.
6. **Findings other tracks need** (section 10): our import in patch mode silently ignores added
   attributes; native `config import` is unreliable under load (a partial or a failed stage); a native
   apply interrupted at "Принятие изменений" left a database that refuses every `ibcmd config` command
   including `config repair`.

## 2. Method

- **Clones.** `restore-clone.ps1` from the БСП 8.3.27 corpus; one cumulative clone per lineage
  (`ibcmd_rs_04_ddl_bsp8327_a`, `_m`, `_c2`), a COPY_ONLY backup before every apply
  (`lab/04/restructure/bak/*_staged.bak`) is the twin source.
- **Staging.** Changes were made in a copy of the reference native export and staged with **native**
  `ibcmd infobase config import` (our patch-mode import cannot stage structural edits, section 10). When the
  native import was unusable (cases h and c) the tree was staged with **our import in `--base-free` mode**.
- **Trace.** An Extended Events session per apply (`sql_batch_completed`, `rpc_completed`,
  `sql_statement_completed`, `object_created/altered/deleted`), filtered to the lab database, 50-60 thousand
  events per apply. `xe_read.py` turns it into JSONL, `xe_shapes.py` into a story of normalised statements.
- **Snapshots.** Before/after: every table's columns and indexes, row count and `CHECKSUM_AGG(BINARY_CHECKSUM(*))`,
  and the service tables (`DBSchema`, `SchemaStorage`, `Params`, `Config`, `ConfigSave`, `Files`, `ConfigCAS`)
  row by row with content hashes (`snapshot.py`, `snapdiff.py`).
- **Cases** (all on rows that exist: 8-716 rows per changed table):

| label | change (all in the БСП demo objects) | table(s) that changed | rows |
|---|---|---|---|
| baseline | nothing (staged unchanged tree) | `_ConfigChngR` only | 20685 |
| a2 | String(50) attribute in `Catalog._ДемоПартнеры` | `_Reference20` (+2 sub-tables) | 14 |
| b | Number(15,2) attribute in `Document._ДемоЗаказПокупателя` | `_Document39` (+3) | 8 |
| c | new catalog `ДемоНовыйСправочник` | new `_Reference11036` | 0 |
| d (`dm`) | String(10) dimension in `InformationRegister._ДемоЦеныНоменклатуры`, Number(15,2) resource in `AccumulationRegister._ДемоОстаткиТоваровВМестахХранения` | `_InfoRg7272`, `_AccumRg505`, `_AccumRgT510` | 13, 50, 3424 |
| e + g (`g`) | that attribute widened 50 -> 100 and its index flag switched on (one apply) | `_Reference20` | 14 |
| f | that attribute (indexed) deleted | `_Reference20` | 14 |
| h | 12 attributes of every basic type on a flat catalog; 1 attribute on a catalog with common attributes; 1 attribute in a tabular section; 1 new tabular section | `_Reference2598`, `_Reference27`, `_Reference20_VT159`, new `_Reference15_VT11039` | 716, 71, 18 |

Case e was merged into g (two failed native imports; section 10); the string widening and the index
creation are still separable in the trace. No trace exists for: type change with data loss, string
narrowing, deleting a whole object, `--dynamic=force`, 8.5, ERP УХ.

## 3. What native apply does

### 3.1 Timeline (case d, clean run, 113 s under a loaded machine)

`lab/04/restructure/xe/dm`, `docs/apply/evidence/restructuring/timeline-dm.txt`:

| phase | seconds | what |
|---|---|---|
| metadata check | 1.7 - 26 | "Проверка корректности метаданных": thousands of single-row `SELECT ... FROM ConfigSave/Config WHERE FileName = @P1` |
| stage copy | 26 - 60 | for **every** staged row (9839 statements): `INSERT Config SELECT '<name>.new', ... FROM ConfigSave WHERE FileName = '<name>'` |
| structure | 63 - 64 | `create table ...NG`, `CREATE INDEX ...NG`, `drop index`, `INSERT INTO ...NG SELECT`, `insert bulk` |
| switch | 75.5 - 76.6 | `Status 400`, `drop table`, `Status 500`, 28 x `sp_rename`, `Status 100`, `UPDATE DBSchema` |
| promotion | 76.7 - 108 | `Params`: `*.sinew` -> `*.si`; `Config`: 9838 x (`DELETE` old + `UPDATE ... SET FileName` from `.new`); `DELETE FROM ConfigSave` |
| help index | 112.8 - 113 | `Files`: `userDocs_ru`, `userVocabulary_ru`, `userPostings_ru`, `MobileVersions.dat` renamed from `*.new` |

Apply wall times (s): a2 63, b 222 (machine busy), c 167, d 115, g 75, f 96, h 122, baseline 139. The
structure DDL is about **1-2 s of the 75-220**; the rest is row-by-row staging and promotion.

### 3.2 The "new generation" protocol and `SchemaStorage`

`SchemaStorage(SchemaID int, Status int, CurrentSchema, NewGenCreated, NewGenDropped varbinary(max))`;
`SchemaID = 0` is the main configuration, `1` the extensions (their tables carry the `X1` suffix). Sequence
of statements of case a2 (`docs/apply/evidence/restructuring/a2-structure-statements.sql` has them
verbatim):

1. `create table dbo._Reference20NG (...) ;alter table dbo._Reference20NG SET (LOCK_ESCALATION = DISABLE);`
   for the table and every sub-table, then all their indexes (`CREATE [UNIQUE] [CLUSTERED] INDEX
   <name>NG ON dbo.<table>NG (...) WITH (SORT_IN_TEMPDB = ON, MAXDOP = 0, ALLOW_PAGE_LOCKS = OFF,
   ALLOW_ROW_LOCKS = ON) [ON [PRIMARY]]`; the clustered index has no `ON [PRIMARY]`).
2. `UPDATE SchemaStorage SET NewGenCreated = @P1, Status = 200 WHERE SchemaID = 0` after each group of created
   tables. `NewGenCreated` is a **mini DBSchema** (`{0,{<n>,<table entry>...}}`, same grammar as `DBSchema`,
   the new definitions of the NG tables created so far; 2.6 KB after the first group, 4.3 KB after the fourth;
   see `newgen-created-a2-*.brace.txt`).
3. Indexes are dropped again (`drop index <name>NG on ...`) so the load runs into a heap, the data is copied
   (`INSERT INTO dbo._Reference20NG WITH(TABLOCK) (cols) SELECT ... FROM dbo._Reference20 T1 WITH(NOLOCK)`),
   the indexes are created again.
4. `UPDATE SchemaStorage SET Status = 400`; `drop table` of every old table; `Status = 500`; `sp_rename
   '<t>NG', '<t>', 'object'` for every table, `sp_rename '<t>.<i>NG', '<i>', 'index'` for every index (via
   `sp_prepexec`); `ALTER INDEX <pk> ON _ConfigChngR SET(ALLOW_PAGE_LOCKS = OFF, ALLOW_ROW_LOCKS = ON)`.
5. `UPDATE SchemaStorage SET Status = 100, CurrentSchema = @P1, NewGenCreated = @P2, NewGenDropped = @P3 WHERE
   SchemaID = 0` with the full new schema and `NewGenCreated = NewGenDropped = EF BB BF 7B 30 2C 0D 0A 7B 30
   7D 0D 0A 7D` (`{0,{0}}`), then `UPDATE DBSchema SET SerializedData = @P1` (the same text).

`NewGenDropped` was never written in these cases (**hypothesis**: it is for the online/high-load path of
milestone 0.5, together with `--dynamic=force`). The statuses make the run resumable; **measured**: an apply
that ended with exit code -1 at "Принятие изменений" (after step 5, before the promotion) left the database
refusing `apply`, `import`, and `config repair --commit|--rollback|--fix-metadata` with "Обнаружена
незавершенная операция сохранения конфигурации" (section 10). An apply that failed earlier was simply
continued by the next apply.

### 3.3 What triggers a rebuild, what is copied

- A table is rebuilt when its **entry in the new DBSchema differs from the old one**. The whole object goes
  with it: main table and all sub-tables (case a2: `_Reference20`, `_Reference20_VT155`, `_Reference20_VT159`
  for one new column in the main table); companion tables follow (`_RefSInf3901` with `_Reference27`).
  Index-only changes rebuild the table too (case g). New tables use the same NG path without data.
- `_ConfigChngR` and `_ConfigChngR_ExtProps` are rebuilt **on every apply**, even with no change (the
  primary key gets a new auto-generated name each time; the row count stayed 20685 in a2, b, g, f, h and
  grew by 3 in c and d, so the platform also registers something there - not decoded). Their copy goes
  through 1C temp tables and `insert bulk` (the log line "Обработка данных Реструктуризация Таблица
  регистрации изменений конфигурации 1000..20000" is this copy).
- Platform-build drift: the databases of the corpus were last touched by an older build. The first
  structural apply also upgraded system tables (`_DbCopies` +`_StorageVariant`, `_DbCopiesUpdates`
  +3 columns in a2; created `_DbCopiesInfoBaseUse`, `_DbCopiesUpdateStat`, `_DbCopiesUpdateTableStat`,
  `_WebSocketClients` in c). Their names take numbers from the same `DBNames` counter (nil uuid).
- **Data copy is `INSERT ... SELECT` with column mapping and defaults** (section 6.3): unchanged
  columns are copied by name, a widened column is `CAST(T1._Fld11034 AS NVARCHAR(100))`, a new one is a typed
  parameter; `(T1._TrNum + 0.0)` is how the query compiler widens a numeric.
- The log messages of the platform name the actions: "Новый объект: Справочник.X", "Объект изменен:
  РегистрСведений.Y", "Создана таблица: ...", "Изменена структура таблиц базы данных", and for registers
  "Реструктуризация ... пересчет итогов" (totals of the accumulation register are copied, not recalculated:
  `_AccumRgT510` 3424 rows go through `INSERT ... SELECT`).

### 3.4 Promotion of the configuration rows

- The staged rows go from `ConfigSave` to `Config` under `.new` names, and after the structure is switched
  they are renamed (`DELETE FROM config WHERE FileName = '<n>' AND EXISTS(... '<n>.new')`, then
  `UPDATE config SET FileName = '<n>' WHERE FileName = '<n>.new'`); `Params` rows `*.si` go through `*.sinew`,
  `Files` rows through `*.new`/`*.datNEW`. **All staged rows are moved, changed or not** (9839 of them).
- `root`, `version` and `versions` are taken **as staged**: the generation printed by native
  ("Создано поколение конфигурации: 56569e24...") is the header uuid of the staged `versions` row
  (`{1,9834,"",<uuid>,...}`); the row is identical before and after the apply.
- Other writes of an apply, none derived from the structure change alone: `Params` `DBNames` and
  `DBNamesVersion-DBNames` (only when names were allocated), the `*.si` cache rows and `siVersions`
  (rewritten with new version guids on **every** apply), two `*.ui` rows, `_ExtensionsRestruct(NGS)` rows
  (state of the extensions' restructure), `Files` help-index rows, and on the first apply of a lineage the
  garbage collection of `ConfigCAS`/`Files` (12797 -> 636 rows, 353 -> 44 rows).

## 4. Formats

### 4.1 Brace text and its layout

Both `DBSchema` and `DBNames` use the platform's brace text. The layout is **byte-exact reproducible** with
two rules: every nested list starts on a new line (`\r\n`), and a list whose last element is itself a list
closes on a new line (`scripts/restructure-lab/bracefmt.py`, `dumps`); `parse -> dumps` gives back the same
bytes for `DBSchema` (977 749 characters + BOM) and `DBNames` of 8.3.27 and 8.5. `DBSchema` and
`NewGenCreated` start with a UTF-8 BOM; the inflated `DBNames` too. Strings are `"..."` with `""` escapes;
unquoted tokens are numbers, uuids, one-letter codes.

### 4.2 `DBSchema`

```
{0,{<count>,<table>,...}}
table    = {"<Name>","N",<number>,"",<fields>,<subtables>,<indexes>,1,"R"|"S",<sepA>,<sepB>,"",0,0}
fields   = {<n>,<field>...}         field = {"<name>",<nullable 0|1>,{<n>,<type>...},"",0}
subtable = {"VT<n>","I",0,"<Owner>",<fields>,{0},<indexes>,1,"S",{0},{0},"",0,0}
index    = {"<name>",<unique 0|1>,{<n>,"<field>"...},<clustered>,<flag>,0,{0},0,0}
sepA/sepB = {0} | {1,{{<n>,"<field>"...}}}
```

The SQL table name is `_` + `<Name>` (the number is already in the name for objects: `Reference20`); sub-tables
are `_<Owner>_<VTn>`. Type entries `{tag,a,b,ref,k[,six]}`:

| tag | SQL | notes |
|---|---|---|
| `B` a | `binary(a)`; a = `0x80000000` -> `varbinary(max)` | `{"B",16}` = uuid, `{"B",2147483648}` = ValueStorage |
| `L` | `binary(1)` | boolean |
| `N` p,s | `numeric(p,s)` | sixth element 1 with s = 0: `int`; 2: `int IDENTITY` |
| `T` | `datetime2(0)` | dates are stored +2000 years (`_YearOffset`) |
| `S` a | `nvarchar(a & 0x7fffffff)` if bit 31 is set, else `nchar(a)`; length 0 -> `nvarchar(max)` | |
| `R` ref,k | typed (`ref` = a table name): `_<F>RRef binary(16)`; untyped: `_<F>TRef binary(4)` + `_<F>RRef binary(16)` | k: 2 = own `ID`, 3 = other reference, 4 = untyped |
| `V` | `timestamp` | row version |
| `E` | `_<F>_TYPE binary(1)` | first entry of a composite type |

A field with several type entries (composite type) becomes several columns: `_<F>_TYPE`, `_<F>_L`, `_<F>_N`,
`_<F>_T`, `_<F>_S`, `_<F>_RTRef`, `_<F>_RRRef` (with a single typed reference only `_RRRef`).
`nullable = 1` -> `NULL`, else `NOT NULL`.

- **Sub-tables** get `_<Owner>_IDRRef binary(16)`, then the **sepB** columns of the owner, then
  `_KeyField binary(4)`, then their fields; a clustered `_<Owner>_<VTn>_SK (sepB, _<Owner>_IDRRef, _KeyField)`
  is implicit; the declared indexes are `_<Owner>_<VTn>_<i>` with sepB in front and the owner key at the
  end (field `ID` means `_<Owner>_IDRRef`).
- **Indexes** of a main table are named `_<Name>_<i>` in list order; **sepA** columns are prepended to each.
  An index with more than 16 key columns is cut to 16 and is not unique (SQL Server limit; found in
  `_InfoRg6849`).
- **Implicit keys** (fields contain `ID` = `{"R",0,0,"<self>",2}`): with sepA -> clustered unique
  `_<Name>_S_HPK (sepA, _IDRRef)`, plus non-clustered unique `_<Name>_S_PK (_IDRRef)` when sepB is empty;
  without sepA -> inline `primary key` on `_IDRRef` (auto-named `PK___...`, hence the new name after
  every rebuild).
- **Table order matters for byte equality**: a rebuilt table is removed from its place and appended at the
  end of the list, in the order of creation of the NG tables; `ConfigChngR` (rebuilt every time) is always
  last. New tables are appended before it. Header count is the number of tables.

Conformance (`dbschema_check.py`; `docs/apply/evidence/restructuring/dbschema-conformance-*.txt`): 8.3.27
БСП 1937/1937 tables (1761 main + 176 sub-tables) columns and indexes, 1761/1761 implicit keys; 8.5 БСП
1922/1922 and 1769/1769. Same grammar, same 14-element table entry, same type tags in 8.5; both databases
report `IBVersion 7`, `PlatformVersionReq 80313`.

### 4.3 `DBNames` and `DBNamesVersion`

`Params.DBNames`: raw deflate of `{<max number>,{<count>,{<uuid>,"<kind>",<number>},...}}`. `<kind>` is the
family of the SQL name (`Reference`, `Fld`, `VT`, `LineNo`, `InfoRg`, `ByDims`, ...), `<number>` the
suffix. Entries are sorted by number; **numbers are never reused and entries never removed** (case f: the
deleted attribute stays). `Params.DBNamesVersion-DBNames` is `{0,<uuid>}` with a **new random guid** at
every change (`DBNamesVersion` itself did not change). Each extension has `DBNames-Ext-<id>` and
`DBNamesVersion-DBNames-Ext-<id>`.

**Allocation** (measured in a2, b, c, d, h): the next number is `max` over the headers of the main **and
all extension** `DBNames` plus one (a2: main header 10824, extension b3fa0ef0 11033 -> 11034). New entries
are appended in the order the platform walks the changed objects: in metadata (configuration) order, i.e.
by kind first (case d: the information register got 11036-11037 before the accumulation register got 11038,
although its table number 7272 is larger than 505), inside a kind in the traced cases both name order and
table number order agree (case h: `Reference15`, `20`, `27`, `2598`); within an object a new tabular section
(`VT`, `LineNo`, its attributes) and attributes in metadata order. The header takes the last number and
the count grows. Built-in tables that the platform adds take numbers from the same counter with the nil
uuid.

Which kinds a metadata class allocates (`docs/apply/evidence/restructuring/dbnames-kinds.txt`, all of
БСП): catalog `Reference` + `ReferenceChngR` (+ `RefSInf` for 74 of 114); attribute `Fld`; tabular
section `VT` + `LineNo`; document `Document` + `DocumentChngR`; constant `Const` + `ConstChngR` + `Fld`;
information register `InfoRg` + `InfoRgChngR` (+ `InfoRgOpt`); dimension `Fld` (+ `ByDims` for 111);
resource `Fld` (+ `ByResource`); register attribute `Fld` (+ `ByProperty`); document/BP/task attribute
`Fld` (+ `ByField` when indexed); enum `Enum`; exchange plan `Node`; scheduled job `ScheduledJobs`. A
new catalog without exchange-plan membership allocated **only** `Reference` (case c: no `ChngR`, no `RefSInf`).

## 5. Metadata -> tables

Verified on the whole БСП by `gen_check.py`, `md_types_check.py` (files in the evidence folder).

**Fields of a catalog main table**: `ID`, `Version`, `Marked`, `PredefinedID`, (`OwnerID` for owned
catalogs: not verified), (`ParentID`, and `Folder` only for `HierarchyFoldersAndItems`), `Code` (if
`CodeLength > 0`: `S` variable/fixed or `N` by `CodeType`), `Description` (if `DescriptionLength > 0`),
attributes in metadata order (`Fld<n>`), then the fields of **common attributes** in their configuration
order (the data separator `Fld2683` is one of them, so is the multilingual `НаименованиеЯзык1`). **Document**:
`ID`, `Version`, `Marked`, `Date_Time`, (`NumberPrefix` `T` when `NumberPeriodicity != Nonperiodical`),
`Number`, `Posted`, attributes, common attributes. A new attribute goes **after the last own attribute and
before the common-attribute fields** (case h2: before `Fld7857`).

**Type -> type entries** (231 distinct XML descriptors in the БСП, one mapping each; 4 are
`TypeSet`/defined types whose mapping depends on the resolved set):

| XML | entries | new-column default in the copy |
|---|---|---|
| `xs:boolean` | `L` | `0x00` |
| `xs:string`, `Length=n`, `Variable` | `S` `0x80000000|n` | `N''` (`@P nvarchar`) |
| `xs:string`, `Fixed` | `S` n | `CAST(@P AS NCHAR(n))` of n blanks |
| `xs:string`, `Length=0` | `S` `0x80000000` | `N''` |
| `xs:decimal` (p,s) | `N` p,s | `CAST(@P AS NUMERIC(p, s))`, @P = 0 |
| `xs:dateTime` (Date or DateTime) | `T` | `'2001-01-01 00:00:00'` (the empty date + 2000 years) |
| `cfg:CatalogRef.X`, `EnumRef`, `DocumentRef` | `R` `<Table>`,3 | 16 zero bytes |
| composite (several types) | `E` + `L` `N` `T` `S` (fixed order) + `R` | `_TYPE = 0x01`, `_N = 0`, `_S = N''`, refs zero |
| `v8:ValueStorage` | `B` `0x80000000` | `0x01010800000000000000EFBBBF7B2255227D` |
| `v8:UUID` | `B` 16 | 16 zero bytes |

The defaults are what the copy `INSERT ... SELECT` of case h used (`types-h-statements.sql`).

**Nullable flag**: 1 exactly when the catalog is `HierarchyFoldersAndItems` **and** the attribute has
`Use = ForItem`: 229 of 229 such attributes; every other attribute (719 + 116 + 36 + 8 catalog attributes,
271 document attributes, all register fields) is 0. For nullable fields the copy is
`CAST(CASE WHEN T1._Folder = 0x01 THEN @P1 END AS NVARCHAR(50))`: **NULL for folders, the default for items**
(`_Folder = 0x01` marks an item).

**Tabular sections**: `VT<n>` with first field `LineNo<m>` `N(5,0)` (`LineNumberLength`), attribute fields
`Fld<k>`, no common attributes; the SQL table has the extra columns above. An indexed attribute makes
`ByFieldFld<k>` `{2,"Fld<k>","ID"}` (non-unique).

**Indexes from the `Indexing` flag**: an attribute of a hierarchical catalog (case g) -> two **unique**
indexes, `ByParentFieldFld<n> {4,"ParentID","Folder","Fld<n>","ID"}` and `ByFieldFld<n>
{2,"Fld<n>","ID"}` (flat catalogs: not traced); a document attribute: two naming forms coexist in the corpus
(`Document39` has `ByField5561` on `Fld2909` and `ByFieldFld3821` on `Fld3821`; only 3 of 271 document
attributes have a `ByField` number in `DBNames`; **hypothesis**: numbered names are older allocations);
information register -> the `ByDims`/`ByPeriod` family (below).

**Registers** (case d): a new dimension is inserted after the last dimension and before the resources;
every index over dimensions is rewritten (`ByPeriod (Period, D1)` -> `(Period, D1, D2)`,
`ByDims (D1, Period)` clustered -> `(D1, D2, Period)`), and a **third** index `ByDims<number>
(D1, Period, D2)` appears with the number allocated from `DBNames` (kind `ByDims`, keyed by the uuid of the
first dimension). A new accumulation register resource is inserted after the last resource and appears in
the totals table as well (`_AccumRgT510`: `numeric(21,2)` for a `numeric(15,2)` resource - totals have
higher precision). The derivation of register indexes from `Master`, `MainFilter`, `Indexing` and register
kind is **not** reproduced (259 information registers, 8 kinds of index sets in the corpus).

**Not reproduced yet** (skipped or untested): owner-subordinated catalogs (`OwnerID`), attributes of
defined types and characteristics (type sets), constants, enums, exchange plans, charts, business processes,
tasks, document journals, accounting/calculation registers, predefined data tables, `ChngR`/`SInf`/`Opt`
companions.

## 6. The algorithm for the simplest case (a new attribute)

Given `ConfigSave` staged by native import and the old `Config`:

1. **Classify**: compare the object rows (`<uuid>`, not `.0`) of `ConfigSave` and `Config`; decode the
   changed catalog/document row (the repo has the decoders `metadata_model::objects_export::decode` and
   `attribute_export`); accept only "attributes appended, simple type", refuse everything else.
2. **Number**: `n = max(header of main DBNames, headers of every DBNames-Ext-*) + 1`; append
   `{<attr uuid>,"Fld",<n>}`; `DBNames` header `{<n>,{<count+1>,...}}`; new guid in
   `DBNamesVersion-DBNames`; deflate.
3. **Schema**: parse `DBSchema`, insert the field entry `{"Fld<n>",<nullable>,{1,<type>},"",0}` before the
   first common-attribute field of the table; if the attribute has an index flag add the index entries;
   move the table to the end (before `ConfigChngR`); write with the exact layout; the header count is
   unchanged.
4. **Rebuild**: the NG protocol of 3.2 for the table and its sub-tables (or, **hypothesis**, a single
   transaction: `BEGIN TRAN` ... the same statements ... `COMMIT`; SQL Server DDL is transactional, so a
   crash rolls everything back and no native state machine is needed in exclusive mode). Copy with the
   default of the type.
5. **Publish**: `UPDATE SchemaStorage ... CurrentSchema`, `UPDATE DBSchema`, the `Params` rows, the promotion
   of the staged `Config` rows (the exclusive activation in `src/mssql_main_activation.rs` already replaces
   `Config` rows from `ConfigSave` by exact name and clears `ConfigSave`), `_ConfigChngR` (hypothesis: not
   required for the structure), derived caches `*.si`/`siVersions`/`*.ui` (hypothesis: the platform
   rebuilds them lazily).

Open on the twin (section 9): whether `ALTER TABLE ADD` (column after `_Fld2683`, i.e. a different physical
order) is accepted by the platform; which derived `Params` rows are required.

## 7. Risks and unknowns

- **Race/flakiness of native staging** (section 10) makes traces expensive; own staging (base-free) worked.
- **Extensions** (4 in the corpus): `SchemaStorage(1)` and `_ExtensionsRestruct*` are updated by the main
  apply (`_ExtensionsRestructNGS` 0 -> 3 rows in the baseline); an extension that adopts a changed object
  may need its own tables rebuilt. Not traced.
- **Common attributes / data separation** append fields to hundreds of tables; a change there rebuilds
  most of the database.
- **Exchange plans**: `ChngR` tables exist only for objects in exchange plans; `_ConfigChngR` registration.
- **Type sets** (defined types, characteristics) need the resolved set to know the composite columns.
- **Data conversion** of type changes (string -> number, narrowing, reference type change) is untraced;
  the platform's rules (losses, warnings, `--force`) are unknown.
- **Predefined data** (tables, `Predefined.xml`) and **subordination** are outside the traced cases.
- **Platform-build drift**: a database from an older build needs the built-in tables of the current build
  (about 40 system tables, 4 upgraded or created in our traces); a reference list per build is required.
- **Recovery**: a native apply that stopped after the switch left the database unusable for `ibcmd`;
  an own implementation must be atomic or resumable. **Size**: the NG copy is O(rows) with a log
  proportional to the table; a single transaction on a 100 GB table is impractical (the native path
  commits in steps).
- **8.5**: formats are identical (section 4.2); 2.21 metadata rows and new-in-8.5 objects may add
  families; nothing traced.

## 8. Estimate

Assumptions: one engineer who knows the repo, Rust, MSSQL, native as an oracle on lab clones, the research
tools of this track available. Numbers are ranges of working weeks; confidence is stated.

| work package | weeks | confidence | note |
|---|---|---|---|
| W1 formats in Rust: brace parser/writer (byte-exact), `DBSchema`, `NewGenCreated`, `DBNames`, `SchemaStorage` | 1.5-2 | high | model validated 100% in Python |
| W2 `DBSchema` -> DDL/DML generator, NG driver, publish, conformance tests on the corpora | 2-3 | high | all statement templates are known |
| W3 metadata -> schema for catalogs, documents and their tabular sections (all attribute kinds, indexes, common attributes) | 3-4 | medium | rules verified for main fields; sub-tables, owners, type sets are the unknown part |
| W4 registers (information, accumulation: dims, resources, indexes, totals, aggregates) | 4-6 | low-medium | index derivation is not decoded |
| W5 other families (constants, enums, charts, BP, tasks, exchange plans, journals, accounting/calculation registers) | 5-8 | low | 106 table families in the corpus |
| W6 data conversion for type changes, deletions of objects and their references | 3-5 | low | untraced |
| W7 extensions and platform-build drift | 3-4 | low | `_ExtensionsRestruct*`, `X1` tables |
| W8 verification harness (native twin per case, session read, parity export) | 2 | medium | the lab kit exists |
| W9 8.5 differences | 2-3 | low | formats equal, rows differ |

- **Minimal useful subset for 0.4** ("S1"): W1 + W2 + the catalog/document part of W3 restricted to
  **append/delete an attribute of a simple type, add/delete a tabular section, index flag, add a plain
  catalog or document, widen a string** + W8: about **6-9 weeks** with the fail-closed classifier and the
  round-trip gate (refuse anything else, use native apply for it). Each step is independently
  verifiable on a twin.
- **Full own restructuring** (W1-W9): the packages add up to 25-37 weeks; with a contingency for the
  untraced areas (a third) **7-11 months** of one engineer, with a long tail; do not promise it for 0.4 or
  0.5.
- **Round-trip gate (recommended)**: before an object family is allowed to change, the generator must
  rebuild the *old* `DBSchema` entries of that family from the *old* `Config` rows and match the stored
  text exactly (the checks of `gen_check.py`/`dbschema_check.py` run at apply time on the affected
  objects). A family the generator cannot reproduce is refused; coverage then grows without risking
  silent damage.
- **Not worth building first**: the online/high-load path (0.5) shares the NG protocol but adds
  `NewGenDropped`, sessions and switching under load; it needs its own traces.

## 9. Prototype for case (a) — not started

By instruction the prototype waits for the checkpoint. Plan: `src/restructure/` (own module; the
`mssql_main_activation` file is owned by the apply track), formats + generator + executor against the
lab twin built from `lab/04/restructure/bak/ibcmd_rs_04_ddl_bsp8327_a_a2_staged.bak` (native staging of
case a; native applies one twin, ours the other). Success criteria are those of the issue: native
`config apply` on our twin finds nothing to do, native export equals the edited tree (`source-diff`),
`DBSchema`/`DBNames`/table structure equal the native twin's (byte for byte where possible), a session
reads the catalog with the new attribute at its default.

## 10. Findings other tracks need

1. **Our patch-mode import silently drops a new attribute.** Case a: 9517 rows staged, every row identical
   to `Config` except `versions`; the apply had nothing to do. It must refuse or stage. `--base-free` staged
   the whole edited tree (9838 rows; 3 multi-part rows `5189beb9..0[1..2]`, `7e3283df..0[1]` are missing
   compared with native) and **native apply accepted it on an existing database** for 12 attributes of
   every type, a new tabular section, and a new catalog (cases h, c).
2. **Native `config import` is unreliable under load.** The first import after an apply staged only a
   subset (9597 / 9615 / 9635 of ~9842 rows) or failed with "Ссылка на неизвестный предопределенный
   элемент - ChartOfCharacteristicTypes.ОбъектыАдресацииЗадач.ВсеОбъектыАдресации" (the reference changed
   between attempts and disappeared on retry; the import uses several writer connections; **hypothesis**:
   a race between the load of predefined items and the objects that refer to them at 90-99% CPU).
   A partial stage makes the following apply fail at once with "Нарушена целостность структуры
   конфигурации" (cases c, e). A second import stages everything. Count the rows of `ConfigSave`
   (ROWS >= the previous full stage) before applying. `--threads` is not accepted by import.
3. **A native apply can die at "Принятие изменений"** (exit -1, no message; case d on clone `a`, while a
   second ibcmd of this track - an import and an apply on another clone - ran at the same time; the clean
   re-run on a fresh clone passed while four ibcmd processes of other tracks were running, so concurrency
   alone is not proven to be the cause). The database was left with
   `DBSchema` new, `Config` with `*.new` rows and `ConfigSave` full, and **every** `ibcmd config` command,
   including `config repair --commit`, `--rollback` and `--fix-metadata`, answered with the same "Обнаружена
   незавершенная операция сохранения конфигурации". Do not use such a clone.
4. `ibcmd infobase config export info|status` (ConfigDumpInfo of the database, differences against a
   given ConfigDumpInfo) and `config import files --partial` exist natively and may help the apply track.
5. The native apply moves **all** staged rows through `.new` even when unchanged (9839 rows, about 65 s of
   the 113 s); an own apply that promotes only changed rows in one transaction should be much faster.
6. `ConfigDumpInfo.xml` did not need an entry for a new object once the stage was complete (the case c
   failure was a partial stage); not tested in isolation.

## 11. Evidence

Repo (`docs/apply/evidence/restructuring/`): `a2-structure-statements.sql` (the NG protocol verbatim),
`newgen-created-a2-first.brace.txt`, `newgen-created-a2-last.brace.txt`, `timeline-dm.txt`,
`types-h-statements.sql`, `dbschema-conformance-8327.txt`, `dbschema-conformance-85.txt`,
`gen-check-catalogs-documents.txt`, `md-types-vs-dbschema.txt`, `dbnames-kinds.txt`, `roundtrip.txt`,
`cases.md`.

Lab (`F:\ibcmd\lab\04\restructure`): `xe/<case>/events.jsonl` (+ `.xel`), `out/diff_<case>.txt`,
`snap/<db>/<label>/{schema.txt,tables.tsv,svc.json}` with blobs in `blobs/`, `bak/*.bak` (twin sources),
`logs/`, `tree/patches/<case>/{before,after}` (the exact XML edits), `STATUS.md`.
Scripts: `scripts/restructure-lab/README.md`.
