# Tasks: MSSQL live generation switching

- [x] 1. Add the `live` CLI contract, required tail-log output, stable report fields, and fail-closed extension rejection.
- [x] 2. Extend main activation planning and recovery artifacts for paired dynamic-marker normalization.
- [x] 3. Render and execute the guarded ordinary promotion plus tail-log `NORECOVERY`/`RECOVERY` transition with deterministic recovery instructions.
- [x] 4. Add focused unit tests and preserve existing `online`/`exclusive` behavior.
- [x] 5. Run Rust regression/release tests and prove same-session client/server switching on the disposable 8.3.27 SQL base with timing evidence.
  - Re-measured 2026-09-29 (`direct-mssql-online-activation/evidence/online-live-8327-20260929/`): on a loaded machine 0 of 5 `live` activations completed; see task 7.
- [x] 6. Restore the laboratory database, authentication, sessions, and temporary source to their original state.
- [x] 7. Replace the fixed five-second inter-cycle delay with a bounded SQL connection-readiness gate and report its timing.
  - Evidence 2026-09-29: the readiness gate aborted (`57234`) after the first recovery cycle in 5 of 5 attempts (CPU 98 % average, one to three sessions); 1C SQL connections came back after 3.5 s, 10.2 s and not at all, against the 4 s / 500 ms constants (`mssql_main_activation.rs:607-609`). The promotion is committed, a retry is a no-op, sessions can end in a modal DB error or a mixed client/server generation. `docs/apply/online-activation.md` section 4.3, F-5. The recorded live run (`evidence/live-stable-v7.md`) used the fixed five-second delay this task replaced.
- [x] 8. Add a debounced editor-watch command with dedicated-worker activation, fail-closed assignment checks, and serialized activation.
- [x] 9. Prove save-to-live client/server latency on the disposable 8.3.27 base, run focused/release checks, and restore the laboratory state.
  - Worker: no new run on 2026-09-29 (shared-cluster rule); documented from `evidence/worker-watch-20260830.md` in `docs/apply/online-activation.md` sections 3.5 and 4.4.
