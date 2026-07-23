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

function Get-EnvironmentValue([string] $Name, [string] $Default) {
    $value = [Environment]::GetEnvironmentVariable($Name)
    if ([string]::IsNullOrWhiteSpace($value)) {
        return $Default
    }
    return $value
}

function New-ParentDirectory([string] $Path) {
    $parent = Split-Path -Parent $Path
    if (-not [string]::IsNullOrWhiteSpace($parent)) {
        New-Item -ItemType Directory -Path $parent -Force | Out-Null
    }
}

function Save-Download([string] $Url, [string] $Destination) {
    $uri = [Uri] $Url
    if ($uri.Scheme -eq [Uri]::UriSchemeFile) {
        Copy-Item -LiteralPath $uri.LocalPath -Destination $Destination -Force
        return
    }
    if (
        $uri.Scheme -ne [Uri]::UriSchemeHttp -and
        $uri.Scheme -ne [Uri]::UriSchemeHttps
    ) {
        throw "unsupported download URL scheme: $($uri.Scheme)"
    }
    Invoke-WebRequest -Uri $uri -OutFile $Destination -UseBasicParsing
}

function Invoke-SourceBuild(
    [string] $Reason,
    [string] $RepoRoot,
    [string] $Out,
    [string] $Cargo
) {
    [Console]::Error.WriteLine(
        "fetch-or-build: prebuilt binary unavailable ($Reason); building from source."
    )

    if ($null -eq (Get-Command -Name $Cargo -ErrorAction SilentlyContinue)) {
        [Console]::Error.WriteLine(
            "fetch-or-build: cargo is required for the source-build fallback."
        )
        [Console]::Error.WriteLine(
            "Install Rust with rustup: https://rustup.rs/"
        )
        exit 1
    }

    Push-Location -LiteralPath $RepoRoot
    try {
        & $Cargo build --locked --release
        $cargoSucceeded = $?
    }
    finally {
        Pop-Location
    }
    if (-not $cargoSucceeded) {
        [Console]::Error.WriteLine(
            "fetch-or-build: cargo build --locked --release failed."
        )
        exit 1
    }

    $targetRoot = Get-EnvironmentValue "CARGO_TARGET_DIR" (Join-Path $RepoRoot "target")
    if (-not [IO.Path]::IsPathRooted($targetRoot)) {
        $targetRoot = Join-Path $RepoRoot $targetRoot
    }
    $builtOut = Join-Path (Join-Path $targetRoot "release") "herdr-kiosk.exe"
    if (-not (Test-Path -LiteralPath $builtOut -PathType Leaf)) {
        [Console]::Error.WriteLine(
            "fetch-or-build: cargo succeeded but did not produce $builtOut"
        )
        exit 1
    }
    if (
        -not $builtOut.Equals(
            $Out,
            [StringComparison]::OrdinalIgnoreCase
        )
    ) {
        New-ParentDirectory $Out
        Copy-Item -LiteralPath $builtOut -Destination $Out -Force
    }
}

$defaultRepoRoot = Join-Path $PSScriptRoot ".."
$repoRoot = Remove-VerbatimPathPrefix (
    Get-EnvironmentValue "HK_REPO_ROOT" $defaultRepoRoot
)
$defaultOut = Join-Path (
    Join-Path $repoRoot "target"
) "release"
$defaultOut = Join-Path $defaultOut "herdr-kiosk.exe"
$out = Remove-VerbatimPathPrefix (
    Get-EnvironmentValue "HK_OUT" $defaultOut
)
$cargo = Get-EnvironmentValue "HK_CARGO" "cargo"
$temporaryDirectory = $null

try {
    $manifest = Join-Path $repoRoot "herdr-plugin.toml"
    $versionMatches = @()
    foreach ($line in (Get-Content -LiteralPath $manifest)) {
        if ($line -match '^\s*version\s*=\s*"([^"]*)"\s*$') {
            $versionMatches += $Matches[1]
        }
    }
    if ($versionMatches.Count -ne 1) {
        throw "expected exactly one version in $manifest, found $($versionMatches.Count)"
    }
    $version = $versionMatches[0]
    if ($version -notmatch '^[0-9A-Za-z.+-]+$') {
        throw "could not parse a valid version from $manifest"
    }

    $target = Get-EnvironmentValue "HK_TARGET" "x86_64-pc-windows-msvc"
    switch ($target) {
        "x86_64-pc-windows-msvc" {
            $asset = "herdr-kiosk-v$version-$target.exe"
        }
        default {
            throw "unmapped target $target"
        }
    }

    $defaultBaseUrl =
        "https://github.com/thomasschafer/herdr-kiosk/releases/download/v$version"
    $baseUrl = (Get-EnvironmentValue "HK_BASE_URL" $defaultBaseUrl).TrimEnd("/")
    $temporaryDirectory = Join-Path (
        [IO.Path]::GetTempPath()
    ) "herdr-kiosk-fetch-$([Guid]::NewGuid().ToString('N'))"
    New-Item -ItemType Directory -Path $temporaryDirectory | Out-Null
    $sumsPath = Join-Path $temporaryDirectory "SHA256SUMS"
    $assetPath = Join-Path $temporaryDirectory $asset

    try {
        Save-Download "$baseUrl/SHA256SUMS" $sumsPath
    }
    catch {
        throw "could not download SHA256SUMS"
    }
    try {
        Save-Download "$baseUrl/$asset" $assetPath
    }
    catch {
        throw "could not download $asset"
    }

    $expected = $null
    foreach ($line in (Get-Content -LiteralPath $sumsPath)) {
        if ($line -match '^\s*(\S+)\s+\*?(\S+)\s*$') {
            if ($Matches[2].Equals($asset, [StringComparison]::Ordinal)) {
                $expected = $Matches[1]
                break
            }
        }
    }
    if ($null -eq $expected -or $expected -notmatch '^[0-9A-Fa-f]{64}$') {
        throw "SHA256SUMS has no valid entry for $asset"
    }

    $actual = (Get-FileHash -LiteralPath $assetPath -Algorithm SHA256).Hash
    if (-not $expected.Equals($actual, [StringComparison]::OrdinalIgnoreCase)) {
        throw "checksum mismatch for $asset"
    }

    New-ParentDirectory $out
    Copy-Item -LiteralPath $assetPath -Destination $out -Force
    Write-Output "fetch-or-build: installed verified $asset at $out"
}
catch {
    Invoke-SourceBuild $_.Exception.Message $repoRoot $out $cargo
}
finally {
    if (
        $null -ne $temporaryDirectory -and
        (Test-Path -LiteralPath $temporaryDirectory)
    ) {
        Remove-Item -LiteralPath $temporaryDirectory -Recurse -Force
    }
}
