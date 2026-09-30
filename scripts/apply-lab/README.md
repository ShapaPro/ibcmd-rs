# Apply lab: the scripts of the removals twins (#393)

Scripts of track apply for the twins that compare `ibcmd-rs mssql-config-apply` with the platform's `ibcmd infobase config apply`
on **the same stage** (`docs/apply/own-apply.md`, "Removals"). They run on the lab of the 0.4 tracks only
(`F:\ibcmd\lab\04\apply`, databases `ibcmd_rs_04_apply_*`, the native lock of `F:\ibcmd\lab\04\tools\heavy-lock.ps1`), and use the
ddl track's kit (`scripts/restructure-lab`: `snapshot.py`, `snapdiff.py`, `si_diff.py`, `dbschema_cmp.py`, `compare_tables.ps1`) with
`DDL_LAB=F:\ibcmd\lab\04\apply\ddlkit`.

| Script | What |
|---|---|
| `native_import.ps1` | the platform's `config import` of a tree into a clone, with a `--data` directory **of its own for the database** (a shared one made the import fail or stage a wrong set), under the native lock |
| `native_cmd.ps1` | any other native `ibcmd` command on a clone (`infobase config apply --force --dynamic=disable`, `config export`, `config check`), the lock taken for the writes |
| `native_twin.ps1` | a fresh clone of the БСП 8.3.27 corpus, native `config import` repeated until the stage is whole (the first import of a fresh clone stages a subset or fails), snapshot, COPY_ONLY backup, native apply, snapshot |
| `refusal_cases.ps1` | five lists that `mssql-config-apply` must refuse as a whole, planned with `--dry-run` on the clone that holds a removal stage |
| `evidence_393.ps1` | collects the checks of the two twins into `docs/apply/evidence/own-apply/removals-393-twin-checks.txt` |
| `deleted_cmp.py`, `deleted_row.py`, `deleted_classify.py` | the `deleted` row of a stage: format and set of names against another row, its text, its names against `Config` |
| `reg_cmp.py` | the change registers of two clones: message numbers and file lists per (node, object) |
| `stage_tools.py` | helpers for stages in disposable clones (copy a stage, add or drop the `deleted` row, ...) |
| `tree_edit.py`, `unused_modules.py`, `module_mentions.py` | edits of a working copy of the exported tree, and which common modules the rest of the tree mentions |

The stage of the first twin is what `ibcmd-rs infobase config import` (branch `feat/0.4-import-override`) writes for the tree of the
import lab with the form `ВсеЗаметки` and the template `ДатыПасха` removed; the second adds the removal of the first attribute of
`Catalog._ДемоГруппыДоступаНоменклатуры` (`edits.py attrdel`).
