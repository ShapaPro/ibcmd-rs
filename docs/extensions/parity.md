# Extension export parity (`mssql-dump-extension`)

Status of issue #348 (milestone 0.5): the tree that `mssql-dump-extension`
writes for a configuration extension equals the tree that the native
`ibcmd infobase config export --extension=<name>` writes for the same database.

## Result

Measured 2026-09-29 on a clone of the БСП 8.3.27 database (four extensions),
native reference exports against ours with `ibcmd-rs source-diff`
(`ConfigDumpInfo.xml` included, byte for byte):

| extension | files | differences before | differences now |
|---|---|---|---|
| `_ДемоПустоеРасширение` | 3 | 2 different, `ConfigDumpInfo.xml` missing | 0 |
| `_ДемоРасширение` | 185 | 47 different, 28 missing | 0 |
| `ServiceDesk` | 465 | 46 different, 6 missing | 0 |
| `VAExtension` | 84 | 16 different, 2 missing | 0 |

"Before" is the export of commit 08de112f (the state the issue was opened
against). The ordinary export is unchanged: the offline export of the main
configuration from saved rows (`mssql-dump-config --rows-dir`) still equals the
native one in all 12198 files of the БСП 8.3.27 corpus and in all 140709 files
of the ERP УХ 8.3.27 corpus. The default export writes descriptors through the
metadata model, which the extension work does not touch; the extension export
shares the *legacy converters* with `--legacy-export`, so that path was measured
too: the БСП 8.3.27 corpus is identical in all 12198 files and the ERP УХ 8.3.27 corpus in all 140709 files (the only extra entry is the dump's own manifest.json).

The export reports `native_xml_parity: true` when no storage row is opaque or
failed. That is a claim about the readers and not a proof: they read fail-closed
(a row whose shape they cannot state exactly is reported, never written
approximately), and the proof is the table above.

## How the export works

An extension stores its objects in `ConfigCAS` in the ordinary row layout of the
main configuration. What differs is written into the rows and not stated by the
XML the platform prints, so the export works in two steps that keep every
ordinary converter blind to extensions (`src/mssql_dump/extension/`):

1. **Normalize.** The md header of an adopted (borrowed) object carries an
   adoption tail after the comment. `normalize_descriptor` rewrites it to the
   ordinary tail and remembers it; the ordinary family converters then print the
   object as if it were an ordinary one.
2. **Project.** `project_object_xml` reduces that print to what the platform
   writes for an adopted object: the properties the header lists (plus name,
   comment and `ExtendedConfigurationObject`), `xr:PropertyState` blocks,
   `MultiState` type lists.

The root object is rendered by `extension/root.rs`, `ConfigDumpInfo.xml` from the
digests of the CAS manifest, and forms get a post-pass over their XML
(`extension/form.rs`, `adjust_form_files`). Every relaxation of an ordinary
reader is gated by `extension::active()`, so an export of an ordinary
configuration takes exactly the old paths.

Adopted header tail, member by member:

```text
belonging(1) N (property guid, state) x N  extended-object uuid  M (guid, state, value) x M
```

An own object of an extension carries `0,0,<nil uuid>,0` there. State 2 is a
property that only records its value, state 3 one the extension overrides; the
`N` pairs are exactly the properties the platform prints. The `M` triples carry
the values an extension adds to a type list.

## Differences found and fixed, by kind of file

### `ConfigDumpInfo.xml`

Not written at all before (the extension CAS has no `root`/`version`/`versions`
rows). `configVersion` of an entry is the SHA-1 of the packed CAS row of that
object; in the main configuration it is the 16-byte identity and `00000000`.

### `Configuration.xml` (root object)

`ObjectBelonging`, `ConfigurationExtensionPurpose`,
`KeepMappingToExtendedConfigurationObjectsByIDs`, `NamePrefix`,
`ConfigurationExtensionCompatibilityMode` (tuple members 44, 41, 42, 43), the
run-mode group (`DefaultRunMode`, `UsePurposes`, `InterfaceCompatibilityMode`),
`DefaultRoles`, `Vendor`, `Version`, the information addresses, the
`PropertyStates` of the root header, `ChildObjects`. Rows of the root's own
modules and command interface are routed by the root's header uuid.

### Adopted objects

Catalog, document, register, common form, common module, role, subsystem, style
item and the other families print only the listed properties. Values of a
`Type`-like list the extension adds to (`DefinedType`, `FilterCriterion`,
`CommonCommand.CommandParameterType`, attributes) are written as `MultiState`
with `xr:ExtendedProperty`.

### Own objects of an extension: shapes the ordinary readers had not met

Each entry is a value that never occurs in the ordinary corpora; the reader was
relaxed for an extension export only unless noted.

* Catalog attributes, `DefinedType`, `FilterCriterion` with an empty
  `{"Pattern"}` (the type stays that of the extended configuration).
* Types of the extended configuration in a pattern (`{"#",<type id>}` with no
  object of the extension behind it): printed as `<v8:TypeId>`, and a fill value
  of such an attribute is `xsi:nil`.
* Charts of accounts, calculation types and characteristic types with no
  standard attributes and no standard tabular sections (`{0}`), and no
  `StandardTabularSections`/`StandardAttributes` element.
* Chart of accounts: no `ExtDimensionTypes` (nil uuid), `AutoOrderByCode` rides
  member 24 (member 27 is the constant), empty `CodeMask` is `<CodeMask/>`, an
  empty `<ChildObjects/>` is written (charts only).
* Chart of calculation types: `DependenceOnCalculationTypes` rides member 27
  (member 35 is the constant), empty `<BaseCalculationTypes/>`.
* Accounting register: `DataLockControlMode` rides header+6 and `FullTextSearch`
  header+7 (the reader had them the other way round; the ordinary corpora write
  `0` in both, so the mistake could not show; the ordinary export is unchanged),
  no standard attributes; calculation register `BasePeriod` rides member 18.
* Role rights: the "defaulted flag is false" value `2`, and for an adopted
  object every nested `View`/`Edit` right is printed.
* Pictures: `TransparentPixel` is written only for a pixel `>= 0` (`-1,-1` is
  "none"); `MinValue`/`MaxValue` of a number, date or boolean keep their XML type
  (`xs:decimal`, `xs:dateTime`, `xs:boolean`; the ordinary readers knew only
  strings and failed the whole attribute).
* Style: standard style items `-47` (`ImportantColor`) and the order
  `ActivityColor`, `NavigationColor`, `AuxiliaryNavigationColor`,
  `ImportantColor`.
* Module text: a character outside the basic plane is stored as two code unit
  escapes, each behind a quote (`"\d83d"\dcce`); the container reader took the
  first quote for the end of the string and lost the module. Both the scan and
  the decoding of `module_blob` now follow the rule `mssql_dump` already used.

### Forms

* **Root namespaces.** An extension in a compatibility mode older than the one
  that introduced the data-composition schema namespace declares no `dcssch` on
  the root of `Ext/Form.xml`.
* **`BaseForm`.** A form the extension adopted carries the extended
  configuration's form after its own tree (container section 6 is `1`, section 7
  is a complete form record). It is rendered by the ordinary form reader and
  written as `<BaseForm version="...">` after `</Parameters>`, one level deeper.
  A form whose base form does not read is not emitted at all.
* **`callType`.** Every event and command handler of an adopted form is written
  with `callType="Before"`.
* **Forms saved by an older platform.** Items the platform completes on load are
  completed in the XML, in document order, with the ids taken from the largest
  item id plus one: a table without an extended tooltip gets one, and each of its
  search-string, view-status and search-control additions gets a context menu and
  an extended tooltip; a command bar without a tooltip gets one; a table over a
  dynamic list gets the list settings, any other table that can filter rows
  (not a value list) gets `<RowFilter xsi:nil="true"/>`.
* **Planner field.** `DisplayImportance` at the common slot, `EnableDrag` in the
  option slot behind `EnableStartDrag`.
* **Record columns.** A calculation register's record set names its columns by
  the register's own standard-attribute markers (`-3` `LineNumber`, `-4`
  `CalculationType`, `-11` `ReversingEntry`, ...).

### References to the extended configuration

The extension's own type lists know nothing of the objects it does not adopt, so
the platform writes their types as ids. A *value* is named through the whole
configuration: the empty reference in a fill value prints as
`Catalog.Пользователи.EmptyRef`. The export reads the metadata rows of the same
database once, when a value first needs them (`extension::base_index_provider`),
and resolves type ids and object ids through them. If the rows cannot be read,
the reference stays an id.

## Inferences from one sample

These are read off a single native sample; the evidence is in the lab folder
(`F:\ibcmd\lab\05\ext`), and a second sample may refine them:

* the `dcssch` boundary: 8.3.14 writes none, 8.3.21 and 8.3.24 write it, 8.3.15 is
  assumed;
* `EnableDrag` of a planner field is option slot 6 (the only planner on record);
* `callType="Before"` on *every* handler of an adopted form (two adopted forms on
  record, one with handlers);
* the completion of old items is proved on 8 forms of one extension (ids, order,
  the dynamic list defaults); other kinds of items (pages, groups) that an older
  platform completed may exist;
* `OnBasePeriod` for `DependenceOnCalculationTypes` `2` and the values other than
  `0`/`1` of the accounting register's lock mode and full-text search are the
  enumeration order, not observed.

## Not covered yet

* the БСП 8.5 extensions (`--source-version 2.21`) and the 8.5 regression of the
  shared readers;
* the load and activation round trip of an exported extension;
* the drop-in `ibcmd infobase config export --extension=<name>` route.

## Reproducing

```bash
# reference: native export of every extension of the clone
pwsh -NoProfile -File F:/ibcmd/lab/05/ext/tools/native_export.ps1
# ours + comparison, listing of what is left
bash F:/ibcmd/lab/05/ext/tools/run_ours.sh <run>
python -X utf8 F:/ibcmd/lab/05/ext/tools/left.py <run>
# the ordinary export must not move
bash F:/ibcmd/lab/05/ext/tools/main_regress.sh bsp8327 <run>
```
