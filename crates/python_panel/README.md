# python_panel

A left-dock panel for working with Python projects: interpreter and
environment details, packages, framework detection and running processes.

## Why it exists

It gives a Python project the same at-a-glance treatment the Node and .NET
panels give theirs — which interpreter is active, what is installed, what is
outdated or vulnerable, and one-click commands to run the project. Ported
from Forge and then brought up to the same level of detail as `dotnet_panel`.

## Using it

Open with `ctrl-k i` (`cmd-k i` on macOS), the status bar icon (tooltip
"Python"), or `python panel: toggle focus`.

| Section | What it does |
| --- | --- |
| Projects | Python projects found under the workspace; pick the active one |
| Environment | Interpreter, pip and virtual-environment details |
| Framework | The detected web framework and entry point, if any |
| Quick Actions | Run commands for the project, plus **Package Manager** |
| Packages | Installed packages |
| Outdated | Packages with newer versions, with **Update all (patch)** / **Update all (minor)** |
| Vulnerabilities | Known advisories for installed packages |
| Processes | Running Python processes |

Useful to know:

- Quick actions run in the **Script Runner** panel, which opens and streams
  the output.
- **Package Manager** opens the Python Package Manager tab
  ([`python_manager_panel`](../python_manager_panel/README.md)).
- **Vulnerability scanning is on demand.** PyPI has no bulk audit endpoint,
  so a scan is one HTTP request per installed package. It runs when you ask
  for it, not on every project switch.

## How it is wired

- `python_panel::init(cx)` registers `python_panel::ToggleFocus`.
- `PythonPanel::load(workspace, cx)` is awaited in `initialize_panels`
  (`crates/zed/src/zed.rs`), which then calls
  `set_script_runner(WeakEntity<ScriptRunnerPanel>)` once both panels exist.
- Implements `workspace::dock::Panel`, fixed to the left dock,
  `activation_priority() = 10`.

### Read-only accessors

`dashboard_panel` reads state through public getters: `python_version()`,
`detected_projects()`, `outdated_count()`, `vulnerable_count()`,
`vulnerabilities()`, `vulnerabilities_scanned()`,
`vulnerabilities_scanning()`, `vulnerabilities_scan_error()`,
`active_project_label()` and `rescan_vulnerabilities(cx)`.

## Where the logic lives

| Concern | Crate |
| --- | --- |
| Detection, project scan, package and PyPI parsing | [`python_backend`](../python_backend/README.md) |
| Vulnerability fetching (`fetch_all_vulnerabilities`) | [`python_manager_panel`](../python_manager_panel/README.md) |
| Running commands | [`script_runner_panel`](../script_runner_panel/README.md) |

The vulnerability fetcher is reused from the manager crate rather than
duplicated; this panel already depends on it for the Package Manager button.

## Development

```sh
cargo check -p python_panel -j 8
cargo test -p python_panel -j 8
```

The tests cover the bulk-update command, including that it refuses names or
versions containing shell syntax.

To run the tests of every fork crate at once: `script/test-fork-crates.ps1`.
