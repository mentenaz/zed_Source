# helm_workspace

The read-only GitHub repository Workspace tab. It uses `helm_backend` for
GitHub requests, `helm_ui` for shared presentation components, and
`gpui_component`'s tree, input, resizable panels, and Markdown rendering.

The tab includes a repository overview and a Code section with a local filename
filter, keyboard-navigable file tree, repository and selected-folder README
rendering, syntax-highlighted text in a read-only app editor, image previews,
binary/LFS/oversized-file states, and exact-commit GitHub permalinks.
Selected file or folder paths are retained across ref changes where possible;
when a file is absent, the nearest available folder is shown. Branch/tag
refresh and update behavior, repository access errors, account/scope
authorization messaging, local checkout status, and the API rate-limit line are
also provided. Files can be opened from a matching local checkout; a commit
mismatch requires confirmation. When no checkout is open, the existing Helm
clone flow can clone the repository and open the selected file.

The Commits section shows paged history for the current branch or tag, and
history for a selected file or folder. Commit detail lists changed files and
shows GitHub's unified patch in a read-only editor; binary or oversized files
without a patch are identified explicitly. Compare shows commits and changed
files between two refs, with the same patch viewer.

The Search section runs repository/ref-scoped GitHub code search when submitted
and opens a result in the read-only editor at its matching snippet line when
available.

`helm_panel` opens the tab from repository details; its
`OpenRepositoryWorkspace` action opens the currently selected repository.

## Why it exists

The Helm dock panel is narrow: good for lists and actions, too small to read
a repository in. The Workspace is the same repository opened as a full tab,
so its code, history and pull requests can be read without cloning it and
without leaving the editor.

It only reads. Nothing in the tab changes anything on GitHub. Editing a file
means opening it from a local clone, which the tab helps you get to.

## Requirements

A signed-in GitHub account in Helm, the same one the panel uses. A private
repository needs the `repo` scope; when it is missing the tab says so and
offers Helm's authorization prompt.

## The pages

| Page | What it shows |
|---|---|
| Overview | Description, default branch, visibility, language, stars, forks, counts of open issues and pull requests, and the last commit. |
| Code | The file tree, a filename filter, and the viewer described above. |
| Commits | History of the ref, or of the selected file or folder, and a commit's changed files and patches. |
| Compare | Commits and changed files between two refs. |
| Search | Code search in this repository. |
| Pull requests, Issues | Open items, drawn with the rows shared in `helm_ui`. An item's comment thread can be read in the tab. |
| Actions | The latest workflow run for the branch in view, which opens in the workflow-run tab. |

A repository that GitHub has disabled shows the Overview only.

The header above the pages has the repository name, the branch and tag
dropdown, the Private / Archived / Fork tags, who is signed in, whether a
local clone is open and how it compares to the commit in view, Refresh, and
Open on GitHub.

## Behaviour worth knowing

- **The ref is pinned to a commit.** A branch is resolved to a commit when it
  is chosen, and everything on screen is that commit. When the branch moves
  on GitHub, the tab notices the next time it comes to the front and offers
  Update; it never changes under you.
- **One tab per repository.** Opening a repository that already has a tab in
  the active pane brings that tab forward. Two repositories in two tabs share
  no state.
- **Not restored on restart.** A tab is opened from the panel and is gone
  when the app closes. This is deliberate: restoring would refetch
  everything at startup.
- **Search has its own, smaller allowance.** It is sent on `enter` or the
  Search button, never per keystroke, and the allowance is shown.
- **A file is fetched once.** Text is remembered by its Git blob, so going
  back to a file already seen sends nothing.
- **Pull requests, Issues and the context of a branch are unfinished.** They
  list open items only, with no Open / Closed / All switch and no context
  pane. That is phase W7 of the plan, which is on hold.

## How it is wired

`open_workspace_tab(repo, gh_state, workspace, window, cx)` is the one way
in. `helm_panel` calls it from the repository screen's button and from its
`OpenRepositoryWorkspace` action.

The tab does not depend on the panel. Where it needs something only the
panel can do, it dispatches an action that the panel handles:

| Action | What the panel does |
|---|---|
| `CloneRepositoryForWorkspace` | Opens Helm's clone dialog for the tab's repository. |
| `AuthorizeRepositoryWorkspace` | Opens Helm's scope-authorization prompt. |
| `OpenWorkspaceWorkflowRun` | Opens the workflow-run tab for the tab's latest run. |

## Layout

```text
src/lib.rs   what the crate exports
src/tab.rs   WorkspaceTab: its state, every page, and the tests
```

`tab.rs` holds the whole tab and is large. Search for the
`WorkspaceSection` variant of the page you need rather than reading top to
bottom. Splitting it per page is listed with W7.

The plan, with what each phase built and what is left, is
`../helm_panel/Helm_Workspace_Plan.md`.

## Development

```sh
cargo check -p helm_workspace -j 8
cargo test -p helm_workspace -j 8
```

The tests cover exact-commit permalinks and their encoding, the line a code
search result opens on, rejecting unsafe local paths, choosing the matching
local clone, the filename filter, and the fall back to the nearest folder
when a path is missing. Nothing in them talks to GitHub.

To run the tests of every fork crate at once: `script/test-fork-crates.ps1`.

This crate is licensed `Apache-2.0`, like the other fork crates. It links
Zed's `workspace` and `editor` crates, which are `GPL-3.0-or-later`, so the
built application as a whole is still distributed under the GPL.
