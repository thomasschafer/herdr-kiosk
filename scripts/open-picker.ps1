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
    if ([string]::IsNullOrWhiteSpace($env:HERDR_BIN_PATH)) {
        throw "HERDR_BIN_PATH is not set"
    }
    $herdr = Remove-VerbatimPathPrefix $env:HERDR_BIN_PATH
    & $herdr plugin pane open `
        --plugin thomasschafer.herdr-kiosk `
        --entrypoint picker-windows
    exit $LASTEXITCODE
}
catch {
    [Console]::Error.WriteLine("herdr-kiosk: $($_.Exception.Message)")
    exit 1
}
