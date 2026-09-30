# S1-K: the chain with OUR import (#407)

Part of #391 (the minimal own restructuring, S1). The chain: an edit of the БСП 8.3.27 XML tree, then our
`ibcmd infobase config import` into an existing base, then our apply (`mssql-config-apply --allow-restructure s1
--i-have-a-backup`), compared with the native chain (native import, native `config apply`) on a twin, by the protocol of
`docs/apply/restructuring.md` 12.6. Track "ext" from 2026-09-30 (it was import's). Two stops: **checkpoint 1** (this
document): the kit, the cases, and what our import does today on `feat/0.4`; **checkpoint 2**: the real run, after the
import track's step 2 of #388 (`feat/0.4-import-override`) and the restructure-check track's wiring of the drop-in
apply to the S1 gate (`feat/0.4-dropin-s1`) are merged.

Everything below is measured on the БСП 8.3.27 corpus clone (four extensions), platform 8.3.27.2214, SQL Server 2025.

## 1. The kit

`scripts/restructure-lab/s1k/` (the ddl track's tools are used as they are: `snapshot.py`, `snapdiff.py`, `si_diff.py`,
`bracefmt.py`, `lab.py`, `db.py`, `session_job.ps1`, the session jobs in `jobs/`). The lab is `F:\ibcmd\lab\05\ext\s1k`
(`S1K_LAB`): `tree\` (a copy of the reference native export, made once with `robocopy`), `bak\base_applied.bak` (the clone
after a native baseline apply: an unchanged file staged and applied, which also folds the `_dynupdate_` rows a corpus clone
carries), `out\<case>\` (the edit, the reports, the record), `snap\` (the snapshot store; never the ddl track's).

| file | what |
|---|---|
| `cases.py` | the case list and the editors (`list`, `edit <case> <out>`); the editors are the ddl track's (`edit_cases_s1.py`, `edit_cases_s2.py`) or small ones of this kit; they write only the edited files under `<out>/stage/`, the originals under `<out>/before/` |
| `overlay.py` | puts a case's edited files over the working tree and takes them off again (`apply`, `restore`, `verify`), so one tree serves every case |
| `import_phase.ps1` | one case, the import phase: the edit, a twin of the base, OUR import (default mode), what it left in `ConfigSave` (`record.py`), OUR apply as a dry run over that stage |
| `run_phase1.ps1`, `assemble_phase1.py` | every built case in a row; the record of all of them as one text (`evidence/restructuring/s1k-checkpoint1.txt`) |
| `twin_case.ps1` | the twin protocol of one case: the native side (native `import files --partial` of the edited files, native apply), OUR apply on our twin (dry run, rehearsal, real run), checks 10, 3, 4, 5, 6, 7, 8, 9 |
| `compare_twins.py`, `check4.py`, `checks56.py`, `check8.py` | EXCEPT both ways over tables (and every table of the extension schema); `Config` row by row; `DBNames`, `DBSchema` entries and the `.si` rows; the native exports |
| `native_main.ps1` | native `ibcmd` on a lab clone: `ImportFiles`, `Apply`, `Export` (the writes under the native lock, one command per hold) |

## 2. The cases

The edits are the ones the S1 operations were traced and twin-verified with, so a difference between the chains is the
chain's, not the edit's. Objects that an extension adopts are avoided by the cases of the built operations (S1-I refuses
them) and used by case i1.

| case | operation | edit | session job |
|---|---|---|---|
| a1 | A: add attributes | attributes of every primitive type on `КлючевыеОперации`, `Удалить_ДемоОбщиеСведения` and the document `_ДемоЗаказПокупателя` (the ddl track's types case without its three adopted catalogs) | to write |
| b1 | B: delete attributes | the middle, the last, the first, an indexed one, the only one, in a hierarchical catalog, in a document, delete + add (6 objects) | `jobs/s2_b1.bsl` |
| b2 | B | an attribute with the additional-order index (catalog `_ДемоПроекты`, document `_ДемоНачислениеЗарплаты`) | `jobs/s2_b2.bsl` |
| c1 | C: widen a string | six strings in five objects (catalog, hierarchical catalog, document, indexed, up to 1024) | `jobs/s2_c1.bsl` |
| d0 | D: the index flag | one flag alone | `jobs/s2_d0.bsl` |
| d1 | D | on and off, additional order, string and number, hierarchical and flat catalog, document | `jobs/s2_d1.bsl` |
| i1 | I: an adopted object | a string attribute on `_ДемоПартнеры` and `_ДемоНоменклатура` (adopted by `_ДемоРасширение`, the second with `_Reference18X1`): S1-I must refuse | - |
| e1 | E: add a tabular section | not built in the gate yet; the editor comes with the operation | - |
| f1 | F: add a catalog / a document | not built yet; the same | - |

## 3. The protocol for two chains that stage apart

12.6 makes the twins from one backup of the staged state, so that native and ours start from the same `ConfigSave`. Here
the point is the two stagings, so each twin is staged by its own import and two things differ by construction:

- **Check 4** (`Config`): the rows of an import carry the `Creation` / `Modified` of that import, and the `versions` row
  the generation of that stage (`root` and `version` too). `check4.py` counts the rows equal in every column, equal in
  the bytes with other dates, equal after inflate, and different in content, and names the last: on twins staged apart by
  the same native import (the harness validation below) it finds 9 837 equal, 3 with other dates and `versions`.
- **Check 8** (the native export): `ConfigDumpInfo.xml` prints a `configVersion` for every object, the generation that
  staging gave it, so it differs for exactly the changed objects. `check8.py` compares every file byte for byte and
  `ConfigDumpInfo.xml` with the `configVersion` values taken out, and lists the objects whose version differs (12 198
  files, one object on the validation).

All the rest is as 12.6: check 3 (EXCEPT both ways over the rebuilt tables and the 243 `X1` tables of the extension schema),
5 (`DBNames` text equal after inflate, `DBSchema` entries equal but `DbCopies*`), 6 (the 16 `.si` rows), 7 (a native apply
afterwards says «не требуется»), 9 (a cluster session on both), 10 (a rehearsal changes nothing), 11 (the refusals), 12 (an
injected failure inside the transaction; the ddl track's `inject_failure.ps1` on the script `twin_case.ps1` writes).

## 4. Checkpoint 1: what our import does today

`feat/0.4` 847f581a, the binary built from it, the import in its default mode (no `--base-free`, no `--no-verify`), the
tree of each case over a base that holds the same configuration. Record: `evidence/restructuring/s1k-checkpoint1.txt`.

| case | our import | in `ConfigSave` | our apply, dry run |
|---|---|---|---|
| a1 | exit -1 in 13.5 s: «Загрузка отменена: конфигурация базы после неё не совпала бы с деревом ... (файлов с расхождениями: 3 из 12197). В ConfigSave ничего не записано.» The differences listed: the new attributes are missing from the assembled configuration | 0 rows | nothing to apply |
| b1 | exit -1 in 14.5 s **before the guard**: `failed to pack Form body ...\_ДемоСтавкиНДС\Forms\ФормаЭлемента\Ext\Form.xml: failed to patch Form layout properties: cannot patch Form UseForFoldersAndItems without an existing property bag`. Bisected: the edit of `_ДемоСтавкиНДС` alone fails so; the other five objects fail on the form of `ТелефонныйЗвонок` (`failed to patch Form layout AutoCommandBar: ... direct child item entry`) | 0 rows | nothing to apply |
| b2 | exit -1 in 8.4 s: guard, 2 of 12 197 files (the attribute of `_ДемоПроекты` and of `_ДемоНачислениеЗарплаты` is still there in the assembled configuration, with its `Indexing` and uuid) | 0 rows | nothing to apply |
| c1 | exit -1 in 8.3 s: guard, 5 of 12 197 (`Length` in the tree 200 / 20 / 100 ..., in the assembled configuration 50 / 12 / 40 ...) | 0 rows | nothing to apply |
| d0 | exit -1 in 8.9 s: guard, 1 of 12 197 (`Indexing` in the tree `Index`, in the assembled configuration `DontIndex`) | 0 rows | nothing to apply |
| d1 | exit -1 in 11 s: guard, 6 of 12 197 (`Indexing`) | 0 rows | nothing to apply |
| i1 | exit -1 in 19.7 s: guard, 2 of 12 197 (the new attribute of `_ДемоНоменклатура` and `_ДемоПартнеры` is missing) | 0 rows | nothing to apply |

So today: **five cases reach the guard of #388 and are refused** with the differences named and nothing written (the guard
does its job; no case loses a change silently), **one (b1) fails earlier**, in the packing of forms of the edited objects,
with an error that has nothing to do with the deletion the case makes (the forms are not edited). Nothing stages, so our
apply has nothing to do and the chain cannot yet be compared: that is checkpoint 2, after step 2 of #388. The guard's own
words offer `--base-free` (loses what the assembly from the tree does not carry) and `--no-verify` (the listed changes
"do not reach the base"); neither is used.

The message of the guard for the built operations names exactly the changes the operations are made of (a new attribute, a
removed attribute, a longer `Length`, another `Indexing`), which is what step 2 has to assemble.

### The rest of the harness, validated on d0 (not a result about our import)

`twin_case.ps1 -Case d0 -StageOwnNative` stages our twin with the native partial import (so nothing here says anything
about our import) and runs everything after it, through `mssql-config-apply --allow-restructure s1 --i-have-a-backup`:

- dry run and rehearsal: exit 0; check 10: the rehearsal changes nothing (`changed (0)`);
- real run: `catalog КлючевыеОперации (Reference2598): switched indexes Fld2646 = ЦелевоеВремя (DontIndex -> Index)`;
- check 3: `_Reference2598` and the 243 `X1` tables equal (EXCEPT both ways); check 5: `DBNames` equal after inflate,
  `DBSchema` equal but `DbCopies*`; check 6: 16 of 16 `.si` rows; check 7: «Обновление конфигурации базы данных не
  требуется»; check 8: 12 198 files, `ConfigDumpInfo.xml` differs in the `configVersion` of `Catalog.КлючевыеОперации` only;
  check 9: the session outputs of the two twins identical (8 lines);
- check 4: 9 837 rows equal, 3 with other dates, `versions` differs in content (the generation): the reason of section 3.

## 5. Checkpoint 2

When step 2 of #388 and the wiring of the drop-in apply merge: `run_phase1.ps1` again (now expecting a stage), then
`twin_case.ps1` per case with its session job, the cases i1 (refusal, check 11) and, when the operations land, e1 and f1,
and one injected failure (check 12). Cases that stage more than S1 covers stay refusals.
