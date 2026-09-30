# The F-4 repro of #408 (finding F-4 of #344), on a lab clone of the БСП 8.3.27 corpus that carries a native online generation:
#   1. an online generation of our own (the module ОбсужденияСлужебныйКлиентСервер, "Telegram" = "G2MARK");
#   2. an exclusive promotion of another module (РаботаСКлассификаторамиКлиентСервер gets ПроверкаIbcmdRsF4());
#   3. what has to be true afterwards: the promotion succeeds, no marker and no `_dynupdate_` row is left, the ordinary rows
#      hold what the aliases held (the native generation and ours), and a NEW session sees both online changes and the new one.
# The acceptance of #408; before step 2 of the issue is done, check 3 (the promotion succeeds) is RED: the old command refuses.
#   pwsh -NoProfile -File f4_repro.ps1 -Exe <ibcmd-rs.exe> -Suffix <name> [-KeepDatabase]
# Lab databases only: ibcmd_rs_04_apply_<Suffix>a_20260930, registered in the 8.3.27 cluster with register-ib.ps1.
param(
    [Parameter(Mandatory = $true)][string]$Exe,
    [Parameter(Mandatory = $true)][ValidatePattern('^[a-z0-9]{2,8}$')][string]$Suffix
)
$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = [Text.Encoding]::UTF8
$env:PYTHONIOENCODING = 'utf-8'
$lab = 'F:\ibcmd\lab\04\apply'
$tools = 'F:\ibcmd\lab\04\tools'
$db = "ibcmd_rs_04_apply_${Suffix}a_20260930"
$rac = 'C:\Program Files\1cv8\8.3.27.2214\bin\rac.exe'
$ras = 'localhost:2545'
$log = "$lab\logs\f4_$Suffix.log"
$failed = New-Object System.Collections.Generic.List[string]
function Say($text) { $line = "$(Get-Date -Format s) $text"; $line | Tee-Object -FilePath $log -Append }
function Check($name, $ok, $detail = '') {
    if ($ok) { Say "PASS  $name $detail" } else { Say "FAIL  $name $detail"; $failed.Add($name) }
}
function Scalar($query) { ([string](sqlcmd -S localhost -E -C -h -1 -W -d $db -Q "SET NOCOUNT ON; $query" | Select-Object -First 1)).Trim() }
# a NEW session of the infobase: "telegram=<value> probe=<value>" (f4_session.ps1)
function Session {
    $o = & powershell.exe -NoProfile -File "$PSScriptRoot\f4_session.ps1" -Database $db 2>&1
    (($o | ForEach-Object { "$_" }) -join ' ').Trim()
}

Say "restore $db"
& pwsh -NoProfile -File "$tools\restore-clone.ps1" -Corpus bsp8327 -Name $db -Track apply -Purpose "#408 F-4 repro" 2>&1 | Select-Object -Last 1 | ForEach-Object { Say $_ }
& pwsh -NoProfile -File "$tools\register-ib.ps1" register -Database $db -Platform 8.3.27 -Track apply 2>&1 | Select-Object -Last 1 | ForEach-Object { Say $_ }
& python "$PSScriptRoot\f4_trees.py" "$lab\src" | ForEach-Object { Say $_ }

$cluster = ((& $rac $ras cluster list) | Select-String '^cluster\s*:' | Select-Object -First 1) -replace '^cluster\s*:\s*', ''
$infobase = $null; $current = $null
foreach ($line in & $rac $ras infobase summary list "--cluster=$cluster") {
    if ($line -match '^infobase\s*:\s*(\S+)') { $current = $Matches[1] } elseif ($line -match '^name\s*:\s*(\S+)' -and $Matches[1] -eq $db) { $infobase = $current }
}
if (-not $infobase) { throw "$db is not registered" }

function Tool($mode, $tree, $module, $tag) {
    $args = @('mssql-apply-source-change', '--platform-profile', 'platform-8.3.27.2214', '--server', 'localhost', '--database', $db,
        '--sqlcmd-trust-cert', '--source-root', "$lab\src\$tree", '--path', "CommonModules/$module/Ext/Module.bsl", '--mode', $mode,
        '--script-output', "$lab\out\f4_$Suffix.$tag.sql", '--recovery-output', "$lab\out\f4_$Suffix.$tag.recovery.json",
        '--rac', $rac, '--ras-endpoint', $ras, '--cluster-id', $cluster, '--infobase-id', $infobase, '--infobase-user', 'Администратор', '--allow-non-lab')
    New-Item -ItemType Directory -Force "$lab\out" | Out-Null
    $out = & $Exe @args 2>"$lab\out\f4_$Suffix.$tag.err"
    $code = $LASTEXITCODE
    $out | Set-Content "$lab\out\f4_$Suffix.$tag.json" -Encoding UTF8
    $code
}

# the base: a native online generation is there
$markers = [int](Scalar "SELECT COUNT(*) FROM dbo.Config WHERE FileName = N'DynamicallyUpdated'")
$aliases0 = [int](Scalar "SELECT COUNT(*) FROM dbo.Config WHERE FileName LIKE N'%[_]dynupdate[_]%'")
Check 'the base carries a native online generation' ($markers -eq 1 -and $aliases0 -gt 0) "(marker rows $markers, alias rows $aliases0)"

# 1. our online generation
$code = Tool 'online' 'g2' 'ОбсужденияСлужебныйКлиентСервер' 'online'
Check 'the online generation is applied' ($code -eq 0) "(exit $code)"
$seen = Session
Check 'a new session sees the online change' ($seen -match 'telegram=G2MARK') "($seen)"

# what the aliases hold now: name, digest
$before = @{}
sqlcmd -S localhost -E -C -h -1 -W -s"|" -d $db -Q "SET NOCOUNT ON; SELECT FileName, CONVERT(varchar(64), HASHBYTES('SHA2_256', BinaryData), 2) FROM dbo.Config WHERE FileName LIKE N'%[_]dynupdate[_]%' AND FileName NOT LIKE N'versions[_]dynupdate%'" |
    ForEach-Object { $p = $_ -split '\|'; if ($p.Count -eq 2) { $before[$p[0].Trim()] = $p[1].Trim() } }
Say "alias rows before the promotion: $($before.Count)"

# 2. the exclusive promotion of another module: RED until #408 step 2
$code = Tool 'exclusive' 'x1' 'РаботаСКлассификаторамиКлиентСервер' 'exclusive'
$stderr = ((Get-Content ("$lab\out\f4_$Suffix.exclusive.err") -Raw -ErrorAction SilentlyContinue) -replace '\s+', ' ')
if ($stderr.Length -gt 300) { $stderr = $stderr.Substring(0, 300) }
Check 'the exclusive promotion succeeds' ($code -eq 0) "(exit $code; $stderr)"
if ($code -ne 0) {
    Check 'the refusal left the database as it was' (([int](Scalar "SELECT COUNT(*) FROM dbo.ConfigSave")) -eq 0) '(ConfigSave rows)'
    Say "RED: $($failed.Count) check(s) failed: $($failed -join '; ')"
    exit 1
}

# 3. afterwards
Check 'no marker is left' (([int](Scalar "SELECT (SELECT COUNT(*) FROM dbo.Config WHERE FileName = N'DynamicallyUpdated') + (SELECT COUNT(*) FROM dbo.Params WHERE FileName = N'DynamicallyUpdated')")) -eq 0)
Check 'no _dynupdate_ row is left' (([int](Scalar "SELECT COUNT(*) FROM dbo.Config WHERE FileName LIKE N'%[_]dynupdate[_]%'")) -eq 0)
$lost = 0
foreach ($name in $before.Keys) {
    $target = $name -replace '_dynupdate_[0-9a-fA-F-]{36}', ''
    $now = Scalar "SELECT CONVERT(varchar(64), HASHBYTES('SHA2_256', BinaryData), 2) FROM dbo.Config WHERE FileName = N'$target' AND PartNo = 0"
    if ($now -ne $before[$name]) { $lost++; Say "  $target differs from the alias it should have become" }
}
Check 'the ordinary rows hold what the aliases held' ($lost -eq 0) "($lost of $($before.Count) differ)"
$seen = Session
Check 'a new session still sees the online change' ($seen -match 'telegram=G2MARK') "($seen)"
Check 'a new session sees the promoted change' ($seen -match 'probe=F4') "($seen)"
if ($failed.Count) { Say "RED: $($failed.Count) check(s) failed: $($failed -join '; ')"; exit 1 }
Say 'GREEN'
