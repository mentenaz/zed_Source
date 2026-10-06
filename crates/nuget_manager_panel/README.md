# nuget_manager_panel

The NuGet Manager: a workspace tab for searching nuget.org and installing,
updating and removing packages in a .NET project.

## Why it exists

It is the .NET counterpart to the Node Package Manager — a full-width place
to browse the registry, read a package's README and manage references,
instead of editing `.csproj` files by hand or running `dotnet add package`
from memory.

## Using it

Open with `ctrl-k g` (`cmd-k g` on macOS), the **NuGet Manager** quick action
in the .NET panel, or the `nuget_manager::OpenNuGetManager` action from the
command palette.

Five pages in the left sidebar:

| Page | What it does |
| --- | --- |
| General | The project the tab is operating on |
| Search | Search nuget.org; **Install** from results, **Load more** to page |
| Installed | Referenced packages; **Remove** |
| Updates | Outdated packages; **Update** |
| Vulnerabilities | Known vulnerabilities from `dotnet list package --vulnerable` |

Selecting a package opens a resizable details pane on the right with
versions, dependency groups and the **README**.

| Key (package list focused) | Action |
| --- | --- |
| `up` / `down` | Move the selection |
| `enter` | Open details for the selected package |
| `space` | Run the page's primary action (Install / Remove / Update) |

Commands run in the **Script Runner** panel so you can watch the output.

If a package name or version contains anything other than letters, digits and
a few punctuation characters, the command is not run and the reason is shown
in the tab instead. Real registry names never trip this; it exists so that a
malformed or hostile value cannot be run as part of a shell command. See
[`script_runner_panel`](../script_runner_panel/README.md#building-commands-safely).

### Single-project by design

Unlike the npm manager, this tab works on **one** project: the first
`.csproj` found under the workspace. `dotnet list package` rejects a
directory containing more than one project file, so there is no aggregate
view. Commands run in that `.csproj`'s parent directory, which is how
`dotnet add package` decides which project to modify.

If the workspace has no `.csproj`, the tab shows a notice with **Rescan**
rather than an empty package list.

The Vulnerabilities page lists findings but offers no per-row fix; `space`
does nothing there.

## How it is wired

- `nuget_manager_panel::init(cx)` registers the actions and the list key
  bindings (context `NuGetPackageList`).
- `nuget_manager_panel::open(workspace, window, cx)` is the programmatic
  entry point.
- `NuGetManagerPanel` is a `workspace::Item` and implements
  `SerializableItem`.

### Actions

All in the `nuget_manager` namespace: `OpenNuGetManager`,
`ReloadNuGetManager`, `SelectNextPackage`, `SelectPrevPackage`,
`OpenSelectedPackage`, `ActSelectedPackage`.

## Layout

| File | Contents |
| --- | --- |
| `src/nuget_manager_panel.rs` | The `Item`, actions, running commands |
| `src/pages.rs` | The five `SettingPage`s |
| `src/search.rs` | HTTP calls to the NuGet search, registration and flat-container APIs |
| `src/details.rs` | The right-hand details pane |

CLI invocation and all JSON/XML parsing live in
[`dotnet_backend`](../dotnet_backend/README.md). The UI follows
`gpui_component::setting` (`Settings` / `SettingPage` / `SettingGroup`), the
same as the npm manager.

## Development

```sh
cargo check -p nuget_manager_panel -j 8
cargo test -p nuget_manager_panel -j 8
```

The tests cover the install, remove and "Update all" commands (including
refusing ids or versions containing shell syntax) and list navigation.

To run the tests of every fork crate at once: `script/test-fork-crates.ps1`.
