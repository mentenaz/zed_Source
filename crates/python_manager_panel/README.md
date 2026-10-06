# python_manager_panel

The Python Package Manager: a workspace tab for looking packages up on PyPI
and installing, updating and uninstalling them.

## Why it exists

It completes the set alongside the npm and NuGet managers, giving Python
projects the same full-width package view — installed, outdated and
vulnerable packages, plus details and READMEs from PyPI.

## Using it

Open with `ctrl-k y` (`cmd-k y` on macOS), the **Package Manager** quick
action in the Python panel, or the `python_manager::OpenPythonManager` action
from the command palette.

Five pages in the left sidebar:

| Page | What it does |
| --- | --- |
| General | Interpreter and environment details |
| Search | Look a package up on PyPI by **exact name**; **Install** |
| Installed | Installed packages; **Uninstall** |
| Updates | Outdated packages; **Update** and **Update All** |
| Vulnerabilities | Known vulnerabilities (PyPI / OSV.dev); **Fix** when the advisory offers one |

Selecting a package opens a details pane with its metadata and **README**.

| Key (package list focused) | Action |
| --- | --- |
| `up` / `down` | Move the selection |
| `enter` | Open the selected package (toggles the row on Vulnerabilities) |
| `space` | Run the page's primary action (Uninstall / Update / Fix) |

Commands run in the **Script Runner** panel so you can watch the output.

If a package name or version contains anything other than letters, digits and
a few punctuation characters, the command is not run and the reason is shown
in the tab instead. Real registry names never trip this; it exists so that a
malformed or hostile value cannot be run as part of a shell command. See
[`script_runner_panel`](../script_runner_panel/README.md#building-commands-safely).

### Two PyPI limitations that shape the UI

- **Search is exact-name only.** PyPI removed its full-text search API in
  2018; only `https://pypi.org/pypi/<name>/json` remains. Typing `requests`
  looks up the `requests` package. There is no fuzzy search to offer.
- **Vulnerability scanning is on demand.** There is no single audit
  endpoint, so a scan sends one request per installed package (with a
  concurrency cap, `VULN_SCAN_CONCURRENCY`). It runs when you press the
  button, not on every reload.

Both match the Forge original; neither is a shortcut taken during the port.

## How it is wired

- `python_manager_panel::init(cx)` registers the actions and the list key
  bindings (context `PythonPackageList`).
- `python_manager_panel::open(workspace, window, cx)` is the programmatic
  entry point.
- `PythonManagerPanel` is a `workspace::Item` and implements
  `SerializableItem`.

### Actions

All in the `python_manager` namespace: `OpenPythonManager`,
`SelectNextPackage`, `SelectPrevPackage`, `OpenSelectedPackage`,
`ActSelectedPackage`.

### Shared API

`fetch_all_vulnerabilities` is public and reused by
[`python_panel`](../python_panel/README.md) for its own Vulnerabilities
section, so the scan logic exists once.

## Layout

| File | Contents |
| --- | --- |
| `src/python_manager_panel.rs` | The `Item`, actions, running commands, vulnerability scan |
| `src/pages.rs` | The five `SettingPage`s |
| `src/details.rs` | The details pane |

Detection and PyPI JSON parsing live in
[`python_backend`](../python_backend/README.md). The UI follows
`gpui_component::setting`, the same as the npm and NuGet managers.

## Development

```sh
cargo check -p python_manager_panel -j 8
cargo test -p python_manager_panel -j 8
```

The tests cover the pip install, uninstall and update commands (including
refusing names or versions containing shell syntax) and list navigation.

To run the tests of every fork crate at once: `script/test-fork-crates.ps1`.
