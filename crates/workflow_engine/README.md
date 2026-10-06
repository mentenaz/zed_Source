# workflow_engine

The engine that validates and runs `.flow.json` workflows. Pure data and
tokio; it has no `gpui` dependency and no UI.

## Why it exists

Flows are this fork's way of describing multi-step automation — start a
service, wait for its port, call an API, run a script, branch on the result —
as a file you can commit. This crate is the part that gives those files
meaning: the schema, the list of action types, the scheduler that works out
what can run in parallel, and the executors that do the work.

Ported out of Forge's `backend/workflow`. Keeping it headless means it can be
driven by the Flows panel, the Designer, or a test, with identical behaviour.

| Crate | Role |
| --- | --- |
| `workflow_engine` (this) | Schema, registry, scheduler, executors |
| [`flows_panel`](../flows_panel/README.md) | Lists flows; create, open, run |
| [`designer_panel`](../designer_panel/README.md) | Visual canvas editor for one flow |

## The file format

A flow is a JSON object with an `id`, a `name` and a map of `actions`. Each
action has a `type`, its `inputs`, and a `runAfter` map naming the actions it
waits for and which outcomes allow it to proceed:

```json
{
  "id": "order-processing",
  "name": "Order Processing",
  "actions": {
    "waitForApi": {
      "type": "WaitForPort",
      "inputs": { "port": 4000, "timeout_ms": 30000 },
      "runAfter": {}
    },
    "fetchOrder": {
      "type": "http",
      "inputs": { "method": "GET", "url": "https://example.com/v1/order" },
      "runAfter": { "waitForApi": ["Succeeded"] }
    }
  }
}
```

Actions with an empty `runAfter` start immediately. Actions whose
dependencies are all satisfied run together as one level.

Complete examples: `DOCS/workflow-schema/example-flow.json` and
`fixtures/order-processing.flow.json`.

### Action types

| `type` | Label | Kind |
| --- | --- | --- |
| `http` | HTTP Request | Leaf |
| `transform` | Transform | Leaf |
| `script` | Script | Leaf |
| `StartProcess` | Start Process | Leaf |
| `WaitForPort` | Wait For Port | Leaf |
| `notify` | Notify | Leaf |
| `sendEmail` | Send Email | Leaf |
| `delay` | Delay | Leaf |
| `log` | Log | Leaf |
| `SetEnv` | Set Environment Variable | Leaf |
| `Foreach` | Foreach | Container |
| `Until` | Until | Container |
| `If` | If | Container |
| `Try` | Try/Catch | Container |
| `DbMigration` | DB Migration | Leaf — **not runnable yet** |
| `AiCheck` | AI Check | Leaf — **not runnable yet** |

Containers hold their own nested `actions` map. The two stubs are registered
so they appear in the catalog, but have no executor; validation blocks a run
that contains them.

Expressions that refer to other actions' outputs are JSONLogic, evaluated
against the run's `RunContext`.

### Sidecar files

Presentation and bulky data are kept out of `<name>.flow.json` so that a git
diff of a flow shows behaviour changes only:

| Path | Holds |
| --- | --- |
| `<name>.flow.layout.json` | Canvas node positions (`layout` module) |
| `<name>.flow-http/<action>.body.json` | An `http` action's request body |
| `<name>.flow-http/<action>.body.schema.json` | Optional user schema for that body |

The `.flow-http` constants live in `schema.rs` because both the Designer
(which writes these files) and the JSON language-server wiring (which
validates them) must agree on the names.

## Using it

```rust
use workflow_engine::{WorkflowDefinition, run_workflow, validate_definition};

let def: WorkflowDefinition = serde_json::from_str(&text)?;

// Optional: collect problems to show the user before running.
let issues = validate_definition(&def);

// `status` is a workflow_engine::StatusSink: a wrapper around an optional
// Fn(&str, RunStatus, Option<String>) callback (action id, new status, detail).
// See `src/status.rs` for its constructors.
run_workflow(def, solution_root, status).await?;
```

`run_workflow` owns its arguments and returns a `Send + 'static` future, so
it can be handed to a tokio runtime. `solution_root` is only used by `script`
actions in file mode. The status callback is invoked from whichever thread is
driving the run — do not touch UI state from it directly (see
`designer_panel`'s `run_state` module for the pattern).

### API

| Area | Items |
| --- | --- |
| Schema | `WorkflowDefinition`, `Action`, `ActionMap`, `Branch`, `Limit`, `JsonLogicExpr`, `ScriptRuntime`, `ScriptSource`, `RunOutcome` |
| Registry | `ActionDef`, `ActionCategory`, `CatalogGroup`, `FieldKind`, `InputField`, `OutputField` |
| Running | `run_workflow`, `RunContext`, `execute_action`, `execute_leaf` |
| Scheduling | `compute_levels`, `validate`, `LevelResult` |
| Validation | `validate_definition`, `ValidationIssue`, `ValidationSeverity` |
| Status | `RunStatus`, `StatusSink` |
| History | `RunHistoryEntry`, `ActionHistoryRecord` |
| Layout | `WorkflowLayout` |
| Project scan | `scan_project`, `DetectedService`, `ServiceKind`, `ServiceSpec` |
| Processes | `ProcessInfo`, `ProcessStatus` |

`scan_project` detects runnable services in a workspace; `task_chain` turns a
list of `ServiceSpec`s into a flow. Together they back the Flows panel's
"Add Task Chain".

## Adding an action type

Add one `ActionDef` entry to `REGISTRY` in `src/registry.rs` and an executor
for it. That is the whole change.

`Action` is intentionally one flat struct with optional fields, not an enum
with a variant per type. The registry entry drives the canvas form, the
engine and expression completion at once, so a new type should never require
touching `schema::Action`, the load/save adapter or an exhaustive `match`. If
it seems to, the action is probably being modelled wrongly.

## Layout

| File | Contents |
| --- | --- |
| `src/schema.rs` | `flow.json` types and sidecar naming |
| `src/registry.rs` | Every action type's definition |
| `src/scheduler.rs` | Dependency levels and structural validation |
| `src/runner.rs` | `run_workflow` |
| `src/executors.rs` | Leaf executors (http, script, log, …) |
| `src/container.rs` | Container executors (Foreach, Until, If, Try) |
| `src/engine.rs` | `RunContext` and JSONLogic plumbing |
| `src/validation.rs` | User-facing validation issues |
| `src/status.rs` | Run status events |
| `src/history.rs` | Run history records |
| `src/layout.rs` | `.flow.layout.json` sidecar |
| `src/process_tracker.rs` | Tracking of started processes |
| `src/project_scan.rs`, `src/task_chain.rs` | Service detection and task-chain generation |

## Development

```sh
cargo check -p workflow_engine -j 8
cargo test -p workflow_engine -j 8
```

This crate is well covered by unit tests, most heavily the executors,
containers, validation and scheduler.

To run the tests of every fork crate at once: `script/test-fork-crates.ps1`.
