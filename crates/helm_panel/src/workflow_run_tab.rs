//! The workflow-run tab: one GitHub Actions run opened as its own workspace
//! tab, with its jobs drawn as a graph and polled until the run finishes.

use super::*;

/// `WorkflowRunItem`'s job-node body — `gpui_flow`'s built-in fallback
/// renderer (used whenever a node has no `node_type`/registered renderer,
/// which every job node here doesn't) hardcodes `text_color(0x1a1a1a)`, a
/// near-black that's invisible against a dark theme's `node_bg_color`
/// (`cx.theme().popover`, set on this graph) — hence job names not being
/// visible. Same `popover_foreground` theme color `designer_panel::render_leaf`
/// uses for its own leaf nodes against the identical `popover` background.
pub(super) fn render_workflow_job_node(node: &FlowNode, _window: &mut Window, cx: &mut App) -> gpui::AnyElement {
    div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .px_2()
        .text_sm()
        .font_weight(gpui::FontWeight::MEDIUM)
        .text_color(cx.theme().popover_foreground)
        .child(node.label.to_string())
        .into_any_element()
}

/// Color for a GitHub Actions status/conclusion pair — shared between a
/// run's own header badge and every job node's `accent_border` on
/// `WorkflowRunItem` (`status`: queued/in_progress/completed; `conclusion`:
/// success/failure/cancelled/timed_out/action_required/skipped/neutral,
/// only set once `status` is "completed").
pub(super) fn workflow_status_color(cx: &App, status: &str, conclusion: &Option<String>) -> gpui::Hsla {
    match conclusion.as_deref() {
        Some("success") => cx.theme().success,
        Some("failure") | Some("cancelled") | Some("timed_out") | Some("action_required") => {
            cx.theme().danger
        }
        Some("skipped") | Some("neutral") => cx.theme().muted_foreground,
        _ if status == "in_progress" || status == "queued" => cx.theme().primary,
        _ => cx.theme().muted_foreground,
    }
}

/// Finds an already-open `WorkflowRunItem` tab for `run` in the active pane
/// and activates it, or opens a new one — same "find existing, else create"
/// dedup `database_panel`'s `open_schema_graph` uses for its own flow-graph
/// tabs.
pub(super) fn open_workflow_run_tab(
    run: WorkflowRun,
    owner: String,
    repo_name: String,
    gh_state: Arc<GhState>,
    workspace: &mut Workspace,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    let run_id = run.id;
    let existing = workspace
        .active_pane()
        .read(cx)
        .items()
        .find_map(|item| item.downcast::<WorkflowRunItem>())
        .filter(|tab| tab.read(cx).run.id == run_id);

    if let Some(existing) = existing {
        workspace.activate_item(&existing, true, true, window, cx);
    } else {
        let tab = cx.new(|cx| WorkflowRunItem::new(run, owner, repo_name, gh_state, cx));
        workspace.add_item_to_active_pane(Box::new(tab), None, true, window, cx);
    }
}

/// A single workflow run's live job-status flow graph, opened as its own
/// workspace tab from `HelmPanel::select_workflow_run` rather than embedded
/// in the panel — one node per job (`sync_flow_nodes`), colored by status
/// (`workflow_status_color`/`FlowNode::accent_border`: "a running/succeeded/
/// failed node in a workflow executor sets a raw color; this crate just
/// draws it"). No edges: the jobs API doesn't expose each job's `needs:`
/// dependencies (only the workflow YAML does), so nodes are laid out
/// left-to-right in API order instead.
pub(super) struct WorkflowRunItem {
    pub(super) run: WorkflowRun,
    pub(super) jobs: Vec<WorkflowJob>,
    pub(super) flow_state: Entity<FlowState>,
    pub(super) graph: Entity<FlowGraph>,
    pub(super) controls: Entity<Controls>,
    pub(super) load_state: LoadState,
    pub(super) error_msg: String,
    /// Re-fetches `run`/`jobs` every few seconds for as long as the run's
    /// status isn't "completed". A `Task` cancels on drop, so this just
    /// stops on its own once the tab is closed (the entity, and this field
    /// with it, gets dropped) — nothing to clean up explicitly.
    pub(super) poll: Option<Task<()>>,
    pub(super) owner: String,
    pub(super) repo_name: String,
    pub(super) gh_state: Arc<GhState>,
    pub(super) focus_handle: FocusHandle,
}

impl WorkflowRunItem {
    pub(super) fn new(
        run: WorkflowRun,
        owner: String,
        repo_name: String,
        gh_state: Arc<GhState>,
        cx: &mut Context<Self>,
    ) -> Self {
        let state = cx.new(|_| FlowState::new(Vec::new(), Vec::new()));
        let graph = cx.new(|cx| {
            FlowGraph::new(state.clone(), cx)
                .bg_color(hex(cx.theme().sidebar))
                .grid_color(hex(cx.theme().sidebar_border))
                .node_bg_color(hex(cx.theme().popover))
                .node_border_color(hex(cx.theme().border))
                .default_renderer(render_workflow_job_node)
        });
        let controls = cx.new(|_| Controls::new(state.clone()));
        let mut this = Self {
            run,
            jobs: Vec::new(),
            flow_state: state,
            graph,
            controls,
            load_state: LoadState::Idle,
            error_msg: String::new(),
            poll: None,
            owner,
            repo_name,
            gh_state,
            focus_handle: cx.focus_handle(),
        };
        this.poll_jobs(cx);
        this
    }

    /// Fetches this run's own status plus its jobs/steps, applies them to
    /// the flow canvas, then — as long as the run hasn't finished — waits
    /// 8s and does it again. 8s matches the grain the GitHub Actions web UI
    /// itself polls at — fast enough to feel live, far short of the REST
    /// rate limit even for an hour-long build.
    pub(super) fn poll_jobs(&mut self, cx: &mut Context<Self>) {
        let run_id = self.run.id;
        let owner = self.owner.clone();
        let name = self.repo_name.clone();
        let gh_state = self.gh_state.clone();

        self.load_state = LoadState::Loading;
        cx.notify();

        self.poll = Some(cx.spawn(async move |this, cx| {
            loop {
                let (owner, name, gh_state) = (owner.clone(), name.clone(), gh_state.clone());
                let (run_result, jobs_result) = on_tokio(async move {
                    let run = gh_get_workflow_run(owner.clone(), name.clone(), run_id, &gh_state)
                        .await;
                    let jobs = gh_get_workflow_run_jobs(owner, name, run_id, &gh_state).await;
                    (run, jobs)
                })
                .await;

                let mut is_completed = false;
                let alive = this
                    .update(cx, |this, cx| {
                        match run_result {
                            Ok(updated_run) => {
                                is_completed = updated_run.status == "completed";
                                this.run = updated_run;
                                this.load_state = LoadState::Idle;
                                this.error_msg.clear();
                            }
                            Err(e) => {
                                this.load_state = LoadState::Error;
                                this.error_msg = e.to_string();
                            }
                        }
                        if let Ok(jobs) = jobs_result {
                            this.jobs = jobs;
                            this.sync_flow_nodes(cx);
                        }
                        cx.notify();
                    })
                    .is_ok();

                if !alive || is_completed {
                    break;
                }

                cx.background_executor().timer(Duration::from_secs(8)).await;
            }
        }));
    }

    /// Pushes `jobs`' current status onto `flow_state`'s nodes — one node
    /// per job. Mutates existing nodes in place and appends new ones as
    /// jobs appear, rather than rebuilding the whole node list, so a user's
    /// pan/zoom isn't reset on every 8s poll tick — only fits the view the
    /// first time real jobs appear.
    pub(super) fn sync_flow_nodes(&mut self, cx: &mut Context<Self>) {
        let jobs = self.jobs.clone();
        let colors: Vec<u32> = jobs
            .iter()
            .map(|job| hex(workflow_status_color(cx, &job.status, &job.conclusion)))
            .collect();
        let flow_state = self.flow_state.clone();

        flow_state.update(cx, |state, cx| {
            let was_empty = state.nodes.is_empty();
            for (ix, job) in jobs.iter().enumerate() {
                let id: NodeId = job.id.to_string().into();
                let color = colors[ix];
                if let Some(node) = state.nodes.iter_mut().find(|n| n.id == id) {
                    node.accent_border = Some(color);
                } else {
                    let x = ix as f32 * 220.0;
                    state.nodes.push(
                        FlowNode::new(id, x, 0.0)
                            .label(job.name.clone())
                            .size(180.0, 70.0)
                            .accent_border(color),
                    );
                }
            }
            state.rebuild_lookup();
            if was_empty && !state.nodes.is_empty() {
                state.fit_view(60.0, 900.0, 400.0);
            }
            cx.notify();
        });
    }
}

impl EventEmitter<()> for WorkflowRunItem {}

impl Focusable for WorkflowRunItem {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for WorkflowRunItem {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let muted_foreground = cx.theme().muted_foreground;
        let foreground = cx.theme().foreground;
        let border = cx.theme().border;

        let run_status_label = self.run.conclusion.clone().unwrap_or_else(|| self.run.status.clone());
        let run_color = workflow_status_color(cx, &self.run.status, &self.run.conclusion);
        let open_url = self.run.html_url.clone();
        let run_name = self.run.name.clone();
        let run_number = self.run.run_number;
        let head_branch = self.run.head_branch.clone().unwrap_or_default();

        let header = v_flex()
            .gap_1()
            .px_3()
            .py_2()
            .child(
                h_flex()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .child(
                        div()
                            .text_sm()
                            .font_semibold()
                            .text_color(foreground)
                            .child(run_name),
                    )
                    .child(
                        Button::new("workflow-run-open-browser")
                            .ghost()
                            .xsmall()
                            .icon(IconName::ExternalLink)
                            .label("Open in browser")
                            .on_click(move |_, _, cx| cx.open_url(&open_url)),
                    ),
            )
            .child(
                h_flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .px_1p5()
                            .py_0p5()
                            .rounded_md()
                            .bg(run_color.opacity(0.2))
                            .text_color(run_color)
                            .text_xs()
                            .child(run_status_label),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted_foreground)
                            .child(format!("#{} · {}", run_number, head_branch)),
                    ),
            );

        let divider = div().h_px().w_full().bg(border);

        if self.jobs.is_empty() && self.load_state == LoadState::Loading {
            return v_flex()
                .size_full()
                .child(header)
                .child(divider)
                .child(
                    v_flex()
                        .flex_1()
                        .items_center()
                        .justify_center()
                        .p_4()
                        .child(
                            h_flex()
                                .gap_2()
                                .items_center()
                                .child(Spinner::new().small())
                                .child(
                                    div()
                                        .text_sm()
                                        .text_color(muted_foreground)
                                        .child("Loading jobs…"),
                                ),
                        ),
                )
                .into_any_element();
        }
        if self.jobs.is_empty() && self.load_state == LoadState::Error {
            return v_flex()
                .size_full()
                .child(header)
                .child(divider)
                .child(
                    v_flex()
                        .gap_3()
                        .p_4()
                        .child(
                            div()
                                .text_sm()
                                .text_color(muted_foreground)
                                .child(self.error_msg.clone()),
                        )
                        .child(
                            Button::new("workflow-run-graph-retry")
                                .outline()
                                .label("Retry")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.poll_jobs(cx);
                                })),
                        ),
                )
                .into_any_element();
        }
        if self.jobs.is_empty() {
            return v_flex()
                .size_full()
                .child(header)
                .child(divider)
                .child(
                    v_flex()
                        .flex_1()
                        .items_center()
                        .justify_center()
                        .p_4()
                        .child(
                            div()
                                .text_sm()
                                .text_color(muted_foreground)
                                .child("No jobs reported for this run yet"),
                        ),
                )
                .into_any_element();
        }

        v_flex()
            .size_full()
            .child(header)
            .child(divider)
            .child(
                div()
                    .relative()
                    .flex_1()
                    .w_full()
                    .child(self.graph.clone())
                    .child(
                        div()
                            .absolute()
                            .bottom(gpui::px(12.))
                            .left(gpui::px(12.))
                            .child(self.controls.clone()),
                    ),
            )
            .into_any_element()
    }
}

impl Item for WorkflowRunItem {
    type Event = ();

    fn tab_content_text(&self, _detail: usize, _cx: &App) -> SharedString {
        format!("{} #{}", self.run.name, self.run.run_number).into()
    }

    fn tab_tooltip_text(&self, _cx: &App) -> Option<SharedString> {
        Some(format!("{} #{}", self.run.name, self.run.run_number).into())
    }
}
