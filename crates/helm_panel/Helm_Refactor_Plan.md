# Helm: Phased Plan for the Foundation and the Big File

**Written:** 7 October 2026. **Status:** phase A is done (7 October 2026): checked by build, tests and a line-for-line comparison, and tried by hand in the running app. Phase B is done and was tried by hand in the app. Phase C is done in code and checked against the live API from a terminal, but not yet tried in the app. Phase D is done in code and checked against the live API, but not yet tried in the app. Phase E is not started.
**Companions:** `Helm_Future_Developments.md` (the roadmap) and `Helm_Phase0_Audit.md` (where the crate stands). This plan covers the roadmap's phase 0 and the restructuring that has to happen before phase 2.

---

## 1. What this plan fixes

Two problems from the audit, in this order:

1. **The UI is one file of 7,989 lines** (`src/helm_panel.rs`): one struct with 88 fields, 17 loaders, 27 render functions, 18 handlers, 13 dialog openers, a 630-line modal and the workflow-run tab.
2. **There is no data layer.** No pagination (lists stop at 100 without saying so), no caching, no rate-limit handling, and every screen calls GitHub by itself.

The file comes first. It is the lower-risk change, it makes every later step smaller to review, and the data-layer work touches all 17 loaders, which is much easier once they are not in the middle of an 8,000-line file.

## 2. Rules for the whole plan

- **A phase that restructures changes no behaviour.** Moving code and changing what it does never go in the same commit. If a move turns up a bug, it is noted and fixed in its own commit.
- **Every phase ends in a working app.** Each one can be released on its own.
- **No file over about 800 lines** once phase A is done. A file that grows past that is split before the next feature goes in.
- **Nothing is deleted as "unused" without checking.** The audit found one unused function (`gh_get_repo`); anything else that looks dead is confirmed first.
- **Each phase is checked two ways:** the crate's tests through `script/test-fork-crates.ps1`, and the manual checklist in section 9 in the running app, because most of this crate is UI that tests do not reach.
- Fork conventions as elsewhere: UI on `gpui_component` widgets, `-j 8`, `run-isolated.ps1` for manual testing, commit only when asked.

## 3. Phase A — Split the file (no behaviour change)

### How

Rust lets one type's methods live in several files. A module can see the private fields of a struct defined in a module above it, so `HelmPanel` stays in the crate's root file and its `impl` blocks move into child modules. No field has to become public and no call site changes. This is what makes the split mechanical.

`src/helm_panel.rs` is the crate's root file (the manifest's `[lib] path`), so its child modules sit beside it in `src/`, as `src/backend.rs` already does.

### Target layout

Line counts are today's, taken from the file's outline, so they are estimates of where each piece lands.

| File | Contents | About |
|---|---|---|
| `src/helm_panel.rs` | `init`, actions, `HelmPanel` struct, `load`, `Panel` and `Render` impls, tests | 600 |
| `src/state.rs` | `HelmScreen`, `LoadState`, `AuthOutcome`, `MenuItem`, `NavRow` | 100 |
| `src/actions.rs` | `HelmAction`, `PendingAction`, scope checks, `run_action`, `action_succeeded`, `action_settled` | 400 |
| `src/auth.rs` | CLI check, sign-in, device code, scope authorization, sign-out, the gate and auth screens | 600 |
| `src/navigation.rs` | `set_screen`, back, forward, home, the nav bar | 200 |
| `src/profile.rs` | profile screen, identity header, menu, user profiles, edit-profile dialog | 650 |
| `src/orgs.rs` | organisation list and detail, loaders | 300 |
| `src/repos.rs` | repository list and detail, create and edit dialogs | 800 |
| `src/clone.rs` | clone dialog, target path, progress, open-in-workspace | 250 |
| `src/issues_pulls.rs` | issues, pull requests, their detail screens, comment thread, create-pull dialog | 800 |
| `src/releases_packages.rs` | releases, packages, tags, create-release dialog | 600 |
| `src/activity.rs` | commits, Actions runs, deployments | 350 |
| `src/insights.rs` | traffic, security alerts | 450 |
| `src/people.rs` | branches, collaborators and their dialogs, invitations | 900 |
| `src/repository_modal.rs` | `HelmRepositoryModal` | 630 |
| `src/workflow_run_tab.rs` | `WorkflowRunItem`, job nodes, status colours | 350 |
| `src/widgets.rs` | `labeled_field`, `step_selected`, `fmt_num`, `short_date`, `hex`, `repo_vis_label` | 150 |

`people.rs` is the one over budget; it splits into `branches.rs`, `collaborators.rs` and `invitations.rs` if it does not shrink in phase B.

### Steps

One commit per row above, smallest and most self-contained first: `widgets`, `state`, `workflow_run_tab`, `repository_modal`, then the screen groups, then `actions`, `auth` and `navigation` last because everything else calls into them. After each: build, tests, and the part of the checklist that covers what moved.

### As done (7 October 2026)

20 files, in 19 commits, one per module. `src/helm_panel.rs` went from 7,989 lines to 675; the largest file is `src/repos.rs` at 799.

Where it differs from the table above:

- `actions.rs` is named **`changes.rs`**, to keep clear of GPUI's `actions!` macro and of GitHub Actions.
- `issues_pulls.rs` came to 935 lines, so it is two files: **`issues.rs`** (which also holds the state filter and comment thread both use) and **`pulls.rs`**.
- `people.rs` is three files, as this section said it might be: **`branches.rs`**, **`collaborators.rs`** and **`invitations.rs`**.
- Moved methods and items are `pub(super)`. A method written in a child module is private to that module unless marked, so this was needed for the screens to call each other. It widens nothing outside the crate.

Checked for the phase as a whole against the commit before it: no line of the old file was lost, and the only lines added are the 19 `mod` declarations, 6 re-exports, and each new file's `use super::*;` and `impl HelmPanel { }` wrapper. The 8 tests pass after every commit.

Found while moving, and left as it was: a doc comment about repository visibility ("Mirrors the old TS `visLabel`") sits on `step_selected` in `widgets.rs` instead of on `repo_vis_label`. It was misplaced in the original file.

### Done when

- `helm_panel.rs` is under 800 lines and no other file is over about 800.
- `git diff --stat` for the whole phase shows moves only; no function body changed.
- The 8 existing tests pass, and the full checklist in section 9 passes in the app.

## 4. Phase B — Take the repetition out

Phase A moves the lines; this is the phase that removes them.

**Loaders.** The 17 `load_*` functions are the same 30 lines with a different call and a different field: set loading, clone state, spawn, await on tokio, store the list or the error, notify. One generic helper that takes "what to fetch" and "where to store it" replaces them. Estimate: about 550 lines become about 150.

**List screens.** Most of the 27 render functions draw the same thing: a header with a filter and a refresh button, then loading, error, empty, or rows with keyboard selection. One shared list view, given a row renderer, replaces that scaffolding in each. This is also where the screens move further onto `gpui_component` widgets.

**State.** The 88 fields are grouped into one small struct per section (`IssuesState`, `ReleasesState`, and so on) held by the panel, so a screen's data, filter, cursor and load state sit together.

**Load state per section.** Today there is one `load_state` and one `error_msg` for the whole panel, so two loads in flight share one spinner and one error. Each section gets its own. This is the one deliberate behaviour change in the phase, and it is a fix.

### Progress (7 October 2026)

- **Loaders: done.** `src/loading.rs` has `load_with` and `load_for_repo`; fifteen loaders use them (456 lines became 173).
- **State and load state per section: done for every list.** `src/section.rs` adds `Section<T>`: rows, load state, last error, cursor and focus handle together. Thirteen lists are on it: issues, pull requests, releases, packages, tags, branches, collaborators, commits, workflow runs, deployments, the two security alert lists, repositories, invitations and organisations. Each replaced three loose fields on `HelmPanel`, and the ones that load from GitHub no longer share the panel-wide spinner and error.
- **Still on the panel-wide state, on purpose:** sign-in, and the three screens that show one thing, not a list (organisation detail, a user's profile, traffic).
- **List screens: done.** `src/list_view.rs` has the shared screen (`list_screen`) on `gpui_component`'s `List`, and `src/lists.rs` builds each screen's list. All thirteen lists use it. A screen is now its header, its labels and a function that draws one row.
- **What that changed for the user:**
  - Keyboard selection and scrolling are the widget's, the same on every screen, and only rows in view are drawn.
  - Headers stay while a list loads or after a failure, and a failed load shows GitHub's reason.
  - The widget draws every row of a list at one height, so three rows were redesigned: a release's and a package's row are always two lines, and a package's versions open in a block above the list in place of unfolding inside its row.
  - Security and Invitations are each one list with two headed sections. An empty section is left out, so Security's header states both counts.
  - Invitations are accepted and declined only with their buttons. Enter used to accept and Space to decline; the widget treats a click on a row as Enter, so keeping that would let a stray click accept one.
- **Line count:** the crate is at 7,773 lines, against 7,989 at the start, and that now includes about 300 lines of new tests. The plan's estimate of about 5,000 was too optimistic: each screen keeps its own row-drawing code, which is most of its bulk. What changed is the shape. The largest file is about 720 lines, and loading, list scaffolding and keyboard handling each exist once.
- **Checked by:** build and the crate's tests (16) after every commit. Tags, issues and the first eight lists were tried by hand in the app; the last five (organisations, security, repositories, collaborators, invitations, packages) have **not** been yet.

### Done when

- No section file is over about 500 lines, and the crate is noticeably smaller than phase A left it. The target is about 5,000 lines of UI in total, down from 7,989; that number is an estimate until the first two screens have been converted.
- Starting two loads at once shows two independent states.
- New unit tests cover the generic loader's state transitions.

## 5. Phase C — A backend crate with no UI

Move `src/backend/` into a new `helm_backend` crate, in the role `npm_backend`, `dotnet_backend` and `cargo_backend` play for their panels.

- **Split every call three ways:** build the request (method, path, body), make it, parse the answer. The first and last are pure functions with tests on fixture JSON; only the middle touches the network.
- **Typed errors** in place of strings: not found, forbidden (with the scope it needs when that is the cause), rate limited (with the reset time), validation, network, unexpected answer. The panel's scope-retry logic then matches on a variant instead of searching the message text for "403".
- **One HTTP client**, created once, not one per request as now.
- **Decision 2 from the audit is made here:** stay on `reqwest` and tokio, or move to the app's shared HTTP client. See section 10.

### Progress (7 October 2026)

- **The crate exists.** `crates/helm_backend` holds sign-in, the REST endpoints, cloning and the response types. `helm_panel` no longer depends on `reqwest`, `reqwest_client` or tokio.
- **Typed errors: done.** `GhError` replaces strings in every API function, and the panel's scope retry asks `error.is_permission()` in place of matching on "GitHub API 403". The request function is split into `send` (the only network code) and `interpret` (pure, with nine tests).
- **One HTTP client: done.** It lives on `GhState`.
- **Found and fixed on the way:** a used-up rate limit arrives as a 403 and used to send the user to re-authorize; requests answered with an empty 204 were parsed as JSON.
- **Requests as data: done.** `requests.rs` builds each of the forty endpoints' requests without sending them, and percent-encodes every name placed in a path or query. 23 tests in the crate, none using the network.
- **HTTP stack (decision 3): switched.** `send` goes through the app's shared HTTP client, handed in as `GhState::http`; `reqwest` is no longer a dependency of either Helm crate. Checked against the live API with the read-only `whoami` example: sign-in token, a list, and a 404 arriving as `NotFound`.
- **Left on tokio, deliberately:** running `gh` (sign-in, token, cloning) and the two progress channels. Rewriting the sign-in flow cannot be tested without signing in by hand.
- **Not yet tried in the running app.**

### Done when

- `helm_panel` has no `reqwest` or `tokio` types in it and the app behaves as before.
- `helm_backend` has unit tests for every request builder and every parser, none of which uses the network.
- The new crate has a README, the `LICENSE-APACHE` link, and an entry in `script/test-fork-crates.ps1`.

## 6. Phase D — Pagination

- The backend reads GitHub's `Link` header and returns a page together with "is there another".
- Lists load the first page, then more on demand (a "Load more" row, as the Cargo Manager's search does), and always say how many are loaded when the list is not complete.
- The repository list, which is filtered locally, either loads every page before filtering or says that the filter only covers what is loaded.

### As done (7 October 2026)

- **Backend:** `ApiRequest::page`, `paging.rs` (reads the `Link` header), and `fetch_page`, `fetch_page_under` and `fetch_all`. Checked against the live API: ten commits of `zed_Source` came back as page 1 of 4040.
- **Pages of ten, with Back and Next** (the size and the buttons were the user's choice, in place of the "Load more" row this section first proposed). `list_screen` shows `gpui_component`'s `Pagination` and "Page 2 of 7" under any list with more than one page.
- **Paged by GitHub, one request a page:** issues, pull requests, branches, collaborators, releases, tags, commits, Actions runs, deployments.
- **Repositories** are fetched in full (decision 5: the search box needs them all) and paged by the panel. A new search starts at its first page.
- **Not paged:** packages (several requests merged), security alerts (Dependabot pages by cursor, not by number), invitations and organisations. These still show at most the first 100.
- **Known oddity:** an Issues page can hold fewer than ten rows, because GitHub counts pull requests in its issue pages and Helm leaves them out.
- **Not yet tried in the running app.**

### Done when

- An organisation with more than 100 repositories shows all of them.
- No list ever shows a cut-off set as if it were the whole thing.
- Tests cover parsing the `Link` header, including the last page and a missing header.

## 7. Phase E — Rate limits, caching and one place for results

- **Rate limits.** Read the rate-limit headers on every answer. Show what is left somewhere quiet (the panel footer). When the limit is reached, say so with the time it resets, and do not retry until then.
- **Conditional requests.** Keep each answer's `ETag` and send `If-None-Match` on the next request. An unchanged list then costs nothing against the limit.
- **A store.** One object owns fetched results, keyed by what was asked. Two screens asking for the same thing share one request, and a screen revisited inside a short window shows the stored result at once and refreshes in the background.
- **After a change,** the store drops what that change makes stale, so the list the user returns to is current. The single write path (`HelmAction`) is the one place this hooks in.

### Done when

- Going back and forth between two screens makes no new full requests when nothing changed.
- A rate-limit answer shows the reset time, not "GitHub API 403".
- Tests cover header parsing, the store's keying and expiry, and what each action invalidates.

## 8. After this plan

With phases A to E done, the roadmap's phase 0 is in place and phase 1 has one gap left. The next pieces, each needing its own short design note:

1. **Global repository search**, on GitHub's search API, paged and rate-limited from the start.
2. **The Workspace tab, read-only:** repository tree, branch selector, file viewer, open in the editor.
3. **Roadmap phase 3:** diffs, reviews, merging, forking.

## 9. Manual checklist

Run after every phase, in the app started with `./script/run-isolated.ps1`. It covers what the tests cannot.

| # | Do | Expect |
|---|---|---|
| 1 | Open Helm signed out | The gate, then the sign-in prompt with a device code |
| 2 | Sign in | The profile screen with your account and avatar |
| 3 | Open Repositories, type in the filter | The list narrows; `up`/`down`/`enter` work |
| 4 | Open a repository | Detail with every section listed |
| 5 | Open each section in turn | Each loads, or shows an empty or error state, not a blank |
| 6 | Open an issue and a pull request | Detail with comments |
| 7 | Open an Actions run | Its own tab, jobs drawn as a graph |
| 8 | Back, forward, home | Each lands where expected |
| 9 | Edit a scratch repository's description | Saved, and shown on return |
| 10 | Try a change your token lacks the scope for | The authorize prompt, then the change is sent again |
| 11 | Clone a small repository | Progress streams; the folder exists afterwards |
| 12 | Open Organizations and Invitations | Lists load |
| 13 | Sign out | Back to the sign-in prompt |

Items 9 to 11 change real things on GitHub or on disk: use a scratch repository.

## 10. Decisions

Needed before the phase named. Suggested defaults are from the audit.

| # | Needed by | Question | Suggested default |
|---|---|---|---|
| 1 | A | Is the target layout in section 3 the right grouping? | Yes; it follows the screens as the README already describes them. |
| 2 | B | Shared list view: one in this crate, or reuse one from `gpui_component`? | Look at `gpui_component`'s list and table widgets first; write one only if they do not fit. |
| 3 | C | Stay on `reqwest` and tokio, or move to the app's shared HTTP client? | Move. One HTTP stack in the fork, and the backend needs no runtime of its own. The cost is rewriting the request function and the clone stream. |
| 4 | C | Keep signing in through the `gh` CLI? | Yes for now. |
| 5 | D | Load every page up front, or on demand? | On demand, except the repository list, where the local filter needs them all. |
| 6 | E | How long is a stored result shown before a background refresh? | 60 seconds, with conditional requests making the refresh free when nothing changed. |

## 11. Size of the work

Rough, and only that: nothing here has been tried yet.

| Phase | Size | Risk |
|---|---|---|
| A. Split the file | Medium, mechanical | Low: moves only, but it touches every line once |
| B. Remove repetition | Large | Medium: every screen is rewritten onto shared pieces |
| C. Backend crate | Medium to large | Medium: the HTTP decision decides how much |
| D. Pagination | Small to medium | Low |
| E. Limits and caching | Medium | Medium: invalidation is the part that goes wrong |

A and C do not depend on each other and could run in either order. B should follow A. D and E need C.
