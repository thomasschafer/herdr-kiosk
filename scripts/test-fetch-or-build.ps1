$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

function Assert-True([bool] $Condition, [string] $Message) {
    if (-not $Condition) {
        throw $Message
    }
}

function Assert-Contains([string] $Path, [string] $Expected) {
    $contents = Get-Content -LiteralPath $Path -Raw
    if (-not $contents.Contains($Expected)) {
        throw "expected '$Expected' in $Path"
    }
}

function Remove-TestFiles([string[]] $Paths) {
    foreach ($path in $Paths) {
        Remove-Item -LiteralPath $path -Force -ErrorAction SilentlyContinue
    }
}

$projectRoot = Split-Path -Parent $PSScriptRoot
$testRoot = Join-Path (
    [IO.Path]::GetTempPath()
) "herdr-kiosk-fetch-test-$([Guid]::NewGuid().ToString('N'))"
$repoRoot = Join-Path $testRoot "repo"
$releaseRoot = Join-Path $testRoot "releases"
$out = Join-Path (Join-Path $testRoot "out") "herdr-kiosk.exe"
$cargoLog = Join-Path $testRoot "cargo.log"
$stdout = Join-Path $testRoot "installer.out"
$stderr = Join-Path $testRoot "installer.err"

try {
    $repoScripts = Join-Path $repoRoot "scripts"
    New-Item -ItemType Directory -Path @(
        $repoScripts,
        $releaseRoot
    ) -Force | Out-Null
    Copy-Item -LiteralPath (
        Join-Path (Join-Path $projectRoot "scripts") "fetch-or-build.ps1"
    ) -Destination (
        Join-Path $repoScripts "fetch-or-build.ps1"
    )
    Copy-Item -LiteralPath (
        Join-Path $projectRoot "herdr-plugin.toml"
    ) -Destination (Join-Path $repoRoot "herdr-plugin.toml")

    $manifestContents = Get-Content -LiteralPath (
        Join-Path $projectRoot "herdr-plugin.toml"
    )
    $versionMatches = @()
    foreach ($line in $manifestContents) {
        if ($line -match '^\s*version\s*=\s*"([^"]*)"\s*$') {
            $versionMatches += $Matches[1]
        }
    }
    Assert-True ($versionMatches.Count -eq 1) "test manifest must contain one version"
    $version = $versionMatches[0]
    $target = "x86_64-pc-windows-msvc"
    $asset = "herdr-kiosk-v$version-$target.exe"
    $base = Join-Path $releaseRoot "v$version"
    New-Item -ItemType Directory -Path $base | Out-Null
    $assetPath = Join-Path $base $asset
    $sumsPath = Join-Path $base "SHA256SUMS"
    $cargoStub = Join-Path $testRoot "cargo-stub.ps1"
    $cargoStubContents = @'
param([Parameter(ValueFromRemainingArguments = $true)][string[]] $CargoArguments)
$ErrorActionPreference = "Stop"
Add-Content -LiteralPath $env:HK_TEST_LOG -Value "cargo $($CargoArguments -join ' ')"
$built = Join-Path (
    Join-Path (Join-Path $env:HK_REPO_ROOT "target") "release"
) "herdr-kiosk.exe"
New-Item -ItemType Directory -Path (Split-Path -Parent $built) -Force | Out-Null
[IO.File]::WriteAllText($built, "built from source`n")
'@
    [IO.File]::WriteAllText($cargoStub, $cargoStubContents)

    $pwsh = (Get-Process -Id $PID).Path
    $installer = Join-Path $repoScripts "fetch-or-build.ps1"
    $baseUrl = ([Uri]::new((Resolve-Path -LiteralPath $base).Path)).AbsoluteUri.TrimEnd("/")
    $env:HK_REPO_ROOT = $repoRoot
    $env:HK_OUT = $out
    $env:HK_BASE_URL = $baseUrl
    $env:HK_TARGET = $target
    $env:HK_CARGO = $cargoStub
    $env:HK_TEST_LOG = $cargoLog
    $env:CARGO_TARGET_DIR = Join-Path $repoRoot "target"

    [IO.File]::WriteAllText($sumsPath, "$("0" * 64)  $asset`n")
    Remove-TestFiles @($assetPath, $cargoLog, $out, $stdout, $stderr)
    & $pwsh -NoLogo -NoProfile -NonInteractive -File $installer 1> $stdout 2> $stderr
    Assert-True ($LASTEXITCODE -eq 0) "missing-asset fallback failed"
    Assert-Contains $stderr "could not download $asset"
    Assert-Contains $stderr "building from source"
    Assert-Contains $cargoLog "cargo build --locked --release"
    Write-Output "fallback when no asset: ok"

    [IO.File]::WriteAllText($assetPath, "downloaded but corrupt`n")
    [IO.File]::WriteAllText($sumsPath, "$("0" * 64)  $asset`n")
    Remove-TestFiles @($cargoLog, $out, $stdout, $stderr)
    & $pwsh -NoLogo -NoProfile -NonInteractive -File $installer 1> $stdout 2> $stderr
    Assert-True ($LASTEXITCODE -eq 0) "checksum-mismatch fallback failed"
    Assert-Contains $stderr "checksum mismatch"
    Assert-Contains $cargoLog "cargo build --locked --release"
    Write-Output "checksum mismatch falls back: ok"

    $fixtureBytes = [Text.Encoding]::UTF8.GetBytes(
        "stubbed release binary`r`n"
    )
    [IO.File]::WriteAllBytes($assetPath, $fixtureBytes)
    $checksum = (Get-FileHash -LiteralPath $assetPath -Algorithm SHA256).Hash
    [IO.File]::WriteAllText($sumsPath, "$checksum *$asset`n")
    Remove-TestFiles @($cargoLog, $out, $stdout, $stderr)
    & $pwsh -NoLogo -NoProfile -NonInteractive -File $installer 1> $stdout 2> $stderr
    Assert-True ($LASTEXITCODE -eq 0) "verified install failed"
    Assert-True (
        Test-Path -LiteralPath $out -PathType Leaf
    ) "verified binary was not installed"
    $expectedBytes = [Convert]::ToBase64String(
        [IO.File]::ReadAllBytes($assetPath)
    )
    $actualBytes = [Convert]::ToBase64String(
        [IO.File]::ReadAllBytes($out)
    )
    Assert-True ($expectedBytes -eq $actualBytes) "installed binary bytes differ"
    Assert-True (
        -not (Test-Path -LiteralPath $cargoLog)
    ) "cargo ran for a verified download"
    Assert-Contains $stdout "installed verified"
    Write-Output "successful stubbed download: ok"

    Write-Output "fetch-or-build ps tests: PASS"
}
finally {
    if (Test-Path -LiteralPath $testRoot) {
        Remove-Item -LiteralPath $testRoot -Recurse -Force
    }
}
