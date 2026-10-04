# Zed Developer Tooling Suite (personal fork)

An unofficial, personal fork of [Zed](https://github.com/zed-industries/zed) that adds a suite of developer-tooling panels and a visual workflow engine. Not affiliated with or endorsed by Zed Industries.

![Workflow engine](docs/screenshots/Flow_Running.png)
![Database schema graph](docs/screenshots/Database_Schema_Graph.png)
![Dashboard](docs/screenshots/dashboard.png)

## What this fork adds

| Area                                      | What it does                                                                                                                                                                    |
| ----------------------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **Node / .NET / Python panels**           | Per-project version and environment management. Select a project as the active target and run, build and test act on it.                                                        |
| **Package managers** (npm, NuGet, Python) | Install, update and remove packages, vulnerability scanning, version search, inline README viewing.                                                                             |
| **Database panel**                        | Simultaneous live connections to SQLite, PostgreSQL, MySQL and MSSQL, schema explorer, relationship graph, SQL workbench.                                                       |
| **Flows (Task Chain)**                    | Visual workflow editor with try/catch, if/else, loops and error paths. Click any node to see its run result. Backed by the `workflow_engine` crate.                             |
| **Dashboard**                             | One view of runtime versions, security findings, outdated packages, git status and system health. Reports failures (project not found, not scanned yet) instead of hiding them. |
| **Cockpit and Processes**                 | Live CPU, RAM, per-core, disk and network metrics, and a process list with kill.                                                                                                |
| **Git panel Details tab**                 | Commit activity, most-changed files and collaborators.                                                                                                                          |
| **Helm**                                  | GitHub client panel: search, clone, repo browsing. Credentials are stored in the OS keychain.                                                                                   |
| **Custom SQL language server**            | Experimental. It starts and responds, but completion quality is still rough.                                                                                                    |

## Changes to existing Zed code

- **`gpui`**: added zoom, pan and fit-view scaling support, which was missing. A standalone upstream PR is planned.
- **Extended to support the panels**: `workspace`, `git`, `git_ui`, `project`, `settings`, `settings_content`, `json_schema_store`, `languages` and the `zed` app crate, plus small supporting edits in `fs`, `icons`, `paths` and `remote`.
- **New crates**: `cockpit_panel`, `dashboard_panel`, `database_backend`, `database_panel`, `flows_panel`, `workflow_engine`, `node_panel`, `dotnet_panel`, `python_panel`, `npm_manager_panel`, `nuget_manager_panel`, `python_manager_panel`, `processes_panel`, `helm_panel`, `script_runner_panel`, `designer_panel` and their backends.

## Status

- Developed and tested on Windows. [CHECK: add "macOS and Linux untested" or change this if you've tested them.]
- Tested mostly by hand. Automated tests are limited (a few in `gpui_flow`).
- Not a packaged release. Build from source with Zed's [Windows build guide](./docs/src/development/windows.md).

## Licence and attribution

The combined work is distributed under **GPL-3.0-or-later**, inherited from the Zed crates modified here (see `LICENSE-GPL`). `gpui` and the GPUI component libraries remain **Apache-2.0** (see `LICENSE-APACHE`). `gpui_flow` is **MIT** © Adib, patched here for zoom, pan and fit-view; its licence text is kept in its crate folder.

# Zed

[![Zed](https://img.shields.io/endpoint?url=https://raw.githubusercontent.com/zed-industries/zed/main/assets/badge/v0.json)](https://zed.dev)
[![CI](https://github.com/zed-industries/zed/actions/workflows/run_tests.yml/badge.svg)](https://github.com/zed-industries/zed/actions/workflows/run_tests.yml)

Welcome to Zed, a high-performance, multiplayer code editor from the creators of [Atom](https://github.com/atom/atom) and [Tree-sitter](https://github.com/tree-sitter/tree-sitter).

---

### Installation

On macOS, Linux, and Windows you can [download Zed directly](https://zed.dev/download) or install Zed via your local package manager ([macOS](https://zed.dev/docs/installation#macos)/[Linux](https://zed.dev/docs/linux#installing-via-a-package-manager)/[Windows](https://zed.dev/docs/windows#package-managers)).

Other platforms are not yet available:

- Web ([tracking discussion](https://github.com/zed-industries/zed/discussions/26195))

### Developing Zed

- [Building Zed for macOS](./docs/src/development/macos.md)
- [Building Zed for Linux](./docs/src/development/linux.md)
- [Building Zed for Windows](./docs/src/development/windows.md)

### Contributing

See [CONTRIBUTING.md](./CONTRIBUTING.md) for ways you can contribute to Zed.

Also... we're hiring! Check out our [jobs](https://zed.dev/jobs) page for open roles.

### Licensing

Zed source code is licensed primarily under GPL-3.0-or-later, with Apache-2.0 components where marked.

License information for third party dependencies must be correctly provided for CI to pass.

We use [`cargo-about`](https://github.com/EmbarkStudios/cargo-about) to automatically comply with open source licenses. If CI is failing, check the following:

- Is it showing a `no license specified` error for a crate you've created? If so, add `publish = false` under `[package]` in your crate's Cargo.toml.
- Is the error `failed to satisfy license requirements` for a dependency? If so, first determine what license the project has and whether this system is sufficient to comply with this license's requirements. If you're unsure, ask a lawyer. Once you've verified that this system is acceptable add the license's SPDX identifier to the `accepted` array in `script/licenses/zed-licenses.toml`.
- Is `cargo-about` unable to find the license for a dependency? If so, add a clarification field at the end of `script/licenses/zed-licenses.toml`, as specified in the [cargo-about book](https://embarkstudios.github.io/cargo-about/cli/generate/config.html#crate-configuration).

## Sponsorship

Zed is developed by **Zed Industries, Inc.**, a for-profit company.

If you’d like to financially support the project, you can do so via GitHub Sponsors.
Sponsorships go directly to Zed Industries and are used as general company revenue.
There are no perks or entitlements associated with sponsorship.
