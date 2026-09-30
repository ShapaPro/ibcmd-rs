# S1-K check 11 (docs/apply/restructuring.md 12.6): the refusal of the case. The own twin holds the stage OUR import made (case i1: an
# attribute on two objects an extension adopts). OUR apply, as a dry run and as a real run (with the backup consent), must refuse with
# the reason named and write nothing: the digest of the database is the same after the real run. The platform's own apply on a
# sibling twin, for the record, does what the refusal hands over to it.
#
#   pwsh -NoProfile -File check11.ps1 -Case i1 -Bin <ibcmd-rs.exe> [-Expect '_ДемоПартнеры,_ДемоНоменклатура'] [-Native]
#
# The twin is ibcmd_rs_05_ext_s1k_<case>_own, staged by import_phase.ps1. Writes out\<case>\check11.txt.
param(
    [Parameter(Mandatory = $true)][string]$Case,
    [Parameter(Mandatory = $true)][string]$Bin,
    [string]$Expect = '_ДемоПартнеры,_ДемоНоменклатура',
    [switch]$Native
)
$ErrorActionPreference = 'Continue'
[Console]::OutputEncoding = [Text.Encoding]::UTF8
. "$PSScriptRoot\digest.ps1"
$lab = if ($env:S1K_LAB) { $env:S1K_LAB } else { 'F:\ibcmd\lab\05\ext\s1k' }
$o = "$lab\out\$Case"
$own = "ibcmd_rs_05_ext_s1k_${Case}_own"
$lines = New-Object System.Collections.Generic.List[string]
function Say($m) { $lines.Add($m); $m }
$common = @('mssql-config-apply', '--platform-profile', 'platform-8.3.27.2214', '--database', $own, '--allow-restructure', 's1', '--allow-non-lab')

$before = Get-LabDigest $own
$ok = $true
foreach ($mode in 'dry run', 'real run') {
    $extra = if ($mode -eq 'dry run') { @('--dry-run') } else { @('--i-have-a-backup', '--recovery-dir', "$o\recovery11") }
    $stem = if ($mode -eq 'dry run') { 'refuse_dry' } else { 'refuse_real' }
    & $Bin @common @extra --report "$o\$stem.json" > "$o\$stem.out" 2> "$o\$stem.err"
    $exit = $LASTEXITCODE
    $text = [regex]::Replace(((Get-Content "$o\$stem.out", "$o\$stem.err" -Encoding UTF8 -Raw) -join "`n"), "`e\[[0-9;]*m", '')
    Say "our apply, ${mode}: exit $exit"
    if ($exit -eq 0) { Say '  NOT REFUSED'; $ok = $false }
    foreach ($name in ($Expect -split ',' | Where-Object { $_ })) {
        $named = $text.Contains($name)
        Say ("  names {0}: {1}" -f $name, $named)
        if (-not $named) { $ok = $false }
    }
    $named = $text -match 'adopted by the extension'
    Say ("  says the object is adopted by an extension: {0}" -f $named)
    if (-not $named) { $ok = $false }
}
$after = Get-LabDigest $own
$same = (($before -join "`n") -eq ($after -join "`n"))
Say "digest unchanged after the real run: $same"
if (-not $same) { $ok = $false }
if ($Native) {
    Say 'the platform apply on the same twin (what the refusal hands over to):'
    $null = pwsh -NoProfile -File "$PSScriptRoot\native_main.ps1" -Database $own -Action Apply *> "$o\native_after_refusal.txt"
    Say ((Get-Content "$o\native_after_refusal.txt" -Encoding UTF8 | Select-String -Pattern 'exit|успешно|ошибк' | ForEach-Object { $_.Line }) -join ' | ')
}
Say "check 11: $(if ($ok) { 'refused with the reasons, nothing written' } else { 'FAILED' })"
$lines | Set-Content -LiteralPath "$o\check11.txt" -Encoding UTF8
if (-not $ok) { exit 1 }
