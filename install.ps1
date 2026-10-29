# e4steam-rust installer.
#
# Puts the e4steam Rust port (Steam friends multiplayer) into an
# elyhorizon-launcher checkout and builds it:
#   1. finds an elyhorizon-launcher clone (or clones sambukuper's fork),
#   2. applies the port with `git am` (skipped when it is already in),
#   3. builds horizon-client with the `steam` feature,
#   4. puts the Steamworks redistributable (steam_api64.dll) beside the game.
#
# Usage:
#   irm https://raw.githubusercontent.com/sambukuper/e4steam-rust/main/install.ps1 | iex
#   powershell -ExecutionPolicy Bypass -File install.ps1 [-LauncherDir <path>] [-SkipBuild] [-PatchPath <file>]
#
# Requirements: git, Rust 1.90+ (rustup), Windows x64. Steam must be running
# and signed in to play; the game also needs its resources prepared from your
# own copy of Minecraft (the launcher's Play page does that).
param(
    # An existing elyhorizon-launcher checkout. Default: one next to the
    # current directory, or a fresh clone of the fork made here.
    [string]$LauncherDir = "",
    # Apply a local patch file instead of downloading it (offline, testing).
    [string]$PatchPath = "",
    # Build the game after integrating the port.
    [switch]$SkipBuild
)

$ErrorActionPreference = "Stop"
$LauncherRepo = "https://github.com/sambukuper/elyhorizon-launcher.git"
$PatchUrl = "https://raw.githubusercontent.com/sambukuper/e4steam-rust/main/horizon-integration/e4steam-port.patch"

function Fail($message) {
    Write-Host ""
    Write-Host "e4steam: $message" -ForegroundColor Red
    exit 1
}
function Step($message) {
    Write-Host ""
    Write-Host "== e4steam: $message" -ForegroundColor Cyan
}

Step "finding the launcher"
if (-not $LauncherDir) {
    foreach ($candidate in @("$PWD\elyhorizon-launcher", "$PWD\..\elyhorizon-launcher", "$PWD")) {
        if (Test-Path "$candidate\crates\horizon-client") {
            $LauncherDir = $candidate
            break
        }
    }
}
if (-not $LauncherDir) {
    Write-Host "No elyhorizon-launcher found here; cloning $LauncherRepo (this can take a minute)..."
    git clone --branch main $LauncherRepo elyhorizon-launcher
    if ($LASTEXITCODE -ne 0) { Fail "git clone failed. Is git installed and reachable? You can also pass -LauncherDir <path to your checkout>." }
    $LauncherDir = "$PWD\elyhorizon-launcher"
}
$LauncherDir = (Resolve-Path $LauncherDir).Path
if (-not (Test-Path "$LauncherDir\Cargo.toml") -or -not (Test-Path "$LauncherDir\crates\horizon-client")) {
    Fail "$LauncherDir is not an elyhorizon-launcher checkout (no Cargo.toml / crates\horizon-client)."
}
Write-Host "Launcher: $LauncherDir"

Step "integrating the port"
if (Test-Path "$LauncherDir\crates\horizon-steam") {
    Write-Host "crates\horizon-steam already exists: the port is in, nothing to apply."
} else {
    if ($PatchPath) {
        if (-not (Test-Path $PatchPath)) { Fail "patch file not found: $PatchPath" }
        $patch = (Resolve-Path $PatchPath).Path
    } else {
        $patch = Join-Path $env:TEMP "e4steam-port.patch"
        Write-Host "Downloading the port patch..."
        try {
            [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
            Invoke-WebRequest -Uri $PatchUrl -OutFile $patch -UseBasicParsing
        } catch {
            Fail "couldn't download the patch: $($_.Exception.Message)"
        }
    }
    # git am needs an identity; supply one only when the machine has none.
    $identity = @()
    if (-not (git config user.email)) {
        $identity = @("-c", "user.name=e4steam installer", "-c", "user.email=e4steam@localhost")
    }
    Write-Host "Applying the port as commits (git am)..."
    git -C $LauncherDir @identity am --3way $patch
    if ($LASTEXITCODE -ne 0) {
        git -C $LauncherDir am --abort 2>$null
        Fail "the patch didn't apply: this checkout isn't the base the port was made on. Clone $LauncherRepo (or pull its main) and try again."
    }
    Write-Host "Applied: horizon-steam crate, client integration, docs (3 commits)."
}

if ($SkipBuild) {
    Step "skipping the build (-SkipBuild)"
    Write-Host "Build later with:"
    Write-Host "  cd `"$LauncherDir`""
    Write-Host "  cargo build --release -p horizon-client --features steam"
} else {
    Step "building the game (cargo release; a few minutes)"
    Push-Location $LauncherDir
    try {
        cargo build --release -p horizon-client --features steam
        if ($LASTEXITCODE -ne 0) {
            Fail "cargo build failed. Rust 1.90+ is required: https://rustup.rs then try again."
        }
    } finally {
        Pop-Location
    }

    Step "placing the Steamworks redistributable"
    # The `steam` feature links steam_api64.dll; the loader wants it beside
    # the game. The steamworks-sys crate ships it inside its own package.
    $dll = Get-ChildItem -ErrorAction SilentlyContinue `
        "$env:USERPROFILE\.cargo\registry\src\*\steamworks-sys-*\lib\steam\redistributable_bin\win64\steam_api64.dll" |
        Select-Object -First 1
    if ($dll) {
        Copy-Item $dll.FullName "$LauncherDir\target\release\steam_api64.dll" -Force
        Write-Host "Copied $($dll.Name) beside target\release\horizon-client.exe."
    } else {
        Write-Host "steam_api64.dll wasn't found in the cargo registry; if the game" -ForegroundColor Yellow
        Write-Host "won't start, copy it from the steamworks-sys crate's" -ForegroundColor Yellow
        Write-Host "lib\steam\redistributable_bin\win64 beside the exe." -ForegroundColor Yellow
    }
}

Step "done"
Write-Host "To play:"
Write-Host "  1. Prepare the game's resources from your own Minecraft (the launcher's Play page, or horizon-prepare)."
Write-Host "  2. Start Steam and sign in."
Write-Host "  3. Run $LauncherDir\target\release\horizon-client.exe"
Write-Host "  4. Open a world, pause, Play with Friends: the Steam section opens the world to Steam friends;"
Write-Host "     guests join from a Steam invitation or the world's steam:<lobby id> address."
Write-Host ""
Write-Host "The port's documentation is docs\steam-friends.md in the launcher checkout."
