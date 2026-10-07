# helm_backend

The GitHub side of Helm: signing in through the `gh` CLI, calling the REST
API, and cloning repositories. No UI.

## Why it exists

It is the part of [`helm_panel`](../helm_panel/README.md) that talks to
GitHub, moved into its own crate so that it can be tested without a window,
and so that the planned Helm Workspace can use it too. It plays the role
`npm_backend`, `dotnet_backend` and `cargo_backend` play for their panels.

This is phase C of `crates/helm_panel/Helm_Refactor_Plan.md`.

## Requirements

The [GitHub CLI](https://cli.github.com/) (`gh`) must be installed and on
PATH. This crate uses it to sign in and to get the access token; it does not
handle credentials itself.

## Trying it

```sh
cargo run -p helm_backend --example whoami -j 8
```

It uses the `gh` CLI's existing sign-in and only reads:

```text
signed in as mentenaz
1 organisation(s)
58 repositories in all
zed_Source: 10 commits on page 1 of 4040
a missing repository: GitHub API 404: Not Found (permission-like: true)
```

## Using it

Everything is under `helm_backend::github`. A call has three parts, each in
its own file:

| Part | Where | Network |
| --- | --- | --- |
| What to ask for: method, path, body | `requests.rs` | no |
| Sending it | `send` in `api.rs` | yes, the only place |
| What the answer means | `error.rs` | no |

HTTP goes through the host application's client, handed in when the state is
created, so requests use the same proxy settings as the rest of the app:

```rust
let gh_state = Arc::new(GhState::new(cx.http_client()));
let repos = on_tokio(async move { gh_get_repos("self".into(), &gh_state).await }).await?;
```

The functions are `async` and still need a tokio runtime, because the `gh`
child processes do; a GPUI host awaits them through
`helm_backend::on_tokio`.

`GhState` also holds the cached token, the API base URL, and two broadcast
channels that the streaming operations (sign-in, clone) report progress on.

| Area | Functions |
| --- | --- |
| Sign-in | `gh_check_cli`, `gh_auth_status`, `gh_login`, `gh_ensure_scope`, `gh_logout` |
| Account | `gh_get_current_user`, `gh_get_user`, `gh_update_user`, `gh_get_org_logins`, `gh_get_org_detail` |
| Repositories | `gh_get_repos`, `gh_get_repo`, `gh_create_repo`, `gh_update_repo`, `gh_update_topics`, `gh_get_branches`, `gh_list_tags`, `gh_list_recent_commits` |
| Collaboration | `gh_list_issues`, `gh_list_pulls`, `gh_list_issue_comments`, `gh_create_pull`, `gh_get_collaborators`, `gh_add_collaborator`, `gh_remove_collaborator` |
| Invitations | `gh_get_repo_invitations`, `gh_list_org_invitations`, and accept and decline for each |
| Releases and packages | `gh_list_releases`, `gh_create_release`, `gh_list_packages`, `gh_list_package_versions` |
| Actions and deployments | `gh_list_workflow_runs`, `gh_get_workflow_run`, `gh_get_workflow_run_jobs`, `gh_list_deployments` |
| Insights | the four `gh_get_traffic_*` functions, `gh_list_dependabot_alerts`, `gh_list_secret_scanning_alerts` |
| Cloning | `gh_clone_repo` |

## Errors

Every API function returns `Result<_, GhError>`. The variants say what kind
of failure it was, so a caller can decide what to do instead of reading the
message:

| Variant | Meaning |
| --- | --- |
| `Unauthorized`, `Forbidden`, `NotFound` | 401, 403, 404. `is_permission()` is true for these: they are what a missing token scope looks like |
| `RateLimited` | 403 or 429 with the limit used up, with the reset time or the wait GitHub asked for. Not a permission error, although it arrives as a 403 |
| `Validation` | 422, with GitHub's per-field reasons in the message |
| `Status` | Any other status of 400 or above |
| `Network` | No answer: no connection, timeout, DNS |
| `Parse` | The answer was not the JSON expected |
| `Cli` | `gh` could not be run, or would not hand over a token |
| `Other` | Something wrong before GitHub was asked |

`GhError` implements `Display`, and converts into a `String`, for code that
only shows the message. The sign-in functions (`gh_login` and the rest of
`cli.rs`) still return `Result<_, String>`.

`interpret` and `interpret_empty` are the whole rule for turning an HTTP
answer into a value or an error. They take a plain `RawResponse`, which is
what lets them be tested without a network.

## Behaviour worth knowing

- **Lists come a page at a time.** `fetch_page(state, request, page,
  per_page)` returns a `Page` with its items, its number and the last page
  there is, read from GitHub's `Link` header. `fetch_page_under` does the
  same for a list GitHub wraps in an object, and `fetch_all` joins every
  page (up to 50) for a list that has to be complete to be useful.
  `gh_get_repos` uses `fetch_all`. The other `gh_list_*` functions still
  return a single page of up to 100, for callers that have not moved to
  `fetch_page`.
- **Nothing is cached** except the token. The rate-limit headers are read
  only to recognise a used-up limit; nothing tracks how much is left. Both
  are phase E.
- **A request that returns nothing is not parsed.** GitHub answers most
  deletes and some updates with 204 and an empty body. These used to be
  read as JSON, which an empty body is not.
- **Names are percent-encoded** wherever they go into a path or a query, so
  a name containing `/`, `?` or `#` cannot change which endpoint is called.
  A container package named `owner/image` is sent with `%2F`.
- **Requests time out after 30 seconds** and follow up to 10 redirects
  (GitHub redirects a renamed or moved repository).
- **Sign-in and clone report progress on channels.** `gh_login` and
  `gh_clone_repo` send each line of `gh`'s output to `GhState::auth_tx` and
  `clone_tx`. A host subscribes before starting them.

## Layout

| File | Contents |
| --- | --- |
| `src/helm_backend.rs` | `on_tokio`, and the event error a host matches on |
| `src/github/mod.rs` | What the crate exports, and `gh_cmd` |
| `src/github/cli.rs` | `gh auth`: check, status, login, scopes, logout |
| `src/github/requests.rs` | Each endpoint's request as data: method, path, body |
| `src/github/api.rs` | The endpoint functions, and `send` |
| `src/github/paging.rs` | `Page`, and reading the `Link` header |
| `src/github/error.rs` | `GhError`, and turning an HTTP answer into a value or an error |
| `src/github/clone.rs` | `gh repo clone` with streamed output |
| `src/github/types.rs` | Response types and `GhState` |
| `examples/whoami.rs` | Asks GitHub who you are (read-only, uses the network) |

## Status

Phase C is done:

- The code is in its own crate, and `helm_panel` no longer depends on
  `reqwest` or tokio directly.
- Typed errors.
- Every request is built as data, and every answer is interpreted by a pure
  function; both are unit-tested.
- HTTP goes through the host's client. `reqwest` is no longer a dependency.

Still on tokio: running `gh` (sign-in, the token, cloning) and the two
progress channels. Moving those off it would mean rewriting the sign-in
flow, which cannot be tested without signing in by hand, so it was left.

Pagination (phase D) is done too. Next in the plan: rate limits and caching
(phase E).

## Development

```sh
cargo check -p helm_backend -j 8
cargo test -p helm_backend -j 8
```

The tests cover every endpoint's request (method, path and body, and that a
name cannot change the endpoint), and every kind of answer: each status to
its error, telling a used-up rate limit from a missing scope, the
rate-limit message, GitHub's field errors, empty answers, and the lists
GitHub wraps in an object. None of them uses the network; `send` is the one
function they do not reach, and the `whoami` example exercises it.

To run the tests of every fork crate at once: `script/test-fork-crates.ps1`.
