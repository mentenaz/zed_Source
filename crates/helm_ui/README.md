# helm_ui

Shared UI components for Helm's dock panel and repository Workspace. This
crate depends on `helm_backend` for GitHub data types and uses `gpui_component`
for the reusable list, row, and screen widgets. Its `Section<T>` and
`Loaded<T>` state types keep independent loading, success, and error state for
lists and single-value screens.
