# The import that carries what the target's rows cannot (#388 step 2, #393 import side)

Checkpoint 2 made `ibcmd infobase config import` refuse what a patch stage drops (`docs/import/guard.md`). Step 2
makes the stage carry it: an attribute, a tabular section, an enum value, a property, a subsystem's content, a new
catalog, form or template, a removed form or template, predefined items, the version of the configuration. The
guard stays on and checks the result of every case.

## 1. What the stage does

A stage into a database that holds the configuration (`stage_mode: patch`) still starts from the target's own rows and
takes from the tree what it can carry: names, synonyms, comments, modules, forms, templates, rights, command
interfaces. What differs from the target in a way those rows cannot carry is now built from the tree
(`src/mssql/override_stage.rs`):

1. **What differs** is asked of `apply_check::check_tree_against_db` (track rcheck): every metadata file of the tree
   against the descriptor the target stores for the same uuid, as the target's export would write it. It answers per
   object: `Added` (the target has no descriptor of this uuid), `Changed`, `Removed` (the target has it, the tree does
   not). With `--path-prefix` only the objects under the prefix count and nothing is removed (a partial tree is not a
   configuration).
2. **A new object** (a catalog, a form, a template, a common module, ...) is built whole the way a base-free stage
   builds it (`prepare_empty_object`): its descriptor and every body row, compiled from the tree. Its descriptor
   row takes no `Attributes` from a Config row (`Kind 0`), and its names are appended to `versions`.
3. **A changed descriptor**: the descriptor is compiled from the tree and used when it is not the row the stage would
   leave anyway. The comparison is between the compiler's text and the patched row (the target's row with the name,
   synonym and comment of the tree), and it matters: the check compares exported XML, and several objects of a
   native tree differ from the target's export only in how the model writes them (on БСП 17 catalogs and charts of
   characteristic types for the predefined data, nested subsystems, ...). A change of the header alone leaves the
   target's row - with the state the XML does not carry, such as the always-used flag of a constant, which is copied
   into a compiled constant in any case. An object a patch stage does not stage at all (a nested subsystem) is
   built whole when its compiled descriptor differs from the target's.
4. **Predefined data**: the stored row is patched item by item as before. When the tree's set of items is not the
   row's (an item added, an item removed; the nil-uuid root row of a hierarchical catalog is not an item) the row
   cannot be patched, and the object is built whole with its predefined data compiled from the tree.
5. **What leaves the configuration.** The rows of an object the tree no longer has (its descriptor and every
   `<uuid>.<n>`) are listed in the platform's `deleted` row and their names go out of `versions`. The rows of an
   online (dynamic) update of the target that are still pending (`DynamicallyUpdated`, `<uuid>_dynupdate_<generation>`,
   `versions_dynupdate_<generation>`) are listed too, as the platform's own import lists them.
6. The rows are staged over the target's as before (one transaction that replaces ConfigSave), with two changes of
   shape that make ConfigSave the platform's own: the dates are 2000 years ahead (the platform's `_YearOffset`, 4026,
   where the stage wrote 2026), and a row larger than 10 000 000 bytes is written in parts of that size, each part
   carrying the whole `DataSize`.
7. **The guard** exports the state the apply would leave - the target's rows as the storage publishes them, the
   `deleted` names removed, the staged rows in place - and compares it with the tree. Whatever the build got wrong
   still refuses the import.

An object the tree names but cannot be built from (a writer refuses its file, a reference names an object the tree
has no file of) refuses the import before ConfigSave is touched, in Russian, with the file and the reason.

## 2. The `deleted` row

The platform writes it as raw deflate of a UTF-8 text with a byte order mark: the number of names, then each name in
quotes followed by `,0` (the flag of a Config row):

```
11,"0384ec55-...",0,"0384ec55-....0",0,"8a7546f4-....0",0,"8a7546f4-...",0,"8a7546f4-....1",0,
"a627e390-..._dynupdate_06cb0442-...",0, ... ,"DynamicallyUpdated",0,"versions_dynupdate_06cb0442-...",0
```

For the tree with a form and a template removed, the native import (`bsp_nat2`) and ours list the same 11 names: the
five rows of the two objects and the six rows of the online update the БСП clone carries. The order differs (native
lists the rows of an object in an order of its own, the dynamic ones by name); the apply does not depend on it.
Native also drops the names `root`, `version` and `versions` from its `versions` and gives every name a new
generation; a patch stage keeps the target's entries and generations for the rows it does not stage.

Not written: the entries the platform makes for removed *children* of an object (an attribute, `<uuid>,1`), and the
rows of a module or another file of an object that stays. See section 6.

## 3. Acceptance on БСП 8.3.27

The lab's kit: `scripts/import-lab/run_override_acceptance.ps1` (our drop-in import with its default flags into a
clone, a summary of what it left in ConfigSave, the platform's own `config apply --force --dynamic=disable` and
`config export`, `source-diff` of the tree against the export) and `run_guard_acceptance.ps1 -Expect override`
(every edit of `edits.py` through the import, the guard on).

| tree | ours import | native apply | native export against the tree |
|---|---|---|---|
| `add5`: an attribute, a tabular section, a new catalog, a synonym, a new form | exit 0; 2 objects built, 4 descriptors compiled, guard 12 201 files | exit 0 | 12 201 of 12 201 files identical, `ConfigDumpInfo.xml` aside |
| `rem2`: a form and a template removed | exit 0; 2 descriptors compiled, 5 rows removed, 11 names in `deleted`, guard 12 190 files | exit 0, Config 9 836 rows (9 847 - 5 - 6) | 12 190 of 12 190 files identical, `ConfigDumpInfo.xml` aside |
| `all19`: the two above and 12 more edits (a property of a catalog, a property of an attribute, a removed attribute, an enum value, a subsystem and a nested subsystem, a command interface, a module, a predefined item added and one edited, the version of the configuration, a new template) | exit 0; 5 objects built (3 new), 10 descriptors compiled, 5 rows removed, 11 names in `deleted`, guard 12 196 files | exit 0 | 12 196 of 12 196 files identical, `ConfigDumpInfo.xml` aside |

`add5` and `rem2` are the acceptance of #388 criteria 1 and 2 and of #393's import side. The twin of the second:
native `config import` of `rem2` into a second clone and native apply give a Config with the same 9 833 row names as
ours (no name only on one side). In `all19` the native apply also dropped the column of the removed attribute
(`_Reference3347` has 8 columns against 9 in an untouched clone) although the `deleted` row does not name the
attribute: the platform finds the removal itself.

The regression of the whole matrix (`run_guard_acceptance.ps1 -Expect override`, 24 cases, the import only, guard on,
the clone never applied): every edit of `edits.py` loads and passes the guard (22 edits and the control; the removed
template `tpldel` is the new one), and the two edits that leave a reference to a removed catalog (`catdel`,
`catfile`) are refused in Russian with ConfigSave untouched: 24 of 24 as expected. An import of a tree the clone
already holds (the applied `all19` onto its clone) builds nothing, compiles nothing, lists no name in `deleted` and
passes the guard: a second import is a no-op.

The tree with all 21 edits also loads and passes the guard; after the native apply its export differs from the tree in
three files, each an inconsistency of the edits and not of the stage: the role whose right the edit set to `false`
(the platform's export drops a `false`; `docs/import/patch-mode.md` section 8, item 6), and two forms of the catalog whose
predefined item `predefdel` removed while the forms' queries still name it (the platform marks their data paths
`~`).

## 4. Cost

COSTS

## 5. Where it lives

`src/mssql/override_stage.rs` (the plan, the build, the `deleted` row, the `versions` entries, the constant's flag),
`src/mssql.rs` (`stage_source_objects`, `StageAdditions`, the rows in parts, the dates), `src/mssql/empty_stage.rs`
(`EmptyStageContext::for_objects`: a context that does not throw the process-wide switches of a base-free stage),
`src/module_blob.rs` (`patch_predefined_data_blob_from_xml`), `src/mssql_dump/offline_rows.rs` and
`src/mssql/stage_guard.rs` (the `deleted` names out of the guarded state), `src/sql/mod.rs` (a detached handle says
why it is detached, which is how a writer knows it works for a base-free object inside a patch stage).

## 6. Limits

- **A single file of an object that stays**, removed from the tree (a module, a picture, a help page): not carried.
  The guard sees the file the target still has and refuses ("удаление отдельных файлов объекта пока не
  переносится"). Listing the row in `deleted` needs to tell such a row from the content-free stub rows the storage
  keeps for an emptied part (`ConfigDumpInfo.xml` lists them, the XML has no file), which the row roles of
  `apply_check::roles` describe; not done.
- **Removed children** of an object (an attribute, a tabular section): the platform's import writes them in
  `deleted` as `<uuid>,1`. Ours does not, and the native apply removes the column anyway (see section 3), so no
  effect was observed; a twin of the native import for this case was not made.
- **State the XML does not carry** stays only for rows the stage does not rebuild: a compiled descriptor takes the
  compiler's values for it, except the always-used flag of a constant, which is copied. On ERP УХ 9 of the 56 758
  descriptors differ from what the XML gives (six of them are those flags); an object of that kind that the tree
  changes loses the difference (`docs/import/patch-mode.md` 4.1). The flowchart of a business process is patched
  as before.
- **The comparison's blind spots** fail closed: a descriptor `check_tree_against_db` calls unchanged stays the
  target's, and the guard refuses the import if the tree says otherwise. Bodies are not compared by it; the
  predefined items are compared by the set of items in the stage itself, the exchange plan's content, the modules,
  forms, templates, rights and command interfaces are always compiled from the tree.
- **A partial import** (`--path-prefix`) builds and compiles inside the prefix, removes nothing and does not clear
  the pending online update. `--per-row` and an offline stage (`--script-only` with `IBCMD_RS_BASE_ROWS_DIR`) do
  not compare the tree with a database at all, so they carry nothing new.
- **Not run:** platform 8.5 (dialect 2.21) with builds; ERP УХ with real changes (a second copy of the tree is not
  allowed in the lab; the run is of the unchanged tree, so the build path costs there are the setup of the
  base-free stage: about 64 s for the walk, the descriptor reads and the context).
- **Edits that contradict themselves** are not the stage's to repair. The native export of a tree where a role
  right was set to `false` drops it, and of a tree where a form's query names a predefined item the tree removed
  marks the data paths with `~`; both showed in the tree with all 21 edits (section 3).
