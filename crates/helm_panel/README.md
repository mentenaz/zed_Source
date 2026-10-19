# helm_panel

"Helm" — a left-dock GitHub client for browsing and managing repositories,
organisations, issues, pull requests, releases and Actions runs.

## Why it exists

To handle everyday GitHub chores — find a repository, clone it, check an
Actions run, accept an invitation — without switching to the browser. Ported
from Forge's GitHub panel, with the UI rebuilt on `gpui_component` widgets in
place of the old `forge_ui` crate.

## Requirements

A GitHub account, and `git` on PATH for cloning. The GitHub CLI is not
needed: Helm signs in through its own OAuth App ("Helm for Forge") and keeps
each account's token in the system credential store (Windows Credential
Manager on Windows), under `helm://github.com/<login>`. Tokens are never
written to `settings.json`.

## Using it

Open with `ctrl-k h` (`cmd-k h` on macOS), the status bar icon (tooltip
"Helm"), or `helm panel: toggle focus`.

### First run

1. **Auth** — **Login with GitHub** shows a one-time code and a link; enter
   the code on GitHub and Helm signs in. **Use a token instead** takes a
   personal access token (a classic one needs `repo`, `read:org` and
   `user`; a fine-grained one limits Helm to the repositories you choose).
   Helm checks that the token has the `repo` scope and prompts for it if
   missing.
2. **Menu** — your profile summary and the main sections.

An organisation that restricts third-party apps does not show up, and its
private repositories cannot be seen, until its owners approve Helm. The
sign-in screen links to Helm's page in your GitHub settings, where you can
request that.

### Accounts

You can sign in to several GitHub accounts. The account button next to
"Helm" in the panel header switches between them without signing out, adds
an account, makes one the default, signs out of one (its token is deleted;
it stays listed) or removes it.

Each workspace remembers which account it uses; a workspace that never
chose one uses the default account. The list of accounts is
`helm_panel.accounts` in settings, and the default is
`helm_panel.default_account`; the tokens are only in the credential store,
so an account listed on a machine without its token shows "Sign in as …".

### What you can do

From the menu: **Repositories**, **Search GitHub**, **Organizations**,
**Invitations**, **Edit Profile**, **Account Security**, **Create Repository**
and **Logout**. Search GitHub searches repositories across GitHub and opens a
result in the existing repository detail view.

Inside a repository:

| Section | Shows |
| --- | --- |
| Branches | Branch list |
| Collaborators | Add or remove collaborators |
| Issues | Issue list and detail with comments |
| Pull requests | List, detail and create |
| Releases | List and create |
| Packages | Packages; opening one shows its versions above the list |
| Commits | Recent commits |
| Actions | Workflow runs; a run opens as its own tab with jobs drawn as a graph |
| Deployments | Deployment list |
| Tags | Tag list |
| Traffic | Views, clones, referrers and popular paths |
| Security | Dependabot and secret-scanning alerts |

Repository settings and topics can be edited, and other users' profiles
viewed.

### Editing a repository

The settings icon on a repository ("Edit repository") opens a dialog for its name, description, homepage,
topics, and switches for Issues, Projects, Wiki and Discussions.

The **Private** switch changes the repository's visibility. It starts at the
repository's current setting, and a warning line appears under it as soon as
you flip it, saying what saving will do. Visibility is only sent to GitHub
when the switch was actually changed, so saving other edits never touches it.

Making a repository public exposes its code and history to everyone, and
GitHub may refuse the change depending on organisation policy or your plan.
A refusal is handled as described below.

### When GitHub rejects a change

Every change Helm sends — creating or editing a repository, pull requests,
releases, collaborators, invitations, your profile — goes through one path.
If GitHub rejects one with a permission error, Helm checks the scopes GitHub
reported for the token to see whether it has the scope that action needs:

| Action | Scope |
| --- | --- |
| Repositories, pull requests, releases, collaborators, repo invitations | `repo` |
| Edit profile | `user` |
| Organisation invitations | `write:org` |

- **Scope missing:** Helm switches to the auth screen with an
  "Authorize <scope> scope" prompt. The button signs in again for the same
  account, asking for that scope on top of the ones it has, and shows the
  code and link. If the browser signs in a different account, Helm stores
  nothing and offers to try again or add that account instead. Once you
  have authorized, Helm returns you to the screen you were on and sends the
  change again. It retries once; a second rejection is reported as a normal
  failure.
- **Scope present, or not reported:** you get a "Failed to …"
  notification. Your account lacks rights on that resource, or the
  organisation blocks the change, and re-authorizing cannot fix that.
  Fine-grained tokens do not report scopes, so for them every rejection is
  reported this way.
- **No longer signed in:** you get the login prompt, and the change is sent
  again after you sign in.

### Cloning

**Clone** runs `git clone` as the account the window uses and streams its
output to a progress screen, then opens the clone as a new workspace. The
token is handed to `git` for that one command and is not stored in the
clone. **Cancel** stops the clone and removes the half-cloned folder.

Helm does not install dependencies itself: when the new workspace has a
`package.json` and no `node_modules`, it offers `npm install`. Pushing and
pulling in the clone later use git's own credential helper (Git Credential
Manager, or `gh` if you set that up), not Helm's token.

### Pages

Lists show ten rows at a time, with **Back** and **Next** arrows and "Page 2
of 7" under the list when there is more than one page.

- Issues, pull requests, branches, collaborators, releases, tags, commits,
  Actions runs and deployments ask GitHub for one page at a time.
- Repositories are fetched in full, because the search box filters them, and
  are then shown ten at a time. Typing in the search box goes back to the
  first page of what matches.
- On Issues a page can show fewer than ten rows while more pages follow.
  GitHub counts pull requests as issues in its pages; Helm leaves them out.
- Packages, security alerts, invitations and organisations are not paged
  and still show at most the first 100.

### Keyboard

Click a list to focus it, then:

| Key | Action |
| --- | --- |
| `up` / `down` | Move the selection |
| `enter` | Open the selected row, where rows open something |

A click on a row does the same as `enter`. On read-only lists (branches,
commits, deployments, tags, security alerts) neither does anything.

Invitations are accepted and declined only with a row's **Accept** and
decline buttons, never by `enter` or a click on the row, so one cannot be
accepted by accident.

## How it is wired

- `helm_panel::init(cx)` registers `helm_panel::ToggleFocus` and the key
  bindings for the profile menu (context `HelmRowList`). The list screens
  get theirs from `gpui_component`'s `List` (context `List`).
- `HelmPanel::load(workspace, cx)` is awaited in `initialize_panels`
  (`crates/zed/src/zed.rs`).
- Implements `workspace::dock::Panel`, docked left by default (the
  `helm_panel.dock` setting moves it left or right; `helm_panel.button` hides
  its status bar button), `activation_priority() = 21`.

### Actions

In the `helm_panel` namespace: `ToggleFocus`, `SelectNextRow`,
`SelectPrevRow`, `OpenSelectedRow`, `OpenRepositoryWorkspace`.

## Layout

Every screen's code is an `impl HelmPanel` block in its own file. The struct
itself, with all of its fields, stays in `src/helm_panel.rs`. See
`Helm_Refactor_Plan.md` for why it is laid out this way and what comes next.

| File | Contents |
| --- | --- |
| `src/helm_panel.rs` | `init`, the actions, the `HelmPanel` struct, `load`, and its `Panel` and `Render` impls |
| `src/section.rs` | `Section<T>`: one list's rows, load state and last error, and the loaders that fill one |
| `src/list_view.rs` | The shared list screen, on `gpui_component`'s `List` |
| `src/lists.rs` | Each list screen's widget: where its rows are, how a row is drawn, what `enter` does |
| `src/loading.rs` | The shared loader for screens that show one thing, not a list |
| `src/state.rs` | Which screen is showing, load state, menu rows |
| `src/navigation.rs` | Switching screens, back and forward, the navigation bar |
| `src/auth.rs` | Sign-in (device flow or pasted token), scope authorization, sign-out, and the auth screen |
| `src/accounts.rs` | Tokens in the credential store, the account list in settings, and the event that tells every panel when an account signs in or out |
| `src/account_switcher.rs` | The account menu, switching account, and each workspace's remembered account |
| `src/changes.rs` | `HelmAction` and `run_action`: the one path every change to GitHub goes through |
| `src/profile.rs` | Your profile and menu, the identity header, other users' profiles, edit profile |
| `src/orgs.rs` | Organisation list and detail |
| `src/repos.rs` | Repository list and detail, create and edit dialogs |
| `src/clone.rs` | The clone dialog, progress, and opening the result |
| `src/branches.rs` | Branches |
| `src/collaborators.rs` | Collaborators and their dialogs |
| `src/invitations.rs` | Repository and organisation invitations |
| `src/issues.rs` | Issues, plus the state filter and comment thread shared with pull requests |
| `src/pulls.rs` | Pull requests and creating one |
| `src/releases_packages.rs` | Releases, packages and tags |
| `src/activity.rs` | Commits, Actions runs and deployments |
| `src/insights.rs` | Traffic and security alerts |
| `src/repository_modal.rs` | The modal used for Helm's forms and confirmations |
| `src/workflow_run_tab.rs` | A workflow run as its own tab, with its jobs as a graph |
| `src/widgets.rs` | Small shared helpers |

The GitHub code itself (the device flow, the REST endpoints, cloning, the
response types) is in [`helm_backend`](../helm_backend/README.md).

### How requests work

The panel calls [`helm_backend`](../helm_backend/README.md) for everything
that touches GitHub. That crate runs on tokio, so UI code goes through
`helm_backend::on_tokio` rather than awaiting it directly on GPUI's
executor. In practice a screen never does that by hand: list screens load
through `load_section` (`section.rs`) and the others through `load_with`
(`loading.rs`).

A clone broadcasts each output line through a channel on `GhState`, which
the progress screen subscribes to.

Switching account gives the panel a new `GhState`. Every loader checks,
when its answer arrives, that the panel still has the state it started with
(`HelmView::is_current_state`), and drops the answer otherwise, so one
account's data never appears under another's.

To add a list screen: add the endpoint and its response type in
`helm_backend`, a `Section` field on the panel, a loader, a row function, an
entry in `lists.rs`, and a render function that calls `list_screen`.

## Things to know

- `src/helm_panel.rs` is large (several thousand lines) and holds every
  screen. Search for the `HelmScreen` variant you need rather than reading
  top to bottom.
- This crate is licensed `Apache-2.0`, like the other fork panels. It links
  Zed's `workspace` and `ui` crates, which are `GPL-3.0-or-later`, so the
  built application as a whole is still distributed under the GPL.

## Development

```sh
cargo check -p helm_panel -j 8
cargo test -p helm_panel -j 8
```

The tests cover the decisions behind the re-authorization flow (which errors
count as permission errors, scope matching, which scope each action needs).
Nothing in them talks to GitHub.

To run the tests of every fork crate at once: `script/test-fork-crates.ps1`.
