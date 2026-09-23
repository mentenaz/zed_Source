//! Load/save adapter between `flow.json` (`workflow_engine::schema`) and
//! `gpui_flow::FlowState`. Ported from
//! `E:\Forge_GPUI\src\forge_shell\panels\designer_panel\workflow_json.rs`,
//! cut down to the workflow-JSON half only (no `.fdgn`/ForgeFlow — never
//! vendored into this tree, see `gpui_flow`/`workflow_engine`'s own module
//! docs).
//!
//! **Position data does not live in `flow.json`.** `Action::pos` is a
//! legacy stopgap field (see `workflow_engine::schema`'s doc comment on
//! it) superseded by the `.flow.layout.json` sidecar
//! (`workflow_engine::layout`, already ported) — `save` here never sets
//! `Action::pos`, only returns a path-keyed `WorkflowLayout` for the
//! caller to persist via `workflow_engine::layout::save`.
//!
//! **`If`/`Try`'s two nested bodies need a visual grouping node that has
//! no equivalent in the JSON schema at all** — `action.actions` (the
//! true/try body) and `action.else_branch`/`action.catch` (the false/catch
//! body) are just fields on one `Action`, but the canvas needs two
//! separate `FlowNode` containers side by side. [`load`] synthesizes them
//! with reserved `node_type`s (`__branch_then`, `__branch_else`,
//! `__branch_try`, `__branch_catch`); [`save`] recognizes those exact
//! strings to unwrap them back into the right `Action` field instead of
//! emitting them as a real action.

use std::collections::HashMap;

use gpui_flow::{FlowEdge, FlowNode, FlowState, HandleDef, HandlePosition};
use serde_json::Value;
use workflow_engine::{Action, ActionMap, Branch, Limit, RunOutcome, WorkflowDefinition, WorkflowLayout};

pub const BRANCH_THEN: &str = "__branch_then";
pub const BRANCH_ELSE: &str = "__branch_else";
pub const BRANCH_TRY: &str = "__branch_try";
pub const BRANCH_CATCH: &str = "__branch_catch";

pub const LEAF_SIZE: (f32, f32) = (172.0, 46.0);
const Y_STEP: f32 = 130.0;
const ROOT_X: f32 = 60.0;
const CHILD_X: f32 = 20.0;
const CHILD_Y0: f32 = 20.0;
// ── Load: WorkflowDefinition -> FlowState ──────────────────────────────

/// Build a `FlowState` from a parsed `flow.json`. Every action is expected
/// to already have a real `pos` (the caller applies the saved sidecar,
/// then auto-layout for anything still unset — see `auto_layout`) — a
/// `None` still falls back to a simple one-column placeholder stack so
/// this never panics/produces an unusable graph on its own.
///
/// `edge_color`/`failed_edge_color` are `0xRRGGBB` — the caller's job to
/// derive from the active theme (`designer_panel::hex(cx.theme()...)`),
/// so every edge (including a plain "Succeeded" one, and a continue-on-
/// failure "Always" one) gets an explicit color instead of falling
/// through to `gpui_flow`'s own hardcoded default gray — that default
/// doesn't move with the theme, since `gpui_flow` itself is vendored
/// as-is (see its own module doc: zero source rewrites).
pub fn load(def: &WorkflowDefinition, edge_color: u32, failed_edge_color: u32) -> FlowState {
    let mut nodes = Vec::new();
    let mut edges = Vec::new();
    let mut next_y = CHILD_Y0;
    walk_actions(
        &def.actions,
        None,
        ROOT_X,
        &mut next_y,
        &mut nodes,
        &mut edges,
        edge_color,
        failed_edge_color,
    );

    let mut state = FlowState::new(nodes, edges);
    state.refit_all_containers();
    state
}

fn walk_actions(
    actions: &ActionMap,
    parent: Option<&str>,
    x: f32,
    next_y: &mut f32,
    nodes: &mut Vec<FlowNode>,
    edges: &mut Vec<FlowEdge>,
    edge_color: u32,
    failed_edge_color: u32,
) {
    for (id, action) in actions {
        let (px, py) = match action.pos {
            Some((px, py)) => (px, py),
            None => {
                let y = *next_y;
                *next_y += Y_STEP;
                (x, y)
            }
        };

        let mut node = FlowNode::new(id.clone(), px, py)
            .node_type(action.type_id.clone())
            .label(action.label.clone().unwrap_or_else(|| id.clone()))
            .context_menu(true)
            .handles(vec![
                HandleDef::target(HandlePosition::Top),
                HandleDef::source(HandlePosition::Bottom),
            ]);
        if let Some(p) = parent {
            node = node.parent(p.to_string());
        }
        node.properties = flatten_action_meta(action);

        let is_container = matches!(action.type_id.as_str(), "Foreach" | "Until" | "If" | "Try");
        if !is_container {
            node = node.size(LEAF_SIZE.0, LEAF_SIZE.1);
        }
        nodes.push(node);

        for (dep_id, outcomes) in &action.run_after {
            let mut edge = FlowEdge::new(format!("e_{dep_id}_{id}"), dep_id.clone(), id.clone())
                .color(edge_color);
            let has_succeeded = outcomes.contains(&RunOutcome::Succeeded);
            let has_failed = outcomes.contains(&RunOutcome::Failed);
            if has_failed && !has_succeeded {
                edge = edge.color(failed_edge_color).label("Failed");
            } else if has_failed && has_succeeded {
                edge = edge.label("Always");
            }
            edges.push(edge);
        }

        match action.type_id.as_str() {
            "Foreach" | "Until" => {
                if let Some(body) = &action.actions {
                    let mut y = CHILD_Y0;
                    walk_actions(body, Some(id), CHILD_X, &mut y, nodes, edges, edge_color, failed_edge_color);
                }
            }
            "If" => {
                if let Some(body) = &action.actions {
                    push_branch(id, BRANCH_THEN, "True", CHILD_X, body, nodes, edges, edge_color, failed_edge_color);
                }
                if let Some(Branch { actions: body }) = &action.else_branch {
                    push_branch(
                        id,
                        BRANCH_ELSE,
                        "False",
                        CHILD_X + 260.0,
                        body,
                        nodes,
                        edges,
                        edge_color,
                        failed_edge_color,
                    );
                }
            }
            "Try" => {
                if let Some(body) = &action.actions {
                    push_branch(id, BRANCH_TRY, "Try", CHILD_X, body, nodes, edges, edge_color, failed_edge_color);
                }
                if let Some(Branch { actions: body }) = &action.catch {
                    push_branch(
                        id,
                        BRANCH_CATCH,
                        "Catch",
                        CHILD_X + 260.0,
                        body,
                        nodes,
                        edges,
                        edge_color,
                        failed_edge_color,
                    );
                }
            }
            _ => {}
        }
    }
}

fn push_branch(
    owner_id: &str,
    branch_type: &'static str,
    label: &'static str,
    x: f32,
    body: &ActionMap,
    nodes: &mut Vec<FlowNode>,
    edges: &mut Vec<FlowEdge>,
    edge_color: u32,
    failed_edge_color: u32,
) {
    let branch_id = format!("{owner_id}{branch_type}");
    let node = FlowNode::new(branch_id.clone(), x, CHILD_Y0)
        .node_type(branch_type)
        .label(label)
        .parent(owner_id.to_string())
        .handles(vec![HandleDef::source(HandlePosition::Bottom)]);
    nodes.push(node);
    let mut y = CHILD_Y0;
    walk_actions(body, Some(&branch_id), CHILD_X, &mut y, nodes, edges, edge_color, failed_edge_color);
}

/// Flatten `Action.inputs` plus the four struct-level fields that have no
/// place in `inputs` (`foreach`/`until`/`limit`/`expression`) into
/// `FlowNode.properties`. The four struct-level fields use a `__` prefix
/// reserved from real input names (`ActionDef.inputs` entries are always
/// plain identifiers — see `registry.rs`) so [`unflatten_action_meta`] can
/// tell them apart on the way back.
fn flatten_action_meta(action: &Action) -> Vec<(gpui::SharedString, gpui::SharedString)> {
    let mut props: Vec<(gpui::SharedString, gpui::SharedString)> = action
        .inputs
        .iter()
        .map(|(k, v)| (k.clone().into(), value_to_string(v).into()))
        .collect();
    if let Some(expr) = &action.foreach {
        props.push(("__foreach".into(), value_to_string(expr).into()));
    }
    if let Some(expr) = &action.until {
        props.push(("__until".into(), value_to_string(expr).into()));
    }
    if let Some(limit) = &action.limit {
        props.push(("__limit_count".into(), limit.count.to_string().into()));
        if let Some(t) = &limit.timeout {
            props.push(("__limit_timeout".into(), t.clone().into()));
        }
    }
    if let Some(expr) = &action.expression {
        props.push(("__expression".into(), value_to_string(expr).into()));
    }
    props
}

fn value_to_string(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn string_to_value(s: &str) -> Value {
    serde_json::from_str(s).unwrap_or_else(|_| Value::String(s.to_string()))
}

// ── Save: FlowState -> WorkflowDefinition ──────────────────────────────

/// Builds both the executable `WorkflowDefinition` (no `pos` fields — see
/// the module doc) and a `WorkflowLayout` capturing every node's current
/// canvas position, path-keyed so the same id at different nesting scopes
/// can't collide. No file I/O — the caller persists both.
pub fn save(state: &FlowState, id: impl Into<String>, name: impl Into<String>) -> (WorkflowDefinition, WorkflowLayout) {
    let mut positions = HashMap::new();
    let actions = build_actions_map(state, None, "", &mut positions);
    (
        WorkflowDefinition {
            id: id.into(),
            name: name.into(),
            actions,
            outputs: Default::default(),
        },
        WorkflowLayout { positions },
    )
}

fn build_actions_map(
    state: &FlowState,
    parent: Option<&str>,
    path_prefix: &str,
    positions: &mut HashMap<String, (f32, f32)>,
) -> ActionMap {
    let mut map = ActionMap::new();
    for node in &state.nodes {
        let node_parent = node.parent_id.as_deref();
        if node_parent != parent {
            continue;
        }
        let type_id = node.node_type.as_deref().unwrap_or("transform");
        if is_branch_wrapper(type_id) {
            continue;
        }

        let id = node.id.to_string();
        let mut action = Action::new(type_id);
        action.label = Some(node.label.to_string());
        positions.insert(format!("{path_prefix}{id}"), (node.position.x, node.position.y));
        let (inputs, foreach, until, limit, expression) = unflatten_action_meta(&node.properties);
        action.inputs = inputs;
        action.foreach = foreach;
        action.until = until;
        action.limit = limit;
        action.expression = expression;
        action.run_after = build_run_after(state, &node.id);

        match type_id {
            "Foreach" | "Until" => {
                let child_prefix = format!("{path_prefix}{id}/");
                action.actions = Some(build_actions_map(state, Some(&id), &child_prefix, positions));
            }
            "If" => {
                let then_id = format!("{id}{BRANCH_THEN}");
                let else_id = format!("{id}{BRANCH_ELSE}");
                let then_prefix = format!("{path_prefix}{id}/then/");
                action.actions = Some(build_actions_map(state, Some(&then_id), &then_prefix, positions));
                if state.get_node(&else_id.clone().into()).is_some() {
                    let else_prefix = format!("{path_prefix}{id}/else/");
                    action.else_branch = Some(Branch {
                        actions: build_actions_map(state, Some(&else_id), &else_prefix, positions),
                    });
                }
            }
            "Try" => {
                let try_id = format!("{id}{BRANCH_TRY}");
                let catch_id = format!("{id}{BRANCH_CATCH}");
                let try_prefix = format!("{path_prefix}{id}/try/");
                action.actions = Some(build_actions_map(state, Some(&try_id), &try_prefix, positions));
                if state.get_node(&catch_id.clone().into()).is_some() {
                    let catch_prefix = format!("{path_prefix}{id}/catch/");
                    action.catch = Some(Branch {
                        actions: build_actions_map(state, Some(&catch_id), &catch_prefix, positions),
                    });
                }
            }
            _ => {}
        }

        map.insert(id, action);
    }
    map
}

fn is_branch_wrapper(type_id: &str) -> bool {
    matches!(type_id, BRANCH_THEN | BRANCH_ELSE | BRANCH_TRY | BRANCH_CATCH)
}

/// Reconstruct `runAfter` for `node_id` from its *incoming* edges.
///
/// **Known lossy edge**: an edge labeled `"Failed"` always round-trips to
/// `["Failed"]`, `"Always"` to `["Succeeded","Failed"]`, anything else to
/// `["Succeeded"]` — this recovers exactly the three shapes `load` ever
/// produces, but a hand-edited `flow.json` using some other outcome
/// combination (e.g. `["Skipped"]` alone) won't round-trip through the
/// canvas unchanged.
fn build_run_after(state: &FlowState, node_id: &gpui_flow::NodeId) -> indexmap::IndexMap<String, Vec<RunOutcome>> {
    let mut run_after = indexmap::IndexMap::new();
    for edge in &state.edges {
        if edge.target != *node_id {
            continue;
        }
        let outcomes = match edge.label.as_deref() {
            Some("Failed") => vec![RunOutcome::Failed],
            Some("Always") => vec![RunOutcome::Succeeded, RunOutcome::Failed],
            _ => vec![RunOutcome::Succeeded],
        };
        run_after.insert(edge.source.to_string(), outcomes);
    }
    run_after
}

fn unflatten_action_meta(
    properties: &[(gpui::SharedString, gpui::SharedString)],
) -> (
    serde_json::Map<String, Value>,
    Option<Value>,
    Option<Value>,
    Option<Limit>,
    Option<Value>,
) {
    let mut inputs = serde_json::Map::new();
    let mut foreach = None;
    let mut until = None;
    let mut limit_count: Option<u32> = None;
    let mut limit_timeout: Option<String> = None;
    let mut expression = None;

    for (k, v) in properties {
        match k.as_ref() {
            "__foreach" => foreach = Some(string_to_value(v)),
            "__until" => until = Some(string_to_value(v)),
            "__limit_count" => limit_count = v.parse().ok(),
            "__limit_timeout" => limit_timeout = Some(v.to_string()),
            "__expression" => expression = Some(string_to_value(v)),
            key => {
                inputs.insert(key.to_string(), string_to_value(v));
            }
        }
    }

    let limit = limit_count.map(|count| Limit {
        count,
        timeout: limit_timeout,
    });
    (inputs, foreach, until, limit, expression)
}

// ── The `.flow.layout.json` sidecar bridge ─────────────────────────────

/// Applies a loaded sidecar's positions onto `def`, path-keyed the same
/// way `save` produces them.
pub fn apply_saved_layout(def: &mut WorkflowDefinition, layout: &WorkflowLayout) {
    apply_layout_to_map(&mut def.actions, "", layout);
}

fn apply_layout_to_map(actions: &mut ActionMap, path_prefix: &str, layout: &WorkflowLayout) {
    for (id, action) in actions.iter_mut() {
        if let Some(&pos) = layout.positions.get(&format!("{path_prefix}{id}")) {
            action.pos = Some(pos);
        }
        match action.type_id.as_str() {
            "Foreach" | "Until" => {
                if let Some(body) = &mut action.actions {
                    apply_layout_to_map(body, &format!("{path_prefix}{id}/"), layout);
                }
            }
            "If" => {
                if let Some(body) = &mut action.actions {
                    apply_layout_to_map(body, &format!("{path_prefix}{id}/then/"), layout);
                }
                if let Some(branch) = &mut action.else_branch {
                    apply_layout_to_map(&mut branch.actions, &format!("{path_prefix}{id}/else/"), layout);
                }
            }
            "Try" => {
                if let Some(body) = &mut action.actions {
                    apply_layout_to_map(body, &format!("{path_prefix}{id}/try/"), layout);
                }
                if let Some(branch) = &mut action.catch {
                    apply_layout_to_map(&mut branch.actions, &format!("{path_prefix}{id}/catch/"), layout);
                }
            }
            _ => {}
        }
    }
}

/// Computes a layered/topological placement (column = dependency depth via
/// `workflow_engine::compute_levels`, row = position within that depth)
/// for every action in `def` that has no position yet after whatever
/// sidecar lookup already ran — a brand-new hand-written flow, or one the
/// Task Chain wizard just generated. Falls back to a one-column stack for
/// just the one scope a cycle/dangling-ref is detected in. Returns a
/// `WorkflowLayout` covering only the actions this pass actually placed,
/// for the caller to merge into what gets persisted.
pub fn auto_layout(def: &mut WorkflowDefinition) -> WorkflowLayout {
    let mut positions = HashMap::new();
    auto_layout_map(&mut def.actions, "", &mut positions);
    WorkflowLayout { positions }
}

const LEVEL_X_STEP: f32 = 220.0;

fn auto_layout_map(actions: &mut ActionMap, path_prefix: &str, positions: &mut HashMap<String, (f32, f32)>) {
    match workflow_engine::compute_levels(actions) {
        Ok(levels) => {
            for (level_ix, ids) in levels.iter().enumerate() {
                for (row_ix, id) in ids.iter().enumerate() {
                    let Some(action) = actions.get_mut(id) else { continue };
                    if action.pos.is_none() {
                        let pos = (CHILD_X + level_ix as f32 * LEVEL_X_STEP, CHILD_Y0 + row_ix as f32 * Y_STEP);
                        action.pos = Some(pos);
                        positions.insert(format!("{path_prefix}{id}"), pos);
                    }
                }
            }
        }
        Err(_) => {
            let mut y = CHILD_Y0;
            for (id, action) in actions.iter_mut() {
                if action.pos.is_none() {
                    let pos = (CHILD_X, y);
                    action.pos = Some(pos);
                    positions.insert(format!("{path_prefix}{id}"), pos);
                    y += Y_STEP;
                }
            }
        }
    }

    for (id, action) in actions.iter_mut() {
        match action.type_id.as_str() {
            "Foreach" | "Until" => {
                if let Some(body) = &mut action.actions {
                    auto_layout_map(body, &format!("{path_prefix}{id}/"), positions);
                }
            }
            "If" => {
                if let Some(body) = &mut action.actions {
                    auto_layout_map(body, &format!("{path_prefix}{id}/then/"), positions);
                }
                if let Some(branch) = &mut action.else_branch {
                    auto_layout_map(&mut branch.actions, &format!("{path_prefix}{id}/else/"), positions);
                }
            }
            "Try" => {
                if let Some(body) = &mut action.actions {
                    auto_layout_map(body, &format!("{path_prefix}{id}/try/"), positions);
                }
                if let Some(branch) = &mut action.catch {
                    auto_layout_map(&mut branch.actions, &format!("{path_prefix}{id}/catch/"), positions);
                }
            }
            _ => {}
        }
    }
}
