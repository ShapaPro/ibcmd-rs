# Restructuring check: does applying the staged configuration change the data structure?

Issue [#338](https://github.com/Untru/ibcmd-rs/issues/338), track "rcheck", 0.4 "Своё применение конфигурации".
Checkpoint 1 of 2026-09-29.

Scope of the measurements: platform 8.3.27.2214 (native `ibcmd`), Microsoft SQL Server 2025, the БСП demo
configuration (4 929 descriptors and 4 906 body rows, 9 838 rows in `Config`) restored from the lab corpus; the 8.5 БСП for the
dialect smoke test. Everything below is **measured** unless it says **hypothesis**. The evidence files are listed
in section 9; the harness that produced them is in the lab folder `F:\ibcmd\lab\04\restructure-check` (its scripts
are summarised in section 9, they are not part of the product).

## 1. Summary

1. **What it is.** `ibcmd-rs mssql-apply-check --database X` reads the `ConfigSave` of a database and says
   whether applying it changes the **data structure** (tables, columns, indexes) or **data the platform derives
   from the configuration** (predefined items, route points of a business process, the content of an exchange
   plan, the scheduled jobs table). The answer is a `Verdict`: `needs_restructuring`, the reasons (object,
   Config row, property path, change) and the harmless changes. The future own apply (#337) refuses with
   `требуется штатный config apply: <причины>` when `needs_restructuring` is true.
2. **Fail closed.** A change is a restructuring unless a rule names its property as harmless, and a rule is
   written only for a property the native platform was **seen** to apply without touching a table, a column, an
   index or a stored row, and that is presentation or behaviour by its meaning. A row the check cannot read, a
   kind it does not know, a body row of an unknown role: a reason (class `unknown`). A false "needed" only costs
   the time of the native apply; a false "not needed" would corrupt a database.
3. **Three modes, one engine.** The ConfigSave of a database against its active Config (`mssql-apply-check`);
   a source tree against the active Config (`mssql-apply-check --tree`, which sees what a stage dropped);
   two source trees (`apply-check-trees`). All three go through the same descriptor rules and give the same
   `Verdict`.
4. **The oracle is not `--dynamic=force`.** The task assumed that `infobase config apply --dynamic=force`
   refuses a change that needs restructuring. It does not (section 5.1): with no sessions it restructured
   dynamically every time, so "force succeeded" says nothing about the structure. The oracle is the native
   apply's own log (`Объект изменен`, `Новый объект`, `Реструктуризация …`) plus digests of the SQL schema.
5. **Evidence.** @@EVIDENCE_SUMMARY@@

## 2. Commands and API

```
ibcmd-rs mssql-apply-check --database X [--server S] [--sql-user U --sql-pwd-env V] [--xml-version 2.20|2.21]
                           [--json] [--fail-on-restructuring]
ibcmd-rs mssql-apply-check --database X --tree DIR [--partial] [--json] [--fail-on-restructuring]
ibcmd-rs apply-check-trees --old DIR --new DIR [--json] [--fail-on-restructuring]
```

All three only read. Exit code: 0 (the check ran; `--fail-on-restructuring` turns "needed" into 10), 1 on an error.
The text report is Russian, `--json` prints the `Verdict`:

| field | meaning |
|---|---|
| `needs_restructuring` | some reason exists |
| `reasons[]` | `class` (`structure`, `data`, `unknown`), `object` (`Catalog._ДемоКассы`), `file_name` (Config row or tree file), `property` (`Properties/CodeLength`, `ChildObjects/Attribute[Код]/Properties/Type`, or a body role such as `Predefined`), `change` (`9 -> 12 (a property no rule covers)`, `added`, `removed`) |
| `notes[]` | the changes that were seen and are harmless (capped at 500, the rest is counted) |
| `objects[]` | **every** object whose descriptor differs, harmless or not, no cap: `object`, `id` (uuid), `file_name`, `kind`, `op` (`added`, `removed`, `changed`), `class` (the most serious among its changes, `null` when all are harmless), `changes` |
| `stats` | files, rows, descriptors compared, body rows by role, unreadable rows, files not compared |
| `source` | `rows`, `tree-db` or `trees` |

Rust: `apply_check::check_staged(sql, database, xml_version)`, `check_tree_against_db(sql, database, tree,
xml_version, partial)`, `check_trees(old, new)`, all returning `Verdict`; `Verdict::refusal()` is the message an
apply prints.

`--tree` is what an **import** must run on its input. The ConfigSave check judges what the ConfigSave holds; it
cannot tell that a stage dropped a change the tree had. The patch-mode import of this tool does exactly that with
an added attribute (finding 1 of the ddl track, reproduced in section 6.3): the ConfigSave has no attribute, so
its check is right to say "nothing to restructure" and wrong to be the only check. `check_tree_against_db` answers
"which objects' descriptors differ between this tree and the database, and is each difference structural"
(`Verdict::objects`); it needs only the tree and a connection. It compares descriptors; the files that carry
stored data (predefined items, flowcharts, the content of exchange plans, aggregates, additional indexes) are
counted in `stats.body_files_not_compared`, not judged: compare an export of the database with the tree
(`apply-check-trees`) or check the ConfigSave the import produced.

## 3. How the check decides

### 3.1 The configuration in the tables

`Config` holds the active configuration, `ConfigSave` the staged one; each is a set of named rows (`FileName`,
`PartNo`, raw-deflated `BinaryData`, parts joined in order):

| row | what |
|---|---|
| `<uuid>` | the **descriptor** of a metadata object (a brace-text tree: properties, child objects) |
| `<uuid>.<suffix>` | a **body** of the object; the meaning of the suffix depends on the owner's kind (`.0` is the module of a catalog and the form text of a form) |
| `root`, `version`, `versions` | the configuration's root, its compatibility record and the **inventory** of every file of the configuration (`{1,<count>,"",<generation>,"<name>",<version>,…}`) |
| `<base>_dynupdate_<generation>[.suffix]`, `DynamicallyUpdated` | the state of an online (dynamic) update, see below |

The inventory is what makes a removal visible: a file the staged `versions` no longer lists is gone, a file the
active one did not list is new, and a file a ConfigSave lists but does not hold is unchanged. So the **effective
staged configuration** is the active Config overlaid by the ConfigSave, restricted to the files the staged
`versions` lists.

**The active Config is not the plain rows.** An online update leaves the plain rows as they were and writes the
rows it changed under `<base>_dynupdate_<generation>`; `DynamicallyUpdated` lists the generations, oldest first,
and for each name the newest generation that carries it wins. Reading the plain rows would compare a stage with
a configuration that no longer exists. An object added by an online update has **only** an alias row, and an
object it dropped keeps its plain row; the check therefore takes the descriptors from the inventory, not from
the list of plain rows (`sql::published_names`).

### 3.2 Three boxes for every staged row

* **Descriptor** (a row without a suffix). Decoded on both sides with the metadata model (`metadata_model`, the
  same decoders the export uses) into the element tree the XML export would write, then diffed **property by
  property** (`tree_diff`): child objects are matched by uuid, then by name, then by position; localized strings
  item by item; a reordered collection is one change. Every change goes through `rules::decide` (section 4). Two
  rows that differ in bytes but decode to the same tree are a reason too (`unknown`): the model does not carry
  something in the row, and the check cannot say what.
* **Body** (`<uuid>.<suffix>`). The owner's kind and the suffix give the role (`roles::body_role`, read off the
  `ConfigDumpInfo.xml` of the four reference exports). Roles that copy rows when applied (modules, forms,
  templates, help, pictures, rights, command interfaces, schedules, packages) are **counted, not compared**.
  Roles that carry data (`Predefined`, `Flowchart`, `Content`, `Aggregates`, `AdditionalIndexes`) are compared by
  content; a difference is a reason of class `data` (or `structure` for `Content`, `Aggregates`,
  `AdditionalIndexes`). The `Content` row of an exchange plan is compared as a **set** (the platform stores the
  items in an order of its own and does not restructure when a stage writes another, section 6.2).
* **Service** (`root`, `version`, `versions`). `root` and `version` must not change; `versions` is the inventory,
  and it also gives every file a version id. A file that the staged `versions` gives another version than the
  active one (or lists as new) and that the ConfigSave does not hold means a partial stage (the native import
  under load made some, see finding 6): `unknown`. (A base-free stage gives *every* file a new version id, so the
  ids cannot say which files changed; they only have to be backed by rows.)

Anything else in the ConfigSave (a row named like an online-update alias or `DynamicallyUpdated`, a row the
inventory does not list, a row of an unknown shape) is a reason of class `unknown`.

### 3.3 Fail-closed defaults

* `rules::decide` returns `structure` unless a rule says `safe`.
* A kind the model has no decoder for, a row that does not parse, a plan that cannot be built completely (a name
  nobody could find, an undecodable Configuration row: the compatibility mode then defaults and thousands of rows
  would misdecode, so the root cause is reported once as `unknown`): reasons.
* A ConfigSave without a `versions` row cannot say what is gone: `unknown`.
* The classes: `structure` (tables, columns, indexes change), `data` (stored data the platform derives from
  the configuration changes: predefined items, enumeration values, route points, scheduled jobs), `unknown`.

### 3.4 Tree against tree, tree against database

The tree modes use the same descriptor rules. Trees are matched **by object uuid**, not by path, so a moved or
renamed file is one object; the other files are told apart by where they lie under the object's `Ext` folder
(`roles::file_role`). `ConfigDumpInfo.xml` is derived from the tree and never a change of its own.

Tree against database exports each descriptor of the active Config with the metadata model (the text an export
would write, `export_descriptor`) and compares it with the tree's file: byte-equal is unchanged, otherwise both
are parsed and diffed. A tree that is an export of the database therefore compares as unchanged whatever the
formatting, and a hand-edited one reports exactly its edits. An empty container written the long way
(`<ChildObjects>` newline `</ChildObjects>`) is the same as `<ChildObjects/>`.

### 3.5 The verdict is built to take a stronger test

The whitelist of harmless properties is the first cut. The platform itself decides by recomputing `DBSchema`
from the metadata and rebuilding exactly the tables whose entry differs (ddl track, `docs/apply/restructuring.md`,
finding 7). A check on `DBSchema` entries is stronger and plugs into the same place without changing the verdict:
`Verdict::objects` lists every changed object with its kind, uuid and Config row, so a generator of table entries
walks the table-owning ones, and a `structure` or `unknown` reason on such an object is what it may confirm or
withdraw (its own reasons would carry `property = DBSchema/<table>`). The generator is not part of this module
(it belongs to the ddl track).

## 4. The rules

`rules::decide(kind, change)` gives every change of a descriptor one of `safe`, `structure`, `data`, and the rule
that gave it (it is printed in the reason: `10 -> 601 (a property no rule covers)`). The default is `structure`.
A property is listed as `safe` only when **both** hold:

* the native platform was **seen** to apply the change without a `Объект изменен` / `Реструктуризация` line and
  without a change of columns, indexes or tables in the digests (section 5), and
* the property is presentation or behaviour by its meaning, not a way the data is kept. A property the platform
  left alone in one probe but that decides how data is stored stays off the list: `HierarchyType` and
  `LimitLevelCount` of a catalog, the attribute-level `DataHistory` (harmless in a probe on an object whose own
  history is off), `Owners`, the type of anything.

The tables below are `rules.rs`; the evidence for each row is the probe (section 6.1), the focused experiment
(6.2) or the validation run (6.3) named there.

### 4.1 Kinds that own no stored data: every change is harmless

`CommonModule`, `CommonForm`, `CommonTemplate`, `CommonPicture`, `CommonCommand`, `CommandGroup`, `Role`,
`Subsystem`, `EventSubscription`, `FunctionalOption`, `FunctionalOptionsParameter`, `StyleItem`, `Style`,
`PaletteColor`, `Interface`, `FilterCriterion`, `Report`, `DataProcessor`, `Form`, `Template`, `WebService`,
`HTTPService`, `WSReference`, `XDTOPackage`, `IntegrationService`, `Bot`. Adding or removing such an object is
harmless too. Why: none of them allocates a table or a field. The ddl track's census of the `DBNames` of the
БСП (`docs/apply/evidence/restructuring/dbnames-kinds.txt`) lists the metadata classes that do (catalogs,
documents, registers, constants, enumerations, exchange plans, charts, business processes, tasks, document
journals, scheduled jobs, common attributes, and the attributes, dimensions, resources, columns and tabular
sections inside them); none of the kinds above is there. The probes touched every top-level property of at least
one object of every kind in the list (6.1) and no property of any of them made the platform act; an attribute
added to a report and to a data processor (`p5_children`), a common module and a role added and dropped
(`p7a`, `p7b`, `v08`, `v13`) likewise.

### 4.2 Properties harmless on every kind that has them

`Synonym`, `Comment`, `Explanation`, `ObjectPresentation`, `ListPresentation`, `IncludeHelpInContents`,
`DefaultObjectForm`, `DefaultFolderForm`, `DefaultListForm`, `DefaultChoiceForm`, `DefaultFolderChoiceForm`,
`DefaultRecordForm`, `DefaultForm`, `QuickChoice`, `ChoiceMode`, `EditType`, `CreateOnInput`,
`ChoiceHistoryOnInput`, `DefaultPresentation`, `SearchStringModeOnInputByString`, `DataLockControlMode`,
`FullTextSearch` @@P8_PRESENTATION@@.

### 4.3 Properties harmless on one kind

| Kind | Properties |
|---|---|
| `Catalog` | `UseStandardCommands`, `Autonumbering`, `CheckUnique`, `PredefinedDataUpdate`, `UpdateDataHistoryImmediatelyAfterWrite`, `ExecuteAfterWriteDataHistoryVersionProcessing` |
| `Document` | `UseStandardCommands`, `Autonumbering`, `PostInPrivilegedMode`, `UnpostInPrivilegedMode`, `RealTimePosting`, `RegisterRecordsDeletion`, `RegisterRecordsWritingOnPost`, `SequenceFilling`, `UpdateDataHistoryImmediatelyAfterWrite`, `ExecuteAfterWriteDataHistoryVersionProcessing` |
| `Constant` | `UseStandardCommands`, `ExtendedEdit`, `FillChecking`, `MarkNegatives`, `MultiLine`, `PasswordMode`, `UpdateDataHistoryImmediatelyAfterWrite`, `ExecuteAfterWriteDataHistoryVersionProcessing` |
| `InformationRegister` | `UseStandardCommands`, `UpdateDataHistoryImmediatelyAfterWrite`, `ExecuteAfterWriteDataHistoryVersionProcessing` |
| `Enum`, `AccumulationRegister`, `AccountingRegister`, `CalculationRegister`, `ChartOfAccounts`, `ChartOfCharacteristicTypes`, `ChartOfCalculationTypes`, `ExchangePlan`, `BusinessProcess`, `Task` | `UseStandardCommands` |
| `Configuration` | `Version`, `Vendor`, `UpdateCatalogAddress`, `Copyright`, `BriefInformation`, `DetailedInformation`, `VendorInformationAddress`, `ConfigurationInformationAddress` |
| `DocumentJournal` | none (the probe of its `UseStandardCommands` made the platform rebuild a registration table) |
| `ScheduledJob` | `Comment` only; every other property is stored data (`data`) |

### 4.4 Child objects

* **Attributes, dimensions, resources, columns, accounting flags, addressing attributes, parameters**: adding,
  dropping or moving one is `structure`; a property is harmless when it is one of `Name` (a rename: the column is
  named by a number), `Synonym`, `Comment`, `ToolTip`, `FillChecking`, `QuickChoice`, `ChoiceHistoryOnInput`,
  `CreateOnInput`, `MultiLine`, `PasswordMode`, `ExtendedEdit`, `MarkNegatives`, `FillFromFillingValue`,
  `FullTextSearch`, `ChoiceFoldersAndItems`, `DenyIncompleteValues`. Anything else (`Type`, `Indexing`, `Use`,
  `DataHistory`, `Master`, `MainFilter`, `Format`, `Length`, ...) is `structure`.
* **Standard attributes** (`Code`, `Description`, `Parent`, ...): the same list of properties.
* **Tabular sections**: adding, dropping or moving one is `structure`; `Name`, `Synonym`, `Comment`, `ToolTip`,
  `FillChecking` and the same list of properties of its own attributes and standard attributes are harmless; adding
  or dropping an attribute of one is `structure`.
* **Enumeration values**: adding, dropping or moving one is `data` (a value is a row of the platform's table);
  `Name`, `Synonym`, `Comment` of a value are harmless.
* **Forms, templates, commands** listed in `ChildObjects`: harmless (they are rows and lists of names).
* `InternalInfo` (the generated types of an object) is `structure`.

### 4.5 Adding and dropping objects

By kind: an object of a kind of 4.1 is harmless, any other kind is `structure`. In `Configuration.xml` the list
`ChildObjects/<Kind>` follows the same rule, so listing a new common module is harmless and listing a new catalog
is not. What the platform does when an object is dropped is bigger than the object: dropping the catalog of
`p7b` made it rebuild the change-registration tables of 29 other objects.

### 4.6 Body rows

Read off the `ConfigDumpInfo.xml` of the four reference exports; `roles::ROLES` has 96 (kind, suffix) pairs.

| effect | roles |
|---|---|
| copies a row when applied | every module (`ObjectModule`, `ManagerModule`, `RecordSetModule`, `CommandModule`, `Module`, `ValueManagerModule`, the configuration's application and session modules), `Form`, `Template`, `Picture`, `Help`, `Rights`, `CommandInterface`, `Schedule`, `Package`, `Style`, `WSDefinition`, `Splash`, `HomePageWorkArea`, `MainSectionCommandInterface`, `MainSectionPicture`, `ClientApplicationInterface`, `ParentConfigurations`, `MobileClientSignature`, `StandaloneConfigurationContent` |
| `data` when the content changes | `Predefined` (catalogs `.1c`, charts of characteristic types `.7`, of accounts `.9`, of calculation types `.2`), `Flowchart` (business process `.7`) |
| `structure` when the content changes | `Content` (exchange plan `.1`, compared as a set), `Aggregates` (accumulation register `.3`), `AdditionalIndexes` (accumulation register `.4`, document `.3`) |

An unknown (kind, suffix) pair is `unknown`.

## 5. The oracle: what the native platform does

### 5.1 `--dynamic=force` does not refuse a restructuring

The task's premise was that `infobase config apply --dynamic=force` fails when the staged configuration needs
restructuring, so that "force succeeded" could serve as the oracle for "no restructuring needed". It does not
fail. Setting: no sessions on the infobase (the lab cannot register a clone in the 1C server cluster: the server's
service account has no SQL login), `--force --dynamic=force --session-terminate=disable`, native 8.3.27.2214. Of 15
applies of `--dynamic=force` recorded in the lab, 13 ended with exit 0 and the two others were rejected by the
metadata check ("Проверка корректности метаданных": probes with invalid values), never by the dynamic mode.
Every structural change in them was **carried out dynamically**: the platform created the new generation of the
table, copied the data and switched it, and printed the same lines as the exclusive apply of the ddl track:

| change staged (one native `force` apply each) | what the platform did (log and SQL digests) |
|---|---|
| a `String(20)` attribute added to `Catalog._ДемоКассы` (`e08_attr_add`) | `Объект изменен: Справочник._ДемоКассы`, `Реструктуризация Справочник._ДемоКассы`; `DBSchema`, `SchemaStorage`, columns changed; exit 0 |
| `CodeLength`, `DescriptionLength`, `DataHistory` of three catalogs (`p1_catalog_props`) | three objects changed and restructured, columns changed; exit 0 |
| `Indexing` and `StringLength` of attributes (`p2_attribute_props`) | two objects restructured, columns and **indexes** changed; exit 0 |
| an enumeration value added, `Use` of an attribute, a dropped index (`p4_children`) | three objects restructured, row counts changed (the value is a row); exit 0 |
| a tabular section added, a table attribute added, a dimension added to an accumulation register, an attribute dropped (`p5_children`) | four objects restructured, a **new table** (`sql-tables`), totals of the register recalculated; exit 0 |
| a new catalog (`p7a_objects_add`) | `Новый объект: Справочник.СправочникRcheck`, `Изменена структура таблиц базы данных`, a new table; exit 0 |
| the same catalog dropped (`p7b_objects_remove`) | `Объект удален: Справочник.СправочникRcheck`, and 29 other objects restructured; exit 0 |
| 20 properties of 20 objects in one stage (`probe_auto_a`, third run) | 20 objects changed and restructured at once; exit 0 |

So `force` succeeding says nothing about the structure: a check must not be validated against it and an apply must
not rely on it. **What was not measured:** what `--dynamic=force` does when sessions are connected (the platform
may then refuse a change that needs exclusive access; the lab has no way to open one). This does not change the
design: the check answers "does applying it change tables or stored data", whatever the way it is applied.

### 5.2 What the oracle is

For every modification the harness stages an edited tree with **our import in `--base-free` mode** (the only mode
that stages descriptor edits; section 6.3 shows why patch mode cannot serve), records the verdicts of the three
modes, runs the native `infobase config apply --force --dynamic=<mode> --user=Администратор` (under the lab's
`native` lock) and reads three things:

1. the platform's own report: `Объект изменен: X` (the object's descriptor changed in a way the platform acts on),
   `Новый объект: X`, `Объект удален: X`, `Создана таблица: …`, `Изменена структура таблиц базы данных`,
   `Реструктуризация X` (rows of X are copied to the new generation of its table);
2. digests of the SQL schema before and after (`tools/snapshot.ps1`): every table's columns, an index signature by
   columns, the set of table object ids, row counts;
3. which of `Config`, `Params`, `DBSchema`, `SchemaStorage` changed.

A modification counts as **restructuring** when the log names the object as changed, new or deleted, or the
columns, indexes, tables, `DBSchema` or `SchemaStorage` differ, or row counts changed. Noise that every apply
makes is ignored: the tables `_ConfigChngR` and `_ConfigChngR_ExtProps` are rebuilt on every apply (the primary key
gets a new name; the log line `Реструктуризация Таблица регистрации изменений конфигурации` is theirs), and
`Params` rows (`*.si`, `siVersions`) are rewritten.

### 5.3 Two ways to apply, one way to read

`--dynamic=disable` (exclusive) promotes every staged row into `Config` under its own name; `--dynamic=force` leaves
the plain rows alone and writes **every staged row** as `<name>_dynupdate_<generation>` (a full ConfigSave doubles
`Config` at each apply: about 10 000 rows; a lab database that had ten of them held 60 000 rows) and appends the
generation to `DynamicallyUpdated`. The check reads the active rows in both cases (section 3.1). The lab
chain for section 6.1-6.2 used `force` (database `a`), the validation lanes of section 6.3 used `disable`
(databases `b1`-`b3`), and the two agree wherever both were run.

## 6. Evidence

### 6.1 Probes: one property of one object, 159 times

`probe.py` of the lab (not part of the product) takes the top-level `<Properties>` leaves of the objects of each
kind, changes **one leaf of one object** (a boolean flipped, an enumeration moved to another value, a number
changed by one, a string lengthened), stages all of them at once and applies once: the native log names the
objects it acted on, and because every object carries exactly one change, each change is attributed. 168 probes
were generated; 9 of them (an exchange plan's `IncludeConfigurationExtensions`, a common attribute's `AutoUse`
and `DataSeparation`, an HTTP service's `RootURL`, a web service's `Namespace` and `DescriptorFileName`, an
information register's `WriteMode`, an XDTO package's `Namespace`) were rejected by the metadata check of the
platform for the value chosen and left out, 159 were applied (native exit 0). 139 were applied without the platform
acting, 20 made it restructure or rewrite stored data. `Applied` below means "no `Объект изменен`, no
`Реструктуризация`, no change of columns, indexes or tables"; the rule tables of section 4 take from this only the
properties that are also presentation or behaviour by meaning.

@@PROBE_TABLE@@

The 20 that acted are exactly the storage properties of section 4: the hierarchy, the code type and length and the
series of a catalog and of a chart of characteristic types, the history of data, the number length, periodicity,
uniqueness and posting of a document, the periodicity of an information register, the kind of an accumulation
register, the standard commands of a document journal, and four of the five properties of a scheduled job.

### 6.2 Focused experiments

Each ran on the БСП database `a` (a cumulative clone) with `--dynamic=force`, several objects per stage and one
change per object; the "native" column is the platform's log.

| run | changes | native acted on | ours |
|---|---|---|---|
| `p1_catalog_props` | 18 catalogs, one property each: `Autonumbering`, `Explanation`, `ObjectPresentation`, `ListPresentation`, `DescriptionLength`, `DefaultPresentation`, `QuickChoice`, `ChoiceMode`, `CodeLength`, `IncludeHelpInContents`, `CheckUnique`, `DataHistory`, `PredefinedDataUpdate`, `FullTextSearch`, `DataLockControlMode`, `UseStandardCommands`, `ChoiceHistoryOnInput`, `EditType` | exactly three: `DescriptionLength`, `CodeLength`, `DataHistory` | the same three |
| `p2_attribute_props` | 16 catalogs, one attribute property each: `Synonym`, `ToolTip`, `FillChecking`, `QuickChoice`, `Indexing`, `FullTextSearch`, `ChoiceHistoryOnInput`, `CreateOnInput`, `MultiLine`, `PasswordMode`, `ExtendedEdit`, `MarkNegatives`, `DataHistory`, `FillFromFillingValue`, `StringLength`, `Name` | exactly two: `Indexing` (index changed) and `StringLength` (column changed) | the same two; the attribute-level `DataHistory` is `structure` for us although the platform left it alone (the object's own history was off) |
| `p4_children` | an attribute's `ChoiceFoldersAndItems` and, on another catalog, its `Use` (`ForItem` to `ForFolderAndItem`, which makes the column nullable); an `Indexing` switched off; five standard-attribute properties (`FillChecking`, `MultiLine`, `FullTextSearch`, `DataHistory`, `ExtendedEdit`) on five catalogs; an enumeration value added; another renamed | three objects: the `Use` change, the `Indexing`, and the enumeration with the new value (a row added) | the same three; the enumeration value is `data` |
| `p5_children` | an attribute added to a report and to a data processor; a tabular section's `FillChecking`; a tabular section attribute's `MultiLine`; an attribute added to a tabular section; a whole tabular section added; a register dimension's `DenyIncompleteValues`; a dimension added to an accumulation register; the last attribute of a catalog dropped | four objects: the tabular section attribute added, the tabular section added, the dimension added (with the totals recalculated), the attribute dropped | the same four |
| `p6_config`, `p6b_kinds` | `Version`, `UpdateCatalogAddress`, `Copyright`, `BriefInformation`, `Vendor`, `DetailedInformation`, `VendorInformationAddress`, `ConfigurationInformationAddress` of the configuration; three scheduled jobs (`MethodName`, `RestartCountOnFailure`, `RestartIntervalOnFailure`); an event of an event subscription; `Comment` of a job, a subscription, a web service, an HTTP service, an XDTO package, a settings storage, a sequence | only the three scheduled jobs | the configuration's information and the comments are harmless; the jobs are `data` |
| `p7a_objects_add` | a common module, a catalog, a role added | the catalog (`Новый объект`, a new table) | the catalog is `structure`; the module and the role are harmless |
| `p7b_objects_remove` | the same three dropped | the catalog (`Объект удален`, and 29 other objects restructured) | the catalog is `structure`; the module and the role are harmless |
| `p8_catalog_presentation` | @@P8_ROW@@ |  |  |

The two properties that a reader may distrust because they were changed in a run that also changed something
structural, `ChoiceFoldersAndItems` and `DenyIncompleteValues`, sit in objects of their own in these runs (the
native log names the neighbours, not them).

### 6.3 Validation: modifications staged by our import, judged by us, applied by the native platform

The rules of section 4 were derived from 6.1 and 6.2 on the database `a`. The validation is a second set of
modifications, none of them probed before, staged on **fresh clones of the corpus backup** (`b1`, `b2`, `b3`,
pristine БСП 8.3.27; `bsp85` for the 8.5 dialect; `n1` for the native import), one after the other on each clone
(the base tree follows the database), each staged by our import, checked in three ways and applied by the native
`infobase config apply --force --dynamic=disable`:

* **ConfigSave check**: `mssql-apply-check --database X` on the staged rows, before the apply;
* **Tree vs database**: `mssql-apply-check --database X --tree <edited tree>` before anything is staged;
* **Two trees**: `apply-check-trees` on the base tree and the edited one (the changed files only).

Several modifications are staged together when they touch objects of their own (a batch): the native log names
the objects it acts on, so each modification is judged on its own object. **Match** compares the ConfigSave check
with the platform: `agree` (both restructure or both do not), `over` (we say restructuring, the platform did
not: costs a native apply), `UNSAFE` (we say none, the platform restructured: must not happen).

@@VALIDATION_TABLE@@

@@VALIDATION_NOTES@@

### 6.4 The reference trees against themselves and against their databases

| tree | files | rows in `Config` | `apply-check-trees` old = new | `mssql-apply-check --tree` against the database restored from the same corpus |
|---|---|---|---|---|
| БСП 8.3.27 (XML 2.20) | 12 197 | 9 838 | no restructuring, 0 reasons, 5 s | no restructuring, 0 reasons, 0 objects, 11 s |
| БСП 8.5 (XML 2.21) | 12 336 | 9 935 | no restructuring, 0 reasons, 6 s | no restructuring, 0 reasons, 0 objects, 14 s |
| ERP УХ 8.3.27 (XML 2.20) | 140 708 | 118 170 | no restructuring, 0 reasons, 54 s | no restructuring, 0 reasons, 0 objects, 144 s |
| ERP УХ 8.5 (XML 2.21) | 140 708 | 118 170 | no restructuring, 0 reasons, 68 s | no restructuring, 0 reasons, 0 objects, 202 s |

Every descriptor of every corpus exports, with the metadata model, to exactly the bytes the native export wrote
(`descriptors_compared: 0`: nothing had to be parsed and diffed). The objects, the kinds and the dialect (2.20 and
2.21) all match: the model decodes the four configurations completely, and a tree that is an export of its database
gives no reason. `cargo test -p ibcmd-rs --lib --no-default-features apply_check::corpus_tests -- --ignored`
repeats it (the trees against themselves; a sample of every seventh metadata file rewritten by the model's writer
and compared with the original, to make the check parse and diff real files; the БСП export against its database
when `IBCMD_RS_APPLY_CHECK_DB` names one).

## 7. Known gaps

1. **The whitelist is as wide as the evidence.** Kinds and properties nobody probed are `structure`: for
   `SettingsStorage`, `Sequence`, `DefinedType`, `SessionParameter`, `Language`, `CommonAttribute` and
   `DocumentNumerator` only the presentation properties of 4.2 are on the list, and the objects of the БСП are a
   sample: a kind that the БСП does not use was never seen. This costs an unnecessary native apply, never a
   corruption.
2. **Extensions.** The check reads `Config` and `ConfigSave`, the main configuration. The rows of the extensions
   (`ConfigCAS`, `ConfigCASSave`, `_ExtensionsRestruct*`) are not judged; an apply with an extension staged must
   refuse on its own account.
3. **Type conversion.** A change of type is always `structure`. Whether the platform can convert the data
   (the ddl track's case k: strings are truncated, numbers saturate) is for the apply that restructures.
4. **Row bodies that copy rows are counted, not compared.** The claim that applying a module, a form, a template,
   a picture, a help page, a set of rights or a command interface changes no table is supported by every run
   (`v01`-`v06`, and the 4 900 rows of these roles that every base-free stage of the БСП carries and the platform
   promotes each time), but it is a claim about the platform.
5. **Byte comparison of the data bodies.** `Predefined`, `Flowchart`, `Aggregates` and `AdditionalIndexes` are
   compared byte for byte (`Content` as a set). A stage that writes the same items in another encoding gives a
   false `data`/`structure` reason. Measured on the БСП: a base-free stage of the unchanged export reproduces all
   28 predefined bodies exactly, but not the flowchart of the business process `Задание` (a counter, 37 stored,
   23 compiled), and the platform does rebuild that process's route-point table for it, so the reason is right.
6. **Both sides go through one model.** A property the metadata model does not decode cannot be seen changing; a
   row that differs while both sides decode to the same tree is reported as `unknown` (fail closed), which also
   catches the non-decoded case.
7. **Sessions.** Nothing here says how a change can be applied to an infobase that has sessions. The check is
   about the content of the change.
8. **Tree against database compares descriptors only**, see section 2; the data-carrying bodies of a tree are
   counted in `stats.body_files_not_compared` (28 in the БСП, 271 in ERP УХ).
9. **`DBSchema` is not consulted** (section 3.5): a property the whitelist does not know is `structure` even when
   the platform would find every table entry unchanged.
10. **Speed.** ConfigSave check on the БСП: about 5 s; on ERP УХ (118 000 rows) not measured (a full УХ stage takes
    minutes). Tree against database: 11 s (БСП 8.3.27), 14 s (БСП 8.5), 144 s (УХ 8.3.27), 202 s (УХ 8.5), of
    which the model's export of every descriptor is most.

## 8. Findings other tracks need

1. **A ConfigSave alone cannot tell that a stage dropped a change** (import track, #388; ddl track finding 10.1,
   reproduced): @@PATCH_FINDING@@. A stage must be checked against its input: `check_tree_against_db` before it,
   or `--base-free`, which stages every row from the tree. The `objects` list of the verdict is the API for it: which
   descriptors differ, their uuid, kind and Config row, whether each difference is structural.
2. **`--dynamic=force` writes an alias of every staged row and does not refuse structure** (apply track, #337):
   section 5. A full ConfigSave doubles `Config` at each apply (127 892 rows, 118 050 of them aliases, after 13
   applies of one lab clone).
3. **Our exporter fails on such a database** (export track): `ibcmd-rs infobase config export` on the clone above
   ends with SQL Server error 8621 ("the query processor ran out of stack space during query optimization") while
   reading `Config` through the derived table that publishes the aliased rows under their published names (the
   export's `dynamic_generation` overlay, 118 050 aliases here). The check reads rows in batches of 200 names and is
   not affected.
4. **A patch-mode stage of a dynamically updated database reads the plain rows** (import track): @@STALE_FINDING@@.
5. **The base-free stage of an unchanged БСП is not neutral** (import track): the flowchart row of the business
   process `Задание` differs from the stored one in a counter (37 stored, 23 compiled) and the platform rebuilds the
   route-point table for it at the first apply; the six exchange plans whose content row it writes in another
   order (a set, no consequence, and the check compares it as one); 3 multi-part rows are missing against the native
   stage (ddl track, 10.1).
6. **Everything native that writes takes the lab's `native` lock** (coordinator, 11:30): the harness holds it around
   each native `apply` and `import`; the row count of the ConfigSave after a native import must be checked (a full
   stage of this configuration is 9 842 rows; a partial one made the next apply fail at once in the ddl track).
7. **The removal of an object rebuilds more than the object.** The native apply that dropped a catalog restructured
   29 other objects (change-registration tables of the exchange plans that register them): an own apply that
   handles a removal will meet this.
8. **The vocabulary of the native apply log** (trace and apply tracks): `Объект изменен: X`, `Новый объект: X`,
   `Объект удален: X`, `Регистрация изменена: X`, `Создана таблица: T`, `Изменена структура таблиц базы данных`,
   `Обработка данных Реструктуризация X [пересчет итогов | таблица регистрации изменений] [n]`,
   `Создано поколение конфигурации: <uuid>`. The first four are printed only for objects that own a table or
   stored data: a new common module or role prints nothing.

@@REPRODUCE@@
