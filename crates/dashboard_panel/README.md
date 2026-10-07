# dashboard_panel

A workspace tab that summarises the open project across the Node, Python,
.NET and Rust ecosystems, plus git status and live system charts.

## Why it exists

Each runtime panel (`node_panel`, `python_panel`, `dotnet_panel`,
`rust_panel`) only shows
its own ecosystem. The Dashboard is the one place that answers "what is the
state of this whole project?" — which runtimes are installed, how many
packages are outdated or vulnerable, and where the repository stands.

It is a lighter take on the Forge original: instead of porting Forge's
`AppState` and broadcast channels, it holds the already-loaded dock panels as
entities and observes them, so the tab updates whenever they do.

## Using it

Click **Dashboard** in the Cockpit panel's header, or run
`cockpit panel: open dashboard` from the command palette. If a Dashboard tab
is already open in the active pane it is re-activated rather than duplicated.

The tab shows:

- **Runtimes** — detected Node, Python, .NET and Rust versions with project
  (for Rust, crate) counts, and how many dependencies are outdated or
  vulnerable.
- **Security** — one row per finding for each ecosystem (severity, package,
  fixed-in version, advisory link). PyPI advisories expand in place.
  **Scan all** re-runs each panel's own vulnerability scan.
  - The Rust card covers the crate selected in the Rust panel, not the whole
    workspace, and names it. Its scan is on demand, so until one has run the
    card says "Not scanned yet" and the Runtimes row carries a "not scanned"
    tag instead of showing nothing: an unscanned crate must not look clean.
  - Rust findings that are not vulnerabilities (unsound, unmaintained) are
    listed after them and tagged with their kind rather than a severity.
- **Git** — repository name, branch, changed-file count and ahead/behind for
  the active project.
- **System** — the same live charts as the Cockpit panel.

An ecosystem whose dock panel failed to load is shown as "not detected"; it
does not stop the tab from opening.

## How it is wired

- `dashboard_panel::init(cx)` (called from `crates/zed/src/main.rs`)
  registers the tab as a serializable item and installs the handler for
  `cockpit_panel::OpenDashboard` on every workspace.
- `dashboard_panel::open(workspace, window, cx)` is the programmatic entry
  point. It looks the dock panels up with `workspace.panel::<T>(cx)`, so no
  extra plumbing through `initialize_panels` is needed.
- `DashboardPanel` is a `workspace::Item` (a tab), not a dock panel, and
  implements `SerializableItem` so it is restored with the workspace.

### Dependencies on other fork crates

`cockpit_panel`, `node_panel`, `python_panel`, `dotnet_panel`, `rust_panel`
for the data they already hold, and `npm_backend`, `python_backend`,
`dotnet_backend`, `cargo_backend` for the finding types. Git status comes straight from
`project::git_store::Repository`.

## Not included

- Forge's LSP update-server tracking was dropped as unrelated.
- No embedded git panel — only the compact summary above.

## Development

```sh
cargo check -p dashboard_panel -j 8
cargo test -p dashboard_panel -j 8
```

The tests cover how severity strings are bucketed and how advisory labels are
worded.

To run the tests of every fork crate at once: `script/test-fork-crates.ps1`.
