# cargo_backend

Finds the crates in a Cargo workspace, lists what each one depends on, says
which exact version each dependency is locked to, works out which are behind
the registry, and reports the security advisories that apply. No UI.

## Why it exists

It is the logic layer for the Rust manager described in
[`docs/Rust_Manager_Design_Note.md`](../../docs/Rust_Manager_Design_Note.md),
in the same role `npm_backend` and `dotnet_backend` play for their panels.
Keeping it free of GPUI means it can be tested without a window and tried
from a terminal before any panel exists.

It deliberately does not involve rust-analyzer. A Rust project is recognised
by its `Cargo.toml` on disk and everything is read from Cargo's own files, so
it works whether or not a language server is running.

**Status:** steps 1 to 3 of the design note's build order (dependency list,
outdated versions, vulnerabilities). Add/remove/update is not here yet.

Used by [`rust_panel`](../rust_panel/README.md) (the dock panel) and
[`cargo_manager_panel`](../cargo_manager_panel/README.md) (the manager tab).

## Trying it

```sh
cargo run -p cargo_backend --example list -j 8 -- <directory> [crate-name]
```

With no crate name it picks the workspace's default crate. On this repo:

```text
rustc 1.98.1  cargo 1.98.1
workspace: E:\zed_Source (287 crates)
crate: npm_backend 0.1.0
  log                          ^0.4         0.4.29
  semver                       ^1.0         1.0.28
  serde                        ^1.0         1.0.229
  serde_json                   ^1.0.144     1.0.151
  tempfile                     ^3.20.0      3.27.0       dev
  windows-registry             ^0.6.0       0.6.1        cfg(windows)
6 listed
92 crates.io packages reachable (the set to check for advisories)
```

To see what is behind the registry (this one makes network requests, one per
distinct dependency, through `curl`):

```sh
cargo run -p cargo_backend --example outdated -j 8 -- . npm_backend
```

```text
rustc 1.98.1
crate: npm_backend 0.1.0
  log                      ^0.4       locked 0.4.29
      cargo update log                          -> 0.4.34 (patch)
  windows-registry         ^0.6.0     locked 0.6.1
      cargo add windows-registry@0.100.0        -> 0.100.0 (minor)
2 of 6 listed dependencies are behind (6 lookups)
```

To see the security advisories (also uses the network: one batch request per
1,000 packages, then one request per advisory):

```sh
cargo run -p cargo_backend --example advisories -j 8 -- .
```

```text
crate: zed 1.24.0 (1425 crates.io packages reachable)
2 batch request(s); 28 packages with advisories; 40 records to fetch

VULNERABILITIES
  high      aws-smithy-json        0.62.5     GHSA-8ffr-xgwf-xj56
            aws-smithy-json: Uncontrolled recursion in the aws-smithy-json ...
            fixed in 0.62.7
  high      quick-xml              0.30.0     RUSTSEC-2026-0194
            Quadratic run time when checking a start tag for duplicate attribute names
            fixed in 0.41.0
  ...

26 vulnerabilities, 5 unsound, 10 unmaintained, 0 other notices
```

To see how each dependency is declared and what removing it would do, and
what updating one would change (nothing is modified: no `cargo add` or
`cargo remove` runs, and the update is a dry run):

```sh
cargo run -p cargo_backend --example changes -j 8 -- . npm_backend log
```

```text
crate: npm_backend 0.1.0
288 member manifests read
  log                          workspace  normal
      cargo remove log -p npm_backend
  tempfile                     workspace  dev
      cargo remove tempfile -p npm_backend --dev
  windows-registry             workspace  normal
      windows-registry is declared under [target.'cfg(windows)'], which can't be passed on a command line safely. Edit Cargo.toml by hand.
  ...
adding serde: written as workspace = true
adding a-crate-nobody-has: a literal version in this crate, outside [workspace.dependencies]: warn first
cargo update log@0.4.29 would change:
  Update log 0.4.29 -> 0.4.34
```

## Using it

```rust
let workspace = cargo_backend::load_workspace(&project_dir)?;
let krate = workspace.default_crate().ok_or("no crates")?;

let lockfile = cargo_backend::has_lockfile(&workspace.root)
    .then(|| cargo_backend::read_lockfile(&workspace.root))
    .transpose()?;

let list = cargo_backend::direct_dependencies(krate, lockfile.as_ref());
for row in &list.listed {
    // row.name, row.requirement, row.locked_version, row.kind, row.target
}
// list.hidden: how many local and git dependencies were left out
```

`load_workspace`, `query_cargo` and `query_rustc` run a process and block;
call them from a background task. Everything else is plain parsing.

Checking for newer versions takes one request per distinct dependency, which
the host makes:

```rust
let toolchain = cargo_backend::query_rustc().ok();
for name in list.registry_names() {
    let body = http_get(&cargo_backend::index_url(name)).await?; // host's client
    let versions = cargo_backend::parse_index(&body)?;
    for row in list.listed.iter().filter(|row| row.name == name) {
        let status = row.status(&versions, toolchain.as_deref());
        // status.in_range, status.out_of_range, status.locked_yanked
    }
}
```

The host must send a descriptive `User-Agent`. Index responses carry an
`ETag` and a ten-minute `Cache-Control`, so cache them and revalidate.

Checking for advisories is two rounds of requests, again made by the host:

```rust
let reachable = lockfile.reachable_crates_io_packages(&krate.name, &krate.version);

// Round 1: which advisories apply? Up to 1,000 packages per request.
let mut hits = Vec::new();
for batch in cargo_backend::osv_batches(&reachable) {
    let answer = http_post(cargo_backend::OSV_BATCH_URL, &batch.body).await?;
    hits.extend(batch.parse_response(&answer)?);
}

// Round 2: what are they? One request per distinct advisory.
let mut records = Vec::new();
for id in cargo_backend::advisory_ids(&hits) {
    let body = http_get(&cargo_backend::advisory_url(id)).await?;
    records.push(cargo_backend::parse_advisory(&body)?);
}

let findings = cargo_backend::merge_findings(&hits, &records);
let counts = cargo_backend::FindingCounts::of(&findings);
```

Round 2 is where the time goes: about 0.7 s per record when fetched one at a
time, so fetch several at once. Each id from round 1 comes with a `modified`
stamp; cache a record under its id and stamp and it never needs refetching
until the stamp changes.

### API

| Area | Items |
| --- | --- |
| Detection | `is_cargo_project(dir)`, `has_lockfile(root)` |
| Workspace | `load_workspace(dir)` → `Workspace` (`root`, `crates`, `default_crates`, `find`, `default_crate`) |
| Crates | `CrateInfo` (`name`, `version`, `manifest_path`, `rust_version`, `dependencies`, `directory()`) |
| Declared dependencies | `Dependency`, `DependencyKind` (normal/build/dev), `DependencySource` (crates.io/path/git/other registry) |
| Lockfile | `read_lockfile(root)` → `Lockfile` (`package`, `locked_version`, `reachable_crates_io_packages`), `LockedPackage` |
| The list the manager shows | `direct_dependencies(crate, lockfile)` → `DependencyList` (`listed`, `hidden`) |
| Toolchain | `query_rustc()`, `query_cargo()`, `version_from_output` |
| Registry index | `index_url(name)`, `index_path(name)`, `parse_index(body)` → `Vec<IndexVersion>` (`version`, `yanked`, `rust_version`) |
| Outdated | `ListedDependency::status(versions, toolchain)` / `version_status(...)` → `VersionStatus` (`latest`, `in_range`, `out_of_range`, `locked_yanked`), `UpdateTarget`, `UpdateKind` |
| Minimum Rust version | `rust_compat(declared, toolchain)` → `RustCompat` (`Compatible`, `TooNew { needs }`, `NotDeclared`) |
| Version picker | `available_versions(versions, toolchain, compatible_only, include_prereleases)` |
| Lookups needed | `DependencyList::registry_names()` |
| Advisories, round 1 | `osv_batches(packages)` → `Vec<OsvBatch>` (`body`, `parse_response`), `PackageAdvisories`, `advisory_ids(hits)` |
| Advisories, round 2 | `advisory_url(id)`, `parse_advisory(json)` → `AdvisoryRecord` |
| Findings | `merge_findings(hits, records)` → `Vec<Finding>` (`package`, `version`, `id`, `other_ids`, `kind`, `severity`, `summary`, `fixed_in`, `url`, `details_missing`), `FindingKind`, `FindingCounts` |
| Severity | `Severity` (low/moderate/high/critical), `cvss3_base_tenths(vector)` |
| Parsers | `parse_metadata(json)`, `parse_lockfile(toml)` |
| Command lines | `add_args(name, version, member, kind)`, `remove_args(name, member, kind, target)`, `update_args(specs, dry_run)` → `Option` (never a bare `cargo update`), `UpdateSpec`, `command_line(args)` |
| Validation | `check_crate_name(name)`, `check_exact_version(version)` |
| What an update would change | `update_dry_run(root, specs)` / `parse_update_output(text)` → `Vec<LockChange>` (`kind`, `name`, `from`, `to`) |
| Manifests | `read_root_manifest(workspace)`, `read_crate_manifest(crate)`, `read_member_manifests(workspace)`, `parse_manifest(toml)` → `Manifest` |
| Side effects | `declaration(root, member, package, kind, target)` → `Declaration` (inherited/literal), `add_effect(root, package)` → `AddEffect`, `remove_effect(root, members, member, package, kind, target)` → `RemoveEffect` |

## Where the data comes from

Four sources, on purpose:

| Question | Source | Why |
| --- | --- | --- |
| What does this crate declare? | `cargo metadata --no-deps` | Cargo applies `workspace = true` inheritance, so `serde.workspace = true` arrives as `^1.0` with its features. `--no-deps` stops Cargo resolving the graph: about one second on this repo, with no network access and no compiling. |
| What is installed? | `Cargo.lock`, read directly | The resolved answer is already on disk. Asking Cargo to resolve it again is slow here and duplicates what rust-analyzer does. |
| What else exists? | The crates.io sparse index, one small file per crate | The same data Cargo resolves from. Each version carries its yanked flag and declared minimum Rust version, so neither needs a second request. This crate builds the URL and parses the answer; it never makes the request. |
| What is known to be wrong? | OSV.dev, which includes the RustSec advisory database | Nothing to install, and it answers for a thousand packages per request. Again this crate builds the requests and parses the answers only. |

## Behaviour worth knowing

- **Only crates.io dependencies are listed.** Path, git and other-registry
  dependencies are left out and counted in `hidden`, so the UI can say
  "12 local and git dependencies not shown". On this repo that is most of any
  crate's dependencies (`zed` lists 37 and hides 160), since crates depend on
  their workspace siblings.
- **A dependency keeps its registry name.** One imported under an alias
  (`scap = { package = "zed-scap" }`) is listed as `zed-scap`, with the alias
  in `rename`. The registry name is what Cargo commands and the lockfile use.
- **One row per declaration.** A package declared as both a normal and a dev
  dependency, or under two targets, gets a row for each.
- **The locked version is per crate.** When two versions of a package are in
  the lockfile, each crate is shown the one it actually uses.
- **An unknown locked version stays unknown.** With no lockfile, or a
  dependency added since it was last written, `locked_version` is `None`. It
  is never guessed.
- **The reachable set walks through hidden dependencies.** A local crate is
  not listed, but the crates.io packages it pulls in are still part of the
  build, so `reachable_crates_io_packages` follows it. For `zed` that is
  1,425 packages. It slightly over-reports for sibling crates, because the
  lockfile does not record which of a workspace member's dependencies are
  dev-only.

### Outdated versions

Every outdated dependency is sorted into one of two moves, because they are
different commands:

| Move | Meaning | Command |
| --- | --- | --- |
| In range | A newer version the declared requirement already allows | `cargo update <name>` (lockfile only) |
| Out of range | A newer version the requirement excludes | `cargo add <name>@<version>` (changes the requirement) |

- **The requirement decides, not the version number.** For `^0.3`, 0.4.0 is
  out of range even though only the second number moved, exactly as Cargo
  treats it.
- **An in-range version is an upper bound.** It is the newest version *this
  crate's* requirement allows. Another package in the workspace can hold
  Cargo lower: here `clap` is `^4.4` and 4.6.7 exists, but `cargo update clap`
  settles on 4.6.1. Present it as "up to", and use
  `cargo update --dry-run <name>` when the exact outcome matters. Of seven
  dependencies cross-checked on this repo, the other six matched Cargo
  exactly.
- **Yanked versions are never offered**, and a locked version that has been
  yanked is flagged even when nothing newer exists.
- **Pre-releases are only offered when the requirement opts into them**,
  which is Cargo's own rule.
- **Minimum Rust version has three states**: compatible, too new (with the
  version needed), and not declared. An unknown toolchain or an unreadable
  declaration is "not declared", never a claimed pass.
- **Nothing is outdated without a locked version.** With no lockfile there
  is no "from", so only `latest` is reported.
- **A package declared twice gives two rows** with the same status (for
  example a normal and a dev dependency). The Updates page should show it
  once.

### Advisories

- **Not every advisory is a vulnerability.** RustSec also publishes notices:
  *unmaintained* (the crate has no maintainer) and *unsound* (it can cause
  undefined behaviour from safe code). `FindingKind` keeps the four kinds
  apart so an abandoned crate is never counted as a security hole. For `zed`
  on this repo: 26 vulnerabilities, 5 unsound, 10 unmaintained.
- **The same advisory under two ids is one finding.** GitHub and RustSec
  publish many advisories separately, each listing the other as an alias (or
  sharing a CVE). They are merged, and shown under the RustSec id.
- **Severity is worked out, not just read.** Only GitHub records carry a
  label. RustSec records usually carry a CVSS vector instead, so
  `cvss3_base_tenths` computes the standard base score from it. For `zed`
  that rates 18 of the 26 vulnerabilities; the other 8 have only a CVSS v4
  vector, or nothing, and stay *unknown* rather than being guessed. Unknown
  is `None`, and must be shown as a warning, not as fine.
- **One finding per locked version.** `quick-xml` is locked at three
  versions in this repo and is reported three times, because an advisory
  applies to specific versions.
- **`fixed_in` only lists versions newer than the locked one**, oldest
  first. An advisory often names one fix per release line.
- **A failed detail request does not lose the advisory.** It is still
  reported, with `details_missing` set, its kind and severity unknown.
- **Withdrawn advisories are dropped.**
- **A truncated answer says so.** If OSV had more results for a package than
  it returned, `PackageAdvisories::truncated` is set and the UI must not
  present the list as complete.
- **The reachable set is an upper bound**, so some findings concern packages
  that are never built on your platform. The lockfile records every edge
  Cargo might need for any target and optional feature. Here `npm_backend`
  is reported as reaching the unmaintained `tokio-io`, through a chain that
  only exists when building for WebAssembly. `cargo audit` reads the
  lockfile the same way and reports the same set.

### Adding, removing and updating

The crate builds the command lines; the host runs them. Checked with Cargo
1.98.1 in a scratch workspace:

- **Every value is validated first.** The arguments end up in a shell
  command line, so a crate name is letters, digits, `-` and `_` only, and a
  version must parse as one exact version. Anything else is refused with a
  message and nothing is built.
- **`update_args` cannot produce a bare `cargo update`.** With nothing to
  update it returns `None`. A bare `cargo update` would change 621 lockfile
  entries on this fork. "Update all" is one command naming every row.
- **Updates name the locked version** (`clap@4.5.49`). With several versions
  of a package in the lockfile, the name alone is rejected by Cargo as
  ambiguous.
- **One update moves several packages.** `cargo update clap` here changes
  six lockfile entries. `update_dry_run` returns the full list, which is
  what a confirmation should show.
- **The table matters.** Cargo will not search: removing a dev-dependency
  needs `--dev`, and a target-specific one needs `--target`. Both come from
  the dependency's own `kind` and `target`.
- **`cfg(...)` targets are refused.** A dependency under
  `[target.'cfg(windows)'.dependencies]` cannot be removed from here: the
  expression's parentheses, quotes and spaces are read differently by each
  shell. Plain target triples work.
- **Adding follows the workspace table** (`add_effect`). If the root
  manifest's `[workspace.dependencies]` has the package, Cargo writes
  `name.workspace = true`. If the table exists without it, Cargo writes a
  literal version into the member; the host should warn first.
- **An inherited dependency's version cannot be changed by a command**
  (`declaration`). `cargo add name@version` on one replaces
  `workspace = true` with a literal version in the member and leaves the
  root manifest alone. No Cargo command edits `[workspace.dependencies]`, so
  the host should not offer this for an inherited dependency.
- **Removing can edit the root manifest** (`remove_effect`). When the
  removed dependency was the last one inheriting a `[workspace.dependencies]`
  entry, Cargo deletes that entry too. A literal version elsewhere does not
  keep it; another table in the same crate does.

### When running the fork with `cargo run`

`rustup` sets `RUSTUP_TOOLCHAIN` for anything started through `cargo run`,
and child processes inherit it. So while developing the fork, the `cargo`
this crate spawns uses the fork's pinned toolchain even inside a project
that pins a different one. An installed build is not affected.

## Layout

| File | Contents |
| --- | --- |
| `src/cargo_backend.rs` | Detection, running Cargo, toolchain versions, `direct_dependencies` |
| `src/metadata.rs` | `cargo metadata` parsing: workspace, crates, declared dependencies |
| `src/lockfile.rs` | `Cargo.lock` parsing, locked versions, the reachable set |
| `src/index.rs` | Sparse-index paths and parsing |
| `src/outdated.rs` | In-range and out-of-range updates, minimum Rust version, version picker |
| `src/advisories.rs` | OSV batch requests, advisory records, CVSS scoring, merging into findings |
| `src/actions.rs` | `cargo add`/`remove`/`update` command lines, dry-run output, manifest side effects |
| `src/path_env.rs` | PATH enrichment on Windows, as in the other backends |
| `examples/list.rs` | Lists a crate's dependencies |
| `examples/outdated.rs` | Shows what is behind the registry (uses the network) |
| `examples/advisories.rs` | Shows the advisories that apply (uses the network) |
| `examples/changes.rs` | Shows how dependencies are declared and what removing or updating would do (changes nothing) |

## Development

```sh
cargo check -p cargo_backend -j 8
cargo test -p cargo_backend -j 8
```

The tests cover all three parsers, dependency sources and kinds,
locked-version lookup with several versions of one package, the reachable
set (including cycles), the listed/hidden split, in-range and out-of-range
updates (including 0.x versions, exact and wildcard requirements, yanked
versions and pre-releases), the three minimum-Rust-version states, OSV batch
splitting and answer matching, CVSS v3 scores against published values,
merging advisories into findings, the add, remove and update command lines
(including that values with shell syntax are refused and that an update
always names a package), reading `cargo update` output, and the manifest
side effects of adding and removing. They use inline fixtures and temporary
directories; none of them runs Cargo or touches the network.

One further test does run Cargo, against this repo, and is ignored by
default. It is the design note's acceptance check for step 1:

```sh
cargo test -p cargo_backend -j 8 -- --ignored
```

To run the tests of every fork crate at once: `script/test-fork-crates.ps1`.
