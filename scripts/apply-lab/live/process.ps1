# Bounded child processes owned by the LIVE lab. No unrelated process is stopped.
function Invoke-LiveBounded {
    param([string]$Executable, [string[]]$Arguments, [int]$TimeoutSeconds = 30)
    $psi = [Diagnostics.ProcessStartInfo]::new($Executable)
    $psi.UseShellExecute = $false; $psi.CreateNoWindow = $true
    $psi.RedirectStandardOutput = $true; $psi.RedirectStandardError = $true
    foreach ($arg in $Arguments) { $psi.ArgumentList.Add($arg) }
    $p = [Diagnostics.Process]::new(); $p.StartInfo = $psi
    try {
        if (-not $p.Start()) { throw 'child process failed to start' }
        $stdout = $p.StandardOutput.ReadToEndAsync(); $stderr = $p.StandardError.ReadToEndAsync()
        if (-not $p.WaitForExit($TimeoutSeconds * 1000)) {
            $p.Kill($true); $p.WaitForExit()
            throw "owned child exceeded $TimeoutSeconds seconds: $Executable"
        }
        [pscustomobject]@{ ExitCode = $p.ExitCode; Stdout = $stdout.GetAwaiter().GetResult(); Stderr = $stderr.GetAwaiter().GetResult() }
    } finally { $p.Dispose() }
}

function Invoke-LiveRac {
    param([string]$Executable, [string[]]$Arguments)
    $result = Invoke-LiveBounded -Executable $Executable -Arguments $Arguments -TimeoutSeconds 5
    if ($result.ExitCode -ne 0) { throw "rac failed: $($result.Stderr)" }
    $result.Stdout -split '\r?\n'
}
