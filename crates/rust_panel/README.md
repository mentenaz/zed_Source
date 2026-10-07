# rust_panel

A left-dock panel for working with Rust projects: toolchain versions, the
workspace's crates, and for the selected crate its dependencies, what is
behind the registry, and the security advisories that apply.

## Why it exists

Rust is the language this fork is written in, yet it was the one runtime
without a panel. This gives a Cargo workspace the same overview Node, Python
and .NET projects already have. It is not a Forge port; it was written for
this tree from
[`docs/Rust_Manager_Design_Note.md`](../../docs/Rust_Manager_Design_Note.md).

**Status:** steps 1 to 3 of that note are visible here (dependencies,
outdated versions, advisories). Removing and updating dependencies is done
in the full-width manager tab,
[`cargo_manager_panel`](../cargo_manager_panel/README.md), which the
**Package Manager** quick action opens on the selected crate.

## Using it

Open with `ctrl-k c` (`cmd-k c` on macOS), the status bar icon (tooltip
"Rust"), or `rust panel: toggle focus`.

| Section | What it does |
| --- | --- |
| Toolchain line | The installed `rustc` and `cargo` versions |
| Crates | Every crate in the workspace. Click one to select it. A filter box appears when there are more than eight |
| Quick Actions | `check`, `build`, `run`, `test` for the selected crate, and **Package Manager**, which opens the Cargo Manager tab on it |
| Dependencies | The crate's crates.io dependencies with the exact version each is locked to |
| Outdated | Dependencies with a newer version on crates.io |
| Advisories | Security advisories for everything the crate pulls in. Press **Scan** |

### Reading the Outdated section

Each row can show two different moves:

- **"up to 1.4.0 … cargo update"** — a newer version the crate's requirement
  already allows. It says "up to" on purpose: it is the newest version this
  crate would accept, and another crate in the workspace can hold Cargo
  lower.
- **"newer 2.0.0 … needs the requirement changed"** — a newer version the
  requirement excludes, usually a major release.

Tags next to a version:

| Tag | Meaning |
| --- | --- |
| `patch` / `minor` / `major` | Which part of the version number moves |
| `needs Rust 1.99` (red) | That version requires a newer Rust than you have installed |
| `Rust version not declared` | The version does not say what Rust it needs. Information only |

A row also appears when the locked version has been **yanked** (withdrawn by
its author), even if nothing newer exists.

### Reading the Advisories section

Findings are grouped: **Vulnerabilities** first, most severe first, then
**Unsound**, **Unmaintained** and **Notices**. The last three are not
security holes; they are notices from the RustSec database, and they are not
counted in the section's number.

- A vulnerability whose severity is not known shows **unknown** in the
  warning colour. It is never shown as fine.
- Click an advisory id to open its page.
- "Fixed in" lists only versions newer than the one you have.
- Until you press **Scan** the section says "Not scanned yet" and shows no
  number. Selecting another crate, or any change to `Cargo.toml` or
  `Cargo.lock`, puts it back to "Not scanned yet".

The scan reads `Cargo.lock`, which covers every platform, so a finding can
concern a package that is never built on your machine. The section says so
under the list. `cargo audit` behaves the same way.

### Things it deliberately does not do

- **It never runs a build or check by itself.** `check` is a button.
- **It does not wait for rust-analyzer.** Everything works with no Rust file
  open and no language server running.
- **It does not list local or git dependencies.** A line under the
  Dependencies list says how many were left out. In a workspace like this
  one that is most of them, since crates depend on their siblings.
- **It does not poll.** It reloads when `Cargo.toml` or `Cargo.lock` changes
  on disk, however that happened.

## What runs, and when

| When | What | Cost |
| --- | --- | --- |
| Panel opens, or a manifest/lockfile changes | `cargo metadata --no-deps`, and a read of `Cargo.lock` | About a second on this repo. Local; nothing compiles |
| A crate is selected | One request per crates.io dependency to the sparse index | Cached for ten minutes per package |
| **Scan** is pressed | One request to OSV.dev per 1,000 packages, then one per advisory found | A few seconds. Advisory records are cached for the session |
| A quick action is pressed | `cargo <action> -p <crate>` in the Script Runner | Whatever Cargo takes |

## How it is wired

- `rust_panel::init(cx)` (from `crates/zed/src/main.rs`) registers
  `rust_panel::ToggleFocus`.
- `RustPanel::load(workspace, cx)` is awaited in `initialize_panels`
  (`crates/zed/src/zed.rs`).
- Implements `workspace::dock::Panel`, fixed to the left dock,
  `activation_priority() = 14`.
- It subscribes to the project's `WorktreeUpdatedEntries` events and reloads
  when a changed file is named `Cargo.toml` or `Cargo.lock`.
- Quick actions find the Script Runner through the workspace when pressed,
  as `dotnet_panel` does. One action runs at a time.

### Read-only accessors

Read by the Dashboard's Rust row and card: `rustc_version()`,
`crate_count()`, `selected_crate_name()`, `outdated_count()`,
`vulnerability_count()`, `advisory_findings()`, `advisories_scanning()` and
`advisory_scan_error()`. The count and the findings are `None` until a scan
has finished, so "not scanned" cannot be read as zero. `rescan_advisories(cx)`
is what the Dashboard's **Scan all** calls.

## Where the logic lives

| Concern | Where |
| --- | --- |
| Workspace, lockfile, outdated and advisory logic | [`cargo_backend`](../cargo_backend/README.md) |
| The HTTP requests that crate leaves to its host | [`cargo_manager_panel::registry`](../cargo_manager_panel/README.md), shared with the manager tab |
| Removing, updating and re-versioning dependencies | [`cargo_manager_panel`](../cargo_manager_panel/README.md) |
| Running commands, and validating what goes into them | [`script_runner_panel`](../script_runner_panel/README.md) |

The requests go through the app's shared HTTP client, which sends Zed's own
`User-Agent`. The crates.io index and OSV both accept it.

## Limitations

- **The workspace must have its `Cargo.toml` at the root of the opened
  folder.** A Rust project in a subfolder is reported as "Cargo.toml not
  found".
- **Only the first worktree is used** when several folders are open.
- **When running the fork with `cargo run`**, the `cargo` this panel starts
  inherits the fork's pinned Rust version, even inside a project that pins a
  different one. An installed build is not affected.

## Development

```sh
cargo check -p rust_panel -j 8
cargo test -p rust_panel -j 8
```

The tests cover the quick-action commands (including refusing shell syntax
in a crate name), the crate filter, and which file changes trigger a reload.
How the Outdated rows are built is tested in `cargo_manager_panel`, where
that code now lives. The tests do not open a window or use the network.

To run the tests of every fork crate at once: `script/test-fork-crates.ps1`.
