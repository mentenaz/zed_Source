<#
.SYNOPSIS
    Builds and runs a local Forge dev build as a separate app
    ("ForgeSuperBuild"), isolated from your regular installed Forge's
    settings/data and Windows single-instance lock.

.DESCRIPTION
    Sets FORGE_DIR_SUFFIX/FORGE_IPC_SUFFIX, which `crates/app_names/build.rs`
    bakes into the generated paths and mutex names, so the build uses its own
    %APPDATA%\ForgeSuperBuild / %LOCALAPPDATA%\ForgeSuperBuild directories
    and its own mutex. No source files are modified; the environment variables
    are restored after the build.

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

$oldDirSuffix = $env:FORGE_DIR_SUFFIX
$oldIpcSuffix = $env:FORGE_IPC_SUFFIX

try {
    $env:FORGE_DIR_SUFFIX = "Build"
    $env:FORGE_IPC_SUFFIX = "-CI"

    Write-Host "Building an isolated Forge dev binary (ForgeSuperBuild)..." -ForegroundColor Cyan

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

    # The fork's `zed` package builds a binary named `forgedev`, not `zed`.
    $binName = if ($Package -eq "zed") { "forgedev" } else { $Package }
    $exePath = "target/$profile/$binName.exe"
    if (-not (Test-Path $exePath)) {
        throw "Build succeeded but $exePath was not found."
    }

    if ($NoRun) {
        Write-Host "Built $exePath (isolated as ForgeSuperBuild). Skipping launch (-NoRun)." -ForegroundColor Green
    } else {
        Write-Host "Launching $exePath (isolated as ForgeSuperBuild)..." -ForegroundColor Cyan
        Start-Process -FilePath $exePath -Wait
    }
}
finally {
    if ($null -eq $oldDirSuffix) { Remove-Item Env:FORGE_DIR_SUFFIX -ErrorAction SilentlyContinue } else { $env:FORGE_DIR_SUFFIX = $oldDirSuffix }
    if ($null -eq $oldIpcSuffix) { Remove-Item Env:FORGE_IPC_SUFFIX -ErrorAction SilentlyContinue } else { $env:FORGE_IPC_SUFFIX = $oldIpcSuffix }
    Pop-Location
}
