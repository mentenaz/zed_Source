//! Helm's activity screens for a repository: recent commits, GitHub Actions
//! workflow runs, and deployments.

use super::*;

impl HelmPanel {
    /// Loads `self.selected_repo`'s recent commits (Commits screen).
    pub(super) fn load_commits(&mut self, cx: &mut Context<Self>) {
        self.load_commits_page(1, cx);
    }

    pub(super) fn load_commits_page(&mut self, page: u32, cx: &mut Context<Self>) {
        self.load_repo_page(cx, |this| &mut this.commits, page, |repo| {
            requests::recent_commits(&repo.owner.login, &repo.name)
        });
    }

    /// Loads `self.selected_repo`'s recent Actions/CI workflow runs.
    pub(super) fn load_workflow_runs(&mut self, cx: &mut Context<Self>) {
        self.load_workflow_runs_page(1, cx);
    }

    pub(super) fn load_workflow_runs_page(&mut self, page: u32, cx: &mut Context<Self>) {
        self.load_section_page(
            cx,
            |this| &mut this.workflow_runs,
            page,
            move |repo, gh_state| {
                let request = requests::workflow_runs(&repo.owner.login, &repo.name);
                peek_page_under(gh_state, request, "workflow_runs", page, PAGE_SIZE)
            },
            move |repo, gh_state| async move {
                // GitHub wraps this list in an object, under `workflow_runs`.
                let request = requests::workflow_runs(&repo.owner.login, &repo.name);
                fetch_page_under::<WorkflowRun>(&gh_state, request, "workflow_runs", page, PAGE_SIZE)
                    .await
            },
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
        self.load_deployments_page(1, cx);
    }

    pub(super) fn load_deployments_page(&mut self, page: u32, cx: &mut Context<Self>) {
        self.load_repo_page(cx, |this| &mut this.deployments, page, |repo| {
            requests::deployments(&repo.owner.login, &repo.name)
        });
    }


    /// The Commits screen — recent commits with GitHub author avatars.
    pub(super) fn render_commits(&self, cx: &mut Context<Self>) -> impl IntoElement {
        self.list_screen(
            self.commits
                .paged_status(|this, page, cx| this.load_commits_page(page, cx)),
            &self.commits_list,
            None,
            ListLabels {
                loading: "Loading commits…",
                error: "Failed to load commits",
                empty: "No commits found",
            },
            |this, cx| this.load_commits_page(this.commits.page, cx),
            cx,
        )
    }

    /// The Actions screen — recent CI workflow runs with status/conclusion.
    /// Clicking a run opens its live job-status flow graph as its own
    /// workspace tab (`select_workflow_run` → `WorkflowRunItem`), rather
    /// than a detail pane embedded in this panel.
    pub(super) fn render_workflow_runs(&self, cx: &mut Context<Self>) -> impl IntoElement {
        self.list_screen(
            self.workflow_runs
                .paged_status(|this, page, cx| this.load_workflow_runs_page(page, cx)),
            &self.workflow_runs_list,
            None,
            ListLabels {
                loading: "Loading workflow runs…",
                error: "Failed to load workflow runs",
                empty: "No workflow runs found",
            },
            |this, cx| this.load_workflow_runs_page(this.workflow_runs.page, cx),
            cx,
        )
    }

    /// The Deployments screen.
    pub(super) fn render_deployments(&self, cx: &mut Context<Self>) -> impl IntoElement {
        self.list_screen(
            self.deployments
                .paged_status(|this, page, cx| this.load_deployments_page(page, cx)),
            &self.deployments_list,
            None,
            ListLabels {
                loading: "Loading deployments…",
                error: "Failed to load deployments",
                empty: "No deployments found",
            },
            |this, cx| this.load_deployments_page(this.deployments.page, cx),
            cx,
        )
    }
}

/// One row of the Commits screen: the author's avatar, the short commit id
/// and the author's login.
pub(super) fn commit_row(ix: usize, commit: &CommitSummary, cx: &App) -> ListItem {
    let foreground = cx.theme().foreground;
    let muted_foreground = cx.theme().muted_foreground;
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
    ListItem::new(("helm-commit", ix)).child(
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
}

/// One row of the Actions screen: the workflow's name, its run number and
/// branch, and how the run ended (or its status while it is still going).
pub(super) fn workflow_run_row(ix: usize, run: &WorkflowRun, cx: &App) -> ListItem {
    let foreground = cx.theme().foreground;
    let muted_foreground = cx.theme().muted_foreground;
    let status_label = run.conclusion.clone().unwrap_or_else(|| run.status.clone());
    let color = match status_label.as_str() {
        "success" => cx.theme().success,
        "failure" | "cancelled" | "timed_out" => cx.theme().danger,
        _ => muted_foreground,
    };
    ListItem::new(("helm-run", ix))
        .child(
            v_flex()
                .gap_0p5()
                .min_w_0()
                .child(
                    div()
                        .truncate()
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
        .suffix(move |_, _| div().text_xs().text_color(color).child(status_label.clone()))
}

/// One row of the Deployments screen: the environment, the ref and short
/// commit deployed, and the deployment's status.
pub(super) fn deployment_row(ix: usize, deployment: &Deployment, cx: &App) -> ListItem {
    let foreground = cx.theme().foreground;
    let muted_foreground = cx.theme().muted_foreground;
    let short_sha: String = deployment.sha.chars().take(7).collect();
    let status = deployment.status.clone();
    ListItem::new(("helm-deployment", ix))
        .child(
            v_flex()
                .gap_0p5()
                .min_w_0()
                .child(
                    div()
                        .truncate()
                        .text_sm()
                        .font_semibold()
                        .text_color(foreground)
                        .child(deployment.environment.clone()),
                )
                .child(div().text_xs().text_color(muted_foreground).child(format!(
                    "{} · {short_sha}",
                    deployment.r#ref
                ))),
        )
        .suffix(move |_, _| {
            div()
                .text_xs()
                .text_color(muted_foreground)
                .child(status.clone())
        })
}
