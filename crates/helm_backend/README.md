# helm_backend

The GitHub side of Helm: signing in through the `gh` CLI, calling the REST
API, and cloning repositories. No UI.

## Why it exists

It is the part of [`helm_panel`](../helm_panel/README.md) that talks to
GitHub, moved into its own crate so that it can be tested without a window,
and so that the planned Helm Workspace can use it too. It plays the role
`npm_backend`, `dotnet_backend` and `cargo_backend` play for their panels.

This is phase C of `crates/helm_panel/Helm_Refactor_Plan.md`, and it is in
progress. See "Status" below for what has and has not been done.

## Requirements

The [GitHub CLI](https://cli.github.com/) (`gh`) must be installed and on
PATH. This crate uses it to sign in and to get the access token; it does not
handle credentials itself.

## Using it

Everything is under `helm_backend::github`. Functions are `async` and need a
tokio runtime, because `reqwest` and the `gh` child processes do. A GPUI host
awaits them through `helm_backend::on_tokio`:

```rust
let gh_state = Arc::new(GhState::default());
let repos = on_tokio(async move { gh_get_repos("self".into(), &gh_state).await }).await?;
```

`GhState` holds the cached token, the API base URL, and two broadcast
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

## Behaviour worth knowing

- **Lists return one page.** Every list asks GitHub for up to 100 items (50
  in a few places) and stops. An account with more than 100 repositories
  gets the first 100. Pagination is phase D of the plan.
- **Nothing is cached** except the token, and the rate-limit headers are not
  read. Both are phase E.
- **Sign-in and clone report progress on channels.** `gh_login` and
  `gh_clone_repo` send each line of `gh`'s output to `GhState::auth_tx` and
  `clone_tx`. A host subscribes before starting them.

## Layout

| File | Contents |
| --- | --- |
| `src/helm_backend.rs` | `on_tokio`, and the event error a host matches on |
| `src/github/mod.rs` | What the crate exports, and `gh_cmd` |
| `src/github/cli.rs` | `gh auth`: check, status, login, scopes, logout |
| `src/github/api.rs` | The REST endpoints, over one request function |
| `src/github/clone.rs` | `gh repo clone` with streamed output |
| `src/github/types.rs` | Response types and `GhState` |

## Status

Done:

- The code is in its own crate, and `helm_panel` no longer depends on
  `reqwest` or tokio directly.

Not done yet, in the order planned:

- Typed errors in place of strings (not found, forbidden, rate limited,
  network), and one HTTP client reused across requests.
- Splitting each call into building the request, sending it, and parsing
  the answer, with unit tests on the first and last.
- The decision on whether to keep `reqwest` and tokio or move to the app's
  shared HTTP client.

## Development

```sh
cargo check -p helm_backend -j 8
cargo test -p helm_backend -j 8
```

There are no tests yet; they come with the split described above. To run
the tests of every fork crate at once: `script/test-fork-crates.ps1`.
