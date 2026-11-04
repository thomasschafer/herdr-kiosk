# Tests install-app.ps1 against a fake release and a fake app, from a checkout and from a
# release fetched with git.
$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

function Assert-True([bool] $Condition, [string] $Message) {
    if (-not $Condition) {
        throw "FAIL: $Message"
    }
}

# Text is compared ignoring line endings, as git on Windows checks files out with CRLF.
function Assert-SameText([string] $Left, [string] $Right) {
    $read = { param($Path) (Get-Content -LiteralPath $Path -Raw) -replace "`r`n", "`n" }
    Assert-True ((& $read $Left) -ceq (& $read $Right)) "$Left differs from $Right"
}

function Assert-Same([string] $Left, [string] $Right) {
    Assert-True ((Get-FileHash -LiteralPath $Left).Hash -eq (Get-FileHash -LiteralPath $Right).Hash) "$Left differs from $Right"
}

$projectRoot = Split-Path -Parent $PSScriptRoot
$testRoot = Join-Path ([IO.Path]::GetTempPath()) "herdr-kiosk-install-test-$([Guid]::NewGuid().ToString('N'))"
$repo = Join-Path $testRoot "repo"
$releases = Join-Path $testRoot "releases"
$bin = Join-Path $testRoot "bin"
$calls = Join-Path $testRoot "herdr-app-calls"
$saved = @{}
foreach ($name in "HERDR_APP_HOME", "XDG_CONFIG_HOME", "HK_BASE_URL", "KIOSK_INSTALL_REPO", "PATH") {
    $saved[$name] = [Environment]::GetEnvironmentVariable($name)
}

# A file:// URL for a local path, which [Uri] only builds from Windows paths.
function ConvertTo-FileUrl([string] $Path) {
    $full = [IO.Path]::GetFullPath($Path).Replace("\", "/")
    if (-not $full.StartsWith("/")) {
        $full = "/$full"
    }
    return "file://$full"
}

function Invoke-Installer([string] $Script, [string[]] $Arguments = @()) {
    $output = & pwsh -NoLogo -NoProfile -File $Script @Arguments 2>&1 | Out-String
    return @{ Code = $LASTEXITCODE; Output = $output }
}

function Assert-Installed([string] $AppHome) {
    $mod = Join-Path (Join-Path $AppHome "mods") "kiosk"
    Assert-Same (Join-Path (Join-Path $mod "bin") "herdr-kiosk.exe") (Join-Path $releases $asset)
    Assert-SameText (Join-Path $mod "main.js") (Join-Path (Join-Path (Join-Path $projectRoot "herdr-app") "kiosk") "main.js")
    Assert-SameText (Join-Path $mod "mod.json") (Join-Path (Join-Path (Join-Path $projectRoot "herdr-app") "kiosk") "mod.json")
    Assert-True ((Get-Content -LiteralPath $calls -Raw).Contains("check $mod")) "the app did not check $mod"
}

try {
    $repoScripts = Join-Path $repo "scripts"
    $repoMod = Join-Path (Join-Path $repo "herdr-app") "kiosk"
    New-Item -ItemType Directory -Path $repoScripts, $repoMod, $releases, $bin -Force | Out-Null
    foreach ($script in "install-app.ps1", "fetch-or-build.ps1") {
        Copy-Item -LiteralPath (Join-Path (Join-Path $projectRoot "scripts") $script) -Destination $repoScripts
    }
    Copy-Item -LiteralPath (Join-Path $projectRoot "herdr-plugin.toml") -Destination $repo
    foreach ($file in "main.js", "mod.json") {
        Copy-Item -LiteralPath (Join-Path (Join-Path (Join-Path $projectRoot "herdr-app") "kiosk") $file) -Destination $repoMod
    }

    $version = $null
    foreach ($line in (Get-Content -LiteralPath (Join-Path $projectRoot "herdr-plugin.toml"))) {
        if ($line -match '^version = "([^"]*)"$') {
            $version = $Matches[1]
        }
    }
    $asset = "herdr-kiosk-v$version-x86_64-pc-windows-msvc.exe"
    Set-Content -LiteralPath (Join-Path $releases $asset) -Value "released"
    $hash = (Get-FileHash -LiteralPath (Join-Path $releases $asset) -Algorithm SHA256).Hash.ToLowerInvariant()
    Set-Content -LiteralPath (Join-Path $releases "SHA256SUMS") -Value "$hash  $asset"

    # A stand-in for the app that records what it is asked.
    if ($IsWindows -or $env:OS -eq "Windows_NT") {
        Set-Content -LiteralPath (Join-Path $bin "herdr-app.cmd") -Value "@echo %HERDR_APP_HOME% %*>>`"$calls`""
    }
    else {
        Set-Content -LiteralPath (Join-Path $bin "herdr-app") -Value "#!/bin/sh`nprintf '%s %s\n' `"`$HERDR_APP_HOME`" `"`$*`" >>'$calls'"
        chmod +x (Join-Path $bin "herdr-app")
    }
    New-Item -ItemType File -Path $calls | Out-Null
    $env:PATH = "$bin$([IO.Path]::PathSeparator)$($env:PATH)"
    $env:HK_BASE_URL = ConvertTo-FileUrl $releases

    Write-Output "test: installs from a checkout with its verified release binary"
    $home1 = Join-Path $testRoot "home"
    $env:HERDR_APP_HOME = $home1
    $run = Invoke-Installer (Join-Path $repoScripts "install-app.ps1")
    Assert-True ($run.Code -eq 0) "the install failed: $($run.Output)"
    Assert-True ($run.Output.Contains("installed verified")) "the binary was not verified: $($run.Output)"
    Assert-Installed $home1

    Write-Output "test: reinstalling replaces the whole folder"
    $stale = Join-Path (Join-Path (Join-Path $home1 "mods") "kiosk") "stale.js"
    Set-Content -LiteralPath $stale -Value ""
    $run = Invoke-Installer (Join-Path $repoScripts "install-app.ps1")
    Assert-True ($run.Code -eq 0) "the reinstall failed: $($run.Output)"
    Assert-True (-not (Test-Path -LiteralPath $stale)) "a stale file survived"
    Assert-Installed $home1

    Write-Output "test: the app home follows XDG_CONFIG_HOME"
    $env:HERDR_APP_HOME = $null
    $env:XDG_CONFIG_HOME = Join-Path $testRoot "xdg"
    $run = Invoke-Installer (Join-Path $repoScripts "install-app.ps1")
    Assert-True ($run.Code -eq 0) "the install failed: $($run.Output)"
    Assert-Installed (Join-Path $env:XDG_CONFIG_HOME "herdr-app")
    $env:XDG_CONFIG_HOME = $null

    Write-Output "test: a linked development install is left alone"
    $linked = Join-Path $testRoot "linked"
    New-Item -ItemType Directory -Path (Join-Path $linked "mods") -Force | Out-Null
    $linkType = if ($IsWindows -or $env:OS -eq "Windows_NT") { "Junction" } else { "SymbolicLink" }
    New-Item -ItemType $linkType -Path (Join-Path (Join-Path $linked "mods") "kiosk") -Target $repoMod | Out-Null
    $env:HERDR_APP_HOME = $linked
    $run = Invoke-Installer (Join-Path $repoScripts "install-app.ps1")
    Assert-True ($run.Code -ne 0) "installed over a link"
    Assert-True ($run.Output.Contains("is a link")) "unclear refusal: $($run.Output)"
    Assert-True (-not (Test-Path -LiteralPath (Join-Path $repoMod "bin"))) "wrote into the linked checkout"

    Write-Output "test: run on its own, it fetches the newest release tag"
    $git = @("-C", $repo, "-c", "user.email=test@example.com", "-c", "user.name=Test", "-c", "commit.gpgSign=false", "-c", "tag.gpgSign=false", "-c", "tag.forceSignAnnotated=false")
    git @git init -q
    git @git add -A
    git @git commit -q -m release
    foreach ($tag in "v0.0.9", "v0.0.10", "v$version", "v0.0.1-rc.1") {
        git @git tag $tag
    }
    $elsewhere = Join-Path $testRoot "elsewhere"
    New-Item -ItemType Directory -Path $elsewhere | Out-Null
    Copy-Item -LiteralPath (Join-Path $repoScripts "install-app.ps1") -Destination $elsewhere
    $released = Join-Path $testRoot "released"
    $env:HERDR_APP_HOME = $released
    $env:KIOSK_INSTALL_REPO = ConvertTo-FileUrl $repo
    $run = Invoke-Installer (Join-Path $elsewhere "install-app.ps1")
    Assert-True ($run.Code -eq 0) "the release install failed: $($run.Output)"
    Assert-True ($run.Output.Contains("fetching herdr-kiosk v$version")) "did not pick v${version}: $($run.Output)"
    Assert-Installed $released

    Write-Output "test: options are checked"
    $run = Invoke-Installer (Join-Path $repoScripts "install-app.ps1") @("-Frobnicate")
    Assert-True ($run.Code -ne 0 -and $run.Output.Contains("unknown option -Frobnicate")) "took an unknown option: $($run.Output)"
    $run = Invoke-Installer (Join-Path $elsewhere "install-app.ps1") @("-FromSource")
    Assert-True ($run.Code -ne 0 -and $run.Output.Contains("needs a checkout")) "built without a checkout: $($run.Output)"

    Write-Output "install-app.ps1 tests passed"
}
finally {
    foreach ($name in $saved.Keys) {
        [Environment]::SetEnvironmentVariable($name, $saved[$name])
    }
    if (-not $env:HK_INSTALL_TEST_KEEP) { Remove-Item -LiteralPath $testRoot -Recurse -Force -ErrorAction SilentlyContinue }
}
