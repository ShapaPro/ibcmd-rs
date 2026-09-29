# import-lab: research kit of the "import" track (issue #388)

Tools that measure what `ibcmd infobase config import` stages into an existing database, in each mode,
against the platform's own import. Findings: `docs/import/patch-mode.md`.

**Lab only.** Databases `ibcmd_rs_04_import_*` (the scripts check the prefix); every native write runs inside
the lab's native lock (`F:\ibcmd\lab\04\tools\heavy-lock.ps1 ... -Name native`). Python 3.13 with `pyodbc` and
the "ODBC Driver 18 for SQL Server", PowerShell 7, SQL Server with Windows authentication, native
`ibcmd.exe` 8.3.27.2214. The lab folder defaults to `F:\ibcmd\lab\04\import` (`IMPORT_LAB`).

| script | what |
|---|---|
| `edits.py` | named edits of an exported XML tree (`list`, `apply <name>[,...]`, `reset`): a new attribute, tabular section, catalog, form, template, predefined item, enum value, role rights, subsystem content, command interface, module, configuration version, and removals. Each on its own object; BOM, CRLF and the bare LF inside text values are kept. The base tree is never written; a work tree is reset from it |
| `rowdiff.py` | row by row comparison of two `Config`-like tables or `<name>__part<N>.bin` folders (parts joined, inflated; classes identical / same text / layout only / v8 container headers only / different / in one side only); `--dump-left` writes a table as such a folder |
| `native.ps1` | `Invoke-NativeImport`, `Invoke-NativeApply`, `Invoke-NativeExport`, `Invoke-OursImport` (the drop-in, patch or `--base-free`), the re-entrant native lock |
| `run_matrix.ps1` | every change x mode staged into a clone that is never applied, compared with the stage of the unchanged tree in the same mode; results in `out\matrix\<tag>` |
| `summarize.py` | the matrix as a table and per-change row lists |
| `run_survival.ps1` | stage in a mode, native `config apply`, native `config export`, `source-diff` against the tree |
| `equiv_cmp.py` | two applied databases: table structure, `DBNames`, `DBSchema`, `Params`, Config rows |
| `ve_tree.sh` | offline round trip of a tree: the rows `--base-free` would stage, exported again from those rows, diffed against the tree |

Typical run:

```powershell
pwsh -NoProfile -File F:\ibcmd\lab\04\tools\restore-clone.ps1 -Corpus bsp8327 -Name ibcmd_rs_04_import_bsp_s1 -Track import -Purpose "..."
# baselines of the unchanged tree, one per mode (out\rows_<patch|bf|native>_pristine)
python scripts\import-lab\rowdiff.py ibcmd_rs_04_import_bsp_s1:ConfigSave x --dump-left F:\ibcmd\lab\04\import\out\rows_patch_pristine
pwsh -NoProfile -File scripts\import-lab\run_matrix.ps1 -Changes attr,ts,newcat -Modes patch,bf,native
python scripts\import-lab\summarize.py F:\ibcmd\lab\04\import\out\matrix\v0 --detail
```

Run one native `ibcmd` of your own at a time and check the `ConfigSave` row count after a native import (a
partial stage makes the apply fail).
