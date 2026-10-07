# Helm: Where the Crate Stands Against the Roadmap

**Written:** 7 October 2026. **Read against:** `Helm_Future_Developments.md` (phases 0 to 10) and `helm_panel` as of commit `3d03f494f0`.
**Method:** reading the source only. Nothing was run, and no GitHub request was made for this note.

---

## 1. Summary

Helm today is a working **left-dock GitHub client**: sign-in through the `gh` CLI, about 40 REST endpoints, and around 20 screens. In roadmap terms it is a good share of **phase 1 (Helm Core)** and pieces of phases 3 to 6, built directly on a very thin request function.

What it does not have is **phase 0**: the shared data layer the roadmap says everything else should sit on. There is no caching, no pagination, no rate-limit handling and no request sharing. Each screen fetches for itself and throws the result away when you leave.

That is the main finding. The roadmap's own rule is "don't let every GPUI component access GitHub independently", and that is how the crate works now. The Workspace (phase 2) would multiply the number of requests, so the data layer has to come first.

The second finding is structural: the whole UI is **one 7,989-line file**. That will not survive a second experience being added to it.

## 2. What exists

| Area | State |
|---|---|
| Sign-in | `gh auth` through the CLI: check, status, login with device code, logout. The token comes from `gh auth token` and is cached in memory. |
| Scopes | Checked at start (`repo`). A change that GitHub rejects for a missing scope sends the user to authorize that scope and is retried once. This is the most finished part of the crate. |
| Requests | One function, `gh_api_fetch`: token, URL, `reqwest`, JSON. About 40 endpoint functions are thin wrappers over it. REST only. |
| Changes | One path for every write (`HelmAction` and `run_action`): create and edit repository, create pull request and release, collaborators, invitations, profile. |
| Screens | Profile, organisations, repository list and detail, branches, collaborators, issues, pull requests, comments, releases, packages, commits, Actions runs, deployments, tags, traffic, security alerts, user profiles, invitations. |
| Tabs | One: a workflow run opens as its own tab with its jobs drawn as a graph. Everything else is in the dock panel. |
| Cloning | `gh repo clone` with streamed output, then optionally `npm install`. |
| Tests | 8, all in the panel file: scope matching, permission-error detection, list stepping. The backend has none. |

## 3. Phase 0, item by item

| Roadmap item | State | Evidence |
|---|---|---|
| GitHub authentication | **Done** | `backend/github/cli.rs`. Depends on the `gh` CLI being installed. |
| GitHub API client | **Partial** | `gh_api_fetch` works, but builds a new HTTP client for every request and returns errors as plain strings. |
| REST API layer | **Partial** | About 40 endpoints. None for search, file contents, trees, compare, forks, reviews or merging. |
| GraphQL layer | **Missing** | No GraphQL anywhere. |
| Centralized request manager | **Missing** | Every screen calls its endpoint function directly. |
| Caching | **Missing** | Lists are kept in panel fields and replaced on each visit. No ETags, so every revisit costs a full request against the rate limit. |
| Pagination | **Missing** | Every list asks for one page (`per_page=100`, or 50) and stops. The `Link` header is never read. **An account or organisation with more than 100 repositories silently shows only the first 100.** The same applies to issues, pull requests, branches, tags and commits. |
| Rate-limit handling | **Missing** | The rate-limit headers are never read. Hitting the limit shows up as a generic "GitHub API 403". |
| Error handling | **Partial** | Errors are strings. Permission errors are recognised by matching the status in the text; that is how the scope retry works. |
| Permission and capability detection | **Partial** | Token scopes are handled well. The per-repository `permissions` GitHub returns (admin, push, pull) are not used to enable or disable actions, so the roadmap's "unavailable, and why" is not there. |
| API state management | **Missing** | One `load_state` for the whole panel, so two loads at once share one spinner and one error message. |
| Repository, user, organisation models | **Done** | `backend/github/types.rs`, 534 lines. |
| Event model | **Missing** | Two broadcast channels, for sign-in and clone progress only. |

## 4. Later phases, briefly

| Phase | State |
|---|---|
| 1. Helm Core | **Mostly there**: navigation with back and forward, repository list with a filter box, organisation selection, repository overview. **Missing:** global search (the filter only narrows the list already loaded), command palette entries beyond toggling the panel, notifications, an activity feed. |
| 2. Workspace | **Not started.** No file tree, file viewer, branch switching, diffs or history view. Nothing reads repository contents. |
| 3. Git and collaboration | **Partial.** Lists and detail for issues and pull requests, and creating a pull request. **Missing:** creating issues, commenting, reviews, approving, merging, checks, branch creation and deletion, forking. |
| 4. Actions and releases | **Partial.** Runs, a run's jobs as a graph, release list and create, tags. **Missing:** logs, rerun, cancel, triggering a workflow, artifacts, editing or publishing a draft release. |
| 5. Administration | **Partial.** Create and edit repository, collaborators, invitations. **Missing:** teams, members, archive, transfer, branch protection, webhooks, variables and secrets. |
| 6. Security and intelligence | **Started.** Dependabot and secret-scanning alert lists. No health view. |
| 7 to 10 | Not started. |

## 5. Risks to deal with before building on this

1. **Lists are cut off at 100 without saying so.** This is a correctness bug today, not a future concern, and it is the same "unknown must not look like complete" problem the Rust manager was built to avoid.
2. **No rate-limit awareness.** GitHub allows 5,000 requests an hour when signed in. A file tree and a diff view will spend that far faster than the current screens do, and the search API has its own much lower limit (30 a minute).
3. **One file for everything.** `helm_panel.rs` holds the panel, every screen, every dialog and the workflow tab. Adding the Workspace to it is not practical.
4. **The backend cannot be tested without a network.** Request building, parsing and the HTTP call are in one function. The fork's other backends separate "build the request" and "parse the answer" from "make the call", which is what let them reach their test coverage.
5. **A second HTTP stack.** Helm uses `reqwest` on a tokio runtime (`on_tokio`), while the rest of the fork's panels use the app's shared HTTP client through `cx.http_client()`. That is workable, but it is a decision to make on purpose, not inherit.
6. **Sign-in requires the `gh` CLI.** Fine for now. It is the one thing a user must install separately, and it rules out GitHub Enterprise hosts unless `gh` is set up for them.

## 6. Suggested order

Small steps, each usable on its own, in the roadmap's own order.

1. **A `helm_backend` crate with no UI in it.** Move `backend/` out of the panel, and split each call into building the request, making it, and parsing the answer. Typed errors (not found, forbidden with the scope needed, rate limited with the reset time, network). Unit tests on fixtures, no network. *Done when:* the panel works exactly as before on the new crate.
2. **Pagination.** Read the `Link` header; lists load further pages on demand and say how many are loaded. *Done when:* an organisation with more than 100 repositories shows all of them, and a list that is not fully loaded says so.
3. **Rate limits and caching.** Read the rate-limit headers and show what is left; send `If-None-Match` so an unchanged list costs nothing; one place that holds results so two screens asking for the same thing share one request.
4. **Split `helm_panel.rs`** into a module per screen group. No behaviour change. This can be done alongside step 1.
5. **Global repository search** (the missing part of phase 1), on GitHub's search API, paged and rate-limited from the start.
6. **The Workspace tab, read-only first:** repository tree, branch selector, file viewer, opening a file in the editor. Built on `gpui_component` (resizable three-pane layout) as a workspace tab, with the dock panel staying as the navigator.
7. Then the roadmap's phase 3: diffs, commits, pull request review and merge, forking.

Steps 1 to 4 change nothing the user sees except that long lists become complete. That is the price of phase 0, and the roadmap is right that it is the most important part technically.

## 7. Decisions needed

| # | Question | Suggested default |
|---|---|---|
| 1 | Keep signing in through the `gh` CLI, or add a built-in OAuth device flow? | Keep `gh` for now. Revisit when Enterprise hosts or a `gh`-free install matters. |
| 2 | Keep Helm's own `reqwest` and tokio stack, or move to the app's shared HTTP client like the other panels? | Move to the shared client when the backend is split out (step 1), so there is one HTTP stack and the backend needs no runtime. |
| 3 | REST only, or add GraphQL? | REST for steps 1 to 6. GraphQL where it saves many requests, which is mostly the repository overview and pull request detail. |
| 4 | Does the Workspace browse a repository **through the API**, or does it **clone first** and use local files? | Through the API for browsing and reading, so looking at a repository needs no clone. Clone when the user wants to edit, which Helm can already do. |
| 5 | Where do Helm Control and Helm Workspace live? | Dock panel stays as navigation and Control. Workspace is a tab, like the managers. |
| 6 | Should the existing screens be finished (comments, merge, logs) before the Workspace starts? | No. Do the data layer, then search and the read-only Workspace, then return to them as phase 3 and 4. |

## 8. Not checked

- Whether the existing screens all still work against the live API. This was a read of the code, not a run.
- How the panel behaves when the rate limit is actually reached.
- How large the largest account or organisation it is used with is, which decides how urgent step 2 is.
