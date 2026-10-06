# cockpit_panel

A left-dock sidebar showing live system metrics for the machine the editor is
running on.

## Why it exists

Ported from the Forge app (`E:\Forge_GPUI`) so this fork has an at-a-glance
view of CPU, memory, disk and network load without leaving the editor. It was
also the first panel ported onto `gpui_component`, so it is the reference for
how every later port is wired into Zed's dock — see
[`crates/gpui_component/PORTING.md`](../gpui_component/PORTING.md).

## Using it

Open it from its icon in the status bar (tooltip "Cockpit"), or run
`cockpit panel: toggle focus` from the command palette. There is no default
key binding.

The panel has a header (logical core count, uptime, and a **Dashboard**
button) followed by collapsible sections:

| Section | Shows |
| --- | --- |
| Metrics | Circular CPU and RAM gauges |
| Cores | One usage bar per logical core |
| Disks | Usage per mounted disk |
| History | Area chart of recent CPU/RAM samples |
| Network | Area chart of recent network throughput |

Readings refresh every 2 seconds. History and Network keep the last 30
samples (about one minute); `sysinfo` only reports the current reading, so
the panel builds the history itself.

The **Dashboard** button opens the cross-ecosystem Dashboard tab — see
[`dashboard_panel`](../dashboard_panel/README.md).

## How it is wired

- `cockpit_panel::init(cx)` is called from `fn main()` in
  `crates/zed/src/main.rs` and registers the `ToggleFocus` action on every
  workspace.
- `CockpitPanel::load(workspace, cx)` is awaited in `initialize_panels`
  (`crates/zed/src/zed.rs`) and the result is added to the dock.
- It implements `workspace::dock::Panel`, fixed to the left dock, with
  `activation_priority() = 4`.

### Actions

| Action | Effect |
| --- | --- |
| `cockpit_panel::ToggleFocus` | Show/focus the panel |
| `cockpit_panel::OpenDashboard` | Dispatched by the header button; handled by `dashboard_panel::init` |

`OpenDashboard` is declared here but handled in `dashboard_panel` on purpose:
the Dashboard embeds this panel, so a direct call the other way would be a
dependency cycle.

### Embedding

`CockpitPanel::new_embedded(source, cx)` builds a second view that mirrors an
existing panel's data without starting another `sysinfo` poller and without
the header's Dashboard button. The Dashboard tab uses this for its "System"
section.

## Layout

| File | Contents |
| --- | --- |
| `src/cockpit_panel.rs` | Panel state, polling task, `Panel` impl, actions |
| `src/header.rs` | Core count / uptime header and the Dashboard button |
| `src/system_metrics.rs` | The Metrics, Cores, Disks, History and Network sections |

## Development

```sh
cargo check -p cockpit_panel -j 8
cargo test -p cockpit_panel -j 8
```

The tests cover the uptime, byte-size and network-rate formatting.

To try it in the running app, launch with `script/run-isolated.ps1`.

To run the tests of every fork crate at once: `script/test-fork-crates.ps1`.
