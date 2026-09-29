# restructure-lab: research kit of the "ddl" track (issue #341)

Tools used to trace the native `ibcmd infobase config apply` on lab clones and to decode the structure of
the database (`DBSchema`, `DBNames`, `SchemaStorage`). Findings: `docs/apply/restructuring.md`; extracts:
`docs/apply/evidence/restructuring/`.

**Lab only.** Everything here writes only to databases named `ibcmd_rs_04_ddl_*` (the scripts check the
prefix) and reads other databases with `SELECT`. The paths default to the lab of the 0.4 tracks; set
`DDL_LAB` (lab folder), `DDL_DB` (default database), `DDL_TREE` (working XML tree) and `DDL_NATIVE_TREE`
(native export used as reference metadata) to use another location. Python 3.13 with `pyodbc` and the
"ODBC Driver 18 for SQL Server"; PowerShell 7; SQL Server with Windows authentication; a native 8.3.27.2214
`ibcmd.exe`.

## Capture

| script | what |
|---|---|
| `xe.ps1` | Extended Events helpers: `Start-DdlXe` / `Stop-DdlXe` (session `ibcmd_rs_04_ddl_<n>`, filtered to one database, files in `C:\temp\ibcmd_rs_04\ddl`), `Invoke-NativeApply` (native apply, notes other ibcmd processes seen while it ran) |
| `run_case.ps1` | one traced apply: light snapshot of the staged state, COPY_ONLY backup (twin source), XE session, native `config apply --force --dynamic=disable`, XE read, full snapshot, diff |
| `run_series.ps1` | cumulative cases: edit the tree, native import until the stage is complete (row count check), `run_case` |
| `try_native.ps1` | fast loop: native import (+ apply) of a tree on a debug clone |
| `snapshot.py` | columns, indexes, row counts and checksums of all tables + service tables with content hashes into a snapshot folder; blobs go to a content-addressed store |
| `snapdiff.py`, `schemadiff.py`, `dbnames_diff.py` | diffs of two snapshots: tables/columns/indexes, service rows, `DBSchema` entries, `DBNames` |
| `xe_read.py`, `xe_shapes.py`, `timeline.py`, `extract_ddl.py` | XE file -> JSONL, normalised statement story per session, phase timeline, the structure statements as a readable SQL log |

## Decode and check

| script | what |
|---|---|
| `bracefmt.py` | parser and writer of the platform's brace text; `dumps(parse(x)) == x` byte for byte (`rt_check.py`) |
| `dbschema.py` | model `DBSchema` -> SQL columns, indexes, implicit primary keys, DDL text |
| `dbschema_check.py` | the model against a real database dump (`schema.txt` of a snapshot): 1937/1937 tables on the 8.3.27 БСП, 1922/1922 on the 8.5 БСП |
| `md_types_check.py`, `gen_check.py` | XML type descriptors -> type entries; the field list of catalogs and documents generated from the XML tree and `DBNames`, compared with the real entries (94/94) |
| `names.py`, `lab.py`, `db.py`, `dbnames_kinds.py`, `make_cases_md.py` | `DBNames` parsing, snapshot store access, pyodbc helpers, the metadata class -> `DBNames` kinds table, the per-case evidence file |

## Edit the tree

`edit_tree.py`, `edit_cases_more.py`, `edit_cases_h.py`: the exact XML edits of the cases (a: attribute,
b: document attribute, c: new catalog, d: dimension + resource, e/g/f: widen / index / delete, h: attribute
types and tabular sections). Each edit keeps the original and the edited file under `tree/patches/<case>/`.

## Typical run

```powershell
$env:DDL_LAB = 'F:\ibcmd\lab\04\restructure'
pwsh -NoProfile -File F:\ibcmd\lab\04\tools\restore-clone.ps1 -Corpus bsp8327 -Name ibcmd_rs_04_ddl_bsp8327_x -Track ddl -Purpose "..."
pwsh -NoProfile -File scripts\restructure-lab\run_series.ps1 -Cases a -Prev <snapshot label> -Database ibcmd_rs_04_ddl_bsp8327_x
python scripts\restructure-lab\snapdiff.py <db> <before-label> <db> <after-label>
```

Run only one native `ibcmd` of your own at a time; verify the `ConfigSave` row count after a native import
(a partial stage makes the apply fail, see the findings in the doc).
