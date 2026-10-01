# Milestone 0.5: first-wave task board

Design: [design.md](design.md). Base: released v0.4.0 (`origin/master`).
Execute with `subagent-dev`; independent tracks may run in parallel.

## Dynamic track: #347

Worktree `F:\ibcmd\src\ibcmd-rs-05-dynamic`, branch `feat/0.5-dynamic-recovery`.
Source work: preserved `feat/0.5-dynamic` and `F:\ibcmd\lab\05\ui\STATUS.md`.
Files: `src/dropin/{apply,help,mod,parse}.rs`,
`src/mssql_config_apply/{dynamic,mod,sqlgen,errors,recovery}.rs`, related
profile/CLI seams and `docs/apply/dropin-dynamic*.md`.

- [x] D1 Recover the old dynamic commits without reverting v0.4 fixes.
- [x] D2 Verify explicit force, profile/size/body gates, no-op, reporting,
  and overlay-deletion refusal with meaningful focused tests.
- [x] D3 Run quick gates and document remaining native/session evidence.
- [x] D4 Independent review; repair findings before integration.

Checkpoint `d807ec00`: root independent review PASS; 3624 root library tests,
focused dynamic/drop-in/activation/profile suites and two CLI integration
targets pass. See [current evidence](../../apply/evidence/dropin-dynamic/recovery-20261001.md).
Fresh native/session acceptance and the pending-overlay import loop remain open.

## Live track: #409 F-5

Worktree `F:\ibcmd\src\ibcmd-rs-05-live`, branch `feat/0.5-live-resume`.
Source work: `feat/0.5-live-worker`, `F:\ibcmd\lab\05\live\STATUS.md`.
Files: `src/mssql_main_activation.rs`, dedicated live readiness/continuation
module(s), `src/mssql.rs`, `src/mssql_apply.rs` only necessary CLI seams,
`scripts/apply-lab/live/`, track-owned evidence/docs.

- [ ] L1 Recover the preserved lab kit and model the first/second cycle states.
- [ ] L2 Implement bounded readiness that does not wait for idle SQL handles.
  First checkpoint: automatic completion only when verified RAS has no user
  sessions; otherwise report continuation required. Adaptive warm-session
  readiness remains a separate, uncompleted acceptance item.
- [ ] L3 Implement guarded continuation with database/artifact identity,
  completion detection and refusal of ambiguous/repeated recovery states.
- [ ] L4 Test idle/active/timeout/already-complete paths and maintain F-9/F-10 gates.
- [ ] L5 Run native/session smoke on new owned clones; record measured limits.
- [ ] L6 Independent review; repair findings before integration.

## Extension track: #348 interceptor remainder

Worktree `F:\ibcmd\src\ibcmd-rs-05-ext`, branch `feat/0.5-extension-interceptors`.
Source work: `F:\ibcmd\lab\05\ext\STATUS.md`, `tl\ic2_orc` evidence.
Files: `src/mssql_dump/extension/form.rs`, the form compiler event-handler
adapter identified by tracing, `docs/extensions/parity.md`, focused fixtures.

- [ ] E1 Confirm native After/Override storage and exported XML shapes.
- [ ] E2 Decode interceptor type from the additional-handler list.
- [ ] E3 Encode the evidenced no-BaseForm shape; keep unsupported forms refused.
- [ ] E4 Test Before/After/Override and native-twin export-back equality;
  record fixture Version changes when applicable.
- [ ] E5 Run quick gates and independent review; repair findings.

## Coordinator

Worktree `F:\ibcmd\src\ibcmd-rs-05-integration`, branch `feat/0.5-integration`.

- [x] C1 Inventory preserved branches and define isolated ownership/acceptance.
- [ ] C2 Resolve shared-file conflicts after independent review.
- [ ] C3 Run appropriate integrated gates and record exact verified scope.
- [ ] C4 Publish a reviewable first-wave PR and attach it to this chat.

Follow-on ownership: dynamic track owns #345/#346 after #347; live track owns
#349 and remaining #409 readiness/load evidence; extension track owns the
remaining #348 drop-in/8.5 routes. The coordinator owns #344 final review and
#418 native marker-rule research after the apply seams have stabilized.

The open milestone is not closed by completing this board. Completion claims
must cite actual tests/evidence; unmeasured paths remain explicitly open.
