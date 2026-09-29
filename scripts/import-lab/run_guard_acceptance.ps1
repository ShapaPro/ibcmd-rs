# Acceptance of the import guard on a БСП clone (issue #388, checkpoint 2).
#
#   pwsh -NoProfile -File run_guard_acceptance.ps1 -Exe F:\...\ibcmd-rs.exe -Tag acc1 [-Database ibcmd_rs_04_import_bsp_s1]
#
# For each edit of edits.py: the edited tree goes through the drop-in `infobase config import` with its default
# flags (patch stage of a database that holds the configuration, guard on) into a staging clone that is never
# applied. The expectation comes from the matrix of docs/import/patch-mode.md:
#   refuse-guard  patch mode loses the edit silently: the guard must refuse (exit -1, files listed) and ConfigSave
#                 must be exactly what it was before the run;
#   refuse-patch  patch mode itself refuses (new objects, dangling references): exit -1, a Russian message, ConfigSave
#                 untouched;
#   pass          patch mode carries the edit: exit 0 and a staged ConfigSave.
# ConfigSave is fingerprinted before and after every run (row count, a checksum of every row's bytes, the last
# Modified time). Results: out\guard-acceptance\<tag>.json and a table on stdout. Lab databases only.
param(
    [Parameter(Mandatory = $true)][string]$Exe,
    [Parameter(Mandatory = $true)][string]$Tag,
    [string]$Database = 'ibcmd_rs_04_import_bsp_s1',
    [string]$Work = '',
    [string[]]$Changes = @()
)
$Changes = @($Changes | ForEach-Object { $_ -split ',' } | Where-Object { $_ })
$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = [Text.Encoding]::UTF8
$env:PYTHONUTF8 = '1'; $env:PYTHONIOENCODING = 'utf-8'; $env:PYTHONDONTWRITEBYTECODE = '1'
$here = Split-Path -Parent $MyInvocation.MyCommand.Path
. "$here\native.ps1"
$lab = $script:Lab
Assert-LabDb $Database
$base = "$lab\tree\base"; $work = if ($Work) { $Work } else { "$lab\tree\combo2" }
$outDir = "$lab\out\guard-acceptance"
New-Item -ItemType Directory -Force $outDir | Out-Null
$free = (Get-PSDrive F).Free / 1GB
if ($free -lt 25) { throw "F: has $([math]::Round($free,1)) GB free; not starting" }

# what a case is expected to do; `noop` first (it fills ConfigSave with a stage), refusals next, carried edits last
$expect = [ordered]@{
    noop = 'pass'
    attr = 'refuse-guard'; ts = 'refuse-guard'; prop = 'refuse-guard'; attrprop = 'refuse-guard'; attrdel = 'refuse-guard'
    enumval = 'refuse-guard'; subsys = 'refuse-guard'; nestsub = 'refuse-guard'; confver = 'refuse-guard'
    predefdel = 'refuse-guard'; formdel = 'refuse-guard'
    newcat = 'refuse-patch'; newform = 'refuse-patch'; newtpl = 'refuse-patch'; predef = 'refuse-patch'
    catdel = 'refuse-patch'; catfile = 'refuse-patch'
    syn = 'pass'; rights = 'pass'; ci = 'pass'; module = 'pass'; predefedit = 'pass'
}
if ($Changes.Count -eq 0) { $Changes = @($expect.Keys) }

function Get-ConfigSaveFingerprint([string]$Db) {
    $q = "SET NOCOUNT ON; SELECT CONVERT(varchar(20), COUNT_BIG(*)) + '|' + CONVERT(varchar(20), ISNULL(CHECKSUM_AGG(BINARY_CHECKSUM(FileName, PartNo, DataSize, Attributes, BinaryData)), 0)) + '|' + ISNULL(CONVERT(varchar(30), MAX(Modified), 126), '') FROM dbo.ConfigSave"
    (sqlcmd -S localhost -E -C -h -1 -W -d $Db -Q $q | Select-Object -First 1).Trim()
}

$results = @()
foreach ($change in $Changes) {
    python "$here\edits.py" reset --base $base --work $work | Out-Null
    if ($change -ne 'noop') { python "$here\edits.py" apply $change --base $base --work $work | Out-Null }
    $before = Get-ConfigSaveFingerprint $Database
    $r = Invoke-OursImport -Db $Database -Tree $work -Tag "$Tag-$change" -Exe $Exe
    $after = Get-ConfigSaveFingerprint $Database
    $report = "$lab\out\import-$Tag-$change.json"
    $message = ''; $verified = $null
    if (Test-Path $report) {
        $j = Get-Content $report -Raw | ConvertFrom-Json
        if ($j.PSObject.Properties.Name -contains 'error') { $message = [string]$j.error }
        if ($j.PSObject.Properties.Name -contains 'verification') { $verified = $j.verification }
    }
    $unchanged = ($before -eq $after)
    $want = $expect[$change]
    if (-not $want) { $want = 'pass' }
    $ok = switch ($want) {
        'refuse-guard' { $r.Exit -ne 0 -and $unchanged -and $message -match 'Загрузка отменена' }
        'refuse-patch' { $r.Exit -ne 0 -and $unchanged -and $message -notmatch 'Config row not found|failed to pack|failed to resolve' -and $message -match '[А-Яа-я]' }
        default { $r.Exit -eq 0 -and -not $unchanged }
    }
    $first = if ($message) { ($message -split "`n")[0] } else { '' }
    $row = [ordered]@{
        change = $change; expected = $want; exit = $r.Exit; seconds = $r.Seconds
        config_save_unchanged = $unchanged; before = $before; after = $after
        checked_files = if ($verified) { $verified.checked_files } else { $null }
        verify_seconds = if ($verified) { $verified.total_seconds } else { $null }
        ok = [bool]$ok; message = $first
    }
    $results += [pscustomobject]$row
    '{0,-10} {1,-13} exit={2,-3} {3,6}s unchanged={4,-5} {5} | {6}' -f $change, $want, $r.Exit, $r.Seconds, $unchanged, $(if ($ok) { 'OK ' } else { 'FAIL' }), $first.Substring(0, [Math]::Min(150, $first.Length))
}
python "$here\edits.py" reset --base $base --work $work | Out-Null
$results | ConvertTo-Json -Depth 4 | Set-Content "$outDir\$Tag.json" -Encoding UTF8
$bad = @($results | Where-Object { -not $_.ok })
"summary: {0} cases, {1} as expected, {2} not" -f $results.Count, ($results.Count - $bad.Count), $bad.Count
if ($bad.Count) { exit 1 }
