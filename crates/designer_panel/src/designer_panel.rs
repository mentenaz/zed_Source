//! The Designer — a workspace tab that renders a `.flow.json` workflow as
//! a `gpui_flow` canvas: view, edit node properties, save, and run.
//!
//! Ported from `E:\Forge_GPUI\src\forge_shell\panels\designer_panel\`,
//! narrowed to the workflow-JSON half only (`.fdgn`/legacy ForgeFlow was
//! never vendored into this tree — see `gpui_flow`/`workflow_engine`'s own
//! module docs). Unlike `flows_panel` (a `workspace::dock::Panel`), this
//! is a per-file `workspace::item::Item` — opened once per flow, not a
//! persistent sidebar.
//!
//! Positions round-trip through the `.flow.layout.json` sidecar
//! (`workflow_engine::layout`), never through `Action::pos` — see
//! `workflow_json`'s module doc. A flow with no sidecar yet gets a
//! layered auto-layout (`workflow_json::auto_layout`, built on
//! `workflow_engine::compute_levels`) instead of the placeholder stack.
//!
//! Property editing is deliberately plain-text (every `FlowNode.property`
//! value is a string, parsed back to JSON on save via `string_to_value`)
//! rather than a typed widget per `registry::FieldKind` — matches
//! `flows_panel`'s own "everything as text for now" pragmatism; a typed
//! form is a natural follow-up, not required to make editing usable.

mod project_item;
mod run_results;
mod run_state;
mod workflow_json;

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gpui::{
    Action, App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, ScrollHandle, SharedString,
    StatefulInteractiveElement as _, Styled as _, Subscription, TaskExt as _, WeakEntity, Window,
    div, prelude::FluentBuilder as _,
};
use gpui_component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, Theme,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{Input, InputEvent, InputState},
    menu::{ContextMenuExt as _, PopupMenuItem},
    resizable::{h_resizable, resizable_panel},
    scroll::Scrollbar,
    spinner::Spinner,
    tag::Tag,
    v_flex,
};
use gpui_flow::{
    Controls, FlowEdge, FlowGraph, FlowNode, FlowPoint, FlowState, HandleDef, HandlePosition,
    Minimap, NodeId,
};
use project::Project;
use run_state::{ActionRun, RunLogLine, RunPhase, RunState, SharedRunState};
use workflow_engine::{
    registry::REGISTRY,
    ActionHistoryRecord, RunHistoryEntry, RunOutcome, RunStatus, StatusSink, WorkflowDefinition,
    run_workflow,
};
use workspace::{OpenOptions, Pane, ProjectItem, Workspace, item::Item};

pub use project_item::FlowFile;
pub use workflow_json::{apply_saved_layout, auto_layout, load, save};

const CANVAS_SIZE: (f32, f32) = (1200.0, 800.0);

#[derive(Clone)]
struct ContextTarget {
    node_id: Option<NodeId>,
    position: FlowPoint,
}

/// `FlowGraph`'s `bg_color`/`grid_color`/`node_bg_color`/`node_border_color`
/// take a raw `0xRRGGBB` `u32`, not an `Hsla` — this converts a theme color
/// at the point the canvas is built, so it opens matching Zed's active
/// theme instead of a fixed dark palette. These four are set once at
/// construction (not re-read on every render like the node renderers
/// below), so a flow tab left open across a live theme switch keeps the
/// canvas chrome it opened with — same one-shot-at-construction tradeoff
/// `gpui_flow`'s own demos make; a full live-resync would need observing
/// `SettingsStore` and rebuilding `FlowGraph`, not just its labels.
fn hex(color: gpui::Hsla) -> u32 {
    u32::from(color.to_rgb()) >> 8
}

/// The fixed set of container node types the canvas ever renders a header
/// for — `Foreach`/`Until`/`If`/`Try` from `registry.rs` (a closed set by
/// that module's own design), plus `workflow_json`'s four synthetic branch
/// wrappers. Leaf types are open-ended (any `REGISTRY` entry), so they go
/// through `default_renderer` instead of a per-type registration.
const CONTAINER_TYPES: &[(&str, &str)] = &[
    ("Foreach", "\u{21bb} Foreach"),
    ("Until", "\u{27f3} Until"),
    ("If", "\u{2442} If"),
    ("Try", "\u{26a0} Try/Catch"),
    (workflow_json::BRANCH_THEN, "True"),
    (workflow_json::BRANCH_ELSE, "False"),
    (workflow_json::BRANCH_TRY, "Try"),
    (workflow_json::BRANCH_CATCH, "Catch"),
];

/// "View Raw" tab-context-menu action — carries the flow's own abs path so
/// the handler (registered once, workspace-wide, in [`init`]) doesn't need
/// to know which `DesignerPanel` instance the click came from. Opens the
/// same plain-JSON tab `flows_panel`'s "Open" button does.
#[derive(Action, Clone, PartialEq, Eq, serde::Deserialize)]
#[action(namespace = designer_panel, no_json)]
pub struct ViewRaw(PathBuf);

/// "Results" action — opens (or reactivates) the `RunResults` tab bound to
/// the flow at `path`. Dispatched from the designer toolbar through the
/// workspace-wide handler below, so it works even for designers opened via
/// `ProjectItem::for_project_item`, which have no `WeakEntity<Workspace>`
/// of their own to call `add_item_to_active_pane` through.
#[derive(Action, Clone, PartialEq, Eq, serde::Deserialize)]
#[action(namespace = designer_panel, no_json)]
pub struct ViewRunResults(PathBuf);

/// Registers the "View Raw" and "Results" handlers on every workspace. Call
/// once at app startup, alongside the other panels' `init` functions — and
/// see `DesignerPanel::for_project_item`'s doc comment for why `.flow.json`
/// opening as the Designer doesn't need a matching registration here (it's
/// wired via `workspace::register_project_item` instead).
pub fn init(cx: &mut gpui::App) {
    cx.observe_new(|workspace: &mut Workspace, _, _| {
        workspace.register_action(|workspace, action: &ViewRaw, window, cx| {
            workspace
                .open_abs_path(action.0.clone(), OpenOptions::default(), window, cx)
                .detach_and_log_err(cx);
        });
        workspace.register_action(|workspace, action: &ViewRunResults, window, cx| {
            run_results::RunResults::open(workspace, &action.0, window, cx);
        });
    })
    .detach();
    workspace::register_project_item::<DesignerPanel>(cx);
}

fn node_label(node: &FlowNode) -> String {
    if node.label.is_empty() {
        node.id.to_string()
    } else {
        node.label.to_string()
    }
}

/// Deterministic FNV-1a over `type_id`, so the same action type always
/// lands on the same one of the theme's 5 categorical `chart_*` colors —
/// stable across renders and across runs, unlike a `HashMap`'s default
/// per-process-random hasher.
fn accent_for_type(type_id: &str, cx: &App) -> gpui::Hsla {
    let mut hash: u32 = 0x811c9dc5;
    for byte in type_id.bytes() {
        hash ^= byte as u32;
        hash = hash.wrapping_mul(0x01000193);
    }
    let theme = cx.theme();
    let palette = [theme.chart_1, theme.chart_2, theme.chart_3, theme.chart_4, theme.chart_5];
    palette[(hash as usize) % palette.len()]
}

fn render_container_header(node: &FlowNode, title: &str, _w: &mut gpui::Window, cx: &mut App) -> gpui::AnyElement {
    div()
        .text_sm()
        .font_weight(gpui::FontWeight::MEDIUM)
        .text_color(cx.theme().popover_foreground)
        .child(format!("{title} \u{2014} {}", node_label(node)))
        .into_any_element()
}

fn render_leaf(node: &FlowNode, _w: &mut gpui::Window, cx: &mut App) -> gpui::AnyElement {
    let type_label = node.node_type.clone().unwrap_or_else(|| "action".into());
    let accent = accent_for_type(&type_label, cx);
    let text_color = cx.theme().popover_foreground;

    // Left accent bar — the one place a node's `type_id` gets a stable,
    // distinct color; everything else is theme-neutral so different
    // action types still read apart from each other at a glance, matching
    // the original Forge canvas's per-type accent bars (see `render_leaf`
    // in the vendored kitchen-sink demo).
    let accent_bar = div()
        .w(gpui::px(3.0))
        .rounded_l_sm()
        .bg(accent)
        .my(gpui::px(-8.0))
        .ml(gpui::px(-16.0))
        .mr(gpui::px(12.0));

    let type_row = div()
        .text_xs()
        .text_color(text_color)
        .font_weight(gpui::FontWeight::MEDIUM)
        .child(type_label);

    let label_row = div()
        .text_sm()
        .text_color(text_color)
        .font_weight(gpui::FontWeight::MEDIUM)
        .child(node_label(node));

    // Same subtle tint `gpui_flow`'s own container-header strip sits on
    // (`graph.rs`'s `container_header_strip`, `bg(0x00000008)`) — without
    // it, identically-colored text reads dimmer here than in a header
    // simply because it has less local contrast against plain `popover`.
    // Leaf nodes have no header/body split to hang a real strip off, so
    // this wraps just the text instead of reproducing the header's
    // viewport-relative full-bleed geometry (not available here anyway —
    // this renderer never sees the canvas's `Viewport`/zoom).
    let text_block = div()
        .flex()
        .flex_col()
        .gap_0p5()
        .rounded_sm()
        .px_1()
        .py_0p5()
        .bg(gpui::rgba(0x00000008))
        .child(type_row)
        .child(label_row);

    div()
        .flex()
        .flex_row()
        .items_center()
        .child(accent_bar)
        .child(text_block)
        .into_any_element()
}

struct PropertyRow {
    key: SharedString,
    value: Entity<InputState>,
    _sub: Subscription,
}

pub struct DesignerPanel {
    focus_handle: FocusHandle,
    /// Not read yet — reserved for wiring `flows_panel::persistence::DesignerDb`
    /// (recording which flow this workspace had open) once that lands.
    /// `None` when opened via `ProjectItem::for_project_item` (a bare
    /// `Entity<Project>`/`Option<&Pane>` don't expose the owning
    /// workspace — `Pane::workspace` is crate-private to `workspace`);
    /// `Some` when opened via `flows_panel`'s "Graph" button, which has it.
    #[allow(dead_code)]
    workspace: Option<WeakEntity<Workspace>>,
    path: PathBuf,
    root: PathBuf,
    flow_id: String,
    flow_name: String,
    state: Entity<FlowState>,
    context_target: Rc<RefCell<ContextTarget>>,
    flow: Entity<FlowGraph>,
    minimap: Entity<Minimap>,
    controls: Entity<Controls>,
    label_input: Option<Entity<InputState>>,
    _label_sub: Option<Subscription>,
    property_rows: Vec<PropertyRow>,
    selected_node_id: Option<NodeId>,
    new_prop_key: Entity<InputState>,
    new_prop_value: Entity<InputState>,
    running: bool,
    status: Option<String>,
    error: Option<String>,
    /// The live run state shared with the `StatusSink` (written off-thread)
    /// and with the "Run Results" tab. Snapshot into `run_snapshot` by the
    /// poll task a few times a second while a run is active.
    run_state: SharedRunState,
    /// UI-side snapshot of `run_state`, driven by [`Self::sync_run`]. All
    /// rendering (toolbar "running" line, per-node results, transcript)
    /// reads this instead of locking the shared state from render.
    run_snapshot: RunState,
    /// Last `RunState::revision` that was merged into `run_snapshot` — a
    /// cheap "did the run make progress since my last paint" check.
    run_revision: u64,
    /// Whether the under-canvas run transcript is expanded.
    show_log: bool,
    log_scroll: ScrollHandle,
    _state_sub: Subscription,
    _theme_sub: Subscription,
}

impl DesignerPanel {
    /// Opens `path` (a `.flow.json` file) as a new canvas tab. `root` is
    /// the project root (used as `run_workflow`'s `solution_root` and the
    /// run-history namespace — same convention `flows_panel` uses). Used
    /// by `flows_panel`'s "Graph" button, which has a real
    /// `WeakEntity<Workspace>` to pass; opening via the project-item path
    /// (double-click, quick-open, ...) goes through `for_project_item`
    /// instead, which doesn't.
    pub fn open(
        path: PathBuf,
        root: PathBuf,
        workspace: WeakEntity<Workspace>,
        window: &mut Window,
        cx: &mut Context<Workspace>,
    ) -> Entity<Self> {
        cx.new(|cx| Self::build(path, root, Some(workspace), window, cx))
    }

    /// Reads and builds the canvas state for `path`, applying its
    /// `.flow.layout.json` sidecar if present and auto-layouting anything
    /// still unpositioned. Infallible by design: a missing/unreadable/
    /// invalid file still produces an open (empty) canvas tab, with the
    /// failure recorded in `self.error` and rendered as this tab's own
    /// inline banner — `ProjectItem::for_project_item` has no `Result` to
    /// return through, so this can't bail out partway the way the old
    /// `open`'s `?`-based version did.
    fn build(
        path: PathBuf,
        root: PathBuf,
        workspace: Option<WeakEntity<Workspace>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let (mut def, error) = match std::fs::read_to_string(&path)
            .map_err(|e| format!("couldn't read {}: {e}", path.display()))
            .and_then(|raw| {
                serde_json::from_str::<WorkflowDefinition>(&raw)
                    .map_err(|e| format!("{} isn't a valid flow: {e}", path.display()))
            }) {
            Ok(def) => (def, None),
            Err(e) => {
                let id = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .and_then(|n| n.strip_suffix(".flow.json"))
                    .unwrap_or("flow")
                    .to_string();
                (
                    WorkflowDefinition {
                        id: id.clone(),
                        name: id,
                        actions: Default::default(),
                        outputs: Default::default(),
                    },
                    Some(e),
                )
            }
        };

        if error.is_none() {
            if let Some(layout) = workflow_engine::layout::load(&path) {
                workflow_json::apply_saved_layout(&mut def, &layout);
            }
            let newly_placed = workflow_json::auto_layout(&mut def);
            if !newly_placed.positions.is_empty() {
                let mut layout = workflow_engine::layout::load(&path).unwrap_or_default();
                layout.positions.extend(newly_placed.positions);
                let _ = workflow_engine::layout::save(&path, &layout);
            }
        }

        let mut flow_state = workflow_json::load(
            &def,
            hex(cx.theme().muted_foreground),
            hex(cx.theme().danger),
        );
        flow_state.fit_view(60.0, CANVAS_SIZE.0, CANVAS_SIZE.1);
        let state = cx.new(|_| flow_state);
        let context_target = Rc::new(RefCell::new(ContextTarget {
            node_id: None,
            position: FlowPoint::new(60.0, 60.0),
        }));

        let flow = cx.new(|cx| {
            let mut graph = FlowGraph::new(state.clone(), cx)
                // `sidebar`/`popover` rather than `background`/`secondary`:
                // in some themes those two are nearly the same value, which
                // reads as a flat, washed-out canvas with no depth (the
                // node cards need to visibly sit "on" the canvas, the way
                // the fixed dark palette this replaced did on purpose).
                .bg_color(hex(cx.theme().sidebar))
                .grid_color(hex(cx.theme().sidebar_border))
                .node_bg_color(hex(cx.theme().popover))
                .node_border_color(hex(cx.theme().border))
                .accent_color(hex(cx.theme().primary))
                .default_renderer(render_leaf);
            let context_target_for_graph = context_target.clone();
            graph = graph.on_canvas_context_menu(move |_, position, node_id, _, _| {
                let mut target = context_target_for_graph.borrow_mut();
                target.position = position;
                target.node_id = node_id;
            });
            let context_target_for_node = context_target.clone();
            graph = graph.on_node_context_menu(move |node_id, _, _, _| {
                context_target_for_node.borrow_mut().node_id = Some(node_id);
            });
            for (type_id, title) in CONTAINER_TYPES {
                graph = graph
                    .node_renderer(*type_id, move |n, w, cx| render_container_header(n, title, w, cx))
                    .container_header(*type_id, move |n, w, cx| render_container_header(n, title, w, cx));
            }
            graph
        });
        let minimap = cx.new(|_| Minimap::new(state.clone()).container_bounds(CANVAS_SIZE.0, CANVAS_SIZE.1));
        let controls = cx.new(|_| Controls::new(state.clone()).container_size(CANVAS_SIZE.0, CANVAS_SIZE.1));

        let flow_id = def.id.clone();
        let flow_name = def.name.clone();

        let _state_sub = cx.observe(&state, |_this, _state, cx| cx.notify());
        let _theme_sub = cx.observe_global::<Theme>(|this: &mut Self, cx| {
            this.sync_canvas_colors(cx);
        });
        let new_prop_key = cx.new(|cx| InputState::new(window, cx).placeholder("property"));
        let new_prop_value = cx.new(|cx| InputState::new(window, cx).placeholder("value"));

        let run_state: SharedRunState = Arc::new(Mutex::new(RunState::default()));
        run_state::register_run_state(path.clone(), run_state.clone());

        Self {
            focus_handle: cx.focus_handle(),
            workspace,
            path,
            root,
            flow_id,
            flow_name,
            state,
            context_target,
            flow,
            minimap,
            controls,
            label_input: None,
            _label_sub: None,
            property_rows: Vec::new(),
            selected_node_id: None,
            new_prop_key,
            new_prop_value,
            running: false,
            status: None,
            error,
            run_state,
            run_snapshot: RunState::default(),
            run_revision: 0,
            show_log: false,
            log_scroll: ScrollHandle::default(),
            _state_sub,
            _theme_sub,
        }
    }

    /// Re-applies the canvas chrome from the active theme. The node renderers
    /// already read `cx.theme()` on every paint, but `FlowGraph`'s
    /// `bg_color`/`grid_color`/`node_bg_color`/`node_border_color`/`accent_color`
    /// are one-shot builder values — this keeps them tracking a live theme
    /// switch instead of freezing at open time.
    fn sync_canvas_colors(&mut self, cx: &mut Context<Self>) {
        self.flow.update(cx, |graph, cx| {
            graph.set_theme_colors(
                hex(cx.theme().sidebar),
                hex(cx.theme().sidebar_border),
                hex(cx.theme().popover),
                hex(cx.theme().border),
                hex(cx.theme().primary),
            );
            cx.notify();
        });
    }

    fn selected_node(&self, cx: &App) -> Option<FlowNode> {
        self.state.read(cx).nodes.iter().find(|n| n.selected).cloned()
    }

    /// Rebuilds the property panel's input entities when the selected node
    /// changes — called at the top of `render` since selection is driven
    /// entirely by `FlowGraph`'s own mouse handling on `self.state`, which
    /// this panel only observes, not owns.
    fn sync_selection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let selected = self.selected_node(cx);
        let selected_id = selected.as_ref().map(|n| n.id.clone());
        if selected_id == self.selected_node_id {
            return;
        }
        self.selected_node_id = selected_id;
        self.property_rows.clear();
        self.label_input = None;
        self._label_sub = None;

        let Some(node) = selected else { return };

        let label_input = cx.new(|cx| InputState::new(window, cx).default_value(node.label.clone()));
        let node_id = node.id.clone();
        let state = self.state.clone();
        let _label_sub = cx.subscribe(&label_input, move |this, input, event, cx| {
            if matches!(event, InputEvent::Change) {
                let value = input.read(cx).value().to_string();
                this.clear_run_marker(&node_id, cx);
                state.update(cx, |state, cx| {
                    if let Some(n) = state.get_node_mut(&node_id) {
                        n.label = value.into();
                    }
                    cx.notify();
                });
            }
        });
        self.label_input = Some(label_input);
        self._label_sub = Some(_label_sub);

        for (key, value) in &node.properties {
            self.push_property_row(key.clone(), value.clone(), window, cx);
        }
    }

    fn push_property_row(&mut self, key: SharedString, value: SharedString, window: &mut Window, cx: &mut Context<Self>) {
        let Some(node_id) = self.selected_node_id.clone() else { return };
        let input = cx.new(|cx| InputState::new(window, cx).default_value(value));
        let state = self.state.clone();
        let row_key = key.clone();
        let sub = cx.subscribe(&input, move |this, input, event, cx| {
            if matches!(event, InputEvent::Change) {
                let value = input.read(cx).value().to_string();
                let row_key = row_key.clone();
                this.clear_run_marker(&node_id, cx);
                state.update(cx, |state, cx| {
                    if let Some(n) = state.get_node_mut(&node_id) {
                        if let Some(prop) = n.properties.iter_mut().find(|(k, _)| *k == row_key) {
                            prop.1 = value.into();
                        }
                    }
                    cx.notify();
                });
            }
        });
        self.property_rows.push(PropertyRow {
            key,
            value: input,
            _sub: sub,
        });
    }

    fn on_add_property_click(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(node_id) = self.selected_node_id.clone() else { return };
        let key = self.new_prop_key.read(cx).value().trim().to_string();
        if key.is_empty() {
            return;
        }
        let value = self.new_prop_value.read(cx).value().to_string();
        self.clear_run_marker(&node_id, cx);
        self.state.update(cx, |state, cx| {
            if let Some(n) = state.get_node_mut(&node_id) {
                if let Some(prop) = n.properties.iter_mut().find(|(k, _)| k.as_ref() == key) {
                    prop.1 = value.clone().into();
                } else {
                    n.properties.push((key.clone().into(), value.clone().into()));
                }
            }
            cx.notify();
        });
        self.push_property_row(key.into(), value.into(), window, cx);
        self.new_prop_key.update(cx, |input, cx| input.set_value("", window, cx));
        self.new_prop_value.update(cx, |input, cx| input.set_value("", window, cx));
    }

    /// Deletes the selected node, every descendant (a container's own
    /// children), and every edge touching any of them.
    fn on_delete_node_click(&mut self, cx: &mut Context<Self>) {
        let Some(node_id) = self.selected_node_id.clone() else { return };
        self.state.update(cx, |state, cx| {
            let mut doomed = vec![node_id.clone()];
            let mut frontier = vec![node_id];
            while let Some(id) = frontier.pop() {
                for child in state.children_of(&id) {
                    doomed.push(child.id.clone());
                    frontier.push(child.id.clone());
                }
            }
            state.nodes.retain(|n| !doomed.contains(&n.id));
            state.edges.retain(|e| !doomed.contains(&e.source) && !doomed.contains(&e.target));
            state.rebuild_lookup();
            state.refit_all_containers();
            cx.notify();
        });
        self.selected_node_id = None;
        self.property_rows.clear();
        self.label_input = None;
    }

    fn on_save_click(&mut self, cx: &mut Context<Self>) {
        let (def, layout) = {
            let state = self.state.read(cx);
            workflow_json::save(state, self.flow_id.clone(), self.flow_name.clone())
        };
        let json = match serde_json::to_string_pretty(&def) {
            Ok(json) => json,
            Err(e) => {
                self.error = Some(format!("Couldn't serialize flow: {e}"));
                cx.notify();
                return;
            }
        };
        if let Err(e) = std::fs::write(&self.path, json) {
            self.error = Some(format!("Couldn't write {}: {e}", self.path.display()));
            cx.notify();
            return;
        }
        if let Err(e) = workflow_engine::layout::save(&self.path, &layout) {
            self.error = Some(format!("Saved the flow, but couldn't save its layout: {e}"));
            cx.notify();
            return;
        }
        self.error = None;
        self.status = Some("Saved".to_string());
        cx.notify();
    }

    fn on_run_click(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.running {
            return;
        }
        let (def, _layout) = {
            let state = self.state.read(cx);
            workflow_json::save(state, self.flow_id.clone(), self.flow_name.clone())
        };

        let mut action_meta = HashMap::new();
        collect_action_meta(&def.actions, &mut action_meta);

        // Reset the shared run state, then let the sink replay into it
        // off-thread as `run_workflow` progresses.
        let run_state = self.run_state.clone();
        let started_at = Instant::now();
        {
            let mut st = run_state.lock().unwrap();
            *st = RunState::default();
            st.started_at = Some(started_at);
        }
        self.run_snapshot = RunState::default();
        self.run_revision = 0;
        self.state.update(cx, |state, cx| {
            for node in &mut state.nodes {
                node.accent_border = None;
            }
            cx.notify();
        });

        // `records` mirrors final outcomes for the persistent `flows_panel`
        // run history (`flows_panel_history_append` below); the shared
        // `RunState` carries the same events plus the `Running` phase and
        // the live transcript.
        let records: Arc<Mutex<Vec<ActionHistoryRecord>>> = Arc::new(Mutex::new(Vec::new()));
        let records_for_sink = records.clone();
        let sink_state = run_state.clone();
        let sink_meta = action_meta.clone();
        let status = StatusSink::new(move |action_id, status, detail| {
            let phase = match status {
                RunStatus::Running => RunPhase::Running,
                RunStatus::Succeeded => RunPhase::Succeeded,
                RunStatus::Failed => RunPhase::Failed,
                RunStatus::Skipped => RunPhase::Skipped,
            };
            let (name, type_id) = sink_meta
                .get(action_id)
                .cloned()
                .unwrap_or_else(|| (action_id.to_string(), "unknown".to_string()));
            sink_state.lock().unwrap().record(
                started_at,
                action_id,
                phase,
                detail.clone(),
                name.clone(),
                type_id.clone(),
            );
            if matches!(phase, RunPhase::Running) {
                return;
            }
            let outcome = match phase {
                RunPhase::Succeeded => RunOutcome::Succeeded,
                RunPhase::Failed => RunOutcome::Failed,
                RunPhase::Skipped => RunOutcome::Skipped,
                RunPhase::Running => return,
            };
            records_for_sink.lock().unwrap().push(ActionHistoryRecord {
                id: action_id.to_string(),
                name,
                type_id,
                outcome,
                message: detail,
                response: None,
            });
        });

        self.error = None;
        self.running = true;
        self.status = Some("Running\u{2026}".to_string());
        self.start_run_poll(window, cx);
        cx.notify();

        let root = self.root.clone();
        let namespace = root.to_string_lossy().to_string();
        let flow_id = self.flow_id.clone();
        let db = cx.global::<db::AppDatabase>();
        let store = db::kvp::KeyValueStore::from_app_db(db);
        let run_state_for_finish = run_state.clone();

        cx.spawn_in(window, async move |this, cx| {
            let result = run_workflow(def, root, status).await;
            let outcome = if result.is_ok() {
                RunOutcome::Succeeded
            } else {
                RunOutcome::Failed
            };
            {
                let mut st = run_state_for_finish.lock().unwrap();
                st.outcome = Some(outcome);
                st.error = result.as_ref().err().cloned();
                st.finished_at = Some(Instant::now());
                st.revision += 1;
            }

            let history_entry = RunHistoryEntry {
                id: format!("run-{}", chrono::Utc::now().timestamp_millis()),
                time: chrono::Utc::now().to_rfc3339(),
                outcome,
                actions: records.lock().unwrap().clone(),
            };
            let _ = flows_panel_history_append(&store, &namespace, &flow_id, history_entry).await;

            let _ = this.update_in(cx, |this, _window, cx| {
                this.running = false;
                this.status = Some(match &result {
                    Ok(()) => "Run succeeded".to_string(),
                    Err(e) => format!("Run failed: {e}"),
                });
                this.sync_run(cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// Merges the shared `RunState` into `self.run_snapshot` and repaints the
    /// nodes' status borders. Returns whether anything new had arrived since
    /// the last snapshot (drives the poll loop's re-render decision).
    fn sync_run(&mut self, cx: &mut Context<Self>) -> bool {
        let (st, revision) = {
            let locked = self.run_state.lock().unwrap();
            (locked.clone(), locked.revision)
        };
        if revision == self.run_revision {
            return false;
        }
        self.apply_status_colors(&st, cx);
        self.run_snapshot = st;
        self.run_revision = revision;
        true
    }

    /// Applies phase-colored `accent_border`s (Running = primary,
    /// Succeeded = success, Failed = danger, Skipped = none) onto the live
    /// `FlowState` nodes — `gpui_flow::FlowNode::accent_border` is the
    /// documented hook for exactly this ("a running/succeeded/failed node in
    /// a workflow executor sets a raw color; this crate just draws it").
    fn apply_status_colors(&mut self, st: &RunState, cx: &mut Context<Self>) {
        let running_hex = hex(cx.theme().primary);
        let success_hex = hex(cx.theme().success);
        let danger_hex = hex(cx.theme().danger);
        self.state.update(cx, |state, cx| {
            for (id, run) in &st.nodes {
                let color = match run.phase {
                    RunPhase::Running => Some(running_hex),
                    RunPhase::Succeeded => Some(success_hex),
                    RunPhase::Failed => Some(danger_hex),
                    RunPhase::Skipped => None,
                };
                if let Some(node) = state.nodes.iter_mut().find(|n| n.id.as_ref() == id.as_str()) {
                    if node.accent_border != color {
                        node.accent_border = color;
                    }
                }
            }
            cx.notify();
        });
    }

    /// Drives the UI off the shared run state while a run is in flight — a
    /// ~10 Hz poll that merges new events into [`Self::run_snapshot`] and
    /// re-renders, exiting once the run has reported `finished_at`.
    fn start_run_poll(&self, window: &mut Window, cx: &mut Context<Self>) {
        let run_state = self.run_state.clone();
        cx.spawn_in(window, async move |this, cx| {
            loop {
                let done = run_state.lock().unwrap().finished_at.is_some();
                if this
                    .update_in(cx, |this, _window, cx| {
                        if this.sync_run(cx) {
                            cx.notify();
                        }
                    })
                    .is_err()
                {
                    break;
                }
                if done {
                    break;
                }
                cx.background_executor()
                    .timer(Duration::from_millis(100))
                    .await;
            }
        })
        .detach();
    }

    /// The most recent action still `Running` (parallel leaves can be
    /// active at the same time) — the toolbar's live "now running" line.
    fn running_action(&self) -> Option<ActionRun> {
        self.run_snapshot
            .nodes
            .values()
            .filter(|r| r.phase == RunPhase::Running)
            .max_by_key(|r| r.started_at)
            .cloned()
    }

    /// Editing a node invalidates its run result: the user chose to keep
    /// phase colors "until you edit", so an edited node drops its marker
    /// (and its accent border) rather than showing stale result data.
    fn clear_run_marker(&mut self, node_id: &NodeId, cx: &mut Context<Self>) {
        let id = node_id.to_string();
        if self.run_state.lock().unwrap().nodes.remove(&id).is_none() {
            return;
        }
        self.run_state.lock().unwrap().revision += 1;
        self.state.update(cx, |state, cx| {
            if let Some(node) = state.get_node_mut(node_id) {
                node.accent_border = None;
            }
            cx.notify();
        });
        self.sync_run(cx);
        cx.notify();
    }

    fn on_clear_results_click(&mut self, cx: &mut Context<Self>) {
        *self.run_state.lock().unwrap() = RunState::default();
        self.run_snapshot = RunState::default();
        self.run_revision = 0;
        self.status = None;
        self.state.update(cx, |state, cx| {
            for node in &mut state.nodes {
                node.accent_border = None;
            }
            cx.notify();
        });
        cx.notify();
    }

    fn on_toggle_log_click(&mut self, cx: &mut Context<Self>) {
        self.show_log = !self.show_log;
        if self.show_log {
            self.log_scroll.scroll_to_bottom();
        }
        cx.notify();
    }

    /// Selects a node on the canvas (used by clicking a transcript row) by
    /// writing `selected` straight onto `FlowState` — the same field
    /// `FlowGraph`'s mouse handling drives, so the selection ring and the
    /// property pane pick it up on the next render.
    fn select_node(&mut self, node_id: NodeId, cx: &mut Context<Self>) {
        self.state.update(cx, |state, cx| {
            for node in &mut state.nodes {
                node.selected = node.id == node_id;
            }
            cx.notify();
        });
        self.selected_node_id = None;
        cx.notify();
    }

    fn on_fit_view_click(&mut self, cx: &mut Context<Self>) {
        self.state.update(cx, |state, cx| {
            state.fit_view(60.0, CANVAS_SIZE.0, CANVAS_SIZE.1);
            cx.notify();
        });
    }

    fn add_action(&mut self, type_id: &'static str, cx: &mut Context<Self>) {
        let context = self.context_target.borrow().clone();
        let (position, parent_id, source_id) = {
            let state = self.state.read(cx);
            match context.node_id.as_ref().and_then(|id| state.get_node(id)) {
                Some(source) => {
                    let height = if state.is_container(&source.id) {
                        source.container_size.map(|(_, height)| height).unwrap_or(220.0)
                    } else {
                        workflow_json::LEAF_SIZE.1
                    };
                    (
                        FlowPoint::new(source.position.x, source.position.y + height + 30.0),
                        source.parent_id.clone(),
                        Some(source.id.clone()),
                    )
                }
                None => (context.position, None, None),
            }
        };
        let id = {
            let state = self.state.read(cx);
            let base = type_id.to_ascii_lowercase();
            let mut index = 1;
            loop {
                let candidate = format!("{base}-{index}");
                if state.get_node(&candidate.clone().into()).is_none() {
                    break candidate;
                }
                index += 1;
            }
        };
        let label = REGISTRY
            .iter()
            .find(|def| def.type_id == type_id)
            .map(|def| def.label)
            .unwrap_or(type_id);
        let is_container = REGISTRY
            .iter()
            .any(|def| def.type_id == type_id && matches!(def.category, workflow_engine::ActionCategory::Container));
        let mut node = FlowNode::new(id.clone(), position.x, position.y)
            .node_type(type_id)
            .label(label)
            .context_menu(true)
            .handles(vec![
                HandleDef::target(HandlePosition::Top),
                HandleDef::source(HandlePosition::Bottom),
            ]);
        if let Some(parent_id) = parent_id {
            node = node.parent(parent_id);
        }
        if !is_container {
            node = node.size(workflow_json::LEAF_SIZE.0, workflow_json::LEAF_SIZE.1);
        }
        self.state.update(cx, |state, cx| {
            state.push_undo();
            state.nodes.push(node);
            if let Some(source_id) = source_id {
                state.edges.push(FlowEdge::new(
                    format!("e_{source_id}_{id}"),
                    source_id,
                    id.clone(),
                ));
            }
            state.rebuild_lookup();
            cx.notify();
        });
        cx.notify();
    }

    fn render_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let run_state_has_results = !self.run_snapshot.log.is_empty() || !self.run_snapshot.nodes.is_empty();
        h_flex()
            .w_full()
            .flex_shrink_0()
            .items_center()
            .justify_between()
            .gap_2()
            .px_3()
            .py_2()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(Icon::new(IconName::Network).text_color(cx.theme().foreground))
                    .child(
                        div()
                            .text_sm()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(cx.theme().foreground)
                            .child(self.flow_name.clone()),
                    ),
            )
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .when(run_state_has_results, |el| {
                        el.child(
                            Button::new("designer-logs")
                                .ghost()
                                .xsmall()
                                .icon(if self.show_log {
                                    IconName::ChevronDown
                                } else {
                                    IconName::SquareTerminal
                                })
                                .tooltip(if self.show_log {
                                    "Close the run log"
                                } else {
                                    "Show the run log"
                                })
                                .on_click(cx.listener(|this, _, _, cx| this.on_toggle_log_click(cx))),
                        )
                        .child(
                            Button::new("designer-open-results")
                                .ghost()
                                .xsmall()
                                .icon(IconName::ChartPie)
                                .tooltip("Open the run results in a new tab")
                                .on_click(cx.listener(|this, _, window, cx| {
                                    window.dispatch_action(Box::new(ViewRunResults(this.path.clone())), cx);
                                })),
                        )
                        .child(
                            Button::new("designer-clear-results")
                                .ghost()
                                .xsmall()
                                .icon(IconName::Close)
                                .tooltip("Clear run results and status colors")
                                .on_click(cx.listener(|this, _, _, cx| this.on_clear_results_click(cx)))
                        )
                    })
                    .when_some(self.running_action(), |el, run| {
                        el.child(
                            h_flex()
                                .gap_1p5()
                                .items_center()
                                .child(Spinner::new().xsmall())
                                .child(
                                    div()
                                        .text_xs()
                                        .font_weight(gpui::FontWeight::MEDIUM)
                                        .text_color(cx.theme().primary)
                                        .child(format!("{} \u{00b7} {}", run.name, run.type_id)),
                                ),
                        )
                    })
                    .when_some(self.status.clone(), |el, status| {
                        el.child(div().text_xs().text_color(cx.theme().muted_foreground).child(status))
                    })
                    .child(
                        Button::new("designer-fit-view")
                            .ghost()
                            .xsmall()
                            .label("Fit View")
                            .on_click(cx.listener(|this, _, _, cx| this.on_fit_view_click(cx))),
                    )
                    .child(
                        Button::new("designer-save")
                            .ghost()
                            .xsmall()
                            .label("Save")
                            .on_click(cx.listener(|this, _, _, cx| this.on_save_click(cx))),
                    )
                    .child(if self.running {
                        Spinner::new().xsmall().into_any_element()
                    } else {
                        Button::new("designer-run")
                            .primary()
                            .xsmall()
                            .icon(IconName::Play)
                            .label("Run")
                            .on_click(cx.listener(|this, _, window, cx| this.on_run_click(window, cx)))
                            .into_any_element()
                    }),
            )
    }

    fn render_properties(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(node_id) = &self.selected_node_id else {
            return div()
                .p_4()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child("Select a node to edit its properties.")
                .into_any_element();
        };
        let type_id = self
            .state
            .read(cx)
            .get_node(node_id)
            .and_then(|n| n.node_type.clone())
            .unwrap_or_default();

        v_flex()
            .size_full()
            .gap_2()
            .p_3()
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(format!("{type_id} \u{00b7} {node_id}")),
            )
            .when_some(
                self.run_snapshot.nodes.get(node_id.as_ref()).cloned(),
                |el, run| el.child(self.render_run_result(&run, cx).into_any_element()),
            )
            .when_some(self.label_input.clone(), |el, input| {
                el.child(labeled_field("Label", cx.theme().muted_foreground, Input::new(&input)))
            })
            .child(div().h(gpui::px(1.0)).w_full().bg(cx.theme().border))
            .children(self.property_rows.iter().map(|row| {
                labeled_field(row.key.clone(), cx.theme().muted_foreground, Input::new(&row.value)).into_any_element()
            }))
            .child(
                h_flex()
                    .gap_1()
                    .child(Input::new(&self.new_prop_key).xsmall().w_24())
                    .child(Input::new(&self.new_prop_value).xsmall().flex_1())
                    .child(
                        Button::new("designer-add-property")
                            .ghost()
                            .xsmall()
                            .icon(IconName::Plus)
                            .on_click(cx.listener(|this, _, window, cx| this.on_add_property_click(window, cx))),
                    ),
            )
            .child(
                Button::new("designer-delete-node")
                    .ghost()
                    .xsmall()
                    .icon(IconName::Delete)
                    .label("Delete Node")
                    .on_click(cx.listener(|this, _, _, cx| this.on_delete_node_click(cx))),
            )
            .into_any_element()
    }
}

impl Drop for DesignerPanel {
    fn drop(&mut self) {
        run_state::unregister_run_state(&self.path);
    }
}

fn labeled_field(label: impl Into<SharedString>, muted: gpui::Hsla, input: Input) -> impl IntoElement {
    v_flex()
        .gap_1()
        .child(div().text_xs().text_color(muted).child(label.into()))
        .child(input)
}

/// Walks a flow's (possibly nested) action tree, collecting each action
/// id's display name and type — used to fill in
/// [`ActionHistoryRecord`]'s `name`/`type` fields from a `StatusSink`
/// callback, which only gets the bare id. Mirrors `flows_panel`'s own
/// helper of the same shape.
fn collect_action_meta(actions: &workflow_engine::ActionMap, out: &mut HashMap<String, (String, String)>) {
    for (id, action) in actions {
        let name = action.label.clone().unwrap_or_else(|| id.clone());
        out.insert(id.clone(), (name, action.type_id.clone()));
        if let Some(nested) = &action.actions {
            collect_action_meta(nested, out);
        }
        if let Some(branch) = &action.else_branch {
            collect_action_meta(&branch.actions, out);
        }
        if let Some(branch) = &action.catch {
            collect_action_meta(&branch.actions, out);
        }
    }
}

/// Appends `entry` to the scoped-kv run history — the same
/// `flows_panel::persistence` key convention (`history:<flow_id>` under
/// the project-root namespace), duplicated here rather than depending on
/// `flows_panel` from this crate (that dependency would run the wrong way
/// — `flows_panel`'s "Graph" button depends on `designer_panel`, not the
/// reverse).
async fn flows_panel_history_append(
    store: &db::kvp::KeyValueStore,
    namespace: &str,
    flow_id: &str,
    entry: RunHistoryEntry,
) -> anyhow::Result<()> {
    let key = format!("history:{flow_id}");
    let raw = store.scoped(namespace).read(&key)?;
    let mut entries: Vec<RunHistoryEntry> = raw
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default();
    entries.push(entry);
    let max = workflow_engine::history::MAX_HISTORY_ENTRIES;
    if entries.len() > max {
        let drop = entries.len() - max;
        entries.drain(0..drop);
    }
    let json = serde_json::to_string(&entries)?;
    store.scoped(namespace).write(key, json).await
}

/// The under-canvas / results-pane display helper shared with `run_results`:
/// a `Succeeded` action's `detail` is serialized `outputs` JSON, so it gets
/// pretty-printed; a `Failed` action's detail is an engine error string and
/// is kept as-is.
pub(crate) fn format_detail(detail: &str) -> String {
    if detail.trim().starts_with('{')
        && let Ok(value) = serde_json::from_str::<serde_json::Value>(detail)
    {
        serde_json::to_string_pretty(&value).unwrap_or_else(|_| detail.to_string())
    } else {
        detail.to_string()
    }
}

/// Compact duration text, e.g. `42ms` / `3.2s`.
pub(crate) fn format_ms(d: Duration) -> String {
    if d.as_secs() >= 1 {
        format!("{:.1}s", d.as_secs_f32())
    } else {
        format!("{}ms", d.as_millis())
    }
}

/// Phase → theme color, shared by the transcript rows and the per-node
/// result card (Skipped gets the muted foreground so it reads as "not run").
pub(crate) fn phase_color(phase: RunPhase, theme: &Theme) -> gpui::Hsla {
    match phase {
        RunPhase::Running => theme.primary,
        RunPhase::Succeeded => theme.success,
        RunPhase::Failed => theme.danger,
        RunPhase::Skipped => theme.muted,
    }
}

impl Focusable for DesignerPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<()> for DesignerPanel {}

impl Item for DesignerPanel {
    type Event = ();

    fn tab_content_text(&self, _detail: usize, _cx: &App) -> SharedString {
        format!("{}.flow.json", self.flow_name).into()
    }

    fn tab_icon(&self, _window: &Window, _cx: &App) -> Option<ui::Icon> {
        Some(ui::Icon::new(ui::IconName::GitBranch))
    }

    fn tab_extra_context_menu_actions(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Vec<(SharedString, Box<dyn Action>)> {
        vec![("View Raw".into(), Box::new(ViewRaw(self.path.clone())))]
    }
}

impl ProjectItem for DesignerPanel {
    type Item = FlowFile;

    /// Resolves the flow's abs path and project root from `item`/`project`
    /// and defers to [`DesignerPanel::build`] — the same infallible
    /// load path `open` uses, so a broken `.flow.json` opened by
    /// double-click still opens a tab (with its own error banner) rather
    /// than silently failing or falling back to `for_broken_project_item`.
    fn for_project_item(
        project: Entity<Project>,
        _pane: Option<&Pane>,
        item: Entity<FlowFile>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let flow_file = item.read(cx);
        let path = flow_file.abs_path().unwrap_or_default();
        let worktree_id = flow_file.worktree_id();
        let root = project
            .read(cx)
            .worktree_for_id(worktree_id, cx)
            .map(|worktree| worktree.read(cx).abs_path().to_path_buf())
            .unwrap_or_default();

        Self::build(path, root, None, window, cx)
    }
}

impl Render for DesignerPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_selection(window, cx);

        v_flex()
            .id("designer-panel")
            .track_focus(&self.focus_handle(cx))
            .size_full()
            .bg(cx.theme().background)
            .child(self.render_toolbar(cx))
            .when_some(self.error.clone(), |el, error| {
                el.child(
                    div()
                        .px_3()
                        .py_1()
                        .text_xs()
                        .text_color(cx.theme().danger)
                        .child(error),
                )
            })
            .child(
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .child(
                        div().flex_1().min_h_0().child(
                            h_resizable("designer-split")
                                .child(
                                    resizable_panel().child(
                                        div()
                                            .relative()
                                            .size_full()
                                            .child(
                                                div()
                                                    .id("designer-flow-context")
                                                    .size_full()
                                                    .context_menu({
                                                        let panel = cx.entity().downgrade();
                                                        move |mut menu, window, cx| {
                                                            let panel_for_actions = panel.clone();
                                                            menu = menu.label("Add Action");
                                                            menu.submenu(
                                                                "Choose action type",
                                                                window,
                                                                cx,
                                                                move |mut submenu, _window, _cx| {
                                                                    for definition in REGISTRY {
                                                                        let type_id = definition.type_id;
                                                                        let panel = panel_for_actions.clone();
                                                                        submenu = submenu.item(
                                                                            PopupMenuItem::new(definition.label)
                                                                                .on_click(move |_, _, cx| {
                                                                                    let _ = panel.update(
                                                                                        cx,
                                                                                        |panel, cx| panel.add_action(type_id, cx),
                                                                                    );
                                                                                }),
                                                                        );
                                                                    }
                                                                    submenu
                                                                },
                                                            )
                                                        }
                                                    })
                                                    .child(self.flow.clone()),
                                            )
                                            .child(
                                                div()
                                                    .absolute()
                                                    .bottom(gpui::px(16.0))
                                                    .left(gpui::px(16.0))
                                                    .child(self.controls.clone()),
                                            )
                                            .child(
                                                div()
                                                    .absolute()
                                                    .bottom(gpui::px(16.0))
                                                    .right(gpui::px(16.0))
                                                    .child(self.minimap.clone()),
                                            ),
                                    ),
                                )
                                .child(
                                    resizable_panel()
                                        .size(gpui::px(280.0))
                                        .size_range(gpui::px(220.0)..gpui::px(420.0))
                                        .child(
                                            div()
                                                .size_full()
                                                .border_l_1()
                                                .border_color(cx.theme().border)
                                                .child(self.render_properties(cx)),
                                        ),
                                ),
                        ),
                    )
                    .when(self.show_log && !self.run_snapshot.log.is_empty(), |el| {
                        el.child(self.render_run_log(cx).into_any_element())
                    }),
            )
    }
}

impl DesignerPanel {
    fn render_run_result(&self, run: &ActionRun, cx: &mut Context<Self>) -> impl IntoElement {
        let tag = match run.phase {
            RunPhase::Running => Tag::primary().outline(),
            RunPhase::Succeeded => Tag::success().outline(),
            RunPhase::Failed => Tag::danger().outline(),
            RunPhase::Skipped => Tag::secondary().outline(),
        };
        let detail_color = match run.phase {
            RunPhase::Failed => cx.theme().danger,
            _ => cx.theme().foreground,
        };
        let detail = run.detail.as_deref().filter(|d| !d.is_empty());

        v_flex()
            .gap_1()
            .p_2()
            .rounded_sm()
            .border_1()
            .border_color(if run.phase == RunPhase::Failed {
                cx.theme().danger
            } else {
                cx.theme().border
            })
            .child(
                h_flex()
                    .gap_1p5()
                    .items_center()
                    .child(tag.child(run.phase.label()))
                    .when(run.phase == RunPhase::Running, |el| el.child(Spinner::new().xsmall()))
                    .when_some(run.elapsed(), |el, d| {
                        el.child(
                            div()
                                .ml_auto()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(format_ms(d)),
                        )
                    }),
            )
            .child(
                div()
                    .mt_1()
                    .text_sm()
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(cx.theme().foreground)
                    .child(run.name.clone()),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(run.type_id.clone()),
            )
            .when_some(detail, |el, d| {
                el.child(
                    div()
                        .id("designer-run-detail")
                        .mt_1()
                        .max_h(gpui::px(140.0))
                        .overflow_y_scroll()
                        .text_xs()
                        .font_family("monospace")
                        .text_color(detail_color)
                        .child(format_detail(d)),
                )
            })
    }

    /// The under-canvas transcript pane (`show_log` toggle in the toolbar):
    /// a chronological, color-coded trace of every status event the run
    /// emitted, with its own idle-state totals and a scrollbar.
    fn render_run_log(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let (succeeded, failed, skipped) = self.run_snapshot.totals();
        v_flex()
            .id("designer-run-log")
            .flex_shrink_0()
            .h(gpui::px(180.0))
            .border_t_1()
            .border_color(cx.theme().border)
            .child(
                h_flex()
                    .px_3()
                    .py_1()
                    .gap_2()
                    .items_center()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(
                        Icon::new(IconName::SquareTerminal)
                            .xsmall()
                            .text_color(cx.theme().muted_foreground),
                    )
                    .child(
                        div()
                            .text_xs()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .text_color(cx.theme().foreground)
                            .child("Run log"),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(format!("{succeeded} ok \u{00b7} {failed} failed \u{00b7} {skipped} skipped")),
                    )
                    .child(
                        Button::new("designer-log-collapse")
                            .ghost()
                            .xsmall()
                            .icon(IconName::ChevronDown)
                            .tooltip("Close the run log")
                            .on_click(cx.listener(|this, _, _, cx| this.on_toggle_log_click(cx))),
                    ),
            )
            .child(
                div()
                    .relative()
                    .flex_1()
                    .size_full()
                    .child(
                        div()
                            .id("designer-run-log-scroll")
                            .size_full()
                            .overflow_y_scroll()
                            .track_scroll(&mut self.log_scroll)
                            .children(
                                self.run_snapshot
                                    .log
                                    .iter()
                                    .enumerate()
                                    .map(|(i, line)| self.render_log_row(i, line, cx).into_any_element()),
                            ),
                    )
                    .child(Scrollbar::vertical(&self.log_scroll)),
            )
    }

    fn render_log_row(&self, index: usize, line: &RunLogLine, cx: &mut Context<Self>) -> impl IntoElement {
        let color = phase_color(line.phase, cx.theme());
        let detail = line.detail.as_deref().unwrap_or("");
        let phase_label = if line.phase == RunPhase::Running {
            "running\u{2026}".to_string()
        } else {
            format!("{} in {:.1}s", line.phase.label(), (line.time_ms as f64) / 1000.0)
        };
        let action_id = line.action_id.clone();
        h_flex()
            .id(SharedString::from(format!("designer-log-row-{index}")))
            .w_full()
            .gap_2()
            .items_center()
            .px_3()
            .py_1()
            .hover(|d| d.bg(cx.theme().muted.opacity(0.2)))
            .on_click(cx.listener(move |this, _event, _window, cx| {
                this.select_node(action_id.clone().into(), cx);
            }))
            .child(div().size_1p5().rounded_full().flex_shrink_0().bg(color))
            .child(
                div()
                    .text_xs()
                    .font_weight(gpui::FontWeight::MEDIUM)
                    .text_color(cx.theme().foreground)
                    .child(line.name.clone()),
            )
            .child(
                div().text_xs().text_color(cx.theme().muted_foreground).child(line.type_id.clone()),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_xs()
                    .font_family("monospace")
                    .text_color(cx.theme().muted_foreground)
                    .child(if detail.is_empty() {
                        phase_label
                    } else {
                        detail.to_string()
                    }),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(format!("{}ms", line.time_ms)),
            )
    }
}
