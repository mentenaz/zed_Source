# cargo_manager_panel

The Cargo Manager: a workspace tab for one crate's crates.io dependencies.
It shows what is installed, what is behind the registry and which security
advisories apply, searches crates.io, and adds, removes, updates and
re-versions dependencies through Cargo.

## Why it exists

It is the Rust counterpart to the npm and NuGet managers: a full-width place
to see a crate's dependencies and change them, instead of editing
`Cargo.toml` by hand or running `cargo update` from memory. The Rust panel
in the left dock shows the same lists in brief; this tab is where they are
acted on.

The design is in [`docs/Rust_Manager_Design_Note.md`](../../docs/Rust_Manager_Design_Note.md).
This crate is all five steps of its build order.

## Using it

Open it with the **Package Manager** quick action in the Rust panel, which
opens it on the crate selected there, or with the
`cargo_manager::OpenCargoManager` action from the command palette, which
opens it on the workspace's default crate. To switch crates, select another
one in the Rust panel and press **Package Manager** again.

Five pages in the left sidebar:

| Page | What it does |
| --- | --- |
| General | The crate, its workspace, the toolchain, and the status of the last action |
| Search | Search crates.io; **Add** from the results, **Load more** to page |
| Installed | Direct crates.io dependencies with their locked versions; **Remove** |
| Updates | Dependencies with a newer version; **Update**, **Update all**, **Change to** |
| Vulnerabilities | Advisories from OSV.dev, on demand; **Scan** |

Selecting a dependency or a search result opens a resizable details pane on
the right: how the crate declares it (or, for a search result, its
description and download counts), links to crates.io and docs.rs, the
**README**, and the versions available, each marked with its minimum Rust
version. For a search result every version has its own **Add**.

| Key (list focused) | Action |
| --- | --- |
| `up` / `down` | Move the selection |
| `enter` | Open the dependency's details, or the advisory's page in the browser |
| `space` | Run the page's action: Remove (Installed) or Update (Updates) |

The Search page's results are not keyboard-navigable yet; `enter` in the
search box runs the search.

Commands run in the **Script Runner** panel so the output is visible. When
one finishes, the tab re-reads `Cargo.toml` and `Cargo.lock` from disk.

## Behaviour worth knowing

### Every change is confirmed, with what it will change

- **Update** asks Cargo first (`cargo update --dry-run`) and shows the
  result as the confirmation. Updating one package often moves others:
  `cargo update clap` on this fork changes six lockfile entries. The version
  Cargo settles on can also be lower than the "up to" version shown, because
  another package in the workspace can hold it down. If Cargo would change
  nothing, the tab says so and runs nothing.
- **Remove** says when the root manifest will change too. Removing the last
  crate that inherits a `[workspace.dependencies]` entry makes Cargo delete
  that entry from the root `Cargo.toml`.
- **Add** says what Cargo will write. If the root manifest's
  `[workspace.dependencies]` has the package, that is
  `name.workspace = true`, and no version is passed (with one, Cargo would
  write it into the crate instead). If that table exists without the
  package, Cargo writes a version into the crate's own manifest, which is
  not how such a workspace declares dependencies: the confirmation is a
  warning, with **Add anyway**.
- **Update all** is one `cargo update` naming every dependency that has a
  newer version inside its declared range. It never runs a bare
  `cargo update`, which on this fork would change 621 lockfile entries.

### What it will not do, and why

- **Change the version of a dependency inherited from the workspace.** For a
  dependency written as `name.workspace = true`, the version is in the root
  manifest's `[workspace.dependencies]`, and no Cargo command edits that
  table. `cargo add name@version` would instead write a literal version into
  the crate, taking it off the workspace's shared version without saying so.
  The Updates page shows the newer version with "set in the root Cargo.toml"
  in place of a button. On a workspace like this fork's, that is most rows.
- **Remove a dependency declared under a `cfg(...)` target**, such as
  `[target.'cfg(windows)'.dependencies]`. Cargo needs the expression passed
  as `--target`, and its quotes and spaces are not safe on a shell command
  line. The tab says to edit `Cargo.toml` by hand. Plain target triples
  work.
- **Change the version of a target-specific dependency.** `cargo add` would
  add a second entry to `[dependencies]` rather than change the one under
  the target.
- **Add a dependency the crate already has.** `cargo add` would rewrite the
  existing one; changing a version is the Updates page's job, with its own
  checks. The same package can still be added to the other table (as a
  dev-dependency when it is already a normal one).
- **Add to a target-specific or build table.** Search adds to
  `[dependencies]`, or to `[dev-dependencies]` with the switch on.

### Unknown never looks like healthy

- The Vulnerabilities page starts at "Not scanned yet", and its sidebar
  badge shows no number until a scan has finished. The scan is a button
  because it covers every package reachable from the crate (1,425 for
  `zed`). Any change to the crate or the lockfile puts the page back to
  "Not scanned yet".
- A finished scan is shared with the Rust panel, and the panel's with this
  tab, so scanning in one shows the findings in both. A result is only ever
  reused for the exact set of packages it covered: once the lockfile
  resolves differently, both are back to "Not scanned yet".
- A dependency with no locked version shows "unknown", never a guess.
- An advisory with no severity shows "unknown" in the warning colour. A
  notice (unsound, unmaintained) with no severity shows none, and is listed
  in its own section so that it is not counted as a vulnerability.
- If the workspace's manifests cannot be read, whether a dependency is
  inherited is unknown: version changes are refused, and a removal's
  confirmation says the root manifest may change.

### Other things

- Path and git dependencies are not listed. One line under the Installed
  list says how many were left out, and they are not checked for advisories.
- Yanked versions are never offered in the version list. If the locked
  version has been yanked, the row says so.
- "Compatible only" in the details pane hides versions that declare a
  minimum Rust version above the installed toolchain. Versions that declare
  none are kept and marked.
- Searches are at least a second apart, which is the rate crates.io asks
  API users to keep to. A second search inside that second waits; it is not
  dropped.
- The README is fetched when asked for and shown as crates.io serves it,
  rendered HTML, through `gpui_component::text::html`. Images in it (badges,
  mostly) may not show.
- Nothing depends on rust-analyzer, and nothing polls. The tab reloads when
  a `Cargo.toml` or `Cargo.lock` in the project changes on disk.
- If a crate name or version contains anything but the characters real ones
  use, the command is not run and the reason is shown on the General page.
  See [`script_runner_panel`](../script_runner_panel/README.md#building-commands-safely).

## How it is wired

- `cargo_manager_panel::init(cx)` is called from `crates/zed/src/main.rs`.
  It registers the tab as a serializable workspace item, the two workspace
  actions, and the list key bindings.
- `cargo_manager_panel::open(root, crate_name, workspace, window, cx)` is
  what the Rust panel's quick action calls.
- All Cargo and registry logic is in [`cargo_backend`](../cargo_backend/README.md).
  This crate holds the UI and, in `registry`, the HTTP requests that crate
  leaves to its host. `rust_panel` uses the same `registry` module.

The UI is composed from `gpui_component` widgets: `setting::{Settings,
SettingPage, SettingGroup, SettingItem}` for the pages, `h_resizable` /
`resizable_panel` for the details split, `text::html` for the README, and
`Input`, `Button`, `Tag`, `Switch` and `Spinner` inside them.

## Layout

| File | Contents |
| --- | --- |
| `src/cargo_manager_panel.rs` | The tab: state, loading, the confirm-then-run actions, `Item` and `Render` |
| `src/pages.rs` | The `SettingPage`s, the General page, and the shared list view behind Installed, Updates and Vulnerabilities |
| `src/search.rs` | The Search page, adding a dependency, and fetching a README |
| `src/details.rs` | The details pane: a dependency's or a search result's details, the version list, the README view |
| `src/registry.rs` | Sparse-index lookups, the OSV advisory scan (both shared with `rust_panel`), search and README requests |

## Development

```sh
cargo check -p cargo_manager_panel -j 8
cargo test -p cargo_manager_panel -j 8
```

The tests cover the command lines the tab builds (including that values
with shell syntax are refused and that "Update all" never produces a bare
`cargo update`, and that no version is passed when adding a package the
workspace declares), the text of the confirmations, the outdated rows, the
index cache, how a shared scan is identified, and list navigation. None of them runs Cargo, opens a window or uses
the network.

To try the actions, use a scratch crate or workspace, never this fork: see
the design note's testing section.

To run the tests of every fork crate at once: `script/test-fork-crates.ps1`.
