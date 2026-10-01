# Resolve before any output mutation; reject links that escape the owned F lab.
function Resolve-LoadLabRoot([string]$Path) {
    $resolved = [IO.Path]::GetFullPath($Path).TrimEnd('\')
    if (-not $resolved.StartsWith('F:\ibcmd\lab\05\', [StringComparison]::OrdinalIgnoreCase)) {
        throw 'workload output must stay in an owned F: 0.5 lab'
    }
    $ancestor = $resolved
    while ($ancestor) {
        if (Test-Path -LiteralPath $ancestor) {
            $item = Get-Item -LiteralPath $ancestor -Force
            if ($item.Attributes -band [IO.FileAttributes]::ReparsePoint) { throw 'linked lab paths are refused' }
        }
        $ancestor = [IO.Path]::GetDirectoryName($ancestor)
    }
    $manifest = Join-Path $resolved 'OWNED.json'
    if (-not (Test-Path -LiteralPath $manifest -PathType Leaf)) { throw 'load OWNED.json is required' }
    $ownership = Get-Content -LiteralPath $manifest -Raw | ConvertFrom-Json
    if ($ownership.track -cne 'load') { throw 'lab is not owned by Track load' }
    return $resolved
}
