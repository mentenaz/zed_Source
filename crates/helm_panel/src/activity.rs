//! Helm's activity screens for a repository: recent commits, GitHub Actions
//! workflow runs, and deployments.

use super::*;

impl HelmPanel {
    /// Loads `self.selected_repo`'s recent commits (Commits screen).
    pub(super) fn load_commits(&mut self, cx: &mut Context<Self>) {
        self.load_for_repo(
            cx,
            |repo, gh_state| async move {
                gh_list_recent_commits(repo.owner.login, repo.name, &gh_state).await
            },
            |this, commits| this.commits = commits,
        );
    }

    /// Loads `self.selected_repo`'s recent Actions/CI workflow runs.
    pub(super) fn load_workflow_runs(&mut self, cx: &mut Context<Self>) {
        self.load_for_repo(
            cx,
            |repo, gh_state| async move {
                gh_list_workflow_runs(repo.owner.login, repo.name, &gh_state).await
            },
            |this, runs| this.workflow_runs = runs,
        );
    }

    /// User opened a row on `WorkflowRuns`: opens the run's live job-status
    /// flow graph as its own workspace tab (`WorkflowRunItem`), reusing an
    /// already-open tab for the same run instead of duplicating it — unlike
    /// the old master-detail layout, this navigates away from the panel
    /// the same way opening a repo/issue/etc. elsewhere in Helm does.
    pub(super) fn select_workflow_run(&mut self, run: WorkflowRun, window: &mut Window, cx: &mut Context<Self>) {
        let Some(repo) = self.selected_repo.clone() else {
            return;
        };
        let owner = repo.owner.login;
        let name = repo.name;
        let gh_state = self.gh_state.clone();
        let Some(workspace) = self.workspace.upgrade() else {
            return;
        };
        workspace.update(cx, |workspace, cx| {
            open_workflow_run_tab(run, owner, name, gh_state, workspace, window, cx);
        });
    }

    /// Loads `self.selected_repo`'s deployments.
    pub(super) fn load_deployments(&mut self, cx: &mut Context<Self>) {
        self.load_for_repo(
            cx,
            |repo, gh_state| async move {
                gh_list_deployments(repo.owner.login, repo.name, &gh_state).await
            },
            |this, deployments| this.deployments = deployments,
        );
    }

    /// Shared loading/error/empty states for the read-only repo-activity
    /// lists below (Commits/Actions/Deployments/Tags) — same shape as
    /// `render_invitations`/`render_branches`, factored out since there are
    /// four of them.
    pub(super) fn activity_list_states(
        &self,
        loading_label: &'static str,
        error_label: &'static str,
        empty_label: &'static str,
        is_empty: bool,
        retry: impl Fn(&mut Self, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let muted_foreground = cx.theme().muted_foreground;

        if self.load_state == LoadState::Loading {
            return Some(
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
                            .child(div().text_sm().text_color(muted_foreground).child(loading_label)),
                    )
                    .into_any_element(),
            );
        }
        if self.load_state == LoadState::Error {
            return Some(
                v_flex()
                    .gap_3()
                    .p_4()
                    .child(div().text_sm().text_color(muted_foreground).child(error_label))
                    .child(
                        Button::new("activity-retry")
                            .outline()
                            .label("Retry")
                            .on_click(cx.listener(move |this, _, _, cx| retry(this, cx))),
                    )
                    .into_any_element(),
            );
        }
        if is_empty {
            return Some(
                v_flex()
                    .flex_1()
                    .items_center()
                    .justify_center()
                    .p_4()
                    .child(div().text_sm().text_color(muted_foreground).child(empty_label))
                    .into_any_element(),
            );
        }
        None
    }

    /// The Commits screen — recent commits with GitHub author avatars.
    pub(super) fn render_commits(&self, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(el) = self.activity_list_states(
            "Loading commits…",
            "Failed to load commits",
            "No commits found",
            self.commits.is_empty(),
            |this, cx| this.load_commits(cx),
            cx,
        ) {
            return el;
        }

        let foreground = cx.theme().foreground;
        let muted_foreground = cx.theme().muted_foreground;
        // Read-only list — no click action, so up/down + a selection
        // highlight only (see `render_branches`'s identical reasoning).
        let commits_len = self.commits.len();
        let commits_cursor = self.commits_list_cursor;

        v_flex()
            .id("helm-commits-list")
            .track_focus(&self.commits_list_focus)
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, window, cx| {
                window.focus(&this.commits_list_focus, cx);
            }))
            .key_context("HelmRowList")
            .on_action(cx.listener(move |this, _: &SelectNextRow, _, cx| {
                this.commits_list_cursor = step_selected(this.commits_list_cursor, commits_len, true);
                cx.notify();
            }))
            .on_action(cx.listener(move |this, _: &SelectPrevRow, _, cx| {
                this.commits_list_cursor =
                    step_selected(this.commits_list_cursor, commits_len, false);
                cx.notify();
            }))
            .py_1()
            .children(self.commits.iter().enumerate().map(|(ix, commit)| {
                let short_sha: String = commit.sha.chars().take(7).collect();
                let author = commit
                    .author
                    .as_ref()
                    .map(|a| a.login.clone())
                    .unwrap_or_else(|| "unknown".to_string());
                let avatar_url = commit
                    .author
                    .as_ref()
                    .map(|a| a.avatar_url.clone())
                    .unwrap_or_default();

                ListItem::new(format!("helm-commit-{}", commit.sha))
                    .selected(commits_cursor == Some(ix))
                    .child(
                        h_flex()
                            .items_center()
                            .gap_2()
                            .child(
                                Avatar::new()
                                    .src(avatar_url)
                                    .name(author.clone())
                                    .with_size(px(20.)),
                            )
                            .child(
                                div()
                                    .text_sm()
                                    .font_family("Cascadia Mono")
                                    .text_color(foreground)
                                    .child(short_sha),
                            )
                            .child(div().text_xs().text_color(muted_foreground).child(author)),
                    )
                    .into_any_element()
            }))
            .into_any_element()
    }

    /// The Actions screen — recent CI workflow runs with status/conclusion.
    /// Clicking a run opens its live job-status flow graph as its own
    /// workspace tab (`select_workflow_run` → `WorkflowRunItem`), rather
    /// than a detail pane embedded in this panel.
    pub(super) fn render_workflow_runs(&self, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(el) = self.activity_list_states(
            "Loading workflow runs…",
            "Failed to load workflow runs",
            "No workflow runs found",
            self.workflow_runs.is_empty(),
            |this, cx| this.load_workflow_runs(cx),
            cx,
        ) {
            return el;
        }

        let foreground = cx.theme().foreground;
        let muted_foreground = cx.theme().muted_foreground;
        let success = cx.theme().success;
        let danger = cx.theme().danger;
        let runs_len = self.workflow_runs.len();
        let runs_cursor = self.workflow_runs_list_cursor;
        let runs_for_open = self.workflow_runs.clone();

        v_flex()
            .id("helm-workflow-runs-list")
            .track_focus(&self.workflow_runs_list_focus)
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, window, cx| {
                window.focus(&this.workflow_runs_list_focus, cx);
            }))
            .key_context("HelmRowList")
            .on_action(cx.listener(move |this, _: &SelectNextRow, _, cx| {
                this.workflow_runs_list_cursor =
                    step_selected(this.workflow_runs_list_cursor, runs_len, true);
                cx.notify();
            }))
            .on_action(cx.listener(move |this, _: &SelectPrevRow, _, cx| {
                this.workflow_runs_list_cursor =
                    step_selected(this.workflow_runs_list_cursor, runs_len, false);
                cx.notify();
            }))
            .on_action(cx.listener(move |this, _: &OpenSelectedRow, window, cx| {
                let Some(run) = this.workflow_runs_list_cursor.and_then(|ix| runs_for_open.get(ix))
                else {
                    return;
                };
                this.select_workflow_run(run.clone(), window, cx);
            }))
            .py_1()
            .children(self.workflow_runs.iter().enumerate().map(|(ix, run)| {
                let status_label = run.conclusion.clone().unwrap_or_else(|| run.status.clone());
                let color = match status_label.as_str() {
                    "success" => success,
                    "failure" | "cancelled" | "timed_out" => danger,
                    _ => muted_foreground,
                };
                let run_for_click = run.clone();

                ListItem::new(format!("helm-run-{}", run.id))
                    .selected(runs_cursor == Some(ix))
                    .child(
                        v_flex()
                            .gap_0p5()
                            .min_w_0()
                            .child(
                                div()
                                    .text_sm()
                                    .font_semibold()
                                    .text_color(foreground)
                                    .child(run.name.clone()),
                            )
                            .child(div().text_xs().text_color(muted_foreground).child(format!(
                                "#{} · {}",
                                run.run_number,
                                run.head_branch.clone().unwrap_or_default()
                            ))),
                    )
                    .suffix(move |_, _| {
                        div().text_xs().text_color(color).child(status_label.clone())
                    })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.workflow_runs_list_cursor = Some(ix);
                        this.select_workflow_run(run_for_click.clone(), window, cx);
                    }))
                    .into_any_element()
            }))
            .into_any_element()
    }

    /// The Deployments screen.
    pub(super) fn render_deployments(&self, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(el) = self.activity_list_states(
            "Loading deployments…",
            "Failed to load deployments",
            "No deployments found",
            self.deployments.is_empty(),
            |this, cx| this.load_deployments(cx),
            cx,
        ) {
            return el;
        }

        let foreground = cx.theme().foreground;
        let muted_foreground = cx.theme().muted_foreground;
        let deployments_len = self.deployments.len();
        let deployments_cursor = self.deployments_list_cursor;

        v_flex()
            .id("helm-deployments-list")
            .track_focus(&self.deployments_list_focus)
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, window, cx| {
                window.focus(&this.deployments_list_focus, cx);
            }))
            .key_context("HelmRowList")
            .on_action(cx.listener(move |this, _: &SelectNextRow, _, cx| {
                this.deployments_list_cursor =
                    step_selected(this.deployments_list_cursor, deployments_len, true);
                cx.notify();
            }))
            .on_action(cx.listener(move |this, _: &SelectPrevRow, _, cx| {
                this.deployments_list_cursor =
                    step_selected(this.deployments_list_cursor, deployments_len, false);
                cx.notify();
            }))
            .py_1()
            .children(self.deployments.iter().enumerate().map(|(ix, dep)| {
                let short_sha: String = dep.sha.chars().take(7).collect();
                ListItem::new(format!("helm-deployment-{}", dep.id))
                    .selected(deployments_cursor == Some(ix))
                    .child(
                        v_flex()
                            .gap_0p5()
                            .min_w_0()
                            .child(
                                div()
                                    .text_sm()
                                    .font_semibold()
                                    .text_color(foreground)
                                    .child(dep.environment.clone()),
                            )
                            .child(div().text_xs().text_color(muted_foreground).child(format!(
                                "{} · {short_sha}",
                                dep.r#ref
                            ))),
                    )
                    .suffix({
                        let status = dep.status.clone();
                        move |_, _| div().text_xs().text_color(muted_foreground).child(status.clone())
                    })
                    .into_any_element()
            }))
            .into_any_element()
    }
}
