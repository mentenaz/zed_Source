# node_panel

A left-dock panel for working with Node.js projects: runtime and NVM
versions, project discovery, scripts, packages and running processes.

## Why it exists

It puts the things you normally do in a terminal for a Node project — check
the Node version, switch it, run a script, see what is outdated — one click
away, and scoped to the project you have open. Ported from Forge.

## Using it

Open with `ctrl-k o` (`cmd-k o` on macOS), the status bar icon (tooltip
"Node"), or `node panel: toggle focus`.

The panel is a stack of collapsible sections:

| Section | What it does |
| --- | --- |
| NVM | Lists installed Node versions; switch, install or uninstall |
| Projects | Every directory with a `package.json` under the workspace; pick the active one |
| Quick Actions | Common commands for the active project, plus **Package Manager** and **Task Chain** |
| Packages | Installed dependencies |
| Outdated | Packages with newer versions, with **Update all (patch)** / **Update all (minor)** |
| Vulnerabilities | `npm audit` findings for the active project |
| Scripts | The `scripts` from `package.json`; run, add, edit or delete them |
| Processes | Running Node processes |
| SPFx | SharePoint Framework actions, shown for SPFx projects |

Useful to know:

- Running a script or quick action sends the command to the **Script Runner**
  panel, which opens and streams the output.
- **Package Manager** opens the Node Package Manager tab
  ([`npm_manager_panel`](../npm_manager_panel/README.md)).
- **Task Chain** opens the Flows panel's task-chain wizard, pre-filled with
  this project ([`flows_panel`](../flows_panel/README.md)).
- Editing scripts in the panel writes back to `package.json`.

## How it is wired

- `node_panel::init(cx)` registers `node_panel::ToggleFocus`.
- `NodePanel::load(workspace, cx)` is awaited in `initialize_panels`
  (`crates/zed/src/zed.rs`). After it and its dependencies have loaded, the
  same function calls:
  - `set_script_runner(WeakEntity<ScriptRunnerPanel>)` — where runs are sent.
  - `set_flows_panel(WeakEntity<FlowsPanel>)` — for the Task Chain action.
- Implements `workspace::dock::Panel`, docked left by default (the
  `node_panel.dock` setting moves it left or right; `node_panel.button` hides
  its status bar button), `activation_priority() = 26`.

If you add a panel that this one must talk to, wire it in that same place
rather than at construction: the panels load concurrently, so none of them
can assume another exists when it is built.

### Read-only accessors

`dashboard_panel` reads this panel's state through public getters instead of
rescanning: `node_version()`, `detected_projects()`, `outdated_count()`,
`vulnerable_count()`, `vulnerable_findings()`, `vulnerable_loading()`,
`vulnerable_error()`, `active_project_label()` and
`rescan_vulnerabilities(cx)`.

## Where the logic lives

| Concern | Crate |
| --- | --- |
| Runtime detection, NVM, project scan | [`node_backend`](../node_backend/README.md) |
| npm CLI and output parsing | [`npm_backend`](../npm_backend/README.md) |
| Running commands | [`script_runner_panel`](../script_runner_panel/README.md) |

The process list is this panel's own lightweight `sysinfo` poll filtered by
process name; Forge fed it from the Cockpit's broadcast tick, which does not
exist here.

## Development

```sh
cargo check -p node_panel -j 8
cargo test -p node_panel -j 8
```

The tests cover reading `package.json`, the bulk-update and run-script
commands (including refusing names containing shell syntax), and SPFx
detection against temporary directories.

To run the tests of every fork crate at once: `script/test-fork-crates.ps1`.
