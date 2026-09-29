# Fast loop on a debug clone: native import of a tree, then native apply (no trace).
#   pwsh -NoProfile -File try_native.ps1 -Database <db> -Tree <dir> [-NoApply]
param(
    [Parameter(Mandatory = $true)][string]$Database,
    [Parameter(Mandatory = $true)][string]$Tree,
    [switch]$NoApply
)
$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = [Text.Encoding]::UTF8
if ($Database -notmatch '^ibcmd_rs_04_ddl_[a-z0-9_]+$') { throw "lab databases only: ibcmd_rs_04_ddl_* (got $Database)" }
$ibcmd = 'C:\Program Files\1cv8\8.3.27.2214\bin\ibcmd.exe'
$labRoot = if ($env:DDL_LAB) { $env:DDL_LAB } else { 'F:\ibcmd\lab\04\restructure' }
$data = "$labRoot\ibdata\$Database"
New-Item -ItemType Directory -Force $data | Out-Null
$sw = [Diagnostics.Stopwatch]::StartNew()
$o = & $ibcmd infobase config import --dbms=MSSQLServer --db-server=localhost --db-name=$Database --data=$data --user=Администратор $Tree 2>&1
"import exit=$LASTEXITCODE in {0:n1}s: {1}" -f $sw.Elapsed.TotalSeconds, (($o | Select-Object -Last 2) -join ' | ')
if ($LASTEXITCODE -ne 0) { exit 1 }
if ($NoApply) { exit 0 }
$sw.Restart()
$o = & $ibcmd infobase config apply --dbms=MSSQLServer --db-server=localhost --db-name=$Database --data=$data --user=Администратор --force --dynamic=disable 2>&1
"apply exit=$LASTEXITCODE in {0:n1}s: {1}" -f $sw.Elapsed.TotalSeconds, (($o | Select-Object -Last 3) -join ' | ')
exit $LASTEXITCODE
