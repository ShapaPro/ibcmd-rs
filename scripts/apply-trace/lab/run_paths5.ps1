# Path experiment, third round (native-apply-trace.md, section 6.4).  One arm per call: stage (no lock), then ONE native apply
# under the native lock, released at once.
#   Y3  common module edited again on c4_paths (its previous apply, Y2, took the short path)
#   Y4  common module edited on c4_x4 (its previous apply, Y1, took the short path)
#   Y5  the stage of case 1c (document + common module + catalog, tree1c) on a FRESH clone: does the union go long by itself?
#   Y6  the document alone (Y1) on a FRESH clone
#   Y7  document + catalog object module (no common module) on a FRESH clone
param(
    [Parameter(Mandatory = $true)][ValidateSet('Y3', 'Y4', 'Y5', 'Y6', 'Y7')][string]$Id,
    [switch]$SkipStage
)
$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = [System.Text.Encoding]::UTF8
$lab = 'F:\ibcmd\lab\04\trace'
$lock = 'F:\ibcmd\lab\04\tools\heavy-lock.ps1'
$ours = 'F:\ibcmd\src\ibcmd-rs-m-simple\target\iter\ibcmd-rs.exe'
$ibcmd = 'C:\Program Files\1cv8\8.3.27.2214\bin\ibcmd.exe'
$out = "$lab\paths"
function Log($m) { Write-Host ('[{0}] PATHS5 {1} {2}' -f (Get-Date -Format 'HH:mm:ss'), $Id, $m) }
function Sql($db, $q) { (& sqlcmd -S localhost -E -C -d $db -h -1 -W -Q "SET NOCOUNT ON; $q" | Out-String).Trim() }
$plans = @{
    Y3 = @{ Db = 'ibcmd_rs_04_trace_c4_paths'; Tree = 'tree1d'; Prefixes = @('CommonModules/_ДемоЛокализацияКлиентСервер'); Restore = $false; Note = 'common module edited again; the previous apply of this database (Y2) took the short path' }
    Y4 = @{ Db = 'ibcmd_rs_04_trace_c4_x4'; Tree = 'tree1d'; Prefixes = @('CommonModules/_ДемоЛокализацияКлиентСервер'); Restore = $false; Note = 'common module edited; the previous apply of this database (Y1) took the short path' }
    Y5 = @{ Db = 'ibcmd_rs_04_trace_c5_union'; Tree = 'tree1c'; Prefixes = @('Documents/_ДемоПоручениеЭкспедитору', 'CommonModules/_ДемоЛокализацияКлиентСервер', 'Catalogs/_ДемоКонтрагенты'); Restore = $true; Note = 'the stage of case 1c (document + common module + catalog object module) on a fresh clone' }
    Y6 = @{ Db = 'ibcmd_rs_04_trace_c5_doc'; Tree = 'tree1c'; Prefixes = @('Documents/_ДемоПоручениеЭкспедитору'); Restore = $true; Note = 'the document alone (manager module edited, its forms and help staged) on a fresh clone' }
    Y7 = @{ Db = 'ibcmd_rs_04_trace_c5_pair'; Tree = 'tree1c'; Prefixes = @('Documents/_ДемоПоручениеЭкспедитору', 'Catalogs/_ДемоКонтрагенты'); Restore = $true; Note = 'document + catalog object module (no common module) on a fresh clone' }
}
$p = $plans[$Id]
if (-not $SkipStage) {
    if ($p.Restore) {
        Log "restore a fresh clone $($p.Db)"
        & pwsh -NoProfile -File 'F:\ibcmd\lab\04\tools\restore-clone.ps1' -Corpus bsp8327 -Name $p.Db -Track trace -Purpose "trace: path-selection experiment $Id (a fresh clone, one staged set, one native apply)"
        if ($LASTEXITCODE -ne 0) { throw "restore of $($p.Db) failed" }
    }
    Log "stage $($p.Prefixes -join ' + ') of $($p.Tree) into $($p.Db)"
    $env:IBCMD_RS_NATIVE_FORM_WRITER = 'always'
    $pp = @(); foreach ($x in $p.Prefixes) { $pp += '--path-prefix'; $pp += $x }
    & $ours mssql-stage-source-objects --database $p.Db --source-root "$lab\$($p.Tree)" --platform 8.3.27 --replace-config-save --allow-non-lab @pp *>&1 | Out-File -Encoding utf8 -LiteralPath "$out\$Id-stage.log"
    $code = $LASTEXITCODE
    $env:IBCMD_RS_NATIVE_FORM_WRITER = $null
    if ($code -ne 0) { throw "stage $Id failed ($code)" }
}
$rows = Sql $p.Db 'SELECT COUNT(*) FROM ConfigSave'
$idBefore = Sql $p.Db 'SELECT TOP 1 CONVERT(varchar(34), _IDRRef, 1) FROM _ConfigChngR ORDER BY _IDRRef'
New-Item -ItemType Directory -Force -Path "$lab\ibdata\$($p.Db)" | Out-Null
Log "staged $rows rows; waiting for the native lock"
& pwsh -NoProfile -File $lock acquire trace -Name native -TimeoutMin 240
if ($LASTEXITCODE -ne 0) { throw 'native lock not acquired' }
try {
    $sw = [Diagnostics.Stopwatch]::StartNew()
    & $ibcmd infobase config apply --dbms=MSSQLServer --db-server=localhost "--db-name=$($p.Db)" "--data=$lab\ibdata\$($p.Db)" --user=Администратор --force --dynamic=disable *>&1 | Out-File -Encoding utf8 -LiteralPath "$out\$Id-apply.log"
    $exit = $LASTEXITCODE
    $sec = [Math]::Round($sw.Elapsed.TotalSeconds, 1)
} finally {
    & pwsh -NoProfile -File $lock release trace -Name native
}
$idAfter = Sql $p.Db 'SELECT TOP 1 CONVERT(varchar(34), _IDRRef, 1) FROM _ConfigChngR ORDER BY _IDRRef'
$log = Get-Content -LiteralPath "$out\$Id-apply.log" -Raw
$line = "{0}`t{1}`tstaged={2}`texit={3}`t{4}s`tstructure_phase={5}`texchange_registration={6}`tregister_rebuilt_log={7}`tregister_ids_changed={8}`t{9}" -f `
    $Id, $p.Note, $rows, $exit, $sec, ($log -match 'Обработка структуры базы данных'), ($log -match 'Регистрация изменений в планах обмена'),
    ($log -match 'Реструктуризация\s+Таблица регистрации'), ($idBefore -ne $idAfter), (Get-Date -Format s)
Add-Content -LiteralPath "$out\summary.tsv" -Value $line -Encoding utf8
Log $line
