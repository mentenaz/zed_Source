# Helm regression matrix

The release gate for anything that touches Helm's accounts, git identity or
credentials: before tagging a `dev-vN` that changes them, run the rows
below in a fresh `script/run-isolated.ps1` build and fill in the results
table. Each row checks the three things that have gone wrong before:

- **Author** and **committer** of the commit: whose name and email it
  carries.
- **Pushed by**: the GitHub account whose credential git actually used.
- **No surprise**: no sign-in window, no console window, no fallback to
  another account.

`script/helm-identity-check.ps1 -Path <repo>` prints all three for a
repository: the repository's and the global commit identity, the last
commit's author and committer, and who made the latest push to the
current branch (from GitHub's activity record, through `gh`; add
`-GhAccount <login>` for a private repository the active `gh` account
cannot see). It changes nothing.

## Setup

| Name | What | Notes |
| --- | --- | --- |
| **A** | `mentenaz`, signed in with Helm's own sign-in | |
| **B** | `FrancoisHuyzers-Sencorp`, imported from the GitHub CLI | Exercises the gh-import path |
| Global git identity | A's (`mentenaz@outlook.de`) | Deliberately A's, so any fallback to it shows up as a wrong author in B's rows |
| `gh` active account | Whichever is *not* the row's account | So a fallback to `gh` shows up as a wrong pusher |
| **R-A** | A's own repository over HTTPS (`mentenaz/zed_Source` will do) | |
| **R-B** | B's own private repository over HTTPS (`DifferentAccount_Test`) | |
| **R-fork** | A's fork with `upstream` set to the original | |
| **R-ssh** | Any repository with an SSH `origin` | |
| **R-org** | A repository in `Sencorp-Interim` | Organisation that has not approved Helm |
| **Empty** | An empty folder, no `.git` | For "Using Helm" |

## Rows

| # | Account | Repository | Do | Expect |
| --- | --- | --- | --- | --- |
| 1 | B | Empty | Git panel, "Using Helm", create a private repository | "Publishing…", then "Created … and pushed"; no sign-in or console window. Author and committer: B's saved identity, or B's noreply address; never A's email. Pushed by @B |
| 2 | B | R-B | Commit and push from the git panel | Author and committer B. Pushed by @B |
| 3 | A | R-A | Commit and push from the git panel | Author and committer A. Pushed by @A |
| 4 | B | R-B | Commit and `git push` in a Zed terminal | Author and committer B. Pushed by @B; no prompt |
| 5 | A → B | R-B | Switch the workspace from A to B in the account menu, then push from the git panel and from an already-open terminal | Push held back briefly while B loads, then pushed by @B from both |
| 6 | A | R-B | Try to push (A has no access) | Push button shown disabled with "A can't push to …"; toast offers "Switch to B" |
| 7 | B | R-B | "Sign out of B", then: commit, push from the git panel, `git push` in a terminal; then "Sign in again" | Banner "B (@…) is signed out". Commit works. Panel push refused with "Sign in again"; terminal push fails without asking A's or gh's login. After signing in, the refused push runs by itself, pushed by @B |
| 8 | B | R-B | "Sign out of B and revoke…" | Signed out; the GitHub page with the GitHub CLI entry opens; toast warns revoking it signs gh out |
| 9 | A | R-fork | Push a branch from the git panel | Goes to `origin` (A's fork), pushed by @A; `upstream` untouched |
| 10 | A | R-ssh | Commit and push | Author A (Helm's identity); push uses the SSH key, Helm stays out of it |
| 11 | A | R-A | With "Keep my email private" and "Block command line pushes that expose my email" on, commit with A's real email and push | Toast explains GH007 and offers "Use noreply address"; after it and `git commit --amend --reset-author`, the push goes through |
| 12 | A and B | R-B in two windows | Open R-B in window 1 as A and window 2 as B; push from window 1; close window 2; push again | Toast in window 2 about the other window's account. First push by @B (window 2 set it up last); after closing window 2, pushed by @A |
| 13 | B, then none | R-B | Close the window, reopen R-B with Helm signed out of B | No push as B from the old window's settings: refused with "Sign in again" |
| 14 | A | R-org | Open its Workspace tab, list its repositories, try an edit | "The Sencorp-Interim organization hasn't approved Helm…" with "Request access" |
| 15 | A | R-A | Disconnect the network, push from the git panel | "git push failed: GitHub could not be reached" with Retry; Retry pushes once back online |
| 16 | B | (B's private repo) | Clone it from Helm, commit, push | No prompt during the clone. Author B, pushed by @B |
| 17 | A | R-A | Workspace tab: search a branch beyond the first 100, open it, close and reopen the tab | Found by typing part of its name; the tab reopens on it |

Account renames (accounts keyed by numeric id) cannot be exercised
without renaming a real GitHub account; they are covered by unit tests.

## Results

Copy for each release and fill in. Paste the script's "Last commit" and
"Pushed by" lines where they disagree with the expectation.

| # | Author / committer | Pushed by | No surprise | Pass |
| --- | --- | --- | --- | --- |
| 1 | | | | |
| 2 | | | | |
| 3 | | | | |
| 4 | | | | |
| 5 | | | | |
| 6 | | | | |
| 7 | | | | |
| 8 | | | | |
| 9 | | | | |
| 10 | | | | |
| 11 | | | | |
| 12 | | | | |
| 13 | | | | |
| 14 | | | | |
| 15 | | | | |
| 16 | | | | |
| 17 | | | | |
