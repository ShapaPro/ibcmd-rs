# Extended Events helpers for the ddl track. Dot-source: . scripts\restructure-lab\xe.ps1
# Sessions are named ibcmd_rs_04_ddl_<n> and filtered to one database; files under
# C:\temp\ibcmd_rs_04\ddl\. Only sessions with that prefix are ever created/stopped/dropped.
# The lab folder is $env:DDL_LAB (default F:\ibcmd\lab\04\restructure).

$script:LabRoot = if ($env:DDL_LAB) { $env:DDL_LAB } else { 'F:\ibcmd\lab\04\restructure' }
$script:XeDir = 'C:\temp\ibcmd_rs_04\ddl'

function Invoke-Sql([string]$Sql, [string]$Db = 'master') {
    $f = Join-Path $env:TEMP ("ddlsql-{0}.sql" -f [guid]::NewGuid().ToString('N'))
    [System.IO.File]::WriteAllText($f, $Sql, [System.Text.UTF8Encoding]::new($true))
    try {
        & sqlcmd -S localhost -E -C -b -f 65001 -d $Db -i $f
        if ($LASTEXITCODE -ne 0) { throw "sqlcmd failed ($LASTEXITCODE)" }
    } finally { Remove-Item -LiteralPath $f -ErrorAction SilentlyContinue }
}

function Start-DdlXe([string]$Name, [string]$Db) {
    if ($Name -notmatch '^ibcmd_rs_04_ddl_[a-z0-9_]+$') { throw "session name must start with ibcmd_rs_04_ddl_ (got $Name)" }
    if ($Db -notmatch '^ibcmd_rs_04_ddl_[a-z0-9_]+$') { throw "database must start with ibcmd_rs_04_ddl_ (got $Db)" }
    New-Item -ItemType Directory -Force $script:XeDir | Out-Null
    $file = Join-Path $script:XeDir "$Name.xel"
    $act = 'ACTION (sqlserver.session_id, sqlserver.client_app_name, sqlserver.database_name, sqlserver.transaction_id, sqlserver.sql_text)'
    $flt = "WHERE ([sqlserver].[database_name] = N'$Db')"
    $sql = @"
SET NOCOUNT ON;
IF EXISTS (SELECT 1 FROM sys.server_event_sessions WHERE name = N'$Name') DROP EVENT SESSION [$Name] ON SERVER;
CREATE EVENT SESSION [$Name] ON SERVER
ADD EVENT sqlserver.sql_batch_completed ($act $flt),
ADD EVENT sqlserver.rpc_completed ($act $flt),
ADD EVENT sqlserver.sql_statement_completed ($act $flt),
ADD EVENT sqlserver.object_created ($act $flt),
ADD EVENT sqlserver.object_altered ($act $flt),
ADD EVENT sqlserver.object_deleted ($act $flt)
ADD TARGET package0.event_file (SET filename = N'$file', max_file_size = 2048, max_rollover_files = 2)
WITH (MAX_MEMORY = 128 MB, EVENT_RETENTION_MODE = NO_EVENT_LOSS, MAX_DISPATCH_LATENCY = 1 SECONDS, TRACK_CAUSALITY = ON, STARTUP_STATE = OFF);
ALTER EVENT SESSION [$Name] ON SERVER STATE = START;
"@
    Invoke-Sql $sql
    "xe started: $Name -> $file"
}

function Stop-DdlXe([string]$Name) {
    if ($Name -notmatch '^ibcmd_rs_04_ddl_[a-z0-9_]+$') { throw "not my session: $Name" }
    $sql = @"
SET NOCOUNT ON;
IF EXISTS (SELECT 1 FROM sys.dm_xe_sessions WHERE name = N'$Name') ALTER EVENT SESSION [$Name] ON SERVER STATE = STOP;
IF EXISTS (SELECT 1 FROM sys.server_event_sessions WHERE name = N'$Name') DROP EVENT SESSION [$Name] ON SERVER;
"@
    Invoke-Sql $sql
    "xe stopped and dropped: $Name"
}

function Get-DdlXeSessions {
    Invoke-Sql "SET NOCOUNT ON; SELECT name FROM sys.server_event_sessions WHERE name LIKE N'ibcmd_rs_04_ddl[_]%'; SELECT name, create_time FROM sys.dm_xe_sessions WHERE name LIKE N'ibcmd_rs_04_ddl[_]%';"
}

# native ibcmd, platform 8.3.27.2214, on one lab database
function Invoke-NativeApply([string]$Db, [string]$Dynamic = 'disable', [string]$Data = '', [int]$TimeoutSec = 3600) {
    $ibcmd = 'C:\Program Files\1cv8\8.3.27.2214\bin\ibcmd.exe'
    if (-not $Data) { $Data = "$($script:LabRoot)\ibdata\$Db" }
    New-Item -ItemType Directory -Force "$($script:LabRoot)\logs" | Out-Null
    if (-not (Test-Path "$($script:LabRoot)\logs\empty.txt")) { New-Item -ItemType File "$($script:LabRoot)\logs\empty.txt" | Out-Null }
    New-Item -ItemType Directory -Force $Data | Out-Null
    $args = @('infobase', 'config', 'apply', '--dbms=MSSQLServer', '--db-server=localhost', "--db-name=$Db",
              "--data=$Data", '--user=Администратор', '--force', "--dynamic=$Dynamic")
    $p = Start-Process -FilePath $ibcmd -ArgumentList $args -NoNewWindow -PassThru `
        -RedirectStandardOutput "$($script:LabRoot)\logs\native-apply-$Db.out.txt" `
        -RedirectStandardError "$($script:LabRoot)\logs\native-apply-$Db.err.txt" `
        -RedirectStandardInput "$($script:LabRoot)\logs\empty.txt"
    # poll instead of a plain wait: note every other ibcmd process seen while ours runs
    # (concurrent native ibcmd runs of other tracks may crash each other - evidence)
    $deadline = (Get-Date).AddSeconds($TimeoutSec)
    $seen = @{}
    while (-not $p.HasExited) {
        Get-Process ibcmd -ErrorAction SilentlyContinue | Where-Object { $_.Id -ne $p.Id } | ForEach-Object { $seen[$_.Id] = $_.StartTime.ToString('s') }
        if ((Get-Date) -gt $deadline) { $p.Kill(); throw "native apply timeout ($TimeoutSec s)" }
        Start-Sleep -Milliseconds 1500
    }
    $p.WaitForExit()
    $note = ($seen.GetEnumerator() | ForEach-Object { "pid=$($_.Key) started=$($_.Value)" }) -join '; '
    Set-Content -LiteralPath "$($script:LabRoot)\logs\native-apply-$Db.concurrent.txt" -Value ("exit=$($p.ExitCode) others=[$note]")
    $p.ExitCode
}

