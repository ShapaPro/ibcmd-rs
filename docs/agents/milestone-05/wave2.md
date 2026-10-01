# Milestone 0.5 — second wave

Base: reviewed first-wave PR #420, `13d9c6dd` (2026-10-01).
User-visible board: https://github.com/users/Untru/projects/9.
Issues #347/#348/#409 remain In progress; the Agent field identifies each
implementer and current scope. Update the board at pickup/state transitions;
local files alone do not communicate task state to the user.

## D: repeated dynamic generation — release_review, #347

Worktree F:/ibcmd/src/ibcmd-rs-05-dynamic-wave2, feat/0.5-dynamic-overlay.
Lab F:/ibcmd/lab/05/wave2/dynamic; clone owner Track ui.

- [ ] D1 Inventory native repeated import/force storage with a fresh BSP 8.3
  twin: versions, alias deletion, registration file lists and Params service
  rows. Reuse original reference trees read-only; no .ui licensing hypothesis.
- [ ] D2 Implement only the measured pending-generation import/force path in
  source-import/effective-row/dynamic seams; retain strict refusal for unknown
  structures, profiles, alias layouts and drift. Request shared-file ownership
  before editing common extension or LIVE seams.
- [ ] D3 Prove two consecutive native/own generations preserve the earlier
  change, export the requested source, and reject tampered/deleted/stale stage;
  retain atomic/CAS/unknown-commit guarantees. Focused regression tests and
  appropriate quick gates; record exact measured limits and cleanup ownership.
- [ ] D4 Root independent code/evidence review before integration.

## L: real native/RAS checkpoint — s1_mix, #409 F-5

Worktree F:/ibcmd/src/ibcmd-rs-05-live-wave2, feat/0.5-live-native.
Lab F:/ibcmd/lab/05/wave2/live; clone owner Track live.

- [ ] L1 Establish a fresh registered BSP 8.3 clone and real idle RAS/native
  staged activation plus continuation on measured SQL 17.0.1135.8. Prefer the
  existing service cluster; change no global cluster settings or other DBs.
  Diagnose the prior empty-registry worker failure without touching others.
- [ ] L2 Exercise a real owned observer session, warm idle versus active work,
  deadline/refusal and repeated continuation. Record client/server generation,
  connection errors and ownership; five repetitions if the path is functional.
- [ ] L3 Repair actual bounded readiness/recovery defects supported by those
  measurements. Do not infer warm readiness from idle SQL handles or expand
  engine/profile support without evidence. Default LIVE and F-9/F-10 preserved.
- [ ] L4 Meaningful tests, raw logs and compact evidence; independent review
  by an available peer. Keep load/zero-error acceptance open if unproved.

## E: guarded extension Version edits — synonyms, #348

Worktree F:/ibcmd/src/ibcmd-rs-05-ext-wave2, feat/0.5-extension-version.
Lab F:/ibcmd/lab/05/wave2/extensions; clone owner Track ext.

- [ ] E1 Trace `src/mssql_extension_tree_load.rs` and
  `src/mssql_extension_load.rs` root-property refusal; measure a native
  Version-only twin and an updated interceptor at the new version.
- [ ] E2 Compile a Version-only root-property change using the existing format
  without inventing metadata or widening unrelated structural acceptance.
  Native validation/export must match source version and retain adopted roots.
- [ ] E3 Prove own load/native activation/source export and four-extension
  baseline parity; targeted negative/root-drift tests and appropriate gates.
  For any delivered CFE, build it only after Version increment and independently
  verify built Version. No CFE-delivery claim without an actual verified CFE.
- [ ] E4 Independent peer code/evidence review before integration.

## Coordinator

- [ ] C1 Review each source delta and real evidence, then integrate accepted
  commits into PR #420; review any shared-file resolution independently.
- [ ] C2 Appropriate combined validation and current Windows/Linux PR CI.
- [ ] C3 Preserve Git/raw evidence/binaries on F, release owned registrations,
  processes/locks and clean new disposable DBs through the shared idle guard.
- [ ] C4 Update board, PR and handover with measured results/remaining criteria.

All tracks read F:/ibcmd/lab/04/README.md and use shared clone/register/FIFO
helpers. Native writes hold one native ticket per command; owned worker runs
hold the worker ticket; heavy operations serialize. Four build/test jobs,
worktree-local iter cache, no full/release track build, no .env reads, no old
DB/worktree/cache changes. Preserve wave1 fixes and fail-closed gates. Agents
commit locally; coordinator alone integrates/pushes and maintains board state.
