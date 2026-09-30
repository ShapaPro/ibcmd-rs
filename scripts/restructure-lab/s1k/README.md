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

Checkpoint 2 in one command (the import phase, the twin protocol with the session job, check 12 per case; check 11 for i1):

```powershell
pwsh -NoProfile -File run_checkpoint2.ps1 -Bin <ibcmd-rs.exe> -Cases "a1,b1,b2,c1,d0,d1,i1" [-Drop]
# or by hand, per case, after import_phase.ps1 staged the own twin:
pwsh -NoProfile -File twin_case.ps1 -Case b1 -Bin <ibcmd-rs.exe> -Job ..\jobs\s2_b1.bsl   # also takes out\<case>\staged.bak
pwsh -NoProfile -File check12.ps1 -Case b1                                                # the injected failure on a fresh twin
pwsh -NoProfile -File check11.ps1 -Case i1 -Bin <ibcmd-rs.exe> [-Native]                  # the refusal (adopted objects)
```

`python cases.py list` prints the cases. Set `PYTHONDONTWRITEBYTECODE=1` (the repository ignores `__pycache__`, but a
kit that leaves bytecode about is a nuisance). Databases are dropped by the caller with
`F:\ibcmd\lab\04\tools\drop-lab-dbs.ps1 -Track ext -MinIdleMinutes 10`.
