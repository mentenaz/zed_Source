<#
.SYNOPSIS
    Installs a locally built `zeddev.exe` for this user only — PATH entry +
    Explorer right-click "Open with Zed Dev" — without Inno Setup, code
    signing, or admin rights.

.DESCRIPTION
    The real installer (`script/bundle-windows.ps1` + `zed.iss`) does far
    more than this (Start Menu shortcuts, file associations, an actual
    uninstaller entry in "Apps & Features", the modern Windows 11 top-level
    context menu via `explorer_command_injector`'s signed AppX package).
    This script only reproduces the two pieces of that which don't need any
    of that machinery — both are just per-user (`HKCU`) registry writes in
    `zed.iss` itself:
      - `HKCU\Environment\Path` gets the install `bin` dir appended.
      - `HKCU\Software\Classes\*\shell\...` /
        `HKCU\Software\Classes\directory\shell\...` /
        `HKCU\Software\Classes\directory\background\shell\...` /
        `HKCU\Software\Classes\Drive\shell\...` get a classic Explorer
        context-menu entry.
    On Windows 11 this lands under "Show more options" rather than the
    top-level menu — that top-level placement is what the signed AppX buys
    you, which needs the code-signing secrets `-p` covered earlier and isn't
    reproduced here.

    Broadcasts `WM_SETTINGCHANGE` after touching PATH so new terminals pick
    it up without a logoff — existing terminals still won't see it until
    restarted (Windows has no way to update an already-running process's
    inherited environment).

.PARAMETER ExePath
    The built `zeddev.exe` to install. Defaults to `target/release/zeddev.exe`
    (build it first with `script/build-release.ps1` if it doesn't exist yet).

.PARAMETER InstallDir
    Where to copy the exe. Defaults to `%LOCALAPPDATA%\ZedDev\bin`.

.PARAMETER Uninstall
    Removes everything this script added — the PATH entry, the context-menu
    registry keys, and the installed exe/directory — instead of installing.

.EXAMPLE
    ./script/install-local.ps1

.EXAMPLE
    ./script/install-local.ps1 -Uninstall
#>
[CmdletBinding()]
Param(
    [string]$ExePath = "target/release/zeddev.exe",
    [string]$InstallDir = "$env:LOCALAPPDATA\ZedDev\bin",
    [switch]$Uninstall
)

$ErrorActionPreference = "Stop"

# Matches `RegValueName`/`ShellNameShort` for the "dev" channel in zed.iss —
# kept distinct from "Zed"/"ZedPreview"/"ZedNightly" so this never collides
# with a real installed Zed's own context-menu/PATH entries.
$RegValueName = "ZedDevLocal"
$ShellLabel = "Open with Zed Dev"
$InstalledExe = Join-Path $InstallDir "zeddev.exe"

function Broadcast-EnvironmentChange {
    if (-not ("Win32.NativeMethods" -as [type])) {
        Add-Type -Namespace Win32 -Name NativeMethods -MemberDefinition @"
            [DllImport("user32.dll", SetLastError = true, CharSet = CharSet.Auto)]
            public static extern IntPtr SendMessageTimeout(
                IntPtr hWnd, uint Msg, UIntPtr wParam, string lParam,
                uint fuFlags, uint uTimeout, out UIntPtr lpdwResult);
"@
    }
    $HWND_BROADCAST = [IntPtr]0xffff
    $WM_SETTINGCHANGE = 0x1a
    $result = [UIntPtr]::Zero
    [Win32.NativeMethods]::SendMessageTimeout(
        $HWND_BROADCAST, $WM_SETTINGCHANGE, [UIntPtr]::Zero, "Environment",
        2, 5000, [ref]$result) | Out-Null
}

function Add-ToUserPath([string]$dir) {
    $current = [Environment]::GetEnvironmentVariable("Path", "User")
    $parts = @()
    if ($current) { $parts = $current -split ";" | Where-Object { $_ -ne "" } }
    if ($parts -contains $dir) {
        Write-Host "Already on PATH: $dir" -ForegroundColor DarkGray
        return
    }
    $new = if ($current -and $current.TrimEnd(";").Length -gt 0) { "$current;$dir" } else { $dir }
    [Environment]::SetEnvironmentVariable("Path", $new, "User")
    Write-Host "Added to user PATH: $dir" -ForegroundColor Green
}

function Remove-FromUserPath([string]$dir) {
    $current = [Environment]::GetEnvironmentVariable("Path", "User")
    if (-not $current) { return }
    $parts = $current -split ";" | Where-Object { $_ -ne "" -and $_ -ne $dir }
    [Environment]::SetEnvironmentVariable("Path", ($parts -join ";"), "User")
    Write-Host "Removed from user PATH: $dir" -ForegroundColor Green
}

# One (root, keyPath) pair per context-menu surface — mirrors zed.iss's
# addcontextmenufiles/addcontextmenufolders `[Registry]` entries, minus the
# `IsWindows11OrLater` split (that only decides *where* the entry surfaces
# in Explorer's menu, not whether these classic keys work at all).
$ContextMenuRoots = @(
    "Software\Classes\*\shell\$RegValueName",
    "Software\Classes\directory\shell\$RegValueName",
    "Software\Classes\directory\background\shell\$RegValueName",
    "Software\Classes\Drive\shell\$RegValueName"
)

function Install-ContextMenu {
    foreach ($keyPath in $ContextMenuRoots) {
        $fullKey = "HKCU:\$keyPath"
        New-Item -Path $fullKey -Force | Out-Null
        Set-ItemProperty -Path $fullKey -Name "(Default)" -Value $ShellLabel
        Set-ItemProperty -Path $fullKey -Name "Icon" -Value """$InstalledExe"""

        $commandKey = "$fullKey\command"
        New-Item -Path $commandKey -Force | Out-Null
        # `*`/Drive/background entries get the clicked item as %1; the two
        # directory-background entries (no file/folder was clicked, just the
        # empty space inside one) get the current folder as %V instead.
        $arg = if ($keyPath -like "*background*") { "%V" } else { "%1" }
        Set-ItemProperty -Path $commandKey -Name "(Default)" -Value """$InstalledExe"" ""$arg"""
    }
    Write-Host "Added '$ShellLabel' to the Explorer context menu." -ForegroundColor Green
}

function Uninstall-ContextMenu {
    foreach ($keyPath in $ContextMenuRoots) {
        $fullKey = "HKCU:\$keyPath"
        if (Test-Path $fullKey) {
            Remove-Item -Path $fullKey -Recurse -Force
        }
    }
    Write-Host "Removed context-menu entries." -ForegroundColor Green
}

if ($Uninstall) {
    Uninstall-ContextMenu
    Remove-FromUserPath $InstallDir
    if (Test-Path $InstallDir) {
        Remove-Item -Path $InstallDir -Recurse -Force
        Write-Host "Removed $InstallDir" -ForegroundColor Green
    }
    Broadcast-EnvironmentChange
    Write-Host "Uninstalled." -ForegroundColor Green
    exit 0
}

if (-not (Test-Path $ExePath)) {
    throw "$ExePath not found — build it first, e.g. ./script/build-release.ps1"
}

New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
Copy-Item -Path $ExePath -Destination $InstalledExe -Force
Write-Host "Installed to $InstalledExe" -ForegroundColor Green

Add-ToUserPath $InstallDir
Install-ContextMenu
Broadcast-EnvironmentChange

Write-Host ""
Write-Host "Done. Open a new terminal to pick up the PATH change — 'zeddev' will launch it." -ForegroundColor Cyan
Write-Host "Right-click a file/folder in Explorer -> 'Show more options' -> '$ShellLabel'." -ForegroundColor Cyan
