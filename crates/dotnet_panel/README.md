# dotnet_panel

A left-dock panel for working with .NET projects: SDK detection, project
discovery, NuGet packages and running processes.

## Why it exists

It gives a .NET solution the same quick overview and one-click commands the
Node and Python panels provide — which SDK is installed, which projects are
in the workspace, what is outdated or vulnerable. Ported from Forge.

## Using it

Open it from the status bar icon (tooltip "Dotnet") or run
`dotnet panel: toggle focus` from the command palette. There is no default
key binding.

| Section | What it does |
| --- | --- |
| Projects | `.csproj` projects found under the workspace; pick the active one |
| Quick Actions | Common `dotnet` commands (run, restore, …) plus **NuGet Manager** |
| Packages | Packages the active project references |
| Outdated | Packages with newer versions, with **Update all (patch)** / **Update all (minor)** |
| Vulnerabilities | Findings from `dotnet list package --vulnerable` |
| Processes | Running `dotnet` processes |

Useful to know:

- Quick actions stream into the **Script Runner** panel. If that panel has
  not loaded yet, the command still runs, silently in the background.
- **NuGet Manager** opens the NuGet Manager tab
  ([`nuget_manager_panel`](../nuget_manager_panel/README.md)) for search,
  install, update and remove.

## How it is wired

- `dotnet_panel::init(cx)` registers `dotnet_panel::ToggleFocus`.
- `DotNetPanel::load(workspace, cx)` is awaited in `initialize_panels`
  (`crates/zed/src/zed.rs`).
- Implements `workspace::dock::Panel`, docked left by default (the
  `dotnet_panel.dock` setting moves it left or right; `dotnet_panel.button` hides
  its status bar button), `activation_priority() = 27`.

Unlike the Node and Python panels, this one is not handed a script-runner
handle at startup. It finds the Script Runner through the workspace when an
action is triggered.

### Read-only accessors

`dashboard_panel` reads state through public getters: `dotnet_version()`,
`detected_projects()`, `outdated_count()`, `vulnerable_count()`,
`vulnerable_findings()`, `vulnerable_loading()`, `vulnerable_error()`,
`active_project_label()` and `rescan_vulnerabilities(cx)`.

## Where the logic lives

| Concern | Crate |
| --- | --- |
| SDK detection, project scan, package and CLI JSON parsing | [`dotnet_backend`](../dotnet_backend/README.md) |
| Registry search and package management UI | [`nuget_manager_panel`](../nuget_manager_panel/README.md) |
| Running commands | [`script_runner_panel`](../script_runner_panel/README.md) |

The process list is this panel's own `sysinfo` poll filtered to `dotnet`
process names.

## Development

```sh
cargo check -p dotnet_panel -j 8
cargo test -p dotnet_panel -j 8
```

The tests cover the bulk-update command, including that it refuses package
ids or versions containing shell syntax.

To run the tests of every fork crate at once: `script/test-fork-crates.ps1`.
