<#
.SYNOPSIS
    Shows who a repository's commits and pushes are made as, for checking
    the Helm regression matrix (crates/helm_panel/REGRESSION_MATRIX.md).

.DESCRIPTION
    For the git repository at -Path (default: the current folder) prints:

      - the remote it pushes to (origin) and whether it is HTTPS or SSH;
      - the commit identity: the repository's own user.name/user.email
        (what Helm writes), and the global ones it would otherwise fall
        back to;
      - the last commit's author and committer;
      - the GitHub account that made the latest push to the current
        branch, from GitHub's record of the repository's activity. That is
        the account whose credential git actually used.

    The push check uses the GitHub CLI (`gh`) with whichever account is
    active in it; pass -GhAccount to read the activity as a specific
    account (needed for a private repository the active account cannot
    see). It never changes anything: no git config, no gh account switch
    that outlives the script.

.PARAMETER Path
    The repository folder. Defaults to the current folder.

.PARAMETER GhAccount
    A gh account login to read GitHub's activity with, for this run only.

.EXAMPLE
    ./script/helm-identity-check.ps1 -Path H:\ForgeDemo\DifferentAccount_Git
#>
param(
    [string]$Path = (Get-Location).Path,
    [string]$GhAccount
)

$ErrorActionPreference = 'Stop'

function Get-RepoValue([string[]]$GitArgs) {
    $output = & git -C $Path @GitArgs 2>$null
    if ($LASTEXITCODE -ne 0) { return $null }
    return ($output | Out-String).Trim()
}

if (-not (Get-RepoValue @('rev-parse', '--is-inside-work-tree'))) {
    Write-Error "$Path is not a git repository."
}

$remote = Get-RepoValue @('remote', 'get-url', 'origin')
$branch = Get-RepoValue @('branch', '--show-current')
$kind = if (-not $remote) { 'none' } elseif ($remote -match '^https://') { 'HTTPS' } else { 'SSH' }

Write-Host "Repository   $Path"
Write-Host "Remote       $remote ($kind)"
Write-Host "Branch       $branch"
Write-Host ''
Write-Host 'Commit identity'
Write-Host "  repository user.name   $(Get-RepoValue @('config', '--local', 'user.name'))"
Write-Host "  repository user.email  $(Get-RepoValue @('config', '--local', 'user.email'))"
Write-Host "  global user.name       $(Get-RepoValue @('config', '--global', 'user.name'))"
Write-Host "  global user.email      $(Get-RepoValue @('config', '--global', 'user.email'))"
Write-Host ''
Write-Host 'Last commit'
Write-Host "  author     $(Get-RepoValue @('log', '-1', '--format=%an <%ae>'))"
Write-Host "  committer  $(Get-RepoValue @('log', '-1', '--format=%cn <%ce>'))"
Write-Host "  sha        $(Get-RepoValue @('log', '-1', '--format=%h %s'))"
Write-Host ''

if ($remote -notmatch 'github\.com[:/](?<owner>[^/]+)/(?<name>[^/]+?)(\.git)?$') {
    Write-Host 'Pushed by    (not a github.com remote)'
    return
}
$repository = "$($Matches.owner)/$($Matches.name)"
if (-not (Get-Command gh -ErrorAction SilentlyContinue)) {
    Write-Host 'Pushed by    (install the GitHub CLI to check this)'
    return
}

$previous = $null
if ($GhAccount) {
    $previous = (& gh api user --jq .login 2>$null)
    & gh auth switch --user $GhAccount 2>$null | Out-Null
}
try {
    # Every push GitHub recorded for the branch (a normal push, a force
    # push, the push that created it), newest first, with the account that
    # made it: the account whose credential git used.
    $activity = & gh api "repos/$repository/activity?per_page=1&ref=refs/heads/$branch" 2>$null
    if ($LASTEXITCODE -ne 0) {
        Write-Host "Pushed by    (gh's account cannot read $repository; try -GhAccount)"
    } else {
        $latest = @($activity | ConvertFrom-Json) | Select-Object -First 1
        if ($latest) {
            Write-Host "Pushed by    @$($latest.actor.login) ($($latest.activity_type) at $($latest.timestamp), latest on $branch)"
        } else {
            Write-Host "Pushed by    (no push to $branch recorded on GitHub yet)"
        }
    }
} finally {
    if ($previous -and $previous -ne $GhAccount) {
        & gh auth switch --user $previous 2>$null | Out-Null
    }
}
