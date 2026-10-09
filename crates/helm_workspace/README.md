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
