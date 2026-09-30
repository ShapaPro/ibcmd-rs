# One external-connection (COM) session of a lab infobase that OPENS A SERVER TRANSACTION, writes a constant in it and holds it open until a
# release file appears -- the "work in flight" that the live switch would roll back (#409 F-10). Then it commits and says how it went.
# Windows PowerShell 5.1 (powershell.exe): the COM connector is 64-bit.
#   powershell -NoProfile -File txn_session.ps1 -Srvr localhost:5541 -Database <db> -ReleaseFile <path> [-HoldSec 300] [-Value 20260930]
# Prints: "READY session-sql-pid ..." once the transaction is open, then "COMMITTED" or "LOST <error text>" (the connection or the
# transaction was taken away: the work is gone), or "ERROR <text>" (exit 3 for a licence text: never worked round).
param(
    [Parameter(Mandatory = $true)][string]$Srvr,
    [Parameter(Mandatory = $true)][ValidatePattern('^ibcmd_rs_0[45]_')][string]$Database,
    [Parameter(Mandatory = $true)][string]$ReleaseFile,
    [int]$HoldSec = 300,
    [int]$Value = 20260930,
    [string]$User = 'Администратор'
)
$ErrorActionPreference = 'Stop'
[Console]::OutputEncoding = [Text.Encoding]::UTF8
$flags = [System.Reflection.BindingFlags]
function Root-Message($e) { $i = $e.Exception; while ($i.InnerException) { $i = $i.InnerException }; $i.Message.Trim() }
function Get-Prop($object, $name) { [System.__ComObject].InvokeMember($name, $flags::GetProperty, $null, $object, $null) }
function Call($object, $name, $arguments) { [System.__ComObject].InvokeMember($name, $flags::InvokeMethod, $null, $object, $arguments) }
try {
    $connector = New-Object -ComObject V83.COMConnector
    $connection = $connector.Connect("Srvr=`"$Srvr`";Ref=`"$Database`";Usr=`"$User`";Pwd=`"`"")
} catch {
    $m = Root-Message $_
    "ERROR $m"
    if ($m -match '(?i)лиценз|licen[cs]e|ключ|HASP') { exit 3 }
    exit 2
}
$opened = $false
try {
    [void](Call $connection 'НачатьТранзакцию' $null)
    $opened = $true
    $constants = Get-Prop $connection 'Константы'
    $constant = Get-Prop $constants '_ДемоКодНовогоУзлаПланаОбмена'
    [void](Call $constant 'Установить' @([object]$Value))
    "READY transaction open, constant set to $Value"
    $end = (Get-Date).AddSeconds($HoldSec)
    while ((Get-Date) -lt $end -and -not (Test-Path -LiteralPath $ReleaseFile)) { Start-Sleep -Milliseconds 300 }
    [void](Call $connection 'ЗафиксироватьТранзакцию' $null)
    'COMMITTED'
} catch {
    $text = Root-Message $_
    if ($opened) { "LOST $text" } else { "ERROR $text" }
} finally {
    [void][System.Runtime.InteropServices.Marshal]::ReleaseComObject($connection)
}
