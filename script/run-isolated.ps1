<#
.SYNOPSIS
    Builds and runs a local Zed dev build as a separate app ("ZedDevBuild"),
    isolated from your regular installed Zed's settings/data and Windows
    single-instance lock.

.DESCRIPTION
    Temporarily patches two files (APP_NAME in crates/paths/src/paths.rs,
    and the single-instance mutex name in
    crates/zed/src/zed/windows_only_instance.rs) so the build uses its own
    %APPDATA%\ZedDevBuild / %LOCALAPPDATA%\ZedDevBuild directories and its
    own mutex, then builds, runs, and reverts the patch — even if the build
    or run fails — via a try/finally, so the repo is always left clean
    afterward. Mirrors .github/workflows/build_windows_dev.yml's "isolate"
    step, for local use.

    Aborts up front if either target file already has uncommitted changes,
    so this script never discards real work sitting in those files.

.PARAMETER Jobs
    Passed through to `cargo build -j`. Defaults to 8 — see the reasoning
    in CLAUDE.md/session notes: this machine has far more logical cores
    than free RAM can support building in parallel; 8 is a safe default,
    raise it only if you know you have the headroom.

.PARAMETER Release
    Build in release mode instead of debug.

.PARAMETER Package
    Cargo package to build. Defaults to "zed".

.PARAMETER NoRun
    Build only; don't launch the resulting .exe.

.EXAMPLE
    ./script/run-isolated.ps1

.EXAMPLE
    ./script/run-isolated.ps1 -Jobs 12 -Release
#>
[CmdletBinding()]
Param(
    [int]$Jobs = 8,
    [switch]$Release,
    [string]$Package = "zed",
    [switch]$NoRun
)

$ErrorActionPreference = "Stop"

$repoRoot = & git rev-parse --show-toplevel
if ($LASTEXITCODE -ne 0) {
    throw "Not inside a git repository."
}
Push-Location $repoRoot

$pathsFile = "crates/paths/src/paths.rs"
$instanceFile = "crates/zed/src/zed/windows_only_instance.rs"

function Assert-Clean([string]$file) {
    $diff = & git diff --quiet -- $file
    if ($LASTEXITCODE -ne 0) {
        throw "$file has uncommitted changes — commit or stash them first. " +
              "This script reverts the file via 'git checkout' when it's done, " +
              "which would discard those changes."
    }
}

try {
    Assert-Clean $pathsFile
    Assert-Clean $instanceFile

    Write-Host "Patching APP_NAME and instance mutex for an isolated build..." -ForegroundColor Cyan

    (Get-Content $pathsFile -Raw) `
        -replace 'pub const APP_NAME: &str = "Zed";', 'pub const APP_NAME: &str = "ZedDevBuild";' |
        Set-Content $pathsFile -NoNewline

    (Get-Content $instanceFile -Raw) `
        -replace '"\{\}-Instance-Mutex"', '"{}-CI-Instance-Mutex"' |
        Set-Content $instanceFile -NoNewline

    $profile = if ($Release) { "release" } else { "debug" }
    $cargoArgs = @("build", "-p", $Package, "-j", $Jobs)
    if ($Release) {
        $cargoArgs += "--release"
    }

    Write-Host "Building: cargo $($cargoArgs -join ' ')" -ForegroundColor Cyan
    & cargo @cargoArgs
    if ($LASTEXITCODE -ne 0) {
        throw "cargo build failed (exit code $LASTEXITCODE)."
    }

    $exePath = "target/$profile/$Package.exe"
    if (-not (Test-Path $exePath)) {
        throw "Build succeeded but $exePath was not found."
    }

    if ($NoRun) {
        Write-Host "Built $exePath (isolated as ZedDevBuild). Skipping launch (-NoRun)." -ForegroundColor Green
    } else {
        Write-Host "Launching $exePath (isolated as ZedDevBuild)..." -ForegroundColor Cyan
        Start-Process -FilePath $exePath -Wait
    }
}
finally {
    Write-Host "Reverting isolation patch..." -ForegroundColor Cyan
    & git checkout -- $pathsFile $instanceFile
    Pop-Location
}
