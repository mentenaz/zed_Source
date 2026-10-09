# helm_ui

Shared UI components for Helm's dock panel and repository Workspace. This
crate depends on `helm_backend` for GitHub data types and uses `gpui_component`
for the reusable list, row, and screen widgets. Its `Section<T>` and
`Loaded<T>` state types keep independent loading, success, and error state for
lists and single-value screens.

## Why it exists

Helm has two places that show GitHub data: the dock panel (`helm_panel`) and
the repository Workspace tab (`helm_workspace`). Both list issues, pull
requests, commits and workflow runs, and both need to show "loading",
"failed, retry" and "nothing here" the same way. This crate holds that once,
so a pull request row is drawn by the same code wherever it appears.

Nothing here asks GitHub for anything by itself, and nothing here is a
screen. A view in one of the two crates above owns the data and decides what
to load; this crate keeps the state of each load and draws the result.

## What is in it

| File | What it gives you |
|---|---|
| `lib.rs` | `HelmView`, the one trait a view implements to use the rest: it hands over the GitHub connection and the repository in view, if any. |
| `section.rs` | `Section<T>` for a list and `Loaded<T>` for a single value. Each keeps its own rows or value, load state, error text and, for a list, the page it is on. `HelmViewExt` adds the loaders (`load_value`, `load_section`, `load_section_page`, `load_repo_page`) that run a request and store its answer. |
| `list_view.rs` | `ListView`, a list on `gpui_component`'s `List`, and `HelmListViewExt::list_screen`, which draws a whole list screen: a header, then the spinner, the error with Retry, the empty line, or the rows, with a pager for a paged list. |
| `rows.rs` | One function per kind of row: `issue_row`, `pull_row`, `branch_row`, `commit_row`, `workflow_run_row`, `tag_row`, `release_row`. Each returns a `gpui_component` `ListItem` with the name on the left and the details on the right. |
| `widgets.rs` | Small shared pieces: `loading_screen`, `note_screen`, `failed_screen`, `chip`, and the formatters `fmt_num`, `short_date` and `repo_vis_label`. |

## Using it

A view that wants a list screen does four things:

1. Implements `HelmView`.
2. Holds a `Section<T>` for the rows and a `ListView` built with
   `ListView::new` (one flat list) or `ListView::sectioned` (several headed
   groups). The `ListView` is given where the rows are, which function draws
   one, and what a click or `enter` does.
3. Loads the rows with one of the `HelmViewExt` loaders.
4. Renders with `list_screen`, passing the section's `status()` or
   `paged_status(...)`, the labels for the three states, and what Retry does.

`helm_panel/src/lists.rs` has one such list per screen and is the place to
copy from.

## Behaviour worth knowing

- **Two loads never share state.** Each `Section` and `Loaded` has its own
  spinner and error, so one failing list does not blank another.
- **A late answer is dropped.** Every load is numbered. An answer that
  arrives after a newer load began, or after the screen was left, is ignored.
- **A remembered page is shown at once.** `load_section_page` shows the last
  known rows while GitHub is asked whether they are still right; an unchanged
  answer costs nothing against the rate limit.
- **Rows must be the same height.** `gpui_component`'s `List` draws only the
  rows in view and needs that to do so.
- **A row does not handle its own click or selection.** The list does, which
  is what gives every screen the same `up`, `down` and `enter`.

## What belongs here

Something moves here when both the panel and the Workspace draw it. A screen
only one of them has stays in that crate. Three shared pieces are still in
the panel and are due to move with phase W7 of
`../helm_panel/Helm_Workspace_Plan.md`, which is on hold: the issue and pull
request detail with its comment thread, the Open / Closed / All switch, and
the workflow-run tab.

## Development

```sh
cargo check -p helm_ui -j 8
cargo test -p helm_ui -j 8
```

The tests cover the load states of `Section` and `Loaded`: success, failure
and retry, paging, a remembered page being checked, and only the latest load
counting. Nothing in them talks to GitHub.

To run the tests of every fork crate at once: `script/test-fork-crates.ps1`.

This crate is licensed `Apache-2.0`, like the other fork crates.
