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
$ghLog = Join-Path $testRoot "gh.log"
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
    $ghStub = Join-Path $testRoot "gh-stub.ps1"
    $ghStubContents = @'
param([Parameter(ValueFromRemainingArguments = $true)][string[]] $GhArguments)
$ErrorActionPreference = "Stop"
if (($GhArguments -join ' ') -eq "auth status") {
    if ($env:HK_TEST_GH_SIGNED_OUT) { exit 1 }
    exit 0
}
Add-Content -LiteralPath $env:HK_TEST_GH_LOG -Value "gh $($GhArguments -join ' ')"
if ($env:HK_TEST_GH_REFUSES) {
    [Console]::Error.WriteLine("verification failed")
    exit 1
}
exit 0
'@
    [IO.File]::WriteAllText($ghStub, $ghStubContents)

    $pwsh = (Get-Process -Id $PID).Path
    $installer = Join-Path $repoScripts "fetch-or-build.ps1"
    $baseUrl = ([Uri]::new((Resolve-Path -LiteralPath $base).Path)).AbsoluteUri.TrimEnd("/")
    $env:HK_REPO_ROOT = $repoRoot
    $env:HK_OUT = $out
    $env:HK_BASE_URL = $baseUrl
    $env:HK_TARGET = $target
    $env:HK_CARGO = $cargoStub
    $env:HK_TEST_LOG = $cargoLog
    $env:HK_GH = Join-Path $testRoot "missing-gh.ps1"
    $env:HK_TEST_GH_LOG = $ghLog
    $env:HK_TEST_GH_REFUSES = $null
    $env:KIOSK_SKIP_ATTESTATION = $null
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
    Assert-Contains $stdout "install the GitHub CLI to check its build attestation too"
    Write-Output "successful stubbed download: ok"

    $env:HK_GH = $ghStub
    Remove-TestFiles @($cargoLog, $ghLog, $out, $stdout, $stderr)
    & $pwsh -NoLogo -NoProfile -NonInteractive -File $installer 1> $stdout 2> $stderr
    Assert-True ($LASTEXITCODE -eq 0) "attested install failed"
    Assert-True (
        Test-Path -LiteralPath $out -PathType Leaf
    ) "attested binary was not installed"
    Assert-True (
        -not (Test-Path -LiteralPath $cargoLog)
    ) "cargo ran for an attested download"
    Assert-Contains $ghLog "--repo thomasschafer/herdr-kiosk --signer-workflow thomasschafer/herdr-kiosk/.github/workflows/release.yml --source-ref refs/tags/v$version --deny-self-hosted-runners"
    Assert-Contains $ghLog "$asset --repo thomasschafer/herdr-kiosk"
    Assert-Contains $stdout "verified ${asset}: checksum and build attestation"
    Write-Output "attested download with the GitHub CLI: ok"

    $env:HK_TEST_GH_REFUSES = "1"
    Remove-TestFiles @($cargoLog, $ghLog, $out, $stdout, $stderr)
    & $pwsh -NoLogo -NoProfile -NonInteractive -File $installer 1> $stdout 2> $stderr
    Assert-True ($LASTEXITCODE -ne 0) "installed a binary the GitHub CLI refused"
    Assert-True (
        -not (Test-Path -LiteralPath $out)
    ) "a refused binary was installed"
    Assert-True (
        -not (Test-Path -LiteralPath $cargoLog)
    ) "cargo ran for a refused download"
    Assert-Contains $stderr "verification failed"
    Assert-Contains $stderr "$asset has no valid build attestation from thomasschafer/herdr-kiosk's release workflow for v$version"
    Write-Output "refused attestation fails without installing or building: ok"

    $env:KIOSK_SKIP_ATTESTATION = "1"
    Remove-TestFiles @($cargoLog, $ghLog, $out, $stdout, $stderr)
    & $pwsh -NoLogo -NoProfile -NonInteractive -File $installer 1> $stdout 2> $stderr
    Assert-True ($LASTEXITCODE -eq 0) "install skipping the attestation failed"
    Assert-True (
        Test-Path -LiteralPath $out -PathType Leaf
    ) "binary was not installed when skipping the attestation"
    Assert-True (
        -not (Test-Path -LiteralPath $ghLog)
    ) "the GitHub CLI ran although KIOSK_SKIP_ATTESTATION was set"
    Assert-Contains $stdout "KIOSK_SKIP_ATTESTATION skips its build attestation"
    Write-Output "KIOSK_SKIP_ATTESTATION skips the attestation check: ok"

    $env:KIOSK_SKIP_ATTESTATION = $null
    $env:HK_TEST_GH_REFUSES = $null
    $env:HK_TEST_GH_SIGNED_OUT = "1"
    Remove-TestFiles @($cargoLog, $ghLog, $out, $stdout, $stderr)
    & $pwsh -NoLogo -NoProfile -NonInteractive -File $installer 1> $stdout 2> $stderr
    Assert-True ($LASTEXITCODE -eq 0) "install with a signed-out GitHub CLI failed"
    Assert-True (
        Test-Path -LiteralPath $out -PathType Leaf
    ) "binary was not installed with a signed-out GitHub CLI"
    Assert-True (
        -not (Test-Path -LiteralPath $ghLog)
    ) "a signed-out GitHub CLI was asked to verify"
    Assert-Contains $stdout "log in to the GitHub CLI to check its build attestation too"
    $env:HK_TEST_GH_SIGNED_OUT = $null
    Write-Output "a signed-out GitHub CLI falls back to the checksum: ok"

    Write-Output "fetch-or-build ps tests: PASS"
}
finally {
    if (Test-Path -LiteralPath $testRoot) {
        Remove-Item -LiteralPath $testRoot -Recurse -Force
    }
}
