//! The action registry — `DOCS/forge-workflow-engine-design.md` §2's "one
//! central abstraction." One `ActionDef` per `type_id` drives the canvas
//! form, the execution engine, and JSONLogic completion simultaneously.
//! Adding an action type is exactly one new entry in [`REGISTRY`] — never a
//! change to [`super::schema::Action`], the load/save adapter, or any
//! exhaustive `match`. If a future action type ever seems to need a Rust
//! code change beyond an entry here, that's a sign it's being modeled
//! wrong, not a sign this registry needs restructuring.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionCategory {
    /// A single unit of work — the common case.
    Leaf,
    /// Has its own nested `actions` map (`Foreach`/`Until`/`If`/`Try`) —
    /// the canvas renders these as `gpui-flow` container nodes
    /// (`FlowNode::parent`/`container_size`), not leaf boxes.
    Container,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldKind {
    String,
    Number,
    Bool,
    /// Arbitrary JSON — an escape hatch for inputs no simpler kind fits
    /// (e.g. `script`'s `source`, `http`'s `body`).
    Json,
    /// A JSONLogic expression specifically — the canvas should offer
    /// expression-completion (referencing other actions' `outputs`) for a
    /// field of this kind, not a plain text/JSON editor.
    Expression,
}

#[derive(Debug, Clone, Copy)]
pub struct InputField {
    pub name: &'static str,
    pub kind: FieldKind,
    pub required: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct OutputField {
    pub name: &'static str,
    pub kind: FieldKind,
}

#[derive(Debug, Clone, Copy)]
pub struct ActionDef {
    pub type_id: &'static str,
    pub label: &'static str,
    pub category: ActionCategory,
    pub inputs: &'static [InputField],
    pub outputs: &'static [OutputField],
}

const fn input(name: &'static str, kind: FieldKind, required: bool) -> InputField {
    InputField {
        name,
        kind,
        required,
    }
}
const fn output(name: &'static str, kind: FieldKind) -> OutputField {
    OutputField { name, kind }
}

/// Every action type this engine currently knows about. `type_id` here is
/// exactly the string that appears as `Action.type_id` (`"type"` in JSON) —
/// the schema keeps that field an open string on purpose (see
/// `schema.rs`'s module doc), this table is what actually closes it at the
/// application layer.
pub static REGISTRY: &[ActionDef] = &[
    // ── Leaf actions carried over from every worked example so far ──
    ActionDef {
        type_id: "http",
        label: "HTTP Request",
        category: ActionCategory::Leaf,
        inputs: &[
            input("method", FieldKind::String, true),
            input("url", FieldKind::String, true),
            input("body", FieldKind::Json, false),
        ],
        outputs: &[
            output("body", FieldKind::Json),
            output("status", FieldKind::Number),
        ],
    },
    ActionDef {
        type_id: "transform",
        label: "Transform",
        category: ActionCategory::Leaf,
        // Was a free-text `strategy: String` with no defined behavior
        // anywhere — every worked example (`"normalize"`/`"discount"`/etc.)
        // was authored purely to exercise the canvas renderer, before
        // execution was ever in scope. Execution-engine design doc §4
        // (decided 2026-09-10): evaluate `expression` against the current
        // `RunContext`, store the result as `outputs.result` — fully
        // generic, no Forge code needed per new transform. Existing flows
        // authored against the old `strategy` field keep that key as an
        // unrecognized extra `inputs` entry (harmless, just not shown by
        // the typed form) until hand-migrated to a real `expression`.
        inputs: &[input("expression", FieldKind::Expression, true)],
        outputs: &[output("result", FieldKind::Json)],
    },
    ActionDef {
        type_id: "script",
        label: "Script",
        category: ActionCategory::Leaf,
        // `source` is a `ScriptSource` (inline/file, §4) — represented as
        // one JSON-kind field here since the registry's `FieldKind` enum
        // doesn't need its own "discriminated union" case just for this;
        // the canvas's script editor already knows to special-case it.
        // `args`, if present, is serialized to JSON and piped to the
        // process's stdin (§4's data contract) — optional, since plenty of
        // scripts don't need any input at all.
        inputs: &[
            input("source", FieldKind::Json, true),
            input("args", FieldKind::Json, false),
        ],
        outputs: &[output("stdout_json", FieldKind::Json)],
    },
    ActionDef {
        type_id: "delay",
        label: "Delay",
        category: ActionCategory::Leaf,
        inputs: &[input("ms", FieldKind::Number, true)],
        outputs: &[],
    },
    ActionDef {
        type_id: "notify",
        label: "Notify",
        category: ActionCategory::Leaf,
        // Webhook-based (decided 2026-09-12): POSTs a Slack/Mattermost-
        // compatible JSON body (`{"channel": ..., "text": ...}` — that
        // shape is also readable by a generic webhook receiver, not just
        // those two specifically) to `url`. `channel` is optional since
        // the webhook URL itself often already implies the destination;
        // when a real webhook supports Slack's channel-override
        // convention, this sets it.
        inputs: &[
            input("url", FieldKind::String, true),
            input("channel", FieldKind::String, false),
            input("message", FieldKind::String, true),
        ],
        outputs: &[output("status", FieldKind::Number)],
    },
    ActionDef {
        type_id: "sendEmail",
        label: "Send Email",
        category: ActionCategory::Leaf,
        inputs: &[
            input("to", FieldKind::String, true),
            input("subject", FieldKind::String, true),
            input("body", FieldKind::String, true),
        ],
        outputs: &[],
    },
    ActionDef {
        type_id: "log",
        label: "Log",
        category: ActionCategory::Leaf,
        inputs: &[
            input("level", FieldKind::String, true),
            input("message", FieldKind::String, true),
        ],
        outputs: &[],
    },
    // ── Long-running / detached processes ──
    ActionDef {
        type_id: "StartProcess",
        label: "Start Process",
        category: ActionCategory::Leaf,
        // Unlike every other leaf, this resolves `Succeeded` the moment
        // the process *starts*, not when it exits — for a dev server
        // (`npm run dev`, `uvicorn ...`) that never exits on its own.
        // `args` is a JSON array of strings, not a single string (a shell
        // command line has quoting ambiguity this avoids entirely — see
        // `execute_start_process`'s doc comment). Pair with `WaitForPort`
        // (`runAfter` this action's own id) to confirm it actually came
        // up. The spawned child is tracked in a dedicated in-process
        // registry (`process_tracker`) so it can be listed/stopped from
        // the Designer's own "Processes" panel — deliberately *not* routed
        // through `ScriptRunner`, which is a different, single-concurrency
        // lifecycle model built around commands that are expected to exit.
        inputs: &[
            input("command", FieldKind::String, true),
            input("args", FieldKind::Json, false),
            input("cwd", FieldKind::String, false),
            // Optional — a dev-server URL to surface in the Processes
            // panel with a copy-to-clipboard button (e.g.
            // `http://localhost:5173`). Purely informational: nothing
            // here validates or connects to it. `task_chain::build`
            // fills this in automatically from the detected/entered
            // port; a hand-authored `StartProcess` action can set it
            // too via the typed property form.
            input("url", FieldKind::String, false),
        ],
        outputs: &[
            output("pid", FieldKind::Number),
            output("processId", FieldKind::String),
        ],
    },
    // ── Legacy Tauri Task Chain `StepKind` variants, folded in for a
    // lossless §5 migration — found by reading the real `chain.rs`, not
    // present in any example prior to that review. ──
    ActionDef {
        type_id: "WaitForPort",
        label: "Wait For Port",
        category: ActionCategory::Leaf,
        inputs: &[
            input("port", FieldKind::Number, true),
            input("timeout_ms", FieldKind::Number, false),
        ],
        outputs: &[output("ready", FieldKind::Bool)],
    },
    ActionDef {
        type_id: "DbMigration",
        label: "DB Migration",
        category: ActionCategory::Leaf,
        inputs: &[
            input("sql_file", FieldKind::String, true),
            input("connection_id", FieldKind::String, true),
        ],
        // The old chain.rs implementation is itself a stub pending DB-panel
        // connection context — carrying the type forward doesn't finish it.
        outputs: &[],
    },
    ActionDef {
        type_id: "SetEnv",
        label: "Set Environment Variable",
        category: ActionCategory::Leaf,
        inputs: &[
            input("key", FieldKind::String, true),
            input("value", FieldKind::String, true),
        ],
        outputs: &[],
    },
    ActionDef {
        type_id: "AiCheck",
        label: "AI Check",
        category: ActionCategory::Leaf,
        inputs: &[input("prompt", FieldKind::String, true)],
        // Old chain.rs implementation is a stub too ("AI check (stub): ...")
        // — wiring to the real AI Twin backend is separate, unstarted work.
        outputs: &[output("result", FieldKind::String)],
    },
    // ── Container constructs (§3/§3a) ──
    ActionDef {
        type_id: "Foreach",
        label: "Foreach",
        category: ActionCategory::Container,
        inputs: &[input("foreach", FieldKind::Expression, true)],
        outputs: &[],
    },
    ActionDef {
        type_id: "Until",
        label: "Until",
        category: ActionCategory::Container,
        inputs: &[
            input("until", FieldKind::Expression, true),
            input("limit", FieldKind::Json, true), // mandatory — §3
        ],
        outputs: &[],
    },
    ActionDef {
        type_id: "If",
        label: "If",
        category: ActionCategory::Container,
        inputs: &[input("expression", FieldKind::Expression, true)],
        outputs: &[],
    },
    ActionDef {
        type_id: "Try",
        label: "Try/Catch",
        category: ActionCategory::Container,
        inputs: &[],
        outputs: &[output("error", FieldKind::Json)], // {actionId, message} inside catch — §3a
    },
];

/// Look up a registered action type by id. `None` means the canvas/engine
/// don't recognize `type_id` — a real error condition for the loader to
/// surface, not something to silently no-op past.
pub fn find(type_id: &str) -> Option<&'static ActionDef> {
    REGISTRY.iter().find(|d| d.type_id == type_id)
}

pub fn is_container(type_id: &str) -> bool {
    matches!(find(type_id), Some(d) if d.category == ActionCategory::Container)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_type_id_is_unique() {
        let mut seen = std::collections::HashSet::new();
        for def in REGISTRY {
            assert!(
                seen.insert(def.type_id),
                "duplicate type_id: {}",
                def.type_id
            );
        }
    }

    #[test]
    fn container_types_match_the_schema() {
        for t in ["Foreach", "Until", "If", "Try"] {
            assert!(is_container(t), "{t} should be a Container");
        }
        for t in ["http", "transform", "script", "log"] {
            assert!(!is_container(t), "{t} should be a Leaf");
        }
    }

    #[test]
    fn find_returns_none_for_unknown_type() {
        assert!(find("NotARealType").is_none());
    }
}
