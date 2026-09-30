# s1k: the end-to-end kit of S1-K (#407)

An edit of the БСП 8.3.27 tree, then OUR `ibcmd infobase config import`, then OUR apply, against the native import and the
native `config apply`, for each built S1 operation. The design, the cases, the protocol and the results:
`docs/apply/restructuring-s1k.md`.

Lab only: every database is `ibcmd_rs_05_ext_s1k_*` (the scripts refuse any other), the lab folder is `S1K_LAB`
(default `F:\ibcmd\lab\05\ext\s1k`), every native write runs under the lab `native` lock, one command per hold.

```powershell
# once: the working tree and the base
robocopy <reference native export of the corpus clone> $lab\tree /E /MT:16
#   restore the corpus clone, stage an unchanged file natively, native apply, BACKUP ... COPY_ONLY to $lab\bak\base_applied.bak

# the import phase (what our import does with the edited tree), every built case
pwsh -NoProfile -File run_phase1.ps1 -Bin <ibcmd-rs.exe> -Cases "a1,b1,b2,c1,d0,d1,i1"
python assemble_phase1.py $lab "<title>" > docs\apply\evidence\restructuring\s1k-checkpoint1.txt

# the twin protocol of one case (after the import staged the own twin), with its session job
pwsh -NoProfile -File twin_case.ps1 -Case d0 -Bin <ibcmd-rs.exe> -Job ..\jobs\s2_d0.bsl
#   -StageOwnNative: HARNESS VALIDATION ONLY (the own twin is staged by the native partial import)
```

`python cases.py list` prints the cases. Set `PYTHONDONTWRITEBYTECODE=1` (the repository ignores `__pycache__`, but a
kit that leaves bytecode about is a nuisance). Databases are dropped by the caller with
`F:\ibcmd\lab\04\tools\drop-lab-dbs.ps1 -Track ext -MinIdleMinutes 10`.
