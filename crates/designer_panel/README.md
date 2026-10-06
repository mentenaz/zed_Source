# designer_panel

The workflow Designer: a tab that shows a `.flow.json` file as a node graph
you can edit, save and run.

## Why it exists

A workflow is a dependency graph, and JSON is a poor way to read one. The
Designer draws each action as a node and each `runAfter` dependency as an
edge, lets you configure actions through generated forms instead of
hand-written JSON, and shows which action is running while a flow executes.

Ported from Forge's `designer_panel`, limited to the `.flow.json` format.

## Using it

### Opening a flow

Any `.flow.json` file opens in the Designer instead of the text editor —
double-click it in the project panel, use quick-open, or click **Graph** on a
row in the Flows panel. Open Designer tabs are restored with the workspace.

A file that fails to parse still opens a tab, with the error shown in a
banner inside it.

### The three panes

| Pane | Purpose |
| --- | --- |
| Action catalog | Searchable list of every action type, grouped by function |
| Canvas | The flow graph: pan, zoom, drag nodes, connect handles |
| Inspector | Properties of the selected node |

### Adding actions

Three ways, all drawing on the same registry:

- Click a catalog row (or press Enter on it) — the action is added near the
  selected node, or near the centre of the canvas.
- Drag a catalog row onto the canvas to place it exactly.
- Right-click the canvas and choose **Add Action**.

Actions that have no executor yet (DB Migration, AI Check) are listed but
disabled.

### Toolbar

| Button | Does |
| --- | --- |
| Run | Run the flow. Disabled while blocking validation errors remain |
| Save | Write the flow and its layout |
| Fit View | Zoom to show the whole graph |
| Raw | Open the underlying JSON as text |
| Results | Open the Run Results tab for this flow |
| Clear results | Clear run results and status colours from the canvas |
| Issues badge | Show what is blocking Run |

Incomplete flows can be saved as drafts. Validation problems are shown on the
affected nodes and in the inspector, and only Run is blocked.

### Editing an action

Select a node to edit its properties in the inspector. **Delete Node**
removes it. Two input kinds get a real editor rather than a text field:

- **Scripts** — choose **Inline** or **File**.
  - Inline code is kept in a companion file at
    `<flow>.flow-scripts/<action-id>.<ext>`, so it gets full language-server
    support. **Open Script** opens it.
  - File mode runs an existing workspace file directly.
- **HTTP bodies** — **Edit Body** opens the request body as a JSON file at
  `<flow>.flow-http/<action-id>.body.json`, with JSON validation.

Both companions are synchronised with the flow in both directions, and are
ordinary files you can commit.

### Running

While a flow runs, nodes are coloured by status and a log pane shows
progress. **Results** opens a read-only Run Results tab for the most recent
run. It keeps updating even if the Designer tab that started the run is
closed.

## Files the Designer writes

| Path | Contents |
| --- | --- |
| `<name>.flow.json` | The workflow itself |
| `<name>.flow.layout.json` | Node positions |
| `<name>.flow-scripts/` | Inline script companions |
| `<name>.flow-http/` | HTTP body companions |

Node positions never go into `.flow.json`, so moving a node does not show up
as a change to the flow's behaviour. A flow with no layout file is laid out
automatically in layers.

## How it is wired

- `designer_panel::init(cx)` (from `crates/zed/src/main.rs`) registers the
  workspace-wide action handlers and calls
  `workspace::register_project_item::<DesignerPanel>` and
  `register_serializable_item::<DesignerPanel>`.
- `FlowFile` (`src/project_item.rs`) is the minimal `project::ProjectItem`
  that claims paths ending in `.flow.json`. Project item types are tried
  most-recently-registered first, and `try_open` returns `None` for anything
  else, so other file types are unaffected.
- `DesignerPanel::open(path, root, workspace, project, window, cx)` is the
  programmatic entry point, used by the Flows panel.
- `DesignerPanel` is a `workspace::Item` — one tab per flow, not a dock
  panel.

### Actions

In the `designer_panel` namespace, each carrying a path: `ViewRaw`,
`ViewScriptCompanion`, `ViewHttpBody`, `ViewRunResults`, plus
`ShowInvalidFlowToast`. They are dispatched to workspace-wide handlers
because a Designer opened through the project-item path has no workspace
handle of its own. None are bindable from a keymap (`no_json`).

`ViewRaw` builds a plain editor for the buffer directly. Going through
`open_abs_path` would route back to the Designer.

## Layout

| File | Contents |
| --- | --- |
| `src/designer_panel.rs` | The tab: canvas, toolbar, inspector, run handling |
| `src/catalog.rs` | Action catalog (a `gpui_component` list over the registry) |
| `src/workflow_json.rs` | Load/save between `flow.json` and `gpui_flow::FlowState`; auto-layout |
| `src/project_item.rs` | `FlowFile`, which routes `.flow.json` here |
| `src/script_companion.rs` | Inline script companion files |
| `src/http_companion.rs` | HTTP body companion files |
| `src/run_state.rs` | Thread-safe run state shared with Run Results |
| `src/run_results.rs` | The Run Results tab |
| `src/persistence.rs` | Restoring open Designer tabs |

Depends on [`workflow_engine`](../workflow_engine/README.md) for the schema,
registry and execution, and on [`gpui_flow`](../gpui_flow/README.md) for the
canvas.

### Implementation notes

- **If and Try have two bodies** (then/else, try/catch) that are just fields
  on one action in JSON, but need two container nodes on the canvas.
  `workflow_json::load` synthesises wrapper nodes with reserved types
  (`__branch_then`, `__branch_else`, `__branch_try`, `__branch_catch`) and
  `save` unwraps them again. They are never written to the file.
- **Run status crosses threads.** The engine's status callback fires off the
  UI thread, so it fills an `Arc<Mutex<RunState>>` that the Designer and Run
  Results tabs poll about ten times a second.
- **HTTP bodies are compared semantically**, not byte for byte, so
  reformatting the JSON does not register as a conflict. A body that is not
  valid JSON yet is stored verbatim.

## Further reading

[`DESIGNER_PANEL_TODO.md`](DESIGNER_PANEL_TODO.md) tracks the agreed UX and
script-editing work, phase by phase.

## Development

```sh
cargo check -p designer_panel -j 8
cargo test -p designer_panel -j 8
```

To run the tests of every fork crate at once: `script/test-fork-crates.ps1`.
