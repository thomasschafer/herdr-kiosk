$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

function Remove-VerbatimPathPrefix([string] $Path) {
    if ($Path.StartsWith("\\?\UNC\", [StringComparison]::OrdinalIgnoreCase)) {
        return "\\" + $Path.Substring(8)
    }
    if ($Path.StartsWith("\\?\", [StringComparison]::OrdinalIgnoreCase)) {
        return $Path.Substring(4)
    }
    return $Path
}

try {
    if ([string]::IsNullOrWhiteSpace($env:HERDR_PLUGIN_ROOT)) {
        throw "HERDR_PLUGIN_ROOT is not set"
    }
    $pluginRoot = Remove-VerbatimPathPrefix $env:HERDR_PLUGIN_ROOT
    $binary = Join-Path $pluginRoot "target\release\herdr-kiosk.exe"
    if (-not (Test-Path -LiteralPath $binary -PathType Leaf)) {
        throw "picker binary not found: $binary"
    }
    & $binary
    exit $LASTEXITCODE
}
catch {
    [Console]::Error.WriteLine("herdr-kiosk: $($_.Exception.Message)")
    exit 1
}
