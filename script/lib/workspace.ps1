
function ParseZedWorkspace {
    $metadata = cargo metadata --no-deps --offline | ConvertFrom-Json
    $env:ZED_WORKSPACE = $metadata.workspace_root
    $env:RELEASE_VERSION = $metadata.packages | Where-Object { $_.name -eq "zed" } | Select-Object -ExpandProperty version
    # The installed app's version (what winget and Windows compare):
    # RELEASE_VERSION plus the dev-vN build number, e.g. 1.27.0.38.
    # RELEASE_VERSION itself stays the crate version, because other code
    # (such as the remote-server download) asks zed.dev for that version.
    $buildNumber = if ($env:FORGE_BUILD_NUMBER -match '^\d+$') { $env:FORGE_BUILD_NUMBER } else { "0" }
    $env:INSTALLER_VERSION = "$env:RELEASE_VERSION.$buildNumber"
}
