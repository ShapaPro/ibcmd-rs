# `--dynamic=force` in the drop-in `infobase config apply` (issue #347)

`ibcmd infobase config apply --dynamic=force` publishes the staged configuration as a **dynamic (online)
generation** while sessions stay connected: the changed rows go beside the active ones under
`<name>_dynupdate_<generation>`, the two `DynamicallyUpdated` markers name the generation, `root` and `version` are
replaced in place, and the writes the platform's own `force` makes besides -- the change registrations of the exchange
plans and `Files.MobileVersions.dat` -- are made in the same transaction. Sessions that are open keep the generation they
loaded; sessions that open afterwards read the new one; the next exclusive apply folds every generation into the ordinary
rows.

The design and the platform measurements that decided it are in [`dropin-dynamic-design.md`](dropin-dynamic-design.md)
(checkpoint 1); this file is what was built and how it was proven (evidence:
[`evidence/dropin-dynamic/acceptance.md`](evidence/dropin-dynamic/acceptance.md)). The exclusive apply the drop-in served
until now is [`dropin-apply.md`](dropin-apply.md).

## Recovery on released 0.4 — 2026-10-01

The preserved feature diff was ported from `1c63b763..c7ab2131` onto released
0.4.0 (`2a55cb34`), using three-way application instead of restoring old files.
The 0.4 S1, delta-import, PaletteColors and synonym/cache fixes remain present;
only the common report initializer required conflict reconciliation.

The native twins below are historical September measurements, not fresh native
acceptance of this port. The current checkpoint validates bounded Rust gates and
regressions. Fresh native/session acceptance on the integrated 0.5 candidate is
still required before expanding or closing #347. No pending-overlay
import -> force -> repeat support is claimed, and Params `.si` collection remains
unimplemented. The profile change adds only `mssql.config.apply.dynamic`:
8.3.27.2214 supported, 8.5.1.1150 unsupported; existing 0.4 capabilities stay as released.

The semantic stage inventory is now bound to the exact image the transaction
asserts. A changed `deleted` list or descriptor between judgment and publication
refuses before any write. A transport failure around COMMIT does not prove
rollback: the error names the recovery artifact and token, requires inspection
of ConfigSave and generation markers before retry, and leaves the outcome
uncertain. A final verification failure after successful execution explicitly
states that the transaction committed. The recovery artifact is manual recovery
evidence, not an automatic resume command.

## 1. What each value of `--dynamic` does

| Value | Nobody else connected | Sessions connected (SQL Server shows them) |
|---|---|---|
| `disable` | exclusive apply (as before) | the lock refusal and the list of sessions, exit -1 (as before) |
| `auto` (the default) | exclusive apply (= the platform run against a database) | the same refusal, exit -1, **plus the line `можно применить динамически: --dynamic=force` when the stage would qualify** |
| `prompt` | exclusive apply (the platform never asks in direct mode) | as `auto` |
| `force` | the dynamic apply if the stage qualifies, else `требуется штатный config apply: <reasons>` (exit 1) | the same: sessions are what it is for |

`auto`, `disable` and `prompt` are **never turned into a dynamic update**: this program has no prompt, and a user who did
not ask for a dynamic update gets a refusal that says how to ask for one. `force` is **never turned into an exclusive
apply** either. `--session-terminate` concerns the exclusive lock only; with `force` it is not consulted (the platform's
`force` ends no session either). `disable` is not pointed at `force` (it said no to dynamic updates).

Words and exit codes:

| Case | Words | Exit |
|---|---|---|
| done | `Обновление конфигурации базы данных...`, `Создано поколение конфигурации: <hex>`, `... успешно завершено` (the same lines as the platform's `force`, the same generation) | 0 |
| nothing staged | `Обновление конфигурации базы данных не требуется` | 0 |
| the stage does not qualify | `требуется штатный config apply: <row>: <reason>; ...` (the first 8, then `и ещё N`) | 1 |
| the platform has no dynamic apply (8.5.1.1150 declares it unsupported, 8.3.27.1989 not at all) | ``Параметр `--dynamic=force` команды `infobase config apply` не поддерживается для платформы ...`` | 1 |
| `auto`/`prompt`/`disable` with sessions | `Ошибка исключительной блокировки информационной базы.`, the sessions, `Закройте их ... и повторите` and, for `auto`/`prompt` when the stage would qualify, `можно применить динамически: --dynamic=force` | -1 (1 with `--session-terminate=force|prompt`, as before) |
| more than 50 generations after the apply | `[WARN] В информационной базе накоплено динамических поколений конфигурации: N; ...` on stderr, and `warnings` in the report | 0 |
| failure | the error and its context | -1 |

The platform's exit 102 (a confirmation `[y/n]` of a structural change answered by a closed stdin) has no counterpart: this
apply never asks and never restructures.

## 2. What qualifies

The platform's `force` publishes whatever `ConfigSave` holds as an overlay, a restructuring included
(`evidence/dropin-dynamic/acceptance.md`, section 6). This apply publishes a **small delta stage** and refuses the rest,
with the reasons named:

1. **Size:** at most 128 rows, 32 MiB, 16 MiB a row, one part each (the online engine's limits, `mssql_main_activation`).
   The stage of this program's own `infobase config import` is a delta since #395 (4 rows for a one-line change of a module
   on a clean base, 7 on a base with a pending online update), as is a stage made by the source-driven route
   (`mssql-stage-source-objects --per-row`) or by the platform's partial import. A whole-tree stage (9 521 rows) is refused
   by its size.
   The `deleted` row of a stage: an empty list is consumed; a list that names the rows of the online update the database
   carries -- every import stage of such a database -- is refused (`evidence/dropin-dynamic/acceptance.md`, section 7).
2. **A generation is made of** `root`, `version` and `versions`; `root` and `version` must have the text of the active rows.
3. **Every other row** is the descriptor or the `.0` body of a **common module or common form** that already exists (the
   kinds a session was measured with, `online-activation.md` section 4; the kind is read from the configuration row the
   `root` row names). A row of another kind, a new object, a `.1` body, a `deleted` list, a name that is not an object's:
   refused. More kinds are #345's, after they are measured with sessions.
4. **A descriptor is published only unchanged in text** (compared with the row the configuration is read from now: an
   alias of an earlier generation counts). A change of an object's properties is not dynamic here.
5. **The restructure check** (`ApplyCheckGate`) finds no restructuring. It is asked last, only for a stage nothing else
   refused; with 1-4 it cannot say yes to a stage that changes a table.
6. **The database is settled:** no unfinished operation (`commit`, `dynamicCommit`, ...), the schema storage at rest, no
   overlay in `Params` (a native dynamic update's `.si` rows: `config repair` / the native apply first).
7. **The platform** declares `mssql.config.apply.dynamic`: `platform-8.3.27.2214` only (the build the online transition and
   the twin were measured on). The storage profile is verified against the database as for the exclusive apply (SQL only,
   no RAS; the drop-in has no cluster options).

Nothing of this needs exclusive access, and none of it is asked for.

## 3. How it is built

`src/mssql_config_apply/dynamic.rs` plans and runs; it writes no row itself.

* **The transition** is the online engine's (`mssql_main_activation`, mode `online`, unchanged): an application lock, the
  exact-stage and exact-active assertions, the alias rows and markers, `root` and `version` replaced in place, the
  postconditions, then `ConfigSave` emptied. **Row and key-range locks** in a short serializable transaction (0.5-0.6 s of
  SQL; the exclusive apply takes `TABLOCKX` on `Config`, `ConfigSave`, `Params` and `Files` for its whole run). Measured with
  `sys.dm_tran_locks` sampled every few milliseconds and with the lock counters of the indexes
  (`evidence/dropin-dynamic/acceptance.md`, section 4a): no lock escalation was even attempted on any of the seven tables it
  touches; what it holds is key locks on the rows it writes, key-range locks on `ConfigSave` and on the register while it
  reads them (about 12 000 on `_ConfigChngR` for the length of the transaction, which only configuration operations
  touch), and a shared table lock on the one-row `_YearOffset`. `SET LOCK_TIMEOUT 30000` is set, so a session that holds a
  row for good makes the apply fail instead of wait.
* **The writes the platform's `force` makes besides** are the exclusive apply's own SQL, not a copy:
  `sqlgen::render_parity_writes` (the change registrations: `_MessageNo` reset for every object that owns a staged row and
  the #412 rows for the nodes with an initial image, and the `Files.MobileVersions.dat` ring) is rendered into the
  exclusive script and handed to the online engine (`MainActivationPlan::with_parity_sql`) to run before the stage is
  consumed; the planning is `plan_mobile_versions`, `registrations::plan`, `node_literals` of `mssql_config_apply`.
  `sqlgen::timestamp_declarations` gives both the same `@now` (local time shifted by the year offset), which the plan also
  stamps the markers with (`with_platform_timestamps`; the source-driven online apply keeps UTC).
* **The words of the checks** are shared: `require_no_unfinished_operation`, `require_settled_storage`, `blank_report`.
* **The recovery artifact** is the exclusive apply's (`recovery::write_recovery`): the rows of `Config` that carry the staged
  names, the markers, `MobileVersions.dat` before, the `_MessageNo` values it resets, the registrations it adds; for a
  dynamic apply the alias rows of earlier generations are not copied (a dynamic apply folds nothing), and the README says how
  to take the generation back.
* **The report** (`--report`) says `"mode": "dynamic"`, `"exclusivity": "not_required"` and `published`: the generation, the
  previous one, the **history** after it, the **alias rows** written and the rows replaced in place.
* **Seam:** `dropin/apply.rs`, `call_apply_dynamic` (the second seam) and `call_would_qualify` (the look that decides
  whether the refusal for the sessions carries the hint).

## 4. What is not done

* **An import stage of a database that carries a pending online update** (section 2, the `deleted` list): the platform's
  `force` writes a `deleted_dynupdate_<g>` row, register file lists and service information for it; this apply refuses it.
  The import-then-`force` loop therefore works from a clean base to the first generation, and the next import stage is the
  exclusive apply's until the update is folded. To finish: publish the list as `deleted_dynupdate_<g>`, take the register
  part from `registrations::plan(.., dropped)`, decide the `Params` `.si` part.
* **More kinds** (object modules, forms of top-level objects, templates, pictures, help, rights, command interfaces): #345.
* **A prompt** (`--dynamic=prompt` asking the terminal) and **ending sessions** (`--session-terminate`): not built.
* **The cluster is not told.** Like the platform run against a database, this apply writes to SQL Server only; a session that
  is open keeps its generation (measured for both), a new one reads the new generation.
* **8.5** (`mssql.config.apply.dynamic` is unsupported there) and **8.3.27.1989**.
* **A stage of the platform's own import** (removals, structure): `требуется штатный config apply`.
* **The overlay grows** with every generation: no cap, a warning past 50. An exclusive apply folds all of it
  (`own-apply.md`, "Dynamic-update rows go even when the stage omits them").

## 5. Findings for the apply track

1. **The `Params` marker after a fold.** For the same start (a database with dynamic generations) and the same 5-row stage
   -- a common module's body changed, its descriptor present and unchanged in text -- the native exclusive apply **leaves
   `Params.DynamicallyUpdated`** and clears the `Config` one, while the own exclusive apply deletes both, because its rule is
   "the `Params` marker goes when the stage carries a descriptor row" (`own-apply.md`, S5C). The measurement contradicts
   the rule as written: the marker goes with a descriptor whose **text changed** (S1-S5: new forms and templates), and stays
   for body rows alone (S5C) and for an unchanged descriptor (this one). Everything else is equal after the fold, and so is
   the native export. Not changed here; the proposed rule is "a descriptor whose inflated text differs from the active
   one".
2. **The platform's `force` registers the nodes with an initial image** (#412) as its exclusive apply does: with all the
   register rows of one node removed, the dynamic update inserted the missing row and its file list, as the own apply does.
