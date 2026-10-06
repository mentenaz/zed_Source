# helm_panel

"Helm" — a left-dock GitHub client for browsing and managing repositories,
organisations, issues, pull requests, releases and Actions runs.

## Why it exists

To handle everyday GitHub chores — find a repository, clone it, check an
Actions run, accept an invitation — without switching to the browser. Ported
from Forge's GitHub panel, with the UI rebuilt on `gpui_component` widgets in
place of the old `forge_ui` crate.

## Requirements

The [GitHub CLI](https://cli.github.com/) (`gh`) must be installed and on
PATH. Helm uses it for sign-in and for the access token; it does not handle
credentials itself.

## Using it

Open with `ctrl-k h` (`cmd-k h` on macOS), the status bar icon (tooltip
"Helm"), or `helm panel: toggle focus`.

### First run

1. **Gate** — Helm checks that `gh` is available.
2. **Auth** — if you are not signed in, start the `gh` login from here. Helm
   also checks that the token has the `repo` scope and prompts for it if
   missing.
3. **Menu** — your profile summary and the main sections.

### What you can do

From the menu: **Repositories**, **Organizations**, **Invitations**,
**Edit Profile**, **Account Security**, **Create Repository** and **Logout**.

Inside a repository:

| Section | Shows |
| --- | --- |
| Branches | Branch list |
| Collaborators | Add or remove collaborators |
| Issues | Issue list and detail with comments |
| Pull requests | List, detail and create |
| Releases | List and create |
| Packages | Packages and their versions |
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
If GitHub rejects one with a permission error, Helm checks `gh auth status`
to see whether your login has the scope that action needs:

| Action | Scope |
| --- | --- |
| Repositories, pull requests, releases, collaborators, repo invitations | `repo` |
| Edit profile | `user` |
| Organisation invitations | `write:org` |

- **Scope missing:** Helm switches to the auth screen with an
  "Authorize <scope> scope" prompt. The button runs
  `gh auth refresh -s <scope> --hostname github.com` and shows the device
  code and link. Once you have authorized, Helm returns you to the screen
  you were on and sends the change again. It retries once; a second
  rejection is reported as a normal failure.
- **Scope present:** you get a "Failed to …" notification. Your account
  lacks rights on that resource, or the organisation blocks the change, and
  re-authorizing cannot fix that.
- **No longer signed in:** you get the login prompt, and the change is sent
  again after you sign in.

### Cloning

**Clone** runs `gh repo clone` and streams its output to a progress screen.
It can then run `npm install` in the cloned folder as a second phase.

### Keyboard

In any list:

| Key | Action |
| --- | --- |
| `up` / `down` | Move the selection |
| `enter` | Open the selected row |
| `space` | Secondary action — Decline on the Invitations screen (Enter is Accept) |

## How it is wired

- `helm_panel::init(cx)` registers `helm_panel::ToggleFocus` and the list key
  bindings (context `HelmRowList`).
- `HelmPanel::load(workspace, cx)` is awaited in `initialize_panels`
  (`crates/zed/src/zed.rs`).
- Implements `workspace::dock::Panel`, fixed to the left dock,
  `activation_priority() = 5`.

### Actions

In the `helm_panel` namespace: `ToggleFocus`, `SelectNextRow`,
`SelectPrevRow`, `OpenSelectedRow`, `ActSelectedRow`.

## Layout

| File | Contents |
| --- | --- |
| `src/helm_panel.rs` | The panel, every screen and modal, and the workflow-run tab |
| `src/backend.rs` | `on_tokio`, bridging tokio futures onto GPUI's `cx.spawn` |
| `src/backend/github/mod.rs` | Types, `GhState`, the `gh_cmd` helper |
| `src/backend/github/cli.rs` | `gh auth` — check, status, login, scopes, logout |
| `src/backend/github/api.rs` | REST endpoints, all thin wrappers over `gh_api_fetch` |
| `src/backend/github/clone.rs` | `gh repo clone` with streamed output |
| `src/backend/github/types.rs` | Response types |

### How requests work

`gh_api_fetch` resolves the token (cached, or from `gh auth token`) and calls
the GitHub REST API with `reqwest`. The backend is tokio-native, so UI code
goes through `backend::on_tokio` rather than awaiting it directly on GPUI's
executor.

Streaming operations (login, clone) broadcast each output line through
channels on `GhState`, which the progress screens subscribe to.

To add an endpoint: add a wrapper in `api.rs`, a response type in
`types.rs`, and a screen or section in `helm_panel.rs`.

The workflow-run job graph is drawn with
[`gpui_flow`](../gpui_flow/README.md).

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
