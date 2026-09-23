//! The Flows panel — a left-docked sidebar listing every `.flow.json`
//! workflow in the open workspace, with create/open/run actions.
//!
//! Ported from `E:\Forge_GPUI\src\forge_shell\panels\flows_panel.rs`, cut
//! down to just the workflow-JSON half of that source (this tree only
//! vendored `workflow_engine`/`gpui_flow` — the engine backing
//! `.flow.json` — not Forge's legacy `.fdgn`/`.fwrk` ForgeFlow format, so
//! there is nothing to list/group for that half here).
//!
//! Deliberate deviations from the source, beyond the format cut:
//! - No `favorite`/`tags`/sqlite `flows` index table — `flows_panel`'s own
//!   persistence layer ([`persistence::DesignerDb`]) only carries the
//!   Designer's last-open-flow record and scoped-kv run history, not a
//!   general flows index. Rows sort by name instead of a favorite flag.
//! - "Add Task Chain" builds directly from every detected service
//!   (`workflow_engine::scan_project`) instead of opening Forge's
//!   review-and-edit wizard modal first — that wizard is a natural
//!   follow-up once this panel has users who hit its limits, not required
//!   to make "create a task chain" work.
//! - Opening a flow uses `Workspace::open_abs_path`, which just opens the
//!   `.flow.json` as a normal JSON file — there is no `designer_panel`
//!   canvas yet for it to route to (that's a separate, later port).
//! - Running a flow drives `workflow_engine::run_workflow` directly and
//!   records the result via [`persistence::append_flow_history`]; there is
//!   no live canvas to stream per-action status onto yet, so only the
//!   final per-action outcomes (not the `Running` transition) are kept.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::Result;
use gpui::{
    Action, App, AppContext as _, AsyncWindowContext, ClickEvent, Context, Entity, EventEmitter,
    FocusHandle, Focusable, InteractiveElement as _, IntoElement, ParentElement as _, Pixels,
    Render, SharedString, StatefulInteractiveElement as _, Styled as _, TaskExt as _, WeakEntity,
    Window, actions, div, prelude::FluentBuilder as _,
};
use gpui_component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    spinner::Spinner,
    tag::Tag,
    v_flex,
};
use project::Project;
use settings::Settings as _;
use workflow_engine::{
    ActionHistoryRecord, ActionMap, RunHistoryEntry, RunOutcome, RunStatus, StatusSink,
    WorkflowDefinition, run_workflow, scan_project, task_chain,
};
use workspace::{
    OpenOptions, Workspace,
    dock::{DockPosition, Panel, PanelEvent},
};

pub mod flows_settings;
pub mod persistence;

pub use flows_settings::FlowsSettings;

/// One `.flow.json` row.
struct WorkflowEntry {
    /// The flow's own `id` field (falls back to the filename stem for a
    /// file that doesn't parse as JSON at all, so a broken file still gets
    /// a row instead of silently vanishing from the list).
    id: String,
    name: String,
    path: PathBuf,
    last_run: Option<RunHistoryEntry>,
}

/// Scans `workflows_dir` for `*.flow.json` files. Matched by filename
/// suffix, not `Path::extension()` — that only splits on the last `.`,
/// which would turn `order-processing.flow.json` into stem
/// `"order-processing.flow"` instead of the real id `"order-processing"`.
/// Missing/unreadable directory (no flows created yet) is just an empty
/// list, not an error.
fn scan_workflows(workflows_dir: &Path) -> Vec<WorkflowEntry> {
    let Ok(entries) = std::fs::read_dir(workflows_dir) else {
        return Vec::new();
    };

    let mut workflows = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(file_name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let Some(stem) = file_name.strip_suffix(".flow.json") else {
            continue;
        };
        if stem.is_empty() {
            continue;
        }

        let (id, name) = match std::fs::read_to_string(&path)
            .ok()
            .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
        {
            Some(value) => {
                let id = value
                    .get("id")
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
                    .unwrap_or_else(|| stem.to_string());
                let name = value
                    .get("name")
                    .and_then(|v| v.as_str())
                    .map(str::to_string)
                    .unwrap_or_else(|| stem.to_string());
                (id, name)
            }
            None => (stem.to_string(), stem.to_string()),
        };

        workflows.push(WorkflowEntry {
            id,
            name,
            path,
            last_run: None,
        });
    }

    workflows.sort_by(|a, b| a.name.cmp(&b.name));
    workflows
}

/// Walks a flow's (possibly nested) action tree, collecting each action
/// id's display name and type — used to fill in
/// [`ActionHistoryRecord`]'s `name`/`type` fields from a [`StatusSink`]
/// callback, which only gets the bare id.
fn collect_action_meta(actions: &ActionMap, out: &mut HashMap<String, (String, String)>) {
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

actions!(
    flows_panel,
    [
        /// Toggles focus on the Flows panel.
        ToggleFocus
    ]
);

/// Registers the Flows panel's actions on every workspace. Call once at
/// app startup, alongside the other panels' `init` functions.
pub fn init(cx: &mut App) {
    cx.observe_new(|workspace: &mut Workspace, _, _| {
        workspace.register_action(|workspace, _: &ToggleFocus, window, cx| {
            workspace.toggle_panel_focus::<FlowsPanel>(window, cx);
        });
    })
    .detach();
}

pub struct FlowsPanel {
    focus_handle: FocusHandle,
    workspace: WeakEntity<Workspace>,
    project: Entity<Project>,
    entries: Vec<WorkflowEntry>,
    /// Flow ids currently running — a flow already in this set won't
    /// start a second concurrent run from another click of its own Run
    /// button (nothing stops two *different* flows running at once).
    running: std::collections::HashSet<String>,
    error: Option<String>,
}

impl FlowsPanel {
    pub async fn load(
        workspace: WeakEntity<Workspace>,
        mut cx: AsyncWindowContext,
    ) -> Result<Entity<Self>> {
        workspace.update_in(&mut cx, |workspace, window, cx| {
            FlowsPanel::new(workspace, window, cx)
        })
    }

    pub fn new(
        workspace: &mut Workspace,
        _window: &mut Window,
        cx: &mut Context<Workspace>,
    ) -> Entity<Self> {
        let workspace_handle = cx.entity().downgrade();
        let project = workspace.project().clone();
        cx.new(|cx| {
            let mut panel = Self {
                focus_handle: cx.focus_handle(),
                workspace: workspace_handle,
                project,
                entries: Vec::new(),
                running: std::collections::HashSet::new(),
                error: None,
            };
            panel.rescan(cx);
            panel
        })
    }

    /// The open workspace's first visible worktree root — `None` with no
    /// folder open, in which case there is nothing to scan/create/run
    /// against.
    fn project_root(&self, cx: &App) -> Option<PathBuf> {
        self.project
            .read(cx)
            .visible_worktrees(cx)
            .next()
            .map(|worktree| worktree.read(cx).abs_path().to_path_buf())
    }

    fn workflows_dir(&self, cx: &App) -> Option<PathBuf> {
        let root = self.project_root(cx)?;
        Some(root.join(FlowsSettings::get_global(cx).workflows_dir.clone()))
    }

    fn rescan(&mut self, cx: &mut Context<Self>) {
        let Some(dir) = self.workflows_dir(cx) else {
            self.entries = Vec::new();
            return;
        };
        let mut entries = scan_workflows(&dir);

        if let Some(root) = self.project_root(cx) {
            let namespace = root.to_string_lossy().to_string();
            let store = db::kvp::KeyValueStore::from_app_db(cx.global::<db::AppDatabase>());
            for entry in &mut entries {
                entry.last_run = persistence::read_flow_history(&store, &namespace, &entry.id)
                    .ok()
                    .and_then(|history| history.into_iter().next_back());
            }
        }

        self.entries = entries;
    }

    fn on_refresh_click(&mut self, _: &ClickEvent, _window: &mut Window, cx: &mut Context<Self>) {
        self.rescan(cx);
        cx.notify();
    }

    /// "+ New Workflow" — writes a blank `.flow.json` into the workspace's
    /// workflows directory and opens it as a JSON file.
    fn on_new_workflow_click(
        &mut self,
        _: &ClickEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(dir) = self.workflows_dir(cx) else {
            self.error = Some("Open a folder first — Flows needs an open workspace.".to_string());
            cx.notify();
            return;
        };
        if let Err(e) = std::fs::create_dir_all(&dir) {
            self.error = Some(format!("Couldn't create {}: {e}", dir.display()));
            cx.notify();
            return;
        }

        let slug = format!("flow-{}", chrono::Utc::now().format("%Y%m%d-%H%M%S"));
        let def = WorkflowDefinition {
            id: slug.clone(),
            name: slug.clone(),
            actions: ActionMap::new(),
            outputs: Default::default(),
        };
        let path = dir.join(format!("{slug}.flow.json"));
        let json = serde_json::to_string_pretty(&def).unwrap_or_default();
        if let Err(e) = std::fs::write(&path, json) {
            self.error = Some(format!("Couldn't write {}: {e}", path.display()));
            cx.notify();
            return;
        }

        self.error = None;
        self.rescan(cx);
        self.open_path(path, window, cx);
        cx.notify();
    }

    /// "+ Add Task Chain" — scans the open workspace for runnable services
    /// and, if it finds any, generates one `StartProcess`/`WaitForPort`
    /// pair per service into a new `.flow.json` (see the module doc's
    /// deviation note on why this skips Forge's review wizard).
    fn on_add_task_chain_click(
        &mut self,
        _: &ClickEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(root) = self.project_root(cx) else {
            self.error = Some("Open a folder first — Add Task Chain scans the open workspace.".to_string());
            cx.notify();
            return;
        };
        let Some(dir) = self.workflows_dir(cx) else {
            return;
        };

        let detected = scan_project(&root);
        if detected.is_empty() {
            self.error =
                Some("No runnable services detected in this workspace.".to_string());
            cx.notify();
            return;
        }

        let specs: Vec<_> = detected
            .into_iter()
            .map(|service| workflow_engine::ServiceSpec {
                name: service.name,
                command: service.command,
                args: service.args,
                cwd: service.relative_dir,
                port: service.port,
            })
            .collect();

        if let Err(e) = std::fs::create_dir_all(&dir) {
            self.error = Some(format!("Couldn't create {}: {e}", dir.display()));
            cx.notify();
            return;
        }

        let slug = format!("flow-{}", chrono::Utc::now().format("%Y%m%d-%H%M%S"));
        let def = task_chain::build(slug.clone(), slug.clone(), &specs);
        let path = dir.join(format!("{slug}.flow.json"));
        let json = serde_json::to_string_pretty(&def).unwrap_or_default();
        if let Err(e) = std::fs::write(&path, json) {
            self.error = Some(format!("Couldn't write {}: {e}", path.display()));
            cx.notify();
            return;
        }

        self.error = None;
        self.rescan(cx);
        self.open_path(path, window, cx);
        cx.notify();
    }

    fn open_path(&self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let Some(workspace) = self.workspace.upgrade() else {
            return;
        };
        workspace.update(cx, |workspace, cx| {
            workspace
                .open_abs_path(path, OpenOptions::default(), window, cx)
                .detach_and_log_err(cx);
        });
    }

    /// "Graph" — opens the flow on the `designer_panel` canvas instead of
    /// as raw JSON. `DesignerPanel::open` is infallible (a missing/broken
    /// file still opens a tab, with the failure shown in *that* tab's own
    /// inline banner — see `DesignerPanel::build`'s doc comment) — this
    /// only needs to bail before that if there's no workspace/project root
    /// to open into at all.
    fn open_graph(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let Some(workspace) = self.workspace.upgrade() else {
            return;
        };
        let Some(root) = self.project_root(cx) else {
            return;
        };
        let workspace_weak = self.workspace.clone();
        workspace.update(cx, |workspace, cx| {
            let project = workspace.project().clone();
            let item =
                designer_panel::DesignerPanel::open(path, root, workspace_weak, project, window, cx);
            workspace.add_item_to_active_pane(Box::new(item), None, true, window, cx);
        });
        self.error = None;
        cx.notify();
    }

    fn on_run_click(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.running.contains(&id) {
            return;
        }
        let Some(entry) = self.entries.iter().find(|e| e.id == id) else {
            return;
        };
        let path = entry.path.clone();
        let Some(root) = self.project_root(cx) else {
            return;
        };
        let namespace = root.to_string_lossy().to_string();

        let raw = match std::fs::read_to_string(&path) {
            Ok(raw) => raw,
            Err(e) => {
                self.error = Some(format!("Couldn't read {}: {e}", path.display()));
                cx.notify();
                return;
            }
        };
        let def: WorkflowDefinition = match serde_json::from_str(&raw) {
            Ok(def) => def,
            Err(e) => {
                self.error = Some(format!("{} isn't a valid flow: {e}", path.display()));
                cx.notify();
                return;
            }
        };

        let mut action_meta = HashMap::new();
        collect_action_meta(&def.actions, &mut action_meta);
        let records: Arc<Mutex<Vec<ActionHistoryRecord>>> = Arc::new(Mutex::new(Vec::new()));
        let records_for_sink = records.clone();
        let status = StatusSink::new(move |action_id, status, detail| {
            if matches!(status, RunStatus::Running) {
                return;
            }
            let (name, type_id) = action_meta
                .get(action_id)
                .cloned()
                .unwrap_or_else(|| (action_id.to_string(), "unknown".to_string()));
            let outcome = match status {
                RunStatus::Succeeded => RunOutcome::Succeeded,
                RunStatus::Failed => RunOutcome::Failed,
                RunStatus::Skipped => RunOutcome::Skipped,
                RunStatus::Running => return,
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
        self.running.insert(id.clone());
        cx.notify();

        let store = db::kvp::KeyValueStore::from_app_db(cx.global::<db::AppDatabase>());

        cx.spawn_in(window, async move |this, cx| {
            let result = run_workflow(def, root, status).await;
            let outcome = if result.is_ok() {
                RunOutcome::Succeeded
            } else {
                RunOutcome::Failed
            };
            let history_entry = RunHistoryEntry {
                id: format!("run-{}", chrono::Utc::now().timestamp_millis()),
                time: chrono::Utc::now().to_rfc3339(),
                outcome,
                actions: records.lock().unwrap().clone(),
            };
            let _ = persistence::append_flow_history(&store, &namespace, &id, history_entry).await;

            let _ = this.update_in(cx, |this, _window, cx| {
                this.running.remove(&id);
                this.rescan(cx);
                this.error = result.err().map(|e| format!("Run failed: {e}"));
                cx.notify();
            });
        })
        .detach();
    }

    fn render_row(&self, entry: &WorkflowEntry, cx: &mut Context<Self>) -> impl IntoElement {
        let id = entry.id.clone();
        let path = entry.path.clone();
        let running = self.running.contains(&entry.id);

        h_flex()
            .id(SharedString::from(format!("flow-row-{id}")))
            .w_full()
            .gap_2()
            .items_center()
            .px_3()
            .py_2()
            .hover(|d| d.bg(cx.theme().muted.opacity(0.3)))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_xs()
                    .child(entry.name.clone()),
            )
            .when_some(entry.last_run.as_ref(), |row, run| {
                row.child(last_run_tag(run))
            })
            .child(if running {
                Spinner::new().xsmall().into_any_element()
            } else {
                Button::new(SharedString::from(format!("flow-run-{id}")))
                    .ghost()
                    .xsmall()
                    .icon(IconName::Play)
                    .tooltip("Run this flow")
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.on_run_click(id.clone(), window, cx);
                    }))
                    .into_any_element()
            })
            .child(
                Button::new(SharedString::from(format!("flow-open-{}", entry.id)))
                    .ghost()
                    .xsmall()
                    .label("Open")
                    .tooltip("Open this flow")
                    .disabled(running)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_path(path.clone(), window, cx);
                    })),
            )
            .child({
                let path = entry.path.clone();
                Button::new(SharedString::from(format!("flow-graph-{}", entry.id)))
                    .ghost()
                    .xsmall()
                    .icon(IconName::Network)
                    .label("Graph")
                    .tooltip("Open this flow on the Designer canvas")
                    .disabled(running)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.open_graph(path.clone(), window, cx);
                    }))
            })
    }
}

fn last_run_tag(run: &RunHistoryEntry) -> impl IntoElement {
    match run.outcome {
        RunOutcome::Succeeded => Tag::success().outline().child("last run ok"),
        _ => Tag::danger().outline().child("last run failed"),
    }
    .xsmall()
}

impl Focusable for FlowsPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<PanelEvent> for FlowsPanel {}

impl Panel for FlowsPanel {
    fn persistent_name() -> &'static str {
        "Flows Panel"
    }

    fn panel_key() -> &'static str {
        "FlowsPanel"
    }

    fn position(&self, _window: &Window, _cx: &App) -> DockPosition {
        DockPosition::Left
    }

    fn position_is_valid(&self, position: DockPosition) -> bool {
        matches!(position, DockPosition::Left)
    }

    fn set_position(&mut self, _position: DockPosition, _window: &mut Window, _cx: &mut Context<Self>) {
        // Fixed to the left dock — see `position_is_valid`.
    }

    fn default_size(&self, _window: &Window, _cx: &App) -> Pixels {
        gpui::px(260.)
    }

    fn icon(&self, _window: &Window, _cx: &App) -> Option<ui::IconName> {
        Some(ui::IconName::GitBranch)
    }

    fn icon_tooltip(&self, _window: &Window, _cx: &App) -> Option<&'static str> {
        Some("Flows")
    }

    fn toggle_action(&self) -> Box<dyn Action> {
        Box::new(ToggleFocus)
    }

    fn activation_priority(&self) -> u32 {
        13
    }
}

impl Render for FlowsPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let has_project = self.project_root(cx).is_some();

        let header = v_flex()
            .flex_shrink_0()
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .justify_between()
                    .px_3()
                    .py_2()
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(Icon::new(IconName::Network).text_color(cx.theme().foreground))
                            .child(
                                div()
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .text_sm()
                                    .text_color(cx.theme().foreground)
                                    .child("Flows"),
                            ),
                    )
                    .child(
                        Button::new("flows-refresh")
                            .ghost()
                            .xsmall()
                            .icon(IconName::Redo2)
                            .tooltip("Refresh")
                            .on_click(cx.listener(Self::on_refresh_click)),
                    ),
            )
            .child(
                h_flex()
                    .gap_1()
                    .px_3()
                    .pb_2()
                    .child(
                        Button::new("flows-new-workflow")
                            .ghost()
                            .xsmall()
                            .icon(IconName::Plus)
                            .label("New Workflow")
                            .tooltip("Create a new blank .flow.json workflow")
                            .disabled(!has_project)
                            .on_click(cx.listener(Self::on_new_workflow_click)),
                    )
                    .child(
                        Button::new("flows-add-task-chain")
                            .ghost()
                            .xsmall()
                            .icon(IconName::Plus)
                            .label("Add Task Chain")
                            .tooltip("Scan the open workspace and generate a multi-service task chain")
                            .disabled(!has_project)
                            .on_click(cx.listener(Self::on_add_task_chain_click)),
                    ),
            )
            .when_some(self.error.clone(), |el, error| {
                el.child(
                    div()
                        .px_3()
                        .pb_2()
                        .text_xs()
                        .text_color(cx.theme().danger)
                        .child(error),
                )
            })
            .child(div().h(gpui::px(1.0)).w_full().bg(cx.theme().border));

        let body = if self.entries.is_empty() {
            div()
                .p_4()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(if has_project {
                    "No flows yet — click \"New Workflow\" to create one."
                } else {
                    "Open a folder to see its flows."
                })
                .into_any_element()
        } else {
            let rows: Vec<_> = self
                .entries
                .iter()
                .map(|entry| self.render_row(entry, cx).into_any_element())
                .collect();
            v_flex().w_full().children(rows).into_any_element()
        };

        v_flex()
            .id("flows-panel")
            .track_focus(&self.focus_handle(cx))
            .size_full()
            .bg(cx.theme().sidebar)
            .border_r_1()
            .border_color(cx.theme().border)
            .child(header)
            .child(
                div()
                    .id("flows-scroll")
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .overflow_y_scroll()
                    .child(body),
            )
    }
}
