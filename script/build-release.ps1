<#
.SYNOPSIS
    Builds `zeddev` in release mode — the plain, no-signing, no-installer
    equivalent of `script/bundle-windows.ps1`.

.DESCRIPTION
    Just `cargo build -p zed --bin zeddev --release`, with the same `-j 8`
    default as `run-isolated.ps1` (see that script's own doc comment for why:
    this machine has far more logical cores than free RAM can support
    building in parallel). No NSIS/Inno Setup installer, no code signing, no
    external SDK downloads — for when you want a release-mode binary to test
    or hand off without going through the full CI-oriented bundling pipeline.

.PARAMETER Jobs
    Passed through to `cargo build -j`. Defaults to 8.

.PARAMETER Run
    Launch the resulting `zeddev.exe` after a successful build.

.EXAMPLE
    ./script/build-release.ps1

.EXAMPLE
    ./script/build-release.ps1 -Jobs 12 -Run
#>
[CmdletBinding()]
Param(
    [int]$Jobs = 8,
    [switch]$Run
)

$ErrorActionPreference = "Stop"

$repoRoot = & git rev-parse --show-toplevel
if ($LASTEXITCODE -ne 0) {
    throw "Not inside a git repository."
}
Push-Location $repoRoot

try {
    $cargoArgs = @("build", "-p", "zed", "--bin", "zeddev", "--release", "-j", $Jobs)
    Write-Host "Building: cargo $($cargoArgs -join ' ')" -ForegroundColor Cyan
    & cargo @cargoArgs
    if ($LASTEXITCODE -ne 0) {
        throw "cargo build failed (exit code $LASTEXITCODE)."
    }

    $exePath = "target/release/zeddev.exe"
    if (-not (Test-Path $exePath)) {
        throw "Build succeeded but $exePath was not found."
    }

    Write-Host "Built $exePath" -ForegroundColor Green

    if ($Run) {
        Write-Host "Launching $exePath..." -ForegroundColor Cyan
        Start-Process -FilePath $exePath
    }
}
finally {
    Pop-Location
}
