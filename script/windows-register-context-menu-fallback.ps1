# Manually registers the classic Explorer "Open with Mentenaz Forge Super"
# context-menu verbs (folders, folder backgrounds, drives, and files).
#
# TEMPORARY STOPGAP: the real fix is `zed.iss`'s `AddAppxPackage()`, which
# now does this automatically whenever `Add-AppxPackage` fails to install
# `zed_explorer_command_injector.appx` — which it always will until this
# project has a real Trusted Signing certificate (our trial expired; see
# crates/explorer_command_injector/AppxManifest.xml's `Publisher` comment).
# That fix only takes effect on the *next* installer build, though, so this
# script exists to apply the same fallback by hand to an install that
# predates it, or to any install where the appx still isn't signed.
#
# Delete this script once a certificate is issued and a signed installer
# build has shipped — at that point `Add-AppxPackage` will succeed and
# `zed.iss` will register the modern context-menu extension instead, same
# as it always intended to.

$ForgeExe = "$env:LOCALAPPDATA\Programs\Mentenaz Forge Super\ForgeSuper.exe"

if (-not (Test-Path $ForgeExe)) {
    throw "Forge Super executable not found: $ForgeExe"
}

# Right-click a file (any type). `*` is a literal key name here (Windows'
# own catch-all for "all file types"), not a wildcard. `New-Item` doesn't
# wildcard-resolve its creation path (confirmed: a plain unescaped `*`
# creates exactly that one key, no `-LiteralPath` parameter exists on it
# anyway), but `Set-Item`/`New-ItemProperty` *do* wildcard-match existing
# items against `-Path`, which is what made this take 15+ minutes — every
# key under `Software\Classes` had to be enumerated to resolve the match.
# `-LiteralPath` on those skips the enumeration entirely.
$FileKey = "HKCU:\Software\Classes\*\shell\ForgeSuperOpen"
New-Item -Path "$FileKey\command" -Force | Out-Null
Set-Item -LiteralPath $FileKey -Value "Open with Mentenaz Forge Super"
New-ItemProperty -LiteralPath $FileKey -Name "Icon" -Value $ForgeExe -PropertyType String -Force | Out-Null
Set-Item -LiteralPath "$FileKey\command" -Value "`"$ForgeExe`" `"%1`""

# Right-click a folder
$FolderKey = "HKCU:\Software\Classes\Directory\shell\ForgeSuperOpen"
New-Item -Path "$FolderKey\command" -Force | Out-Null
Set-Item -Path $FolderKey -Value "Open with Mentenaz Forge Super"
New-ItemProperty -Path $FolderKey -Name "Icon" -Value $ForgeExe -PropertyType String -Force | Out-Null
Set-Item -Path "$FolderKey\command" -Value "`"$ForgeExe`" `"%V`""

# Right-click the background inside a folder
$BackgroundKey = "HKCU:\Software\Classes\Directory\Background\shell\ForgeSuperOpen"
New-Item -Path "$BackgroundKey\command" -Force | Out-Null
Set-Item -Path $BackgroundKey -Value "Open with Mentenaz Forge Super"
New-ItemProperty -Path $BackgroundKey -Name "Icon" -Value $ForgeExe -PropertyType String -Force | Out-Null
Set-Item -Path "$BackgroundKey\command" -Value "`"$ForgeExe`" `"%V`""

# Right-click a drive
$DriveKey = "HKCU:\Software\Classes\Drive\shell\ForgeSuperOpen"
New-Item -Path "$DriveKey\command" -Force | Out-Null
Set-Item -Path $DriveKey -Value "Open with Mentenaz Forge Super"
New-ItemProperty -Path $DriveKey -Name "Icon" -Value $ForgeExe -PropertyType String -Force | Out-Null
Set-Item -Path "$DriveKey\command" -Value "`"$ForgeExe`" `"%V`""

Write-Host "Forge Super context menus registered successfully." -ForegroundColor Green
