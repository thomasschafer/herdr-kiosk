# Installs kiosk's Herdr app mod on Windows: the mod files and herdr-kiosk.exe, together in
# <app home>\mods\kiosk, where the mod looks for them.
#
#   irm https://raw.githubusercontent.com/thomasschafer/herdr-kiosk/main/scripts/install-app.ps1 | iex
#   scripts\install-app.ps1 [-FromSource] [-Version <tag>]
#
# Run on its own, it fetches the latest release (or -Version, or $env:KIOSK_INSTALL_VERSION)
# and installs its verified prebuilt binary, building from source only where there is none.
# Run from a checkout, it installs that checkout's mod with that checkout's release binary,
# or with -FromSource, a binary built from the checkout. The app home is $env:HERDR_APP_HOME,
# else $env:XDG_CONFIG_HOME\herdr-app, else ~\.config\herdr-app, as the app has it.
$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

# Read from $args rather than a param block, which `irm ... | iex` would refuse.
$FromSource = $false
$Version = $env:KIOSK_INSTALL_VERSION
for ($i = 0; $i -lt $args.Count; $i++) {
    switch ($args[$i]) {
        "-FromSource" { $FromSource = $true }
        "-Version" {
            if ($i + 1 -ge $args.Count) {
                throw "install-app: -Version needs a tag, such as v0.3.0"
            }
            $i++
            $Version = $args[$i]
        }
        default { throw "install-app: unknown option $($args[$i])" }
    }
}

$repoUrl = if ($env:KIOSK_INSTALL_REPO) { $env:KIOSK_INSTALL_REPO } else { "https://github.com/thomasschafer/herdr-kiosk.git" }
$modName = "kiosk"

function Write-Step([string] $Message) {
    [Console]::Error.WriteLine("install-app: $Message")
}

function Stop-Install([string] $Message) {
    throw "install-app: $Message"
}

if ($env:HERDR_APP_HOME) {
    $appHome = $env:HERDR_APP_HOME
}
elseif ($env:XDG_CONFIG_HOME) {
    $appHome = Join-Path $env:XDG_CONFIG_HOME "herdr-app"
}
else {
    $appHome = Join-Path (Join-Path $HOME ".config") "herdr-app"
}
$modsDir = Join-Path $appHome "mods"
$modDir = Join-Path $modsDir $modName

# The checkout this script is in, if it is in one and no other version was asked for.
$checkout = $null
if (-not $Version -and $PSScriptRoot) {
    $candidate = Split-Path -Parent $PSScriptRoot
    if (
        (Test-Path -LiteralPath (Join-Path $candidate "herdr-plugin.toml")) -and
        (Test-Path -LiteralPath (Join-Path (Join-Path (Join-Path $candidate "herdr-app") "kiosk") "main.js"))
    ) {
        $checkout = $candidate
    }
}
if (-not $checkout -and $FromSource) {
    Stop-Install "-FromSource needs a checkout: clone the repository and run scripts\install-app.ps1 from it"
}

# A linked checkout is a development install: writing into it would put the binary in the
# checkout. It keeps working as it is.
if (Test-Path -LiteralPath $modDir) {
    $existing = Get-Item -LiteralPath $modDir -Force
    if ($existing.LinkType) {
        Stop-Install "$modDir is a link (to $($existing.Target)); remove it to install a copy instead"
    }
}

$temporaryDirectory = Join-Path ([IO.Path]::GetTempPath()) "herdr-kiosk-install-$([Guid]::NewGuid().ToString('N'))"
New-Item -ItemType Directory -Path $temporaryDirectory | Out-Null
try {
    if (-not $checkout) {
        if (-not (Get-Command git -ErrorAction SilentlyContinue)) {
            Stop-Install "git is needed to fetch the release"
        }
        if (-not $Version) {
            $tags = git ls-remote --tags --refs $repoUrl "v*" |
                ForEach-Object { ($_ -split "refs/tags/")[-1] } |
                Where-Object { $_ -match '^v\d+\.\d+\.\d+$' }
            $Version = $tags | Sort-Object { [version] $_.Substring(1) } | Select-Object -Last 1
            if (-not $Version) {
                Stop-Install "no release tags found at $repoUrl"
            }
        }
        Write-Step "fetching herdr-kiosk $Version"
        $checkout = Join-Path $temporaryDirectory "checkout"
        git -c advice.detachedHead=false clone --quiet --depth 1 --branch $Version $repoUrl $checkout
        if ($LASTEXITCODE -ne 0) {
            Stop-Install "could not fetch $Version from $repoUrl"
        }
    }

    $binary = Join-Path $temporaryDirectory "herdr-kiosk.exe"
    if ($FromSource) {
        Write-Step "building $checkout"
        if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
            Stop-Install "cargo is needed to build from source: https://rustup.rs/"
        }
        Push-Location $checkout
        try {
            cargo build --locked --release
            if ($LASTEXITCODE -ne 0) {
                Stop-Install "the build failed"
            }
        }
        finally {
            Pop-Location
        }
        $targetDir = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { Join-Path $checkout "target" }
        Copy-Item -LiteralPath (Join-Path (Join-Path $targetDir "release") "herdr-kiosk.exe") -Destination $binary
    }
    else {
        $savedRoot = $env:HK_REPO_ROOT
        $savedOut = $env:HK_OUT
        try {
            $env:HK_REPO_ROOT = $checkout
            $env:HK_OUT = $binary
            & (Join-Path (Join-Path $checkout "scripts") "fetch-or-build.ps1") | ForEach-Object { [Console]::Error.WriteLine($_) }
        }
        finally {
            $env:HK_REPO_ROOT = $savedRoot
            $env:HK_OUT = $savedOut
        }
        if (-not (Test-Path -LiteralPath $binary)) {
            Stop-Install "could not get the herdr-kiosk binary"
        }
    }

    $staged = Join-Path $temporaryDirectory $modName
    $stagedBin = Join-Path $staged "bin"
    New-Item -ItemType Directory -Path $stagedBin -Force | Out-Null
    $source = Join-Path (Join-Path $checkout "herdr-app") "kiosk"
    Copy-Item -LiteralPath (Join-Path $source "main.js"), (Join-Path $source "mod.json") -Destination $staged
    Copy-Item -LiteralPath $binary -Destination (Join-Path $stagedBin "herdr-kiosk.exe")

    # Swapped in whole, so the app never loads half of an update.
    New-Item -ItemType Directory -Path $modsDir -Force | Out-Null
    $old = "$modDir.old"
    if (Test-Path -LiteralPath $old) {
        Remove-Item -LiteralPath $old -Recurse -Force
    }
    if (Test-Path -LiteralPath $modDir) {
        Move-Item -LiteralPath $modDir -Destination $old
    }
    Move-Item -LiteralPath $staged -Destination $modDir
    if (Test-Path -LiteralPath $old) {
        Remove-Item -LiteralPath $old -Recurse -Force
    }
    Write-Step "installed $modDir"

    if (Get-Command herdr-app -ErrorAction SilentlyContinue) {
        $savedHome = $env:HERDR_APP_HOME
        try {
            $env:HERDR_APP_HOME = $appHome
            herdr-app check $modDir | ForEach-Object { [Console]::Error.WriteLine($_) }
            if ($LASTEXITCODE -ne 0) {
                Stop-Install "the app could not load $modDir"
            }
        }
        finally {
            $env:HERDR_APP_HOME = $savedHome
        }
    }
    Write-Step "bind a key to open it in $(Join-Path $appHome 'keys.json'), for example:"
    [Console]::Error.WriteLine('  { "global": { "ctrl-shift-r": "mod.open:kiosk" } }')
}
finally {
    if (Test-Path -LiteralPath $temporaryDirectory) {
        Remove-Item -LiteralPath $temporaryDirectory -Recurse -Force
    }
}
