# S1-G lab kit (track trace): the platform proof of a cache row and the new-document case

Scripts of the `trace` track for `docs/apply/derived-caches.md`. They are copies of the `ddl` kit
(`F:\ibcmd\lab\04\restructure\tools`, `scripts/restructure-lab`) with another database prefix
(`ibcmd_rs_05_trace_*`), another port (5614) and another data folder (`F:\ibcmd\lab\05\s1g\kit`); the
ddl probe (`ddl_probe.epf`, `probe\jobs\newcat.bsl`) is used from its lab folder as it is. Native commands run
under the lab "native" lock (`DDL_LOCK_TRACK=trace`). Nothing here touches a database of another track.

| script | what it does |
|---|---|
| `make_kit.py`, `make_kit2.py` | make `dbrow.ps1`, `srv.ps1`, `job.ps1`, `snapshot.py`, `db.py` from the ddl kit |
| `dbrow.ps1` | read, put or delete one `Params` row (`list`, `dump`, `put`, `delete`) |
| `swap_row.ps1` | put a stored (deflated) cache row and give it a new guid in `siVersions`, as the apply does |
| `srv.ps1`, `job.ps1` | a stand-alone `ibsrv` on a lab database, and a thin-client session that runs a BSL job |
| `prove.ps1` | the proof: stand-alone server + session, cluster session, native `config check`, native `config apply` |
| `make_case_d.py` | case d: a new document with a tabular section cloned from a small БСП document |
| `stage_d.ps1` | stage case d with the native partial import, snapshot, apply natively, snapshot |

## The proof of `c4629235` (derived-caches.md 4)

```
restore-clone.ps1 -Corpus bak -Bak ...bsp8327_c2_native_after.bak -Name ibcmd_rs_05_trace_c2_base  ...   # untouched
restore-clone.ps1 -Corpus bak -Bak ...bsp8327_c2_native_after.bak -Name ibcmd_rs_05_trace_c2_ours  ...   # the twin
IBCMD_RS_CACHES_DUMP=<dir> cargo test --lib restructure::caches::tests_cases::dump_the_case_c_rows_for_the_twin_proof
swap_row.ps1 -Database ibcmd_rs_05_trace_c2_ours -Row c4629235-4823-4320-b8b5-1d08f4c6d612.si -Blob <dir>\c4629235-...si.deflated
prove.ps1 -Database ibcmd_rs_05_trace_c2_base -Label base
prove.ps1 -Database ibcmd_rs_05_trace_c2_ours -Label ours
```

Results: `docs/apply/evidence/derived-caches/prove_base.txt` and `prove_ours.txt`.

## Case d

```
restore-clone.ps1 -Corpus bsp8327 -Name ibcmd_rs_05_trace_d_base ...
python make_case_d.py          # needs the native export tree of a БСП clone (F:\ibcmd\lab\04\restructure\export\s2_b1_nat)
stage_d.ps1 -Database ibcmd_rs_05_trace_d_base       # snapshots d_staged and d_after into F:\ibcmd\lab\05\s1g\store
cargo test --lib restructure::caches                  # IBCMD_RS_TRACE_LAB=<store> when the store is elsewhere
```
