# processes_panel

A bottom-dock panel with a live, sortable table of every process on the
machine.

## Why it exists

Dev servers, watchers and stray interpreters started from the editor are easy
to lose track of. This panel lets you find and stop them without switching to
Task Manager, and lets you mark the ones you care about as "managed" so they
stand out from the rest.

Ported from Forge. There the managed set lived in a shared `AppState` so
other panels could adopt processes too; here it lives on the panel itself,
because nothing else needs it yet.

## Using it

Open with `ctrl-k x` (`cmd-k x` on macOS), the status bar icon (tooltip
"Processes"), or `processes panel: toggle focus`.

- **Columns** — PID, Name, CPU %, Memory, Source, Status. Click a header to
  sort.
- **Filter** — type in "Filter by name or PID…" to narrow the list.
- **Refresh** — the table refreshes every 2 seconds on its own; the button
  forces it immediately.
- **Source** — `Managed` for processes you have adopted, `External` for
  everything else.

Each row has Adopt/Release and Kill actions, also available from the
right-click menu.

| Key (table focused) | Action |
| --- | --- |
| `a` | Adopt or release the selected row |
| `delete` / `backspace` | Kill the selected row |

Kill always asks for confirmation first, because it cannot be undone.

## How it is wired

- `processes_panel::init(cx)` registers the actions and the table key
  bindings (context `DataTable`).
- `ProcessesPanel::load(workspace, cx)` is awaited in `initialize_panels`
  (`crates/zed/src/zed.rs`).
- Implements `workspace::dock::Panel`, fixed to the bottom dock,
  `activation_priority() = 9`.

### Actions

| Action | Effect |
| --- | --- |
| `processes_panel::ToggleFocus` | Show/focus the panel |
| `processes_panel::KillSelected` | Arm the kill confirmation for the selected row |
| `processes_panel::ToggleAdoptSelected` | Adopt or release the selected row |

## Implementation notes

The table is `gpui_component`'s `DataTable`: a
`TableState<ProcessTableDelegate>` drives column layout, sorting, selection
and the context menu, and the delegate decides what each cell renders. It
follows the `DataTableStory` template in `gpui_component_story`. The panel
owns its own `sysinfo::System`.

## Development

```sh
cargo check -p processes_panel -j 8
cargo test -p processes_panel -j 8
```

The tests cover filtering, sorting, the managed count, adopt/release, and
that killing an unknown process reports an error. No real process is touched.

To run the tests of every fork crate at once: `script/test-fork-crates.ps1`.
