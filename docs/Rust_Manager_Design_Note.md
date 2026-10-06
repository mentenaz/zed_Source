# Rust (Cargo) Manager: Design Note

**Status:** the backend for steps 1 to 3 (`cargo_backend`) is built and tested; no panel yet. **Written:** October 2026. **Reviewed against the fork:** 6 October 2026 (Cargo 1.98.1).
**Pattern to follow:** the existing runtime panel plus manager pairs, each with a GPUI-free backend (`node_panel` + `npm_manager_panel` + `node_backend`/`npm_backend`, `dotnet_panel` + `nuget_manager_panel` + `dotnet_backend`, `python_panel` + `python_manager_panel` + `python_backend`).

---

## 1. Goal

Give Rust projects the same experience the other runtimes have: pick a crate, see its dependencies, see what is outdated or vulnerable, and add, remove or update dependencies without leaving the editor.

Rust is the language used every day in this fork, so this panel is a daily-driver, not a breadth exercise.

## 2. Non-goals for v1

- Editing feature flags
- Publishing crates, or alternative registries
- Workspace-wide dependency view (per-crate first)
- Path and git dependencies: hidden from every list in v1 (decision 10). No tagging, updating or vulnerability checking for them.
- Error counts or diagnostics on the Dashboard (the Dashboard has no diagnostics code today)
- A "reload language server" button

## 3. The rule that shapes the design

**The manager is self-sufficient. rust-analyzer is optional.**

Observed in testing (October 2026): the language server only starts once a Rust file is opened, and outside edits to the manifest are not picked up until then. So nothing in the manager may wait on, or depend on, rust-analyzer.

Consequences:

- Detect a Rust project from `Cargo.toml` on disk, the way Node uses `package.json`. Never from "is rust-analyzer running".
- After a Cargo command, refresh by re-reading the manifests and `Cargo.lock`. Do not wait for the analyzer.
- Never run a check automatically. "Does it still build?" is an explicit button that runs `cargo check` on click.
- Anything that needs analyzer data (error counts and similar) is shown as *unknown*, never as healthy.

## 4. Decisions

| # | Decision | Default | Why | Alternative |
|---|---|---|---|---|
| 1 | Dependency list scope | The selected crate's **direct** dependencies | Matches the active-target model across Node, .NET and Python. Keeps the Zed fork's 1,964 locked packages out of the UI. | Whole-workspace view (later) |
| 2 | Vulnerability scope | Every locked package **reachable from the selected crate** (direct and transitive) | Most real advisories are in transitive dependencies, so direct-only would miss them. Scanning the whole lockfile would report packages the selected crate never uses. | Direct only (too little), whole lockfile (too much) |
| 3 | Vulnerability source | **OSV.dev** (ecosystem `crates.io`) | No install needed by the user. RustSec advisories feed OSV. | `cargo audit` (needs an install) |
| 4 | "Compatible versions only" | Each candidate version's declared **MSRV** (`rust-version`) vs the installed toolchain, in **three states** (see section 6) | Closest match to what the npm manager does. No crate in this fork declares its own `rust-version` (0 of 286), so the toolchain is the only thing to compare against. | Plus semver ranges (later) |
| 5 | Adding a dependency the workspace doesn't have yet | **Warn and ask** before running | Plain `cargo add` writes a literal version into the member crate, which is not how this repo declares dependencies (see section 7). Editing the root manifest from a panel affects every crate, so v1 leaves that to the user. | Add it to `[workspace.dependencies]` first, then to the member (a contained change later, if the warning proves tedious) |
| 6 | A version that declares no `rust-version` | **Neutral "not declared" marker**, never an error | About a third of this fork's locked registry packages declare none (21 of a 60-package sample), and for most of those the newest release declares none either (17 of 60). An error would be permanent and unfixable, and would bury real warnings. It is still visibly marked, so unknown does not pass for healthy. | Treat as an error (rejected), or hide the marker (rejected) |
| 7 | Yanked versions | **Hidden** from version lists; an **installed** yanked version is flagged as a warning on the Installed page | A withdrawn version is not a useful install target, but already having one locked usually signals a bug or security problem. | Show struck through |
| 8 | An advisory with no severity | Shown as **"unknown" in the warning colour** | An advisory exists; only its rating is missing. Showing it neutrally would rank a real vulnerability below a rated low one. | Neutral colour (rejected) |
| 9 | Offline | Show the **last cached results with their age** ("as of 2 hours ago") | A blank panel is worse than a stale, labelled one. The timestamp is needed for cache expiry anyway. | Show nothing |
| 10 | Path and git dependencies | **Hidden** from the Installed and Updates lists. A single footer line states how many were left out. | The lean option. They have no registry release to compare against, and on this fork they are most of any crate's dependencies (workspace siblings), so listing them would bury the registry packages the manager exists for. The footer keeps the omission visible rather than silent. | Show them with a "path"/"git" tag (later, if wanted) |
| 11 | A crate reachable at more than one version | Installed and Updates show the version the selected crate depends on **directly** (always one per dependency). Vulnerabilities shows **one row per name-and-version pair**. | An advisory applies to specific versions, so merging two versions would hide which one is affected. Nothing extra to build: this is the shape the data already has. | A combined row (rejected) |
| 12 | Advisory details | Fetched **for every advisory found**, several at a time, and cached by id and `modified` stamp | Supersedes "fetch lazily when a row is opened". Without the details two ids cannot be recognised as one advisory, and a notice cannot be told from a vulnerability, so the list and its counts would be wrong until every row had been opened. The cost is bounded: 40 records for `zed`. | Lazy fetch (rejected: wrong counts) |
| 13 | Advisories that are not vulnerabilities | **Kept apart**: Vulnerabilities, Unsound, Unmaintained, Notices, each with its own count. Only vulnerabilities feed the Vulnerabilities badge. | RustSec publishes *unmaintained* and *unsound* notices alongside vulnerabilities. For `zed`, 15 of 41 findings are notices. Counting an abandoned crate as a security hole would make the number meaningless. | One combined list (rejected) |
| 14 | Severity when the record has no label | **Computed from the CVSS v3 vector** when there is one; otherwise unknown | Only GitHub records carry a label; RustSec records carry a vector. For `zed` a label alone rates 6 of 26 vulnerabilities, computing rates 18. CVSS v4 is a lookup table, not a formula, so v4-only records stay unknown rather than approximated. | Label only (leaves most rows unknown) |

## 5. Data sources

| Need | Source | Notes |
|---|---|---|
| Declared requirements per crate | `cargo metadata --no-deps` | **Verified on the fork.** It resolves `workspace = true` to the real requirement and merged features, and reports kind (normal/dev/build), target conditions, renames and git/path sources. Takes about 1.2 s. The output is the whole workspace (286 packages, 1.6 MB): keep only the selected crate. Never run plain `cargo metadata` (without `--no-deps`) here. |
| Resolved (installed) versions | `Cargo.lock` | Read the file directly. A crate name can appear at several versions, so resolve through the selected crate's own `dependencies` list in the lockfile, not by name alone. |
| Packages reachable from the selected crate | `Cargo.lock` | Walk the lockfile's dependency edges from the selected crate. The walk goes **through** path and git dependencies, since the registry packages they pull in are still reachable, but only registry packages are sent to OSV. |
| Latest versions, MSRV, yanked flag | Sparse index (`index.crates.io`) | One small request per registry dependency, returning one JSON line per published version. Each line carries `rust_version` and `yanked`, so MSRV filtering needs no extra requests; older versions often have no `rust_version` (46 of 135 for `time`). Responses are served with `Cache-Control: max-age=600` and an `ETag`, so revalidate with `If-None-Match` rather than refetching. Path and git dependencies need no lookup. |
| Search | crates.io Web API (`crates.io/api/v1/crates?q=`) | A descriptive `User-Agent` is mandatory: a request without one gets HTTP 403. Paginates with a `seek` token in `meta.next_page`. Keep request volume low and cache results. **Verify** the published rate limit; assume one request per second until then. |
| README | `crates.io/api/v1/crates/<name>/<version>/readme` | Answers with a 302 redirect to `static.crates.io/readmes/...html`. The body is **rendered HTML, not Markdown**, so the flyout needs `gpui_component`'s HTML rendering, and the HTTP client must follow a redirect to a different host. |
| Vulnerabilities | OSV.dev, `POST https://api.osv.dev/v1/querybatch` | At most **1,000 queries per request** (1,001 is rejected with "too many queries"), so chunk the reachable set. The batch response holds only an `id` and `modified` date per advisory. Severity, summary, fixed-in versions, aliases and the notice marker all need a second request per advisory (`GET /v1/vulns/<id>`). Fetch them for every advisory found (decision 12), several at once: one at a time they take about 0.7 s each. Each id arrives with a `modified` stamp to cache by. Results paginate past 1,000 per query or 3,000 in total. The ecosystem must be exactly `crates.io`; anything else is rejected as "invalid ecosystem". **One advisory can come back twice**, once under its GitHub id and once under its RustSec id, each listing the other in `aliases`: merge them into one row. Only the GitHub record carries a severity. |
| Add | `cargo add <name> -p <crate>` | Edits the member's `Cargo.toml` and `Cargo.lock`. Does not compile. |
| Remove | `cargo remove <name> -p <crate>` | Edits the member's `Cargo.toml` and `Cargo.lock`, **and may edit the root `Cargo.toml`** (see section 7). Does not compile. |
| Update within the declared range | `cargo update <name>` | Only changes `Cargo.lock`. **Always pass a package name** (see section 7). Does not compile. |
| Move to a version outside the declared range | `cargo add <name>@<version> -p <crate>` | `cargo update` cannot do this; the requirement itself has to change. |
| Does it still build? | `cargo check` | On demand only, via an explicit button. Output goes to the Script Runner. |

## 6. UI

**Left panel (`rust_panel`)**

- Header: `rustc` and `cargo` versions
- Projects: crates in the workspace. The selected crate is the active target.
- Quick actions (new buttons, following the pattern in `node_panel` and `dotnet_panel`): `check`, `build`, `run`, `test`, plus a **Package Manager** button
- Summary counts: Dependencies, Outdated, Vulnerabilities

**Manager tab (`cargo_manager_panel`):** the same pages as the other managers: General, Search, Installed, Updates, Vulnerabilities, with the README flyout.

**Updates page.** Classify each row as patch, minor or major, as the other managers do, and make clear which action it gets:

- *Within the declared range:* `cargo update <name>`. Lockfile only. Shown as "up to <version>", because the version is the newest this crate's requirement allows and another package in the workspace can hold Cargo lower. Found on this fork: `clap` is `^4.4` and 4.6.7 exists, but `cargo update clap` settles on 4.6.1. The confirmation step runs `cargo update --dry-run <name>` and shows the exact result.
- *Outside the declared range* (usually a major version): `cargo add <name>@<version>`. Changes the requirement, and for a workspace-inherited dependency that means the root manifest.

"Update all" only ever covers the first kind.

**Minimum Rust version, three states.** Applied wherever a version is listed or chosen:

| State | Shown as | Action offered |
|---|---|---|
| Declares a minimum the installed toolchain meets | Normal | None needed |
| Declares a minimum **above** the installed toolchain | **Error**, styled like a vulnerability: "Needs Rust 1.99. You have 1.98.1." | Update the toolchain, or choose an older version |
| Declares nothing | Small neutral "not declared" marker, with a tooltip: "No minimum Rust version declared" | None; information only |

The "compatible versions only" filter hides the second state and keeps the other two.

**Path and git dependencies** are not listed. Below the Installed list, one line says how many were left out: "12 local and git dependencies not shown." They are not checked for vulnerabilities, and the same line says so on the Vulnerabilities page.

**Yanked versions** are left out of every version list. If the version currently locked is yanked, the Installed page flags that row as a warning.

**Vulnerability rows** merge an advisory's GitHub and RustSec records into one. A vulnerability with no severity shows "unknown" in the warning colour.

**The Vulnerabilities page has sections**, in this order: Vulnerabilities (most severe first), Unsound, Unmaintained, Notices. Notices show no severity; for them a missing one is not "unknown", there simply isn't one. The page's badge counts vulnerabilities only.

**A note under the list** says the scan covers every package in the lockfile reachable from the crate, on any platform, so a finding may concern a package that is never built here. Found on this fork: `npm_backend` is reported as reaching the unmaintained `tokio-io`, through a chain that only exists when building for WebAssembly. `cargo audit` reports the same set.

**Honest states, same as the Dashboard:** *not scanned yet*, *Cargo.toml not found*, *waiting for Cargo*, *offline (showing results as of …)*. Unknown must never look like healthy. Vulnerability badges use the warning colour, not the neutral one.

## 7. Behaviour rules

1. **One Cargo action at a time.** Disable the other buttons while one runs.
2. **Waiting state.** A running build holds the build-directory lock, which `cargo add` mostly doesn't need, but the package-cache lock can still make a command wait. Show "waiting for Cargo".
3. **Refresh by re-reading files** after each action: the member manifest, the root manifest and the lockfile.
4. **`cargo update` always names a package.** Run with no package on this fork, it would change 621 lockfile entries (dry-run, October 2026). The panel must never issue a bare `cargo update`. "Update all" issues one named update per listed row.
5. **Removing can touch the root manifest.** When the removed dependency was the last user of a `[workspace.dependencies]` entry, `cargo remove` deletes that entry from the root `Cargo.toml` as well (confirmed in a scratch workspace). The confirmation dialog says so before running.
6. **Adding respects the workspace convention.** If the dependency already exists in `[workspace.dependencies]`, `cargo add` writes `name.workspace = true`, which is correct. If it does not, `cargo add` writes a literal version into the member crate. Per decision 5, warn and ask first.
7. **Hint after a change, only when it is true.** If no Rust file is open: "Cargo.toml updated. rust-analyzer will pick it up when you next open a Rust file." If one is open, the analyzer notices the change by itself, so show nothing.
8. **Cache** crates.io and index lookups, each with the time it was fetched. Honour the index's own ten-minute lifetime and revalidate with its `ETag`; keep API results for about an hour. When offline, keep showing the cached results and state their age instead of clearing them.
9. **Destructive actions** (remove, update) get the same confirmation pattern as the other panels.
10. **Validate before running.** Crate names and versions go through `script_runner_panel::command` (`check_package_name`, `check_version`) before being placed in a command, like the other managers. A refused value shows its message instead of running.

## 8. Performance budget

- **Idle:** nothing runs. Load when the panel opens, refresh when a manifest or the lockfile changes. No polling.
- **Local:** one `cargo metadata --no-deps` (about 1.2 s on the fork) per refresh, plus a lockfile parse.
- **Network, outdated check:** one index request per registry dependency. Typically 20 to 50, but not always: `zed` has 214 direct dependencies. Many of those are local path crates that need no lookup, so count registry dependencies only, cap concurrency, and show progress.
- **Network, vulnerabilities:** one batched OSV request per 1,000 reachable packages (2 for `zed`, about 4 s), then one request per distinct advisory (40 for `zed`). Fetch those several at a time and cache them by id and `modified` stamp, so a repeat scan costs only the batch requests.
- **UI:** virtualised lists, lazy loading.
- **Stress test:** the Zed fork itself (286 workspace crates, 1,964 locked packages).

## 9. Edge cases to define

Already answered by `cargo metadata --no-deps`, so they need display rules rather than detection:
`workspace = true` inheritance · git and path dependencies · renamed dependencies (`package = "..."`) · target-specific dependencies · `[dev-dependencies]` and `[build-dependencies]`.

Decided (see section 4):
yanked versions (decision 7) · offline mode (decision 9) · versions with no declared `rust-version` (decision 6) · a new dependency that is not in `[workspace.dependencies]` (decision 5) · an advisory with no severity (decision 8).

Also decided:
a crate reachable at several versions (decision 11) · path and git dependencies (decision 10).

Nothing is left undefined for v1.

## 10. Structure

Three crates, following the existing pairs:

| Crate | Contents |
|---|---|
| `cargo_backend` | No GPUI. Parsing `cargo metadata` output and `Cargo.lock`, walking the reachable set, sparse-index and OSV response parsing, version classification, and building the Cargo command lines. |
| `rust_panel` | The left dock panel. |
| `cargo_manager_panel` | The manager tab. |

Keeping the logic in `cargo_backend` is what makes it testable without a window; that split is how the other backends reached their test coverage.

Fork conventions that apply to all three:

- The UI is composed from `gpui_component` widgets (`setting::{Settings, SettingPage, SettingGroup, SettingItem}`, `h_resizable`), not hand-rolled `div` lists. See `CLAUDE.md` and `crates/gpui_component/PORTING.md`.
- Each crate gets a README, a `LICENSE-APACHE` link, and unit tests.
- Add the three crates to `script/test-fork-crates.ps1`.

## 11. Build order (each step shippable)

1. Detect the crate, list its direct dependencies. *Done when:* the list matches `Cargo.toml` on a scratch crate and on one Zed crate.
   - **Backend done (6 October 2026):** `crates/cargo_backend`. Checked on a scratch workspace, on `npm_backend` (6 listed) and on `zed` (37 listed, 160 hidden); the counts match Cargo's own output. The reachable-set walk for step 3 is included. The panel is not started.
2. Outdated versions from the sparse index.
   - **Backend done (6 October 2026):** index paths and parsing, in-range and out-of-range targets, the three minimum-Rust-version states, yanked handling and the version picker. Checked against live data: of seven dependencies compared with `cargo update --dry-run`, six matched exactly. The request itself is left to the panel.
3. Vulnerabilities via OSV.
   - **Backend done (6 October 2026):** batch requests, advisory records, CVSS v3 scoring, and merging into findings. Checked against live data for `zed`: 1,425 reachable packages, 2 batch requests, 40 records, giving 26 vulnerabilities, 5 unsound and 10 unmaintained. The requests themselves are left to the panel. See decisions 12 to 14 for what the real data changed.
4. Add, remove and update through Cargo.
5. Search and README viewing.

Timebox steps 1 to 3 to a couple of evenings. Do not add it to the CV, README or site until it works.

## 12. To verify before or during the build

- [x] `cargo metadata --no-deps` resolves workspace-inherited requirements on the Zed fork (yes; 1.2 s, 286 packages)
- [x] `cargo add` and `cargo remove` edit both the manifest and the lockfile; `cargo update` edits only the lockfile; none compile (yes, Cargo 1.98.1)
- [x] OSV batch endpoint (`POST https://api.osv.dev/v1/querybatch`; IDs and modified dates only)
- [x] OSV maximum queries per batch (1,000; tested)
- [x] OSV ecosystem name for Rust crates (`crates.io`; tested)
- [x] crates.io requires a `User-Agent` (403 without one; tested)
- [x] Sparse index carries `rust_version` and `yanked` per version (tested)
- [x] README endpoint format (302 redirect to rendered HTML; tested)
- [x] In-range targets against Cargo's own resolution (six of seven matched; `clap` is held back by another package, hence "up to")
- [x] OSV record shapes, on live data for this fork (GitHub and RustSec records, informational notices, CVSS v3 and v4 vectors, several fixed versions per advisory)
- [ ] crates.io published rate limit, and whether the sparse index is exempt from it (no rate-limit headers are returned, so this needs the policy page)
- [ ] Whether a lighter "reload" action than a full language-server restart exists in the fork (only needed if the hint proves annoying)

## 13. Testing

- Scratch crate at `H:\Demos\rust-scratch` for basic add, remove and update.
- A scratch **workspace** with a `[workspace.dependencies]` table, to cover: adding a dependency the workspace already has, adding one it does not, and removing the last user of a workspace entry.
- The Zed fork for scale, **read-only**: never run add, remove or update against it while testing.
- Run each action once with no Rust file open, to confirm the manager works without rust-analyzer.
- Unit tests in `cargo_backend` for the parsers and command builders, including that a bulk update never produces a bare `cargo update`, that the three minimum-Rust-version states are classified correctly, that yanked versions are filtered, that an advisory's GitHub and RustSec records merge into one row, that path and git dependencies are excluded from the lists but still walked through, and that two versions of one crate stay separate on the Vulnerabilities page.
