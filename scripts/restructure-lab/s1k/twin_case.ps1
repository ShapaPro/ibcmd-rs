# S1-K, the twin protocol (docs/apply/restructuring.md, 12.6) for one case: the native side (native `import files --partial` of the
# edited files + native `config apply`) against OUR side (the twin our import staged in import_phase.ps1, then OUR apply:
# `mssql-config-apply --allow-restructure s1 --i-have-a-backup`).
#
#   pwsh -NoProfile -File twin_case.ps1 -Case b1 -Bin <ibcmd-rs.exe> [-Job jobs\s2_b1.bsl] [-Skip export,session,noop]
#   -StageOwnNative   HARNESS VALIDATION ONLY: stage our twin with the native partial import instead of our import (it says nothing
#                     about our import; it exercises the rest of the chain)
#
# Checks: 10 (a rehearsal changes nothing), 3 (EXCEPT both ways: the rebuilt tables, Config, every table of the extension schema),
# 4 (Config rows; part of 3), 5 (DBSchema, DBNames), 6 (the 16 .si rows), 7 (a native apply on our twin), 8 (native export of both,
# ibcmd-rs source-diff), 9 (a cluster session on both, outputs compared). Check 1 (the offline plan) is the corpus tests'; 2 is the
# snapshot diff printed here; 11 and 12 are the refusal (check11.ps1, case i1) and the injected failure (check12.ps1, on the script
# and the backup of the staged twin this writes: out\<case>\script_real.sql, staged.bak; -Skip check12 leaves the backup out).
param(
    [Parameter(Mandatory = $true)][string]$Case,
    [Parameter(Mandatory = $true)][string]$Bin,
    [switch]$StageOwnNative,
    [string]$Job = '',
    [string[]]$Skip = @()
)
$ErrorActionPreference = 'Continue'
[Console]::OutputEncoding = [Text.Encoding]::UTF8
$env:PYTHONUTF8 = '1'
$env:PYTHONDONTWRITEBYTECODE = '1'
$lab = if ($env:S1K_LAB) { $env:S1K_LAB } else { 'F:\ibcmd\lab\05\ext\s1k' }
$env:S1K_LAB = $lab
$env:DDL_LAB = "$lab\snap"
$kit = $PSScriptRoot
$rk = Split-Path $kit -Parent
$o = "$lab\out\$Case"
$nat = "ibcmd_rs_05_ext_s1k_${Case}_nat"
$own = "ibcmd_rs_05_ext_s1k_${Case}_own"
function Log($m) { "[{0}] {1}" -f (Get-Date -Format s), $m }
function Exists($db) { (sqlcmd -S localhost -E -C -h -1 -W -Q "SET NOCOUNT ON; SELECT COUNT(*) FROM sys.databases WHERE name = N'$db'").Trim() -ne '0' }
function Restore($db, $why) {
    if (-not (Exists $db)) {
        pwsh -NoProfile -File F:\ibcmd\lab\04\tools\restore-clone.ps1 -Corpus bak -Bak "$lab\bak\base_applied.bak" -Name $db -Track ext -Purpose $why | Select-Object -Last 1
    }
}
$files = (Get-Content "$o\files.txt" -Encoding UTF8 | Where-Object { $_ }) -join ','
if (-not $files) { throw "no edit of case $Case in $o (run import_phase.ps1 first)" }

Log "native side ($nat)"
Restore $nat "S1-K case $Case (native)"
if ((sqlcmd -S localhost -E -C -d $nat -h -1 -W -Q "SET NOCOUNT ON; SELECT COUNT(*) FROM ConfigSave").Trim() -ne '0') { "the native twin has a stage already" }
else {
    pwsh -NoProfile -File "$kit\native_main.ps1" -Database $nat -Action ImportFiles -BaseDir "$o\stage" -Files $files | Select-String -Pattern 'exit' | ForEach-Object { $_.Line }
    pwsh -NoProfile -File "$kit\native_main.ps1" -Database $nat -Action Apply | Select-String -Pattern 'exit|успешно|ошибк' | ForEach-Object { $_.Line }
}
python "$rk\snapshot.py" $nat "${Case}_nat_after" | Select-Object -Last 1

if ($StageOwnNative) {
    Log 'HARNESS VALIDATION: staging our twin natively'
    Restore $own "S1-K case $Case (own, staged natively for the harness)"
    pwsh -NoProfile -File "$kit\native_main.ps1" -Database $own -Action ImportFiles -BaseDir "$o\stage" -Files $files | Select-String -Pattern 'exit' | ForEach-Object { $_.Line }
}
python "$rk\snapshot.py" $own "${Case}_staged" | Select-Object -Last 1

$common = @('mssql-config-apply', '--platform-profile', 'platform-8.3.27.2214', '--database', $own, '--allow-restructure', 's1', '--allow-non-lab')
Log 'our apply: dry run'
& $Bin @common --dry-run --report "$o\dry.json" --script-output "$o\script_dry.sql" > "$o\dry.txt" 2> "$o\dry.err"
"dry exit $LASTEXITCODE"
if ($LASTEXITCODE -ne 0) { Get-Content "$o\dry.txt", "$o\dry.err" -Tail 6 | ForEach-Object { if ($_.Length -gt 400) { $_.Substring(0, 400) } else { $_ } }; throw 'the dry run refused' }
Log 'our apply: rehearsal (check 10)'
& $Bin @common --rehearse --i-have-a-backup --report "$o\rehearse.json" --script-output "$o\script_rehearse.sql" > "$o\rehearse.txt" 2> "$o\rehearse.err"
"rehearse exit $LASTEXITCODE"
python "$rk\snapshot.py" $own "${Case}_rehearsed" | Select-Object -Last 1
python "$rk\snapdiff.py" $own "${Case}_staged" $own "${Case}_rehearsed" > "$o\rehearse_diff.txt"
"check 10: " + ((Get-Content "$o\rehearse_diff.txt" -Encoding UTF8 | Select-String -Pattern '^changed \(0\)|^\(0\)' ) -join ' ')
if ($Skip -notcontains 'check12') {
    # the staged state, for check12.ps1 (a fresh twin the injected failure runs on)
    sqlcmd -S localhost -E -C -b -Q "BACKUP DATABASE [$own] TO DISK = N'$o\staged.bak' WITH COPY_ONLY, COMPRESSION, INIT" | Select-Object -Last 1
}
Log 'our apply: real run'
& $Bin @common --i-have-a-backup --report "$o\real.json" --script-output "$o\script_real.sql" --recovery-dir "$o\recovery" > "$o\real.txt" 2> "$o\real.err"
"real exit $LASTEXITCODE"
if ($LASTEXITCODE -ne 0) { Get-Content "$o\real.err" -Tail 5; throw 'the real run failed' }
$report = Get-Content "$o\real.json" -Raw -Encoding UTF8 | ConvertFrom-Json
"structure: " + (($report.structure.objects) -join ' | ')
python "$rk\snapshot.py" $own "${Case}_own_after" | Select-Object -Last 1

Log 'checks 2-6'
python "$rk\snapdiff.py" $nat "${Case}_nat_after" $own "${Case}_own_after" --max 12 > "$o\nat_vs_own.txt"
Get-Content "$o\nat_vs_own.txt" -Encoding UTF8 | Select-String -Pattern '^(changed|added|removed) \(|^  T |^\(\d+\)' | ForEach-Object { $_.Line }
$tables = @($report.structure.tables)   # Config is check 4 (dates and the generation differ when the twins were staged apart)
python "$kit\compare_twins.py" $nat $own @tables 2>&1 | Tee-Object "$o\check3.txt" | Select-Object -Last 6
python "$kit\check4.py" $nat $own 2>&1 | Tee-Object "$o\check4.txt"
python "$kit\checks56.py" $nat "${Case}_nat_after" $own "${Case}_own_after" 2>&1 | Tee-Object "$o\check56.txt"

if ($Skip -notcontains 'noop') {
    Log 'check 7: a native config apply on our twin'
    pwsh -NoProfile -File "$kit\native_main.ps1" -Database $own -Action Apply *> "$o\native_noop.txt"
    "check 7: " + ((Get-Content "$o\native_noop.txt" -Encoding UTF8 | Select-String -Pattern 'не требуется|exit') -join ' | ')
}
if ($Skip -notcontains 'export') {
    Log 'check 8: native exports and source-diff'
    foreach ($db in $nat, $own) {
        Remove-Item -Recurse -Force "$lab\export\$db" -ErrorAction SilentlyContinue
        pwsh -NoProfile -File "$kit\native_main.ps1" -Database $db -Action Export -Out "$lab\export\$db" | Select-Object -First 1
    }
    python "$kit\check8.py" "$lab\export\$nat" "$lab\export\$own" 2>&1 | Tee-Object "$o\check8.txt"
    Remove-Item -Recurse -Force "$lab\export\$nat", "$lab\export\$own" -ErrorAction SilentlyContinue
}
if ($Job -and $Skip -notcontains 'session') {
    Log 'check 9: cluster session'
    $reg = 'F:\ibcmd\lab\04\tools\register-ib.ps1'
    foreach ($side in 'nat', 'own') {
        $db = if ($side -eq 'nat') { $nat } else { $own }
        pwsh -NoProfile -File $reg register -Database $db -Platform 8.3.27 -Track ext 2>&1 | Select-Object -Last 1
        try {
            pwsh -NoProfile -File "$rk\session_job.ps1" -Job $Job -Database $db -Epf F:\ibcmd\lab\04\restructure\probe\ddl_probe.epf -TimeoutSec 600 > "$o\session_$side.txt" 2>&1
        } finally {
            pwsh -NoProfile -File $reg unregister -Database $db -Platform 8.3.27 -Track ext 2>&1 | Select-Object -Last 1
        }
    }
    "check 9: session outputs identical: " + ((Get-Content "$o\session_nat.txt" -Raw) -eq (Get-Content "$o\session_own.txt" -Raw)) + " (" + (Get-Content "$o\session_own.txt").Count + " lines)"
}
Log "done $Case"
