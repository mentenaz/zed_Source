use std::{collections::HashMap, sync::Arc};

use editor::{Editor, SelectionEffects, scroll::Autoscroll};
use git::{GitHostingProviderRegistry, parse_git_remote_url};
use gpui::{
    Action as _, App, AppContext as _, ClipboardItem, Context, Entity, EventEmitter, FocusHandle,
    Focusable, ImageFormat, ImageSource, IntoElement, ObjectFit, ParentElement as _, Render,
    SharedString, Styled as _, StyledImage as _, WeakEntity, Window, div, img,
    prelude::FluentBuilder as _, px,
};
use gpui_component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, IndexPath, Selectable as _, Sizable as _,
    StyledExt,
    button::{Button, ButtonVariants as _},
    h_resizable,
    input::{Input, InputEvent, InputState},
    list::ListItem,
    resizable_panel,
    scroll::ScrollableElement as _,
    searchable_list::SearchableVec,
    select::{Select, SelectEvent, SelectState},
    separator::Separator,
    tag::Tag,
    tree::{Tree, TreeEntry as TreeRowEntry, TreeItem, TreeState},
    v_flex,
};
use helm_backend::{
    github::{
        BlobData, BlobKind, CommitDetail, CommitSummary, CompareResult, GhError, GhState, Page,
        Pull, Readme, RefMovement, Repo, RepoTree, ResolvedRef, SearchCodeMatch, TreeEntry,
        TreeEntryKind, TreeLoadResult, build_tree, compare_ref_movement, fetch_blob_at_path,
        fetch_commit_detail, fetch_compare, fetch_page, fetch_page_under, fetch_readme,
        fetch_readme_at_path, fetch_repo_tree, gh_auth_status, gh_get_branches,
        gh_get_current_user, gh_get_repo, gh_list_tags, requests, resolve_ref, search_code_page,
    },
    on_tokio,
};
use helm_ui::{LoadState, Loaded, Section};
use language::{Capability, LanguageRegistry, Point};
use markdown::{Markdown, MarkdownElement, MarkdownFont, MarkdownStyle};
use workspace::{Item, Workspace};

gpui::actions!(
    helm_workspace,
    [
        AuthorizeRepositoryWorkspace,
        CloneRepositoryForWorkspace,
        OpenWorkspaceWorkflowRun
    ]
);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WorkspaceSection {
    Overview,
    Code,
    Commits,
    Compare,
    Search,
    PullRequests,
    Issues,
    Actions,
}

struct OverviewCounts {
    open_issues: u64,
    open_pull_requests: u64,
}

#[derive(Clone)]
struct LocalClone {
    root: std::path::PathBuf,
    branch: Option<String>,
    commit_sha: Option<String>,
}

const MAX_OVERVIEW_PULL_PAGES: u32 = 50;
const CODE_SEARCH_PAGE_SIZE: usize = 20;

impl WorkspaceSection {
    const ALL: [(Self, &'static str); 8] = [
        (Self::Overview, "Overview"),
        (Self::Code, "Code"),
        (Self::Commits, "Commits"),
        (Self::Compare, "Compare"),
        (Self::Search, "Search"),
        (Self::PullRequests, "Pull requests"),
        (Self::Issues, "Issues"),
        (Self::Actions, "Actions"),
    ];
}

/// Opens the repository Workspace, reusing an already-open tab for the same
/// repository in the active pane.
pub fn open_workspace_tab(
    repo: Repo,
    gh_state: Arc<GhState>,
    workspace: &mut Workspace,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    let languages = workspace.app_state().languages.clone();
    let repository = repo.full_name.clone();
    let pane = workspace.active_pane().clone();
    let existing = pane
        .read(cx)
        .items()
        .find_map(|item| item.downcast::<WorkspaceTab>())
        .filter(|tab| {
            tab.read(cx)
                .repo
                .full_name
                .eq_ignore_ascii_case(&repository)
        });

    if let Some(existing) = existing {
        workspace.activate_item(&existing, true, true, window, cx);
    } else {
        let tab = cx.new(|cx| WorkspaceTab::new(repo, gh_state, languages, pane, window, cx));
        workspace.add_item_to_active_pane(Box::new(tab), None, true, window, cx);
    }
}

pub struct WorkspaceTab {
    repo: Repo,
    gh_state: Arc<GhState>,
    repository_loading: bool,
    repository_error: Option<GhError>,
    repository_scope_missing: bool,
    account: Loaded<String>,
    focus_handle: FocusHandle,
    ref_selector: Entity<SelectState<SearchableVec<String>>>,
    refs_error: Option<String>,
    ref_name: String,
    ref_is_tag: bool,
    resolved_ref: Option<ResolvedRef>,
    ref_load: Loaded<ResolvedRef>,
    pending_ref: Option<ResolvedRef>,
    overview_counts: Loaded<OverviewCounts>,
    last_commit: Loaded<CommitDetail>,
    commits: Section<CommitSummary>,
    commits_path: Option<String>,
    commit_tags: HashMap<String, Vec<String>>,
    selected_commit_sha: Option<String>,
    selected_commit: Loaded<CommitDetail>,
    selected_commit_file: Option<String>,
    patch_buffer: Option<Entity<language::Buffer>>,
    patch_editor: Option<Entity<Editor>>,
    compare_base: Entity<InputState>,
    compare_head: Entity<InputState>,
    compare_result: Loaded<CompareResult>,
    branch_pull_request: Loaded<Option<Pull>>,
    branch_pull_status: Loaded<helm_backend::github::CombinedCommitStatus>,
    latest_workflow_run: Loaded<Option<helm_backend::github::WorkflowRun>>,
    workspace_issues: Section<helm_backend::github::Issue>,
    workspace_pulls: Section<Pull>,
    selected_discussion: Option<(u64, String, String, String, String)>,
    discussion_comments: Loaded<Vec<helm_backend::github::Comment>>,
    code_search_input: Entity<InputState>,
    code_search: Section<SearchCodeMatch>,
    code_search_query: String,
    code_search_error: Option<String>,
    search_result_line: Option<(String, u32)>,
    pending_search_line: Option<(String, String, Option<String>)>,
    code_commit_sha: String,
    languages: Arc<LanguageRegistry>,
    tree_state: Entity<TreeState>,
    tree_filter: Entity<InputState>,
    tree: Loaded<RepoTree>,
    tree_nodes: Vec<helm_backend::github::TreeNode>,
    tree_entries: HashMap<String, TreeEntry>,
    tree_directories: HashMap<String, ()>,
    tree_empty: bool,
    tree_selection: Option<String>,
    tree_missing_path: Option<String>,
    readme: Loaded<Readme>,
    readme_markdown: Option<Entity<Markdown>>,
    folder_readme: Loaded<Option<Readme>>,
    folder_readme_markdown: Option<Entity<Markdown>>,
    blob: Loaded<BlobData>,
    blob_cache: HashMap<String, BlobData>,
    blob_buffer: Option<Entity<language::Buffer>>,
    blob_editor: Option<Entity<Editor>>,
    blob_image: Option<Arc<gpui::Image>>,
    blob_error: Option<String>,
    blob_language_error: Option<String>,
    active_file_path: Option<String>,
    active_folder_path: Option<String>,
    section: WorkspaceSection,
    workspace: Option<WeakEntity<Workspace>>,
    confirm_local_open: Option<(String, Option<String>, Option<String>)>,
    local_open_error: Option<String>,
}

impl WorkspaceTab {
    fn render_workspace_pulls(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let branch_pull = self
            .branch_pull_request
            .value
            .as_ref()
            .and_then(Option::as_ref);
        let status = self.branch_pull_status.value.as_ref();
        v_flex()
            .size_full()
            .min_h_0()
            .gap_2()
            .p_3()
            .when_some(branch_pull, |this, pull| {
                this.child(
                    v_flex()
                        .gap_1()
                        .p_3()
                        .bg(cx.theme().muted.opacity(0.15))
                        .child(
                            div()
                                .text_sm()
                                .font_semibold()
                                .child(format!("Pull request for {}", self.ref_name)),
                        )
                        .child(
                            div()
                                .text_sm()
                                .child(format!("#{} · {}", pull.number, pull.title)),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(format!("{} → {}", pull.head.r#ref, pull.base.r#ref)),
                        )
                        .child(status_line(
                            status.map_or_else(
                                || "Commit checks unavailable.".to_string(),
                                |status| {
                                    format!(
                                        "Checks: {} · {} statuses",
                                        status.state, status.total_count
                                    )
                                },
                            ),
                            cx,
                        ))
                        .when(!pull.html_url.is_empty(), |this| {
                            let url = pull.html_url.clone();
                            this.child(
                                Button::new("helm-workspace-open-branch-pull")
                                    .ghost()
                                    .small()
                                    .label("Open pull request on GitHub")
                                    .on_click(move |_, _, cx| cx.open_url(&url)),
                            )
                        }),
                )
            })
            .when(
                self.branch_pull_request.state == LoadState::Loading,
                |this| {
                    this.child(status_line(
                        "Checking for a pull request on this branch…",
                        cx,
                    ))
                },
            )
            .when(
                self.branch_pull_request.state == LoadState::Idle
                    && self
                        .branch_pull_request
                        .value
                        .as_ref()
                        .is_some_and(Option::is_none),
                |this| {
                    this.child(status_line(
                        format!(
                            "No pull request currently uses {} as its head branch.",
                            self.ref_name
                        ),
                        cx,
                    ))
                },
            )
            .when(self.branch_pull_request.state == LoadState::Error, |this| {
                this.child(status_line(
                    format!(
                        "Could not load this branch's pull request: {}",
                        self.branch_pull_request.error
                    ),
                    cx,
                ))
            })
            .when(self.branch_pull_status.state == LoadState::Error, |this| {
                this.child(status_line(
                    format!(
                        "Could not load commit checks: {}",
                        self.branch_pull_status.error
                    ),
                    cx,
                ))
            })
            .child(
                gpui_component::h_flex()
                    .items_center()
                    .justify_between()
                    .child(div().text_sm().font_semibold().child("Open pull requests"))
                    .child(
                        gpui_component::h_flex()
                            .gap_1()
                            .child(
                                Button::new("helm-workspace-pulls-previous")
                                    .ghost()
                                    .small()
                                    .label("Previous")
                                    .disabled(
                                        self.workspace_pulls.page <= 1
                                            || self.workspace_pulls.state == LoadState::Loading,
                                    )
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.load_workspace_pulls(
                                            this.workspace_pulls.page.saturating_sub(1),
                                            cx,
                                        );
                                    })),
                            )
                            .child(div().text_xs().child(format!(
                                "{} / {}",
                                self.workspace_pulls.page, self.workspace_pulls.last_page
                            )))
                            .child(
                                Button::new("helm-workspace-pulls-next")
                                    .ghost()
                                    .small()
                                    .label("Next")
                                    .disabled(
                                        self.workspace_pulls.page >= self.workspace_pulls.last_page
                                            || self.workspace_pulls.state == LoadState::Loading,
                                    )
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.load_workspace_pulls(
                                            this.workspace_pulls.page + 1,
                                            cx,
                                        );
                                    })),
                            ),
                    ),
            )
            .when(self.workspace_pulls.state == LoadState::Loading, |this| {
                this.child(status_line("Loading pull requests…", cx))
            })
            .when(self.workspace_pulls.state == LoadState::Error, |this| {
                this.child(status_line(
                    format!(
                        "Could not load pull requests: {}",
                        self.workspace_pulls.error
                    ),
                    cx,
                ))
            })
            .child(v_flex().flex_1().min_h_0().overflow_y_scrollbar().children(
                self.workspace_pulls.items.iter().map(|pull| {
                    let pull = pull.clone();
                    Button::new(format!("helm-workspace-pull-{}", pull.number))
                        .ghost()
                        .child(helm_ui::pull_row(0, &pull, cx))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.open_discussion(
                                pull.number,
                                pull.title.clone(),
                                pull.body.clone(),
                                if pull.merged { "merged" } else { &pull.state }.into(),
                                pull.html_url.clone(),
                                cx,
                            );
                        }))
                }),
            ))
            .child(self.render_discussion_detail(cx))
    }

    fn render_workspace_issues(&self, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .size_full()
            .min_h_0()
            .gap_2()
            .p_3()
            .child(
                gpui_component::h_flex()
                    .items_center()
                    .justify_between()
                    .child(div().text_sm().font_semibold().child("Open issues"))
                    .child(
                        gpui_component::h_flex()
                            .gap_1()
                            .child(
                                Button::new("helm-workspace-issues-previous")
                                    .ghost()
                                    .small()
                                    .label("Previous")
                                    .disabled(
                                        self.workspace_issues.page <= 1
                                            || self.workspace_issues.state == LoadState::Loading,
                                    )
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.load_workspace_issues(
                                            this.workspace_issues.page.saturating_sub(1),
                                            cx,
                                        );
                                    })),
                            )
                            .child(div().text_xs().child(format!(
                                "{} / {}",
                                self.workspace_issues.page, self.workspace_issues.last_page
                            )))
                            .child(
                                Button::new("helm-workspace-issues-next")
                                    .ghost()
                                    .small()
                                    .label("Next")
                                    .disabled(
                                        self.workspace_issues.page
                                            >= self.workspace_issues.last_page
                                            || self.workspace_issues.state == LoadState::Loading,
                                    )
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.load_workspace_issues(
                                            this.workspace_issues.page + 1,
                                            cx,
                                        );
                                    })),
                            ),
                    ),
            )
            .when(self.workspace_issues.state == LoadState::Loading, |this| {
                this.child(status_line("Loading issues…", cx))
            })
            .when(self.workspace_issues.state == LoadState::Error, |this| {
                this.child(status_line(
                    format!("Could not load issues: {}", self.workspace_issues.error),
                    cx,
                ))
            })
            .child(v_flex().flex_1().min_h_0().overflow_y_scrollbar().children(
                self.workspace_issues.items.iter().map(|issue| {
                    let issue = issue.clone();
                    Button::new(format!("helm-workspace-issue-{}", issue.number))
                        .ghost()
                        .child(helm_ui::issue_row(0, &issue, cx))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.open_discussion(
                                issue.number,
                                issue.title.clone(),
                                issue.body.clone(),
                                issue.state.clone(),
                                issue.html_url.clone(),
                                cx,
                            );
                        }))
                }),
            ))
            .child(self.render_discussion_detail(cx))
    }

    fn render_discussion_detail(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let Some((number, title, body, state, url)) = self.selected_discussion.as_ref() else {
            return status_line(
                "Select an issue or pull request to view its discussion.",
                cx,
            )
            .into_any_element();
        };
        v_flex()
            .max_h(px(300.))
            .min_h_0()
            .gap_2()
            .p_3()
            .border_t_1()
            .border_color(cx.theme().border)
            .child(
                div()
                    .text_sm()
                    .font_semibold()
                    .child(format!("#{number} · {title} · {state}")),
            )
            .child(
                div()
                    .max_h(px(100.))
                    .overflow_y_scrollbar()
                    .text_sm()
                    .child(if body.is_empty() {
                        "No description.".to_string()
                    } else {
                        body.clone()
                    }),
            )
            .when(!url.is_empty(), |this| {
                let url = url.clone();
                this.child(
                    Button::new("helm-workspace-open-discussion")
                        .ghost()
                        .small()
                        .label("Open on GitHub")
                        .on_click(move |_, _, cx| cx.open_url(&url)),
                )
            })
            .when(
                self.discussion_comments.state == LoadState::Loading,
                |this| this.child(status_line("Loading comments…", cx)),
            )
            .when(self.discussion_comments.state == LoadState::Error, |this| {
                this.child(status_line(
                    format!(
                        "Could not load comments: {}",
                        self.discussion_comments.error
                    ),
                    cx,
                ))
            })
            .when_some(self.discussion_comments.value.as_ref(), |this, comments| {
                this.child(
                    v_flex()
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scrollbar()
                        .gap_2()
                        .children(comments.iter().map(|comment| {
                            v_flex()
                                .gap_1()
                                .p_2()
                                .border_1()
                                .border_color(cx.theme().border)
                                .child(div().text_xs().child(format!(
                                    "{} · {}",
                                    comment
                                        .user
                                        .as_ref()
                                        .map(|user| user.login.as_str())
                                        .unwrap_or("Unknown"),
                                    comment.created_at
                                )))
                                .child(
                                    div().text_sm().child(
                                        comment
                                            .body
                                            .clone()
                                            .unwrap_or_else(|| "No comment body.".into()),
                                    ),
                                )
                        })),
                )
            })
            .into_any_element()
    }

    fn render_workspace_actions(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let run = self
            .latest_workflow_run
            .value
            .as_ref()
            .and_then(Option::as_ref);
        v_flex()
            .size_full()
            .min_h_0()
            .gap_3()
            .p_4()
            .child(div().text_lg().font_semibold().child("Latest Actions run"))
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(format!("Branch: {}", self.ref_name)),
            )
            .when(
                self.latest_workflow_run.state == LoadState::Loading,
                |this| this.child(status_line("Loading the latest workflow run…", cx)),
            )
            .when(self.latest_workflow_run.state == LoadState::Error, |this| {
                this.child(status_line(
                    format!(
                        "Could not load workflow runs: {}",
                        self.latest_workflow_run.error
                    ),
                    cx,
                ))
            })
            .when(
                self.latest_workflow_run.state == LoadState::Idle
                    && self
                        .latest_workflow_run
                        .value
                        .as_ref()
                        .is_some_and(Option::is_none),
                |this| {
                    this.child(status_line(
                        "No Actions runs were found for this branch.",
                        cx,
                    ))
                },
            )
            .when_some(run, |this, run| {
                let url = run.html_url.clone();
                this.child(
                    v_flex()
                        .gap_2()
                        .p_3()
                        .bg(cx.theme().muted.opacity(0.15))
                        .child(helm_ui::workflow_run_row(0, run, cx))
                        .child(
                            div()
                                .text_sm()
                                .font_semibold()
                                .child(format!("{} · run #{}", run.name, run.run_number)),
                        )
                        .child(div().text_sm().child(format!(
                            "Status: {} · conclusion: {}",
                            run.status,
                            run.conclusion.as_deref().unwrap_or("pending")
                        )))
                        .child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(format!("{} · {}", run.event, run.created_at)),
                        )
                        .child(
                            gpui_component::h_flex()
                                .gap_2()
                                .child(
                                    Button::new("helm-workspace-open-workflow-run")
                                        .primary()
                                        .label("Open run details")
                                        .on_click(move |_, window, cx| {
                                            window.dispatch_action(
                                                OpenWorkspaceWorkflowRun.boxed_clone(),
                                                cx,
                                            )
                                        }),
                                )
                                .when(!url.is_empty(), |this| {
                                    this.child(
                                        Button::new("helm-workspace-open-workflow-github")
                                            .ghost()
                                            .label("Open on GitHub")
                                            .on_click(move |_, _, cx| cx.open_url(&url)),
                                    )
                                }),
                        ),
                )
            })
    }

    pub fn repository(&self) -> &Repo {
        &self.repo
    }

    pub fn workflow_run_context(
        &self,
    ) -> Option<(
        helm_backend::github::WorkflowRun,
        String,
        String,
        Arc<GhState>,
    )> {
        Some((
            self.latest_workflow_run.value.as_ref()?.clone()?,
            self.repo.owner.login.clone(),
            self.repo.name.clone(),
            self.gh_state.clone(),
        ))
    }

    pub fn requested_clone_file(&self) -> Option<&str> {
        self.active_file_path.as_deref()
    }

    fn matching_local_clone(&self, cx: &mut Context<Self>) -> Option<LocalClone> {
        let workspace = self.workspace.as_ref()?.upgrade()?;
        let project = workspace.read(cx).project().clone();
        if project.read(cx).is_remote() {
            return None;
        }
        let provider_registry = GitHostingProviderRegistry::default_global(cx);
        let matching_clones = project
            .read(cx)
            .repositories(cx)
            .values()
            .filter_map(|repository| {
                let snapshot = repository.read(cx).snapshot();
                let matches_repo = [
                    snapshot.remote_upstream_url.as_deref(),
                    snapshot.remote_origin_url.as_deref(),
                ]
                .into_iter()
                .flatten()
                .any(|remote_url| {
                    parse_git_remote_url(provider_registry.clone(), remote_url).is_some_and(
                        |(_, remote)| {
                            remote.owner.eq_ignore_ascii_case(&self.repo.owner.login)
                                && remote.repo.eq_ignore_ascii_case(&self.repo.name)
                        },
                    )
                });
                matches_repo.then(|| LocalClone {
                    root: snapshot.work_directory_abs_path.to_path_buf(),
                    branch: snapshot.branch.map(|branch| branch.ref_name.to_string()),
                    commit_sha: snapshot.head_commit.map(|commit| commit.sha.to_string()),
                })
            })
            .collect::<Vec<_>>();
        matching_clones.into_iter().min_by_key(|local_clone| {
            local_clone_preference(
                local_clone,
                &self.ref_name,
                self.ref_is_tag,
                &self.code_commit_sha,
            )
        })
    }

    fn request_local_file_open(
        &mut self,
        path: String,
        confirm_mismatch: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(local_clone) = self.matching_local_clone(cx) else {
            self.confirm_local_open = None;
            self.local_open_error =
                Some("No matching local GitHub checkout is open in this workspace.".into());
            cx.notify();
            return;
        };
        let branch_mismatch =
            self.ref_is_tag || local_clone.branch.as_deref() != Some(self.ref_name.as_str());
        let commit_mismatch =
            local_clone.commit_sha.as_deref() != Some(self.code_commit_sha.as_str());
        let ref_mismatch = branch_mismatch || commit_mismatch;
        let confirmation_matches = confirm_mismatch
            && self.confirm_local_open.as_ref().is_some_and(
                |(confirmed_path, confirmed_sha, confirmed_branch)| {
                    confirmed_path == &path
                        && *confirmed_sha == local_clone.commit_sha
                        && *confirmed_branch == local_clone.branch
                },
            );
        if ref_mismatch && !confirmation_matches {
            self.confirm_local_open = Some((
                path,
                local_clone.commit_sha.clone(),
                local_clone.branch.clone(),
            ));
            self.local_open_error = None;
            cx.notify();
            return;
        }
        let Some(file_path) = safe_local_file_path(&local_clone.root, &path) else {
            self.confirm_local_open = None;
            self.local_open_error = Some("The repository returned an invalid file path.".into());
            cx.notify();
            return;
        };
        match std::fs::metadata(&file_path) {
            Ok(metadata) if metadata.is_file() => {}
            Ok(_) => {
                self.confirm_local_open = None;
                self.local_open_error =
                    Some("This path is not a file in the local checkout.".into());
                cx.notify();
                return;
            }
            Err(error) => {
                self.confirm_local_open = None;
                self.local_open_error = Some(format!(
                    "This file is not available in the local checkout: {error}"
                ));
                cx.notify();
                return;
            }
        }
        let Some(workspace) = self
            .workspace
            .as_ref()
            .and_then(|workspace| workspace.upgrade())
        else {
            self.confirm_local_open = None;
            self.local_open_error = Some("The editor workspace is no longer available.".into());
            cx.notify();
            return;
        };
        self.confirm_local_open = None;
        self.local_open_error = None;
        let task = workspace.update(cx, |workspace, cx| {
            workspace.open_abs_path(file_path, Default::default(), window, cx)
        });
        cx.spawn_in(window, async move |this, cx| {
            if let Err(error) = task.await {
                this.update_in(cx, |this, _, cx| {
                    this.local_open_error =
                        Some(format!("Could not open the local file: {error:#}"));
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }

    fn new(
        repo: Repo,
        gh_state: Arc<GhState>,
        languages: Arc<LanguageRegistry>,
        pane: Entity<workspace::pane::Pane>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let ref_name = repo.default_branch.clone();
        let refs = if repo.disabled || ref_name.is_empty() {
            Vec::new()
        } else {
            vec![format!("branch: {ref_name}")]
        };
        let ref_selector = cx.new(|cx| {
            SelectState::new(
                SearchableVec::new(refs),
                (!repo.disabled && !ref_name.is_empty()).then_some(IndexPath::default()),
                window,
                cx,
            )
            .searchable(true)
        });
        let tree_state = cx.new(|cx| TreeState::new(cx));
        let tree_filter = cx.new(|cx| InputState::new(window, cx).placeholder("Filter files…"));
        let compare_base = cx.new(|cx| InputState::new(window, cx).placeholder("Base ref"));
        let compare_head = cx.new(|cx| InputState::new(window, cx).placeholder("Head ref"));
        let code_search_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("Search code in this repository…"));
        cx.subscribe(&code_search_input, |this, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::PressEnter { .. }) {
                this.load_code_search(1, cx);
            }
        })
        .detach();
        cx.subscribe_in(&ref_selector, window, Self::on_ref_selected)
            .detach();
        cx.subscribe(&tree_filter, |this, _, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                this.update_tree_items(cx);
            }
        })
        .detach();
        cx.observe(&tree_state, |this, state, cx| {
            let selected = state
                .read(cx)
                .selected_item()
                .map(|item| item.id.to_string());
            if selected != this.tree_selection {
                this.tree_selection = selected.clone();
                if let Some(path) = selected {
                    this.on_tree_selection(path, cx);
                }
            }
        })
        .detach();
        cx.subscribe_in(
            &pane,
            window,
            |this, pane, event: &workspace::pane::Event, _, cx| {
                if !matches!(event, workspace::pane::Event::ActivateItem { .. }) {
                    return;
                }
                let this_id = cx.entity().entity_id();
                let is_active = pane
                    .read(cx)
                    .active_item()
                    .is_some_and(|item| item.item_id() == this_id);
                if is_active && this.resolved_ref.is_some() {
                    this.refresh_ref(cx);
                }
            },
        )
        .detach();

        let mut tab = Self {
            repo,
            gh_state,
            repository_loading: false,
            repository_error: None,
            repository_scope_missing: false,
            account: Loaded::default(),
            focus_handle: cx.focus_handle(),
            ref_selector,
            refs_error: None,
            ref_name,
            ref_is_tag: false,
            resolved_ref: None,
            ref_load: Loaded::default(),
            pending_ref: None,
            overview_counts: Loaded::default(),
            last_commit: Loaded::default(),
            commits: Section::default(),
            commits_path: None,
            commit_tags: HashMap::default(),
            selected_commit_sha: None,
            selected_commit: Loaded::default(),
            selected_commit_file: None,
            patch_buffer: None,
            patch_editor: None,
            compare_base,
            compare_head,
            compare_result: Loaded::default(),
            branch_pull_request: Loaded::default(),
            branch_pull_status: Loaded::default(),
            latest_workflow_run: Loaded::default(),
            workspace_issues: Section::default(),
            workspace_pulls: Section::default(),
            selected_discussion: None,
            discussion_comments: Loaded::default(),
            code_search_input,
            code_search: Section::default(),
            code_search_query: String::new(),
            code_search_error: None,
            search_result_line: None,
            pending_search_line: None,
            code_commit_sha: String::new(),
            languages,
            workspace: Workspace::for_window(window, cx).map(|workspace| workspace.downgrade()),
            tree_state,
            tree_filter,
            tree: Loaded::default(),
            tree_nodes: Vec::new(),
            tree_entries: HashMap::default(),
            tree_directories: HashMap::default(),
            tree_empty: false,
            tree_selection: None,
            tree_missing_path: None,
            readme: Loaded::default(),
            readme_markdown: None,
            folder_readme: Loaded::default(),
            folder_readme_markdown: None,
            blob: Loaded::default(),
            blob_cache: HashMap::default(),
            blob_buffer: None,
            blob_editor: None,
            blob_image: None,
            blob_error: None,
            blob_language_error: None,
            active_file_path: None,
            active_folder_path: None,
            section: WorkspaceSection::Code,
            confirm_local_open: None,
            local_open_error: None,
        };
        if !tab.repo.disabled {
            tab.load_repository(window, cx);
        }
        tab
    }

    fn load_repository(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let owner = self.repo.owner.login.clone();
        let name = self.repo.name.clone();
        let gh_state = self.gh_state.clone();
        let account_load = self.account.begin();
        self.repository_loading = true;
        self.repository_error = None;
        self.repository_scope_missing = false;
        cx.notify();
        cx.spawn_in(window, async move |this, cx| {
            let (result, auth_status, user_info) = on_tokio(async move {
                let result = gh_get_repo(owner, name, &gh_state).await;
                let auth_status = gh_auth_status().await;
                // `/user` gives the display name (and login); prefer the
                // display name for the "Signed in as" line. Falls back to the
                // `gh auth status` account if the API call fails.
                let user_info = gh_get_current_user(&gh_state).await.ok();
                (result, auth_status, user_info)
            })
            .await;
            let scope_missing = result.as_ref().is_err_and(GhError::is_permission)
                && matches!(
                    &auth_status,
                    Ok(Some(info)) if !info.scopes.iter().any(|scope| scope == "repo")
                );
            this.update_in(cx, |this, window, cx| {
                this.repository_loading = false;
                if this.account.is_current(account_load) {
                    match user_info {
                        Some(user) => this.account.finish(Ok(
                            user.name.unwrap_or_else(|| user.login.clone()),
                        )),
                        None => match auth_status {
                            Ok(Some(info)) => this.account.finish(Ok(info.account)),
                            Ok(None) => this.account.finish(Ok("Not signed in".into())),
                            Err(error) => this.account.finish(Err(error)),
                        },
                    }
                }
                match result {
                    Ok(repo) => {
                        this.repo = repo;
                        this.load_overview_counts(cx);
                        this.load_refs_and_resolve(window, cx);
                    }
                    Err(error) => {
                        this.repository_scope_missing = scope_missing;
                        this.repository_error = Some(error);
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn load_overview_counts(&mut self, cx: &mut Context<Self>) {
        let owner = self.repo.owner.login.clone();
        let name = self.repo.name.clone();
        let total_open_issues = self.repo.open_issues_count;
        let has_issues = self.repo.has_issues;
        let gh_state = self.gh_state.clone();
        let load = self.overview_counts.begin();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = on_tokio(async move {
                if total_open_issues == 0 {
                    return Ok::<_, GhError>(OverviewCounts {
                        open_issues: 0,
                        open_pull_requests: 0,
                    });
                }
                let mut page_number = 1;
                let mut open_pull_requests = 0u64;
                let request = requests::pulls(&owner, &name, "open");
                for _ in 0..MAX_OVERVIEW_PULL_PAGES {
                    let page: helm_backend::github::Page<Pull> =
                        fetch_page(&gh_state, request.clone(), page_number, 100).await?;
                    open_pull_requests += u64::try_from(page.items.len()).unwrap_or(u64::MAX);
                    if !page.has_next() {
                        let open_issues = if has_issues {
                            total_open_issues.saturating_sub(open_pull_requests)
                        } else {
                            0
                        };
                        return Ok(OverviewCounts {
                            open_issues,
                            open_pull_requests,
                        });
                    }
                    page_number = page.page + 1;
                }
                Err(GhError::Other(
                    "Could not count more than 5,000 open pull requests.".to_string(),
                ))
            })
            .await;
            this.update(cx, |this, cx| {
                if !this.overview_counts.is_current(load) {
                    return;
                }
                this.overview_counts
                    .finish(result.map_err(|error| error.to_string()));
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn load_last_commit(&mut self, sha: String, cx: &mut Context<Self>) {
        let owner = self.repo.owner.login.clone();
        let name = self.repo.name.clone();
        let gh_state = self.gh_state.clone();
        let load = self.last_commit.begin();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result =
                on_tokio(async move { fetch_commit_detail(&gh_state, &owner, &name, &sha).await })
                    .await;
            this.update(cx, |this, cx| {
                if !this.last_commit.is_current(load) {
                    return;
                }
                this.last_commit
                    .finish(result.map_err(|error| error.to_string()));
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn load_commits(&mut self, page: u32, path: Option<String>, cx: &mut Context<Self>) {
        let owner = self.repo.owner.login.clone();
        let repo = self.repo.name.clone();
        let gh_state = self.gh_state.clone();
        let request = path.as_deref().map_or_else(
            || requests::recent_commits_for_ref(&owner, &repo, &self.ref_name),
            |path| requests::commits_for_path(&owner, &repo, path, &self.ref_name),
        );
        if self.commits_path != path {
            self.commits.clear();
        }
        self.commits_path = path;
        let load = self.commits.begin_page(page);
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result =
                on_tokio(
                    async move { fetch_page::<CommitSummary>(&gh_state, request, page, 30).await },
                )
                .await
                .map_err(|error| error.to_string());
            this.update(cx, |this, cx| {
                if this.commits.is_current(load) {
                    this.commits.finish_page(result);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn select_commit(&mut self, sha: String, cx: &mut Context<Self>) {
        if self.selected_commit_sha.as_deref() == Some(sha.as_str()) {
            return;
        }
        self.selected_commit_sha = Some(sha.clone());
        self.selected_commit_file = None;
        self.patch_buffer = None;
        self.patch_editor = None;
        self.selected_commit.clear();
        let owner = self.repo.owner.login.clone();
        let repo = self.repo.name.clone();
        let gh_state = self.gh_state.clone();
        let load = self.selected_commit.begin();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result =
                on_tokio(async move { fetch_commit_detail(&gh_state, &owner, &repo, &sha).await })
                    .await
                    .map_err(|error| error.to_string());
            this.update(cx, |this, cx| {
                if this.selected_commit.is_current(load) {
                    this.selected_commit.finish(result);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn select_patch(&mut self, path: String, patch: Option<String>, cx: &mut Context<Self>) {
        self.selected_commit_file = Some(path);
        self.patch_buffer = None;
        self.patch_editor = None;
        if let Some(patch) = patch {
            let buffer = cx.new(|cx| {
                let mut buffer = language::Buffer::local(patch, cx);
                buffer.set_capability(Capability::ReadOnly, cx);
                buffer
            });
            self.patch_buffer = Some(buffer);
        }
        cx.notify();
    }

    fn compare_refs(&mut self, cx: &mut Context<Self>) {
        let base = self.compare_base.read(cx).value().trim().to_string();
        let head = self.compare_head.read(cx).value().trim().to_string();
        self.compare_result.clear();
        if base.is_empty() || head.is_empty() {
            self.compare_result
                .finish(Err("Enter both a base ref and a head ref.".into()));
            cx.notify();
            return;
        }
        let owner = self.repo.owner.login.clone();
        let repo = self.repo.name.clone();
        let gh_state = self.gh_state.clone();
        let load = self.compare_result.begin();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result =
                on_tokio(
                    async move { fetch_compare(&gh_state, &owner, &repo, &base, &head).await },
                )
                .await
                .map_err(|error| error.to_string());
            this.update(cx, |this, cx| {
                if this.compare_result.is_current(load) {
                    this.compare_result.finish(result);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn load_code_search(&mut self, page: u32, cx: &mut Context<Self>) {
        let query = if page == 1 {
            self.code_search_input.read(cx).value().trim().to_string()
        } else {
            self.code_search_query.clone()
        };
        if query.is_empty() {
            self.code_search.clear();
            self.code_search_query.clear();
            self.code_search_error = Some("Enter a search query.".into());
            cx.notify();
            return;
        }
        if query != self.code_search_query {
            self.code_search.clear();
        }
        let qualified_query = format!("ref:{} {query}", self.ref_name);
        let full_name = self.repo.full_name.clone();
        let gh_state = self.gh_state.clone();
        self.code_search_query = query;
        self.code_search_error = None;
        let load = self.code_search.begin_page(page);
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = on_tokio(async move {
                search_code_page(
                    &gh_state,
                    &qualified_query,
                    Some(&full_name),
                    page,
                    CODE_SEARCH_PAGE_SIZE as u32,
                )
                .await
            })
            .await
            .map_err(|error| error.to_string());
            this.update(cx, |this, cx| {
                if this.code_search.is_current(load) {
                    this.code_search.finish_page(result);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn open_code_search_result(
        &mut self,
        result: SearchCodeMatch,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !result.repo.eq_ignore_ascii_case(&self.repo.full_name) {
            self.code_search_error = Some("This result is outside the current repository.".into());
            cx.notify();
            return;
        }
        if !self.tree_entries.contains_key(&result.path) {
            self.code_search_error = Some(format!(
                "{} is not present in the selected ref {}.",
                result.path, self.ref_name
            ));
            cx.notify();
            return;
        }
        self.code_search_error = None;
        self.search_result_line = None;
        self.pending_search_line = Some((
            result.path.clone(),
            self.code_search_query.clone(),
            result.line.clone(),
        ));
        self.tree_filter
            .update(cx, |filter, cx| filter.set_value("", window, cx));
        self.active_file_path = Some(result.path.clone());
        self.active_folder_path = None;
        self.tree_selection = Some(result.path.clone());
        self.section = WorkspaceSection::Code;
        self.open_tree_entry(result.path.clone(), cx);
        self.update_tree_items(cx);
        cx.notify();
    }

    fn load_code_for_ref(&mut self, commit_sha: String, cx: &mut Context<Self>) {
        self.load_context_for_ref(cx);
        let preserve_path = self
            .active_file_path
            .clone()
            .or_else(|| self.active_folder_path.clone());
        let owner = self.repo.owner.login.clone();
        let repo = self.repo.name.clone();
        let gh_state = self.gh_state.clone();
        self.tree.clear();
        self.readme.clear();
        self.folder_readme.clear();
        self.commits.clear();
        self.commits_path = None;
        self.code_search.clear();
        self.code_search_error = None;
        self.search_result_line = None;
        self.pending_search_line = None;
        self.selected_commit_sha = None;
        self.selected_commit.clear();
        self.selected_commit_file = None;
        self.patch_buffer = None;
        self.patch_editor = None;
        self.compare_result.clear();
        let tree_load = self.tree.begin();
        let readme_load = self.readme.begin();
        self.code_commit_sha = commit_sha.clone();
        self.tree_entries.clear();
        self.tree_nodes.clear();
        self.tree_directories.clear();
        self.tree_empty = false;
        self.tree_missing_path = None;
        self.active_file_path = None;
        self.active_folder_path = None;
        self.tree_selection = None;
        self.confirm_local_open = None;
        self.local_open_error = None;
        self.readme_markdown = None;
        self.folder_readme_markdown = None;
        self.blob.clear();
        self.blob_buffer = None;
        self.blob_editor = None;
        self.blob_image = None;
        self.blob_error = None;
        self.blob_language_error = None;
        self.tree_state
            .update(cx, |state, cx| state.set_items(Vec::new(), cx));
        cx.notify();

        cx.spawn(async move |this, cx| {
            let result =
                on_tokio(
                    async move { fetch_repo_tree(&gh_state, &owner, &repo, &commit_sha).await },
                )
                .await;
            this.update(cx, |this, cx| {
                if !this.tree.is_current(tree_load) {
                    return;
                }
                match result {
                    Ok(TreeLoadResult::EmptyRepository) => {
                        this.tree_empty = true;
                        this.tree.finish(Ok(RepoTree::default()));
                    }
                    Ok(TreeLoadResult::Tree(tree)) => {
                        let nodes = build_tree(&tree.entries);
                        for entry in &tree.entries {
                            if entry.kind != TreeEntryKind::Tree {
                                this.tree_entries.insert(entry.path.clone(), entry.clone());
                            }
                        }
                        index_tree_directories(&nodes, &mut this.tree_directories);
                        this.tree_nodes = nodes;
                        this.tree.finish(Ok(tree));
                        let retained_file = preserve_path.as_ref().filter(|path| {
                            this.tree_entries
                                .get(*path)
                                .is_some_and(|entry| entry.kind == TreeEntryKind::Blob)
                        });
                        let retained_directory = preserve_path
                            .as_ref()
                            .filter(|path| this.tree_directories.contains_key(*path))
                            .cloned()
                            .or_else(|| {
                                preserve_path.as_deref().and_then(|path| {
                                    nearest_tree_directory(path, &this.tree_directories)
                                })
                            });
                        if let Some(path) = retained_file {
                            this.active_file_path = Some(path.clone());
                            this.tree_selection = Some(path.clone());
                            this.open_tree_entry(path.clone(), cx);
                        } else if let Some(path) = retained_directory {
                            this.active_folder_path = Some(path.clone());
                            this.tree_selection = Some(path.clone());
                            this.tree_missing_path =
                                preserve_path.filter(|missing| missing != &path);
                            this.load_folder_readme(path, cx);
                        } else if preserve_path.is_some() {
                            this.tree_missing_path = preserve_path.clone();
                        }
                    }
                    Err(error) => this.tree.finish(Err(error.to_string())),
                }
                this.update_tree_items(cx);
                cx.notify();
            })
            .ok();
        })
        .detach();

        let owner = self.repo.owner.login.clone();
        let repo = self.repo.name.clone();
        let gh_state = self.gh_state.clone();
        let readme_commit_sha = self.code_commit_sha.clone();
        cx.spawn(async move |this, cx| {
            let result = on_tokio(async move {
                fetch_readme(&gh_state, &owner, &repo, &readme_commit_sha).await
            })
            .await;
            this.update(cx, |this, cx| {
                if !this.readme.is_current(readme_load) {
                    return;
                }
                match result {
                    Ok(readme) => {
                        let source = readme.content.clone();
                        let languages = Some(this.languages.clone());
                        this.readme_markdown =
                            Some(cx.new(|cx| Markdown::new(source.into(), languages, None, cx)));
                        this.readme.finish(Ok(readme));
                    }
                    Err(error) => this.readme.finish(Err(error.to_string())),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn load_context_for_ref(&mut self, cx: &mut Context<Self>) {
        self.branch_pull_request.clear();
        self.branch_pull_status.clear();
        self.latest_workflow_run.clear();
        self.workspace_issues.clear();
        self.workspace_pulls.clear();
        self.selected_discussion = None;
        self.discussion_comments.clear();
        if self.ref_is_tag || self.ref_name.is_empty() {
            return;
        }
        let owner = self.repo.owner.login.clone();
        let repo = self.repo.name.clone();
        let branch = self.ref_name.clone();
        let branch_load = self.branch_pull_request.begin();
        let status_load = self.branch_pull_status.begin();
        let gh_state = self.gh_state.clone();
        cx.spawn(async move |this, cx| {
            let result = on_tokio(async move {
                let page: Page<Pull> = fetch_page(
                    &gh_state,
                    requests::pull_for_head(&owner, &repo, &format!("{owner}:{branch}")),
                    1,
                    100,
                )
                .await?;
                let pull = page
                    .items
                    .into_iter()
                    .find(|pull| pull.head.r#ref == branch);
                let status = match pull.as_ref() {
                    Some(pull) => Some(
                        helm_backend::github::gh_get_combined_status(
                            &owner,
                            &repo,
                            &pull.head.sha,
                            &gh_state,
                        )
                        .await,
                    ),
                    None => None,
                };
                Ok::<_, GhError>((pull, status))
            })
            .await;
            this.update(cx, |this, cx| {
                if this.branch_pull_request.is_current(branch_load) {
                    match result {
                        Ok((pull, status)) => {
                            this.branch_pull_request.finish(Ok(pull));
                            if this.branch_pull_status.is_current(status_load) {
                                match status {
                                    Some(Ok(status)) => {
                                        this.branch_pull_status.finish(Ok(status));
                                    }
                                    Some(Err(error)) => {
                                        this.branch_pull_status.finish(Err(error.to_string()));
                                    }
                                    None => this.branch_pull_status.clear(),
                                }
                            }
                        }
                        Err(error) => {
                            this.branch_pull_request.finish(Err(error.to_string()));
                            if this.branch_pull_status.is_current(status_load) {
                                this.branch_pull_status.clear();
                            }
                        }
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();

        let owner = self.repo.owner.login.clone();
        let repo = self.repo.name.clone();
        let branch = self.ref_name.clone();
        let gh_state = self.gh_state.clone();
        let run_load = self.latest_workflow_run.begin();
        cx.spawn(async move |this, cx| {
            let result = on_tokio(async move {
                let page: Page<helm_backend::github::WorkflowRun> = fetch_page_under(
                    &gh_state,
                    requests::workflow_runs_for_branch(&owner, &repo, &branch),
                    "workflow_runs",
                    1,
                    50,
                )
                .await?;
                Ok::<_, GhError>(page.items.into_iter().next())
            })
            .await;
            this.update(cx, |this, cx| {
                if this.latest_workflow_run.is_current(run_load) {
                    this.latest_workflow_run
                        .finish(result.map_err(|error| error.to_string()));
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn load_workspace_issues(&mut self, page: u32, cx: &mut Context<Self>) {
        let owner = self.repo.owner.login.clone();
        let repo = self.repo.name.clone();
        let gh_state = self.gh_state.clone();
        let load = self.workspace_issues.begin_page(page);
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = on_tokio(async move {
                let mut page_result =
                    fetch_page(&gh_state, requests::issues(&owner, &repo, "open"), page, 30)
                        .await?;
                page_result
                    .items
                    .retain(|issue: &helm_backend::github::Issue| issue.pull_request.is_none());
                Ok::<_, GhError>(page_result)
            })
            .await
            .map_err(|error| error.to_string());
            this.update(cx, |this, cx| {
                if this.workspace_issues.is_current(load) {
                    this.workspace_issues.finish_page(result);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn load_workspace_pulls(&mut self, page: u32, cx: &mut Context<Self>) {
        let owner = self.repo.owner.login.clone();
        let repo = self.repo.name.clone();
        let gh_state = self.gh_state.clone();
        let load = self.workspace_pulls.begin_page(page);
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = on_tokio(async move {
                fetch_page(&gh_state, requests::pulls(&owner, &repo, "open"), page, 30).await
            })
            .await
            .map_err(|error| error.to_string());
            this.update(cx, |this, cx| {
                if this.workspace_pulls.is_current(load) {
                    this.workspace_pulls.finish_page(result);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn open_discussion(
        &mut self,
        number: u64,
        title: String,
        body: Option<String>,
        state: String,
        url: String,
        cx: &mut Context<Self>,
    ) {
        self.selected_discussion = Some((number, title, body.unwrap_or_default(), state, url));
        let owner = self.repo.owner.login.clone();
        let repo = self.repo.name.clone();
        let gh_state = self.gh_state.clone();
        let load = self.discussion_comments.begin();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = on_tokio(async move {
                helm_backend::github::gh_list_issue_comments(owner, repo, number, &gh_state).await
            })
            .await
            .map_err(|error| error.to_string());
            this.update(cx, |this, cx| {
                if this.discussion_comments.is_current(load) {
                    this.discussion_comments.finish(result);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn load_empty_repository_tree(&mut self, cx: &mut Context<Self>) {
        self.tree.clear();
        self.readme.clear();
        self.folder_readme.clear();
        self.blob.clear();
        self.code_commit_sha.clear();
        self.tree_entries.clear();
        self.tree_nodes.clear();
        self.tree_directories.clear();
        self.tree_empty = false;
        self.active_file_path = None;
        self.active_folder_path = None;
        self.tree_selection = None;
        self.readme_markdown = None;
        self.folder_readme_markdown = None;
        self.blob_buffer = None;
        self.blob_editor = None;
        self.blob_image = None;
        self.blob_error = None;
        self.blob_language_error = None;
        let load = self.tree.begin();
        self.tree_state
            .update(cx, |state, cx| state.set_items(Vec::new(), cx));
        let owner = self.repo.owner.login.clone();
        let repo = self.repo.name.clone();
        let gh_state = self.gh_state.clone();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result =
                on_tokio(async move { fetch_repo_tree(&gh_state, &owner, &repo, "").await }).await;
            this.update(cx, |this, cx| {
                if !this.tree.is_current(load) {
                    return;
                }
                match result {
                    Ok(TreeLoadResult::EmptyRepository) => {
                        this.tree_empty = true;
                        this.tree.finish(Ok(RepoTree::default()));
                    }
                    Ok(TreeLoadResult::Tree(tree)) => {
                        this.tree_entries = tree
                            .entries
                            .iter()
                            .filter(|entry| entry.kind != TreeEntryKind::Tree)
                            .map(|entry| (entry.path.clone(), entry.clone()))
                            .collect();
                        let nodes = build_tree(&tree.entries);
                        index_tree_directories(&nodes, &mut this.tree_directories);
                        this.tree_nodes = nodes;
                        this.tree.finish(Ok(tree));
                        this.update_tree_items(cx);
                    }
                    Err(error) => this.tree.finish(Err(error.to_string())),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn update_tree_items(&mut self, cx: &mut Context<Self>) {
        if self.tree.value.is_none() {
            return;
        }
        let filter = self.tree_filter.read(cx).value().to_lowercase();
        let items = self
            .tree_nodes
            .iter()
            .filter_map(|node| {
                filtered_tree_item(
                    node,
                    &filter,
                    self.active_file_path.as_deref().unwrap_or_default(),
                )
            })
            .collect::<Vec<_>>();
        let selected = self
            .active_file_path
            .as_deref()
            .or(self.active_folder_path.as_deref())
            .and_then(|path| find_tree_item(&items, path));
        self.tree_state.update(cx, |state, cx| {
            state.set_items(items, cx);
            if let Some(item) = selected.as_ref() {
                state.set_selected_item(Some(item), cx);
            }
        });
    }

    fn on_tree_selection(&mut self, path: String, cx: &mut Context<Self>) {
        if self.tree_directories.contains_key(&path) {
            self.active_file_path = None;
            self.active_folder_path = Some(path.clone());
            self.tree_missing_path = None;
            self.confirm_local_open = None;
            self.local_open_error = None;
            self.blob.clear();
            self.folder_readme.clear();
            self.folder_readme_markdown = None;
            self.blob_buffer = None;
            self.blob_editor = None;
            self.blob_image = None;
            self.blob_error = None;
            self.blob_language_error = None;
            self.load_folder_readme(path, cx);
            cx.notify();
        } else {
            self.active_folder_path = None;
            self.folder_readme.clear();
            self.folder_readme_markdown = None;
            self.confirm_local_open = None;
            self.local_open_error = None;
            self.active_file_path = Some(path.clone());
            self.open_tree_entry(path, cx);
        }
    }

    fn load_folder_readme(&mut self, path: String, cx: &mut Context<Self>) {
        let owner = self.repo.owner.login.clone();
        let repo = self.repo.name.clone();
        let commit_sha = self.code_commit_sha.clone();
        let gh_state = self.gh_state.clone();
        let load = self.folder_readme.begin();
        self.folder_readme_markdown = None;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = on_tokio(async move {
                fetch_readme_at_path(&gh_state, &owner, &repo, &path, &commit_sha).await
            })
            .await;
            this.update(cx, |this, cx| {
                if !this.folder_readme.is_current(load) {
                    return;
                }
                match result {
                    Ok(readme) => {
                        if let Some(readme) = &readme {
                            let languages = Some(this.languages.clone());
                            this.folder_readme_markdown = Some(cx.new(|cx| {
                                Markdown::new(readme.content.clone().into(), languages, None, cx)
                            }));
                        }
                        this.folder_readme.finish(Ok(readme));
                    }
                    Err(error) => this.folder_readme.finish(Err(error.to_string())),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn open_tree_entry(&mut self, path: String, cx: &mut Context<Self>) {
        let Some(entry) = self.tree_entries.get(&path).cloned() else {
            self.blob.clear();
            self.blob_error = Some("This file is not present in the selected commit.".into());
            self.blob_buffer = None;
            self.blob_editor = None;
            self.blob_image = None;
            cx.notify();
            return;
        };
        self.active_file_path = Some(path.clone());
        self.active_folder_path = None;
        self.confirm_local_open = None;
        self.local_open_error = None;
        self.folder_readme.clear();
        self.folder_readme_markdown = None;
        self.tree_missing_path = None;
        self.blob_buffer = None;
        self.blob_editor = None;
        self.blob_image = None;
        self.blob_error = None;
        self.blob_language_error = None;
        let load = self.blob.begin();
        let cache_key = format!("{}:{}", entry.sha, path);
        if let Some(cached) = self.blob_cache.get(&cache_key).cloned() {
            self.finish_blob(cached, load, cx);
            return;
        }

        let owner = self.repo.owner.login.clone();
        let repo = self.repo.name.clone();
        let gh_state = self.gh_state.clone();
        let commit_sha = self.code_commit_sha.clone();
        let cache_path = path.clone();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = on_tokio(async move {
                if entry.kind != TreeEntryKind::Blob {
                    return Ok::<_, GhError>(BlobData {
                        sha: entry.sha,
                        kind: BlobKind::Binary,
                        size: entry.size.unwrap_or_default(),
                        content: Vec::new(),
                        encoding: None,
                        lfs_size: None,
                    });
                }
                fetch_blob_at_path(&gh_state, &owner, &repo, &path, &entry.sha, entry.size).await
            })
            .await;
            this.update(cx, move |this, cx| {
                if !this.blob.is_current(load) || this.code_commit_sha != commit_sha {
                    return;
                }
                match result {
                    Ok(blob) => {
                        this.blob_cache
                            .insert(format!("{}:{}", blob.sha, cache_path), blob.clone());
                        this.finish_blob(blob, load, cx);
                    }
                    Err(error) => {
                        this.blob.finish(Err(error.to_string()));
                        this.blob_error = Some(error.to_string());
                        cx.notify();
                    }
                }
            })
            .ok();
        })
        .detach();
    }

    fn finish_blob(&mut self, blob: BlobData, load: u64, cx: &mut Context<Self>) {
        if !self.blob.is_current(load) {
            return;
        }
        self.blob_buffer = None;
        self.blob_editor = None;
        self.blob_image = None;
        self.blob_error = None;
        self.blob_language_error = None;
        if blob.kind == BlobKind::Text {
            match String::from_utf8(blob.content.clone()) {
                Ok(source) => {
                    if let Some((path, query, snippet)) = self.pending_search_line.take()
                        && self.active_file_path.as_deref() == Some(path.as_str())
                    {
                        self.search_result_line =
                            find_search_result_line(&source, &query, snippet.as_deref())
                                .map(|line| (path, line));
                    }
                    let language_registry = self.languages.clone();
                    let buffer = cx.new(|cx| {
                        let mut buffer = language::Buffer::local(source, cx);
                        buffer.set_language_registry(language_registry);
                        buffer.set_capability(Capability::ReadOnly, cx);
                        buffer
                    });
                    self.blob_buffer = Some(buffer.clone());
                    let path = self.active_file_path.clone().unwrap_or_default();
                    let extension = path
                        .rsplit_once('.')
                        .map(|(_, ext)| ext.to_ascii_lowercase());
                    let language_registry = self.languages.clone();
                    cx.spawn(async move |this, cx| {
                        let language = match extension {
                            Some(extension)
                                if language_registry
                                    .language_name_for_extension(&extension)
                                    .is_some() =>
                            {
                                language_registry
                                    .language_for_name_or_extension(&extension)
                                    .await
                            }
                            _ => Ok(language::PLAIN_TEXT.clone()),
                        };
                        match language {
                            Ok(language) => buffer.update(cx, |buffer, cx| {
                                buffer.set_language_async(Some(language), cx);
                            }),
                            Err(error) => {
                                buffer.update(cx, |buffer, cx| {
                                    buffer
                                        .set_language_async(Some(language::PLAIN_TEXT.clone()), cx);
                                });
                                this.update(cx, |this, cx| {
                                    if this.active_file_path.as_deref() == Some(path.as_str())
                                        && this.blob_buffer.as_ref().is_some_and(|current| {
                                            current.entity_id() == buffer.entity_id()
                                        })
                                    {
                                        this.blob_language_error = Some(format!(
                                            "Syntax highlighting could not be loaded: {error}"
                                        ));
                                        cx.notify();
                                    }
                                })
                                .ok();
                            }
                        }
                    })
                    .detach();
                }
                Err(_) => {
                    self.blob_error = Some(
                        "GitHub returned text that is not valid UTF-8; it cannot be shown safely."
                            .into(),
                    );
                }
            }
        } else if blob.kind == BlobKind::Image {
            let format = self
                .active_file_path
                .as_deref()
                .and_then(image_format_for_path);
            if let Some(format) = format {
                self.blob_image = Some(Arc::new(gpui::Image::from_bytes(
                    format,
                    blob.content.clone(),
                )));
            }
        }
        self.blob.finish(Ok(blob));
        cx.notify();
    }

    fn on_ref_selected(
        &mut self,
        _: &Entity<SelectState<SearchableVec<String>>>,
        event: &SelectEvent<SearchableVec<String>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let SelectEvent::Confirm(Some(selection)) = event else {
            return;
        };
        let Some((kind, name)) = selection.split_once(": ") else {
            return;
        };
        self.ref_is_tag = kind == "tag";
        self.ref_name = name.to_string();
        self.resolved_ref = None;
        self.ref_load.clear();
        self.last_commit.clear();
        self.pending_ref = None;
        self.tree.clear();
        self.readme.clear();
        self.blob.clear();
        self.code_commit_sha.clear();
        self.tree_entries.clear();
        self.tree_nodes.clear();
        self.tree_directories.clear();
        self.tree_empty = false;
        self.tree_selection = None;
        self.readme_markdown = None;
        self.blob_buffer = None;
        self.blob_editor = None;
        self.blob_image = None;
        self.blob_error = None;
        self.blob_language_error = None;
        self.tree_state
            .update(cx, |state, cx| state.set_items(Vec::new(), cx));
        self.tree_missing_path = None;
        self.resolve_selected_ref(cx);
    }

    fn load_refs_and_resolve(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let owner = self.repo.owner.login.clone();
        let name = self.repo.name.clone();
        let gh_state = self.gh_state.clone();
        let default_branch = self.ref_name.clone();
        cx.spawn_in(window, async move |this, cx| {
            let result = on_tokio(async move {
                let branches = match gh_get_branches(owner.clone(), name.clone(), &gh_state).await {
                    Ok(branches) => branches,
                    Err(GhError::EmptyRepository) => Vec::new(),
                    Err(error) => return Err(error),
                };
                let tags = match gh_list_tags(owner, name, &gh_state).await {
                    Ok(tags) => tags,
                    Err(GhError::EmptyRepository) => Vec::new(),
                    Err(error) => return Err(error),
                };
                Ok::<_, GhError>((branches, tags))
            })
            .await;
            this.update_in(cx, |this, window, cx| {
                match result {
                    Ok((branches, tags)) => {
                        this.commit_tags.clear();
                        for tag in &tags {
                            this.commit_tags
                                .entry(tag.commit.sha.clone())
                                .or_default()
                                .push(tag.name.clone());
                        }
                        let mut items = branches
                            .iter()
                            .map(|branch| format!("branch: {}", branch.name))
                            .collect::<Vec<_>>();
                        items.extend(tags.iter().map(|tag| format!("tag: {}", tag.name)));
                        if items.is_empty() && !default_branch.is_empty() {
                            items.push(format!("branch: {default_branch}"));
                        }
                        let selected_label = if this.ref_is_tag {
                            format!("tag: {}", this.ref_name)
                        } else {
                            format!("branch: {}", this.ref_name)
                        };
                        this.ref_selector.update(cx, |state, cx| {
                            state.set_items(SearchableVec::new(items), window, cx);
                            state.set_selected_value(&selected_label, window, cx);
                        });
                        this.refs_error = None;
                    }
                    Err(error) => this.refs_error = Some(error.to_string()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
        self.resolve_selected_ref(cx);
    }

    fn resolve_selected_ref(&mut self, cx: &mut Context<Self>) {
        if self.ref_name.is_empty() {
            self.ref_load.clear();
            if self.repo.default_branch.is_empty() {
                self.load_empty_repository_tree(cx);
            } else {
                let load = self.ref_load.begin();
                if self.ref_load.is_current(load) {
                    self.ref_load
                        .finish(Err("This repository has no default branch.".into()));
                }
            }
            self.resolved_ref = None;
            cx.notify();
            return;
        }
        let owner = self.repo.owner.login.clone();
        let repo = self.repo.name.clone();
        let ref_name = self.ref_name.clone();
        let gh_state = self.gh_state.clone();
        let load = self.ref_load.begin();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result =
                on_tokio(async move { resolve_ref(&gh_state, &owner, &repo, &ref_name).await })
                    .await;
            this.update(cx, |this, cx| {
                if !this.ref_load.is_current(load) {
                    return;
                }
                match result {
                    Ok(resolved) => {
                        this.ref_load.finish(Ok(resolved.clone()));
                        this.load_last_commit(resolved.commit_sha.clone(), cx);
                        this.load_code_for_ref(resolved.commit_sha.clone(), cx);
                        this.resolved_ref = Some(resolved);
                        this.pending_ref = None;
                    }
                    Err(error) => this.ref_load.finish(Err(error.to_string())),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn refresh_ref(&mut self, cx: &mut Context<Self>) {
        let Some(previous) = self.resolved_ref.clone() else {
            self.resolve_selected_ref(cx);
            return;
        };
        let owner = self.repo.owner.login.clone();
        let repo = self.repo.name.clone();
        let ref_name = self.ref_name.clone();
        let gh_state = self.gh_state.clone();
        let load = self.ref_load.begin();
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = on_tokio(async move {
                let current = resolve_ref(&gh_state, &owner, &repo, &ref_name).await?;
                let movement = compare_ref_movement(
                    &gh_state,
                    &owner,
                    &repo,
                    &previous.commit_sha,
                    &current.commit_sha,
                )
                .await?;
                Ok::<_, GhError>((current, movement))
            })
            .await;
            this.update(cx, |this, cx| {
                if !this.ref_load.is_current(load) {
                    return;
                }
                match result {
                    Ok((resolved, RefMovement::Unchanged)) => {
                        this.ref_load.finish(Ok(resolved.clone()));
                        this.resolved_ref = Some(resolved);
                        this.pending_ref = None;
                    }
                    Ok((resolved, _movement)) => {
                        this.ref_load.finish(Ok(resolved.clone()));
                        this.pending_ref = Some(resolved);
                    }
                    Err(error) => this.ref_load.finish(Err(error.to_string())),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn apply_pending_ref(&mut self, cx: &mut Context<Self>) {
        if let Some(resolved) = self.pending_ref.take() {
            self.load_last_commit(resolved.commit_sha.clone(), cx);
            self.load_code_for_ref(resolved.commit_sha.clone(), cx);
            self.resolved_ref = Some(resolved);
            cx.notify();
        }
    }
}

impl EventEmitter<()> for WorkspaceTab {}

impl Focusable for WorkspaceTab {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for WorkspaceTab {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(error) = &self.repository_error {
            let message = match error {
                GhError::NotFound { .. } => {
                    "This repository does not exist, or this sign-in cannot see it.".to_string()
                }
                _ => format!("Could not open this repository: {error}"),
            };
            let account = self
                .account
                .value
                .as_deref()
                .map(|account| format!("Signed in to GitHub as {account}."))
                .unwrap_or_else(|| "Could not determine the signed-in GitHub account.".into());
            let error = error.clone();
            let scope_missing = self.repository_scope_missing;
            let authorization_url = match error {
                GhError::SsoRequired {
                    authorization_url, ..
                } => authorization_url.clone(),
                _ => None,
            };
            let repo_url = self.repo.html_url.clone();
            return v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .gap_3()
                .p_6()
                .child(Icon::new(IconName::Info))
                .child(div().text_sm().child(message))
                .child(div().text_sm().child(account))
                .when(scope_missing, |this| {
                    this.child(
                        div()
                            .max_w(px(560.))
                            .text_sm()
                            .text_color(cx.theme().muted_foreground)
                            .child("This GitHub login is missing the repo scope. Authorize it in Helm, then retry this tab."),
                    )
                    .child(
                        Button::new("helm-workspace-authorize-repo")
                            .primary()
                            .label("Authorize repo scope")
                            .on_click(|_, window, cx| {
                                window.dispatch_action(
                                    AuthorizeRepositoryWorkspace.boxed_clone(),
                                    cx,
                                )
                            }),
                    )
                })
                .when_some(authorization_url, |this, url| {
                    this.child(
                        Button::new("helm-workspace-authorize-sso")
                            .primary()
                            .label("Authorize on GitHub")
                            .on_click(move |_, _, cx| cx.open_url(&url)),
                    )
                })
                .when(!repo_url.is_empty(), |this| {
                    this.child(
                        Button::new("helm-workspace-open-github-error")
                            .ghost()
                            .label("Open on GitHub")
                            .on_click(move |_, _, cx| cx.open_url(&repo_url)),
                    )
                })
                .child(
                    Button::new("helm-workspace-retry-repository")
                        .ghost()
                        .label(if self.repository_loading {
                            "Retrying…"
                        } else {
                            "Retry"
                        })
                        .disabled(self.repository_loading)
                        .on_click(
                            cx.listener(|this, _, window, cx| this.load_repository(window, cx)),
                        ),
                )
                .into_any_element();
        }
        if self.repository_loading {
            return v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .child(div().text_sm().child("Loading repository…"))
                .into_any_element();
        }

        let repo_name = self.repo.full_name.clone();
        let repo_url = self.repo.html_url.clone();
        let default_branch = self.repo.default_branch.clone();
        let local_clone = self.matching_local_clone(cx);
        let local_clone_status = local_clone.as_ref().map_or_else(
            || "Not cloned".to_string(),
            |local_clone| {
                let branch = local_clone.branch.as_deref().unwrap_or("detached HEAD");
                let commit_matches =
                    local_clone.commit_sha.as_deref() == Some(self.code_commit_sha.as_str());
                let branch_matches = !self.ref_is_tag
                    && local_clone.branch.as_deref() == Some(self.ref_name.as_str());
                format!(
                    "Local: {branch} · {}",
                    if commit_matches && branch_matches {
                        "same branch and commit"
                    } else if commit_matches {
                        "same commit, branch differs"
                    } else {
                        "different commit"
                    }
                )
            },
        );
        let ref_is_tag = self.ref_is_tag;
        let ref_name = self.ref_name.clone();
        let ref_selector = self.ref_selector.clone();
        let refreshing = self.ref_load.state == LoadState::Loading;
        let pending = self.pending_ref.is_some();
        let sections = if self.repo.disabled {
            WorkspaceSection::ALL[..1].to_vec()
        } else {
            WorkspaceSection::ALL.to_vec()
        };
        let current_section = self.section;
        // Three rows: repo identity with the ref dropdown above the action
        // buttons, the state tags on their own line, and the account/clone
        // status alongside the buttons. Cramming everything into one row left
        // every item squashed at normal widths.
        let header = v_flex()
            .gap_2()
            .px_4()
            .py_3()
            .child(
                gpui_component::h_flex()
                    .items_center()
                    .gap_2()
                    .w_full()
                    // The name takes the room that is left and is cut short
                    // before it wraps; the dropdown keeps its width at the
                    // right edge. `Select` fills whatever holds it, so its
                    // width is set on a wrapper that cannot grow or shrink.
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .font_semibold()
                            .child(repo_name),
                    )
                    .child(
                        div().flex_none().w(px(240.)).child(
                            Select::new(&ref_selector)
                                .search_placeholder("Search branches and tags…")
                                .disabled(self.repo.disabled || default_branch.is_empty())
                                .when(self.repo.disabled, |select| {
                                    select.placeholder("Repository disabled")
                                })
                                .when(
                                    !self.repo.disabled && default_branch.is_empty(),
                                    |select| select.placeholder("No default branch"),
                                ),
                        ),
                    ),
            )
            .child(
                gpui_component::h_flex()
                    .gap_1()
                    .mb_2()
                    .when(self.repo.private, |this| {
                        this.child(Tag::secondary().outline().xsmall().child("Private"))
                    })
                    .when(self.repo.archived, |this| {
                        this.child(Tag::secondary().outline().xsmall().child("Archived"))
                    })
                    .when(self.repo.fork, |this| {
                        this.child(Tag::secondary().outline().xsmall().child("Fork"))
                    })
                    .when(ref_is_tag, |this| {
                        this.child(Tag::secondary().outline().xsmall().child("Tag"))
                    }),
            )
            .child(
                gpui_component::h_flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(match (self.account.state, self.account.value.as_deref()) {
                                (_, Some(account)) => format!("Signed in as {account}"),
                                (LoadState::Error, None) => {
                                    "GitHub account unavailable".to_string()
                                }
                                (LoadState::Loading | LoadState::Idle, None) => {
                                    "Checking GitHub account…".to_string()
                                }
                            }),
                    )
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(local_clone_status),
                    )
                    .child(div().flex_1())
                    .child(
                        Button::new("helm-workspace-refresh")
                            .ghost()
                            .label("Refresh")
                            .tooltip(if refreshing {
                                "Checking for updates…"
                            } else {
                                "Check for updates"
                            })
                            .disabled(refreshing || ref_name.is_empty() || self.repo.disabled)
                            .on_click(cx.listener(|this, _, _, cx| this.refresh_ref(cx))),
                    )
                    .child(
                        Button::new("helm-workspace-open-github")
                            .ghost()
                            .icon(IconName::ExternalLink)
                            .tooltip("Open on GitHub")
                            .disabled(repo_url.is_empty())
                            .on_click(move |_, _, cx| cx.open_url(&repo_url)),
                    ),
            );

        let navigation =
            gpui_component::h_flex()
                .gap_1()
                .px_3()
                .py_1()
                .children(sections.iter().copied().map(|(section, label)| {
                    let selected = section == current_section;
                    Button::new(format!("helm-workspace-section-{label}"))
                        .ghost()
                        .small()
                        .selected(selected)
                        .label(label)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.section = section;
                            if section == WorkspaceSection::Commits
                                && this.commits.state == LoadState::Idle
                                && this.commits.items.is_empty()
                            {
                                this.load_commits(1, None, cx);
                            }
                            if section == WorkspaceSection::PullRequests
                                && this.workspace_pulls.state == LoadState::Idle
                                && this.workspace_pulls.items.is_empty()
                            {
                                this.load_workspace_pulls(1, cx);
                            }
                            if section == WorkspaceSection::Issues
                                && this.workspace_issues.state == LoadState::Idle
                                && this.workspace_issues.items.is_empty()
                            {
                                this.load_workspace_issues(1, cx);
                            }
                            cx.notify();
                        }))
                }));

        let update_line = pending.then(|| {
            gpui_component::h_flex()
                .items_center()
                .justify_between()
                .px_4()
                .py_2()
                .bg(cx.theme().accent.opacity(0.12))
                .child(div().text_sm().child("This ref has changed on GitHub."))
                .child(
                    Button::new("helm-workspace-update-ref")
                        .primary()
                        .small()
                        .label("Update")
                        .on_click(cx.listener(|this, _, _, cx| this.apply_pending_ref(cx))),
                )
        });

        let ref_status = if self.ref_load.state == LoadState::Error {
            Some(status_line(self.ref_load.error.clone(), cx))
        } else {
            self.refs_error
                .as_ref()
                .map(|error| status_line(format!("Could not load branches and tags: {error}"), cx))
        };

        let page = match current_section {
            WorkspaceSection::Overview => self.render_overview(cx).into_any_element(),
            WorkspaceSection::Code => self.render_code(window, cx).into_any_element(),
            WorkspaceSection::Commits => self.render_commits(window, cx).into_any_element(),
            WorkspaceSection::Compare => self.render_compare(window, cx).into_any_element(),
            WorkspaceSection::Search => self.render_code_search(cx).into_any_element(),
            WorkspaceSection::PullRequests => self.render_workspace_pulls(cx).into_any_element(),
            WorkspaceSection::Issues => self.render_workspace_issues(cx).into_any_element(),
            WorkspaceSection::Actions => self.render_workspace_actions(cx).into_any_element(),
        };
        let rate_line = self.gh_state.rate_limit().map(|rate| {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |elapsed| elapsed.as_secs());
            gpui_component::h_flex()
                .justify_end()
                .px_3()
                .py_1()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(rate.summary(now))
                .into_any_element()
        });

        v_flex()
            .size_full()
            .child(header)
            .child(Separator::horizontal())
            .child(navigation)
            .child(Separator::horizontal())
            .children(update_line)
            .children(ref_status)
            .child(page)
            .child(Separator::horizontal())
            .children(rate_line)
            .into_any_element()
    }
}

impl WorkspaceTab {
    fn render_code_search(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let input = self.code_search_input.clone();
        let limit = self.gh_state.rate_limit_for("code_search");
        let rate_line = limit.and_then(|rate| {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |elapsed| elapsed.as_secs());
            (now < rate.reset_at).then(|| {
                div()
                    .text_xs()
                    .text_color(if rate.is_low() {
                        cx.theme().warning
                    } else {
                        cx.theme().muted_foreground
                    })
                    .child(format!("Code search allowance: {}", rate.summary(now)))
            })
        });
        v_flex()
            .size_full()
            .min_h_0()
            .gap_3()
            .p_3()
            .child(
                gpui_component::h_flex()
                    .gap_2()
                    .child(Input::new(&input).flex_1())
                    .child(
                        Button::new("helm-workspace-code-search")
                            .primary()
                            .label(if self.code_search.state == LoadState::Loading {
                                "Searching…"
                            } else {
                                "Search"
                            })
                            .disabled(self.code_search.state == LoadState::Loading)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.load_code_search(1, cx);
                            })),
                    ),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(format!(
                        "Search code in {} on ref {}. Search runs only when submitted.",
                        self.repo.full_name, self.ref_name
                    )),
            )
            .children(rate_line)
            .when_some(self.code_search_error.as_ref(), |this, error| {
                this.child(status_line(error.clone(), cx))
            })
            .when(self.code_search.state == LoadState::Loading, |this| {
                this.child(status_line("Searching GitHub code…", cx))
            })
            .when(self.code_search.state == LoadState::Error, |this| {
                this.child(status_line(
                    format!("Code search failed: {}", self.code_search.error),
                    cx,
                ))
            })
            .when(
                self.code_search.state == LoadState::Idle
                    && self.code_search.items.is_empty()
                    && self.code_search_error.is_none(),
                |this| this.child(status_line("Enter a query to search this repository.", cx)),
            )
            .child(v_flex().flex_1().min_h_0().overflow_y_scrollbar().children(
                self.code_search.items.iter().map(|result| {
                    let result = result.clone();
                    let path = result.path.clone();
                    let snippet = result.line.as_deref().unwrap_or("Open matching file");
                    Button::new(format!("helm-workspace-code-result-{}", result.path))
                        .ghost()
                        .label(format!("{path}\n{snippet}"))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.open_code_search_result(result.clone(), window, cx)
                        }))
                }),
            ))
            .child(
                gpui_component::h_flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(format!(
                                "Page {} / {}",
                                self.code_search.page, self.code_search.last_page
                            )),
                    )
                    .child(
                        gpui_component::h_flex()
                            .gap_2()
                            .child(
                                Button::new("helm-workspace-code-search-previous")
                                    .ghost()
                                    .small()
                                    .label("Previous")
                                    .disabled(
                                        self.code_search.page <= 1
                                            || self.code_search.state == LoadState::Loading,
                                    )
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.load_code_search(this.code_search.page - 1, cx);
                                    })),
                            )
                            .child(
                                Button::new("helm-workspace-code-search-next")
                                    .ghost()
                                    .small()
                                    .label("Next")
                                    .disabled(
                                        self.code_search.page >= self.code_search.last_page
                                            || self.code_search.state == LoadState::Loading,
                                    )
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.load_code_search(this.code_search.page + 1, cx);
                                    })),
                            ),
                    ),
            )
    }

    fn render_code(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let local_clone = self.matching_local_clone(cx);
        let filter = self.tree_filter.clone();
        let tree_panel = v_flex()
            .size_full()
            .min_w_0()
            .gap_2()
            .p_2()
            .child(Input::new(&filter).small())
            .child(match self.tree.state {
                LoadState::Loading | LoadState::Idle if self.tree.value.is_none() => div()
                    .flex_1()
                    .items_center()
                    .justify_center()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child("Loading file tree…")
                    .into_any_element(),
                LoadState::Error => v_flex()
                    .flex_1()
                    .items_center()
                    .justify_center()
                    .p_3()
                    .child(status_line(
                        format!("Could not load the file tree: {}", self.tree.error),
                        cx,
                    ))
                    .into_any_element(),
                _ if self.tree_empty => v_flex()
                    .flex_1()
                    .items_center()
                    .justify_center()
                    .gap_2()
                    .p_3()
                    .child(div().text_sm().child("This repository is empty."))
                    .child(
                        Button::new("helm-workspace-empty-clone-url")
                            .ghost()
                            .label("Copy clone URL")
                            .on_click({
                                let clone_url = self.repo.clone_url.clone();
                                move |_, _, cx| {
                                    cx.write_to_clipboard(ClipboardItem::new_string(
                                        clone_url.clone(),
                                    ));
                                }
                            }),
                    )
                    .into_any_element(),
                _ => Tree::new(&self.tree_state, |_, entry: &TreeRowEntry, _, _, _| {
                    let marker = if entry.is_folder() {
                        if entry.is_expanded() { "▾" } else { "▸" }
                    } else {
                        " "
                    };
                    ListItem::new(entry.item().id.clone()).child(
                        gpui_component::h_flex()
                            .gap_2()
                            .pl(px((entry.depth() * 12) as f32))
                            .child(div().w(px(12.)).child(marker))
                            .child(entry.item().label.clone()),
                    )
                })
                .flex_1()
                .min_h_0()
                .into_any_element(),
            });
        let viewer_path = self.active_file_path.as_deref().or_else(|| {
            self.active_folder_path.as_deref().or_else(|| {
                self.readme
                    .value
                    .as_ref()
                    .map(|readme| readme.path.as_str())
            })
        });
        let viewer_title = viewer_path.unwrap_or("README");
        let permalink = if self.active_folder_path.is_some() {
            github_tree_permalink(&self.repo.html_url, &self.code_commit_sha, viewer_title)
        } else if viewer_path.is_some() {
            github_permalink(&self.repo.html_url, &self.code_commit_sha, viewer_title)
        } else {
            String::new()
        };
        let copy_path = viewer_path.map(str::to_string);
        let mut toolbar = gpui_component::h_flex()
            .items_center()
            .gap_2()
            .px_3()
            .py_2()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_sm()
                    .font_semibold()
                    .child(viewer_title.to_string()),
            )
            .when_some(copy_path, |this, path| {
                this.child(
                    Button::new("helm-workspace-copy-path")
                        .ghost()
                        .small()
                        .label("Copy path")
                        .on_click(move |_, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(path.clone()));
                        }),
                )
            })
            .child(
                Button::new("helm-workspace-copy-permalink")
                    .ghost()
                    .small()
                    .label("Copy permalink")
                    .disabled(permalink.is_empty())
                    .on_click(move |_, _, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(permalink.clone()));
                    }),
            );
        if let Some(path) = self.active_file_path.clone() {
            if local_clone.is_some() {
                toolbar = toolbar.child(
                    Button::new("helm-workspace-open-local-file")
                        .primary()
                        .small()
                        .label("Open in editor")
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.request_local_file_open(path.clone(), false, window, cx)
                        })),
                );
            } else {
                toolbar = toolbar.child(
                    Button::new("helm-workspace-clone-open-file")
                        .primary()
                        .small()
                        .label("Clone default branch & open")
                        .tooltip(format!(
                            "Clone the repository's default branch ({}) and open this path",
                            self.repo.default_branch
                        ))
                        .on_click(|_, window, cx| {
                            window.dispatch_action(CloneRepositoryForWorkspace.boxed_clone(), cx)
                        }),
                );
            }
        }
        if let Some((_path, line)) = self
            .search_result_line
            .as_ref()
            .filter(|(path, _)| self.active_file_path.as_ref() == Some(path))
        {
            toolbar = toolbar.child(
                Tag::secondary()
                    .outline()
                    .xsmall()
                    .child(format!("Search match · line {line}")),
            );
        }
        if self.active_file_path.is_some() || self.active_folder_path.is_some() {
            let path = self
                .active_file_path
                .clone()
                .or_else(|| self.active_folder_path.clone());
            toolbar = toolbar.child(
                Button::new("helm-workspace-file-history")
                    .ghost()
                    .small()
                    .label("History")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.section = WorkspaceSection::Commits;
                        this.load_commits(1, path.clone(), cx);
                    })),
            );
        }
        let selected_file_path = self.active_file_path.clone();
        let content = if let Some(path) = selected_file_path {
            self.render_blob(&path, window, cx).into_any_element()
        } else if let Some(path) = self.active_folder_path.as_deref() {
            self.render_directory(path, window, cx).into_any_element()
        } else {
            self.render_readme(window, cx).into_any_element()
        };
        let open_confirmation = self
            .confirm_local_open
            .as_ref()
            .and_then(|(path, _, _)| {
            (self.active_file_path.as_deref() == Some(path.as_str())).then(|| {
                let branch = local_clone
                    .as_ref()
                    .and_then(|local_clone| local_clone.branch.as_deref())
                    .unwrap_or("detached HEAD");
                let local_sha = local_clone
                    .as_ref()
                    .and_then(|local_clone| local_clone.commit_sha.as_deref())
                    .map(short_sha)
                    .unwrap_or_else(|| "unknown commit".into());
                let remote_sha = short_sha(&self.code_commit_sha);
                let path = path.clone();
                gpui_component::h_flex()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .py_2()
                    .bg(cx.theme().warning.opacity(0.12))
                    .child(
                        div()
                            .flex_1()
                            .text_sm()
                            .child(format!(
                                "Local {branch} is at {local_sha}; GitHub ref {} is at {remote_sha}. Open the local version?",
                                self.ref_name
                            )),
                    )
                    .child(
                        Button::new("helm-workspace-open-local-file-confirm")
                            .primary()
                            .small()
                            .label("Open anyway")
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.request_local_file_open(path.clone(), true, window, cx)
                            })),
                    )
                    .child(
                        Button::new("helm-workspace-open-local-file-cancel")
                            .ghost()
                            .small()
                            .label("Cancel")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.confirm_local_open = None;
                                cx.notify();
                            })),
                    )
                    .into_any_element()
            })
        });
        let open_error = self
            .local_open_error
            .as_ref()
            .map(|error| status_line(error.clone(), cx));

        div().size_full().child(
            h_resizable(("helm-workspace-code", cx.entity().entity_id()))
                .child(
                    resizable_panel()
                        .size(px(280.))
                        .size_range(px(180.)..px(520.))
                        .child(tree_panel),
                )
                .child(
                    resizable_panel().child(
                        v_flex()
                            .size_full()
                            .min_w_0()
                            .child(toolbar)
                            .child(Separator::horizontal())
                            .children(open_confirmation)
                            .children(open_error)
                            .child(content),
                    ),
                ),
        )
    }

    fn render_directory(
        &self,
        path: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let missing_message = self.tree_missing_path.as_ref().map(|missing| {
            status_line(
                format!("{missing} is not present in this commit; showing its nearest folder."),
                cx,
            )
        });
        let Some(node) = find_tree_node(&self.tree_nodes, path) else {
            return v_flex()
                .flex_1()
                .items_center()
                .justify_center()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child("This folder is not present in the selected commit.")
                .into_any_element();
        };
        let contents = node.children.iter().map(|child| {
            let marker = if child.entry.kind == TreeEntryKind::Tree || !child.children.is_empty() {
                "Folder"
            } else {
                "File"
            };
            let size = child.entry.size.map(format_bytes);
            ListItem::new(format!("helm-workspace-directory-entry-{}", child.path)).child(
                gpui_component::h_flex()
                    .gap_3()
                    .child(
                        div()
                            .w(px(52.))
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child(marker),
                    )
                    .child(child.name.clone())
                    .when_some(size, |row, size| {
                        row.child(
                            div()
                                .ml_auto()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child(size),
                        )
                    }),
            )
        });
        let readme_content = if let Some(markdown) = &self.folder_readme_markdown {
            Some(
                MarkdownElement::new(
                    markdown.clone(),
                    MarkdownStyle::themed(MarkdownFont::Editor, window, cx),
                )
                .into_any_element(),
            )
        } else {
            match self.folder_readme.state {
                LoadState::Loading => {
                    Some(status_line("Loading folder README…", cx).into_any_element())
                }
                LoadState::Error => Some(
                    status_line(
                        format!(
                            "Could not load this folder's README: {}",
                            self.folder_readme.error
                        ),
                        cx,
                    )
                    .into_any_element(),
                ),
                LoadState::Idle => None,
            }
        };
        v_flex()
            .flex_1()
            .min_h_0()
            .overflow_y_scrollbar()
            .gap_2()
            .p_4()
            .children(missing_message)
            .child(div().text_sm().font_semibold().child("Folder contents"))
            .children(contents)
            .children(readme_content)
            .into_any_element()
    }

    fn render_commits(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let list = v_flex()
            .size_full()
            .min_w_0()
            .gap_2()
            .p_3()
            .child(
                gpui_component::h_flex()
                    .items_center()
                    .justify_between()
                    .child(div().text_sm().font_semibold().child(
                        match self.commits_path.as_deref() {
                            Some(path) => format!("History for {path}"),
                            None => format!("History for {}", self.ref_name),
                        },
                    ))
                    .child(
                        gpui_component::h_flex()
                            .gap_2()
                            .child(
                                Button::new("helm-workspace-commits-previous")
                                    .ghost()
                                    .small()
                                    .label("Previous")
                                    .disabled(
                                        self.commits.page <= 1
                                            || self.commits.state == LoadState::Loading,
                                    )
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        let path = this.commits_path.clone();
                                        this.load_commits(this.commits.page - 1, path, cx);
                                    })),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(format!(
                                        "{} / {}",
                                        self.commits.page, self.commits.last_page
                                    )),
                            )
                            .child(
                                Button::new("helm-workspace-commits-next")
                                    .ghost()
                                    .small()
                                    .label("Next")
                                    .disabled(
                                        self.commits.page >= self.commits.last_page
                                            || self.commits.state == LoadState::Loading,
                                    )
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        let path = this.commits_path.clone();
                                        this.load_commits(this.commits.page + 1, path, cx);
                                    })),
                            ),
                    ),
            )
            .child(Separator::horizontal())
            .child(v_flex().flex_1().min_h_0().overflow_y_scrollbar().children(
                self.commits.items.iter().enumerate().map(|(ix, commit)| {
                    let sha = commit.sha.clone();
                    let selected = self.selected_commit_sha.as_deref() == Some(&sha);
                    let tags = self.commit_tags.get(&sha).cloned().unwrap_or_default();
                    helm_ui::commit_row(ix, commit, cx)
                        .selected(selected)
                        .when(!tags.is_empty(), |row| {
                            row.suffix(move |_, _| {
                                gpui_component::h_flex()
                                    .flex_none()
                                    .gap_1()
                                    .children(tags.iter().map(|tag| helm_ui::chip(tag.clone())))
                            })
                        })
                        .on_click(
                            cx.listener(move |this, _, _, cx| this.select_commit(sha.clone(), cx)),
                        )
                }),
            ))
            .when(self.commits.state == LoadState::Loading, |this| {
                this.child(status_line("Loading commit history…", cx))
            })
            .when(self.commits.state == LoadState::Error, |this| {
                this.child(status_line(
                    format!("Could not load commit history: {}", self.commits.error),
                    cx,
                ))
            });

        let detail = if let Some(commit) = self.selected_commit.value.as_ref() {
            let files = commit.files.iter().map(|file| {
                let path = file.path.clone();
                let patch = file.patch.clone();
                let selected = self.selected_commit_file.as_deref() == Some(&path);
                changed_file_row(
                    format!("helm-workspace-commit-file-{path}"),
                    &file.status,
                    &path,
                    file.additions,
                    file.deletions,
                    selected,
                    cx,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.select_patch(path.clone(), patch.clone(), cx)
                }))
            });
            v_flex()
                .size_full()
                .min_w_0()
                .gap_2()
                .p_3()
                .child(
                    div()
                        .text_sm()
                        .font_semibold()
                        .child(short_sha(&commit.sha)),
                )
                .child(
                    div()
                        .max_h(px(100.))
                        .overflow_y_scrollbar()
                        .text_sm()
                        .child(commit.message.clone()),
                )
                .child(Separator::horizontal())
                .child(div().text_xs().font_semibold().child("Changed files"))
                .child(
                    v_flex()
                        .max_h(px(260.))
                        .overflow_y_scrollbar()
                        .children(files),
                )
                .child(Separator::horizontal())
                .child(self.render_patch(window, cx))
                .into_any_element()
        } else {
            match self.selected_commit.state {
                LoadState::Loading => status_line("Loading commit details…", cx).into_any_element(),
                LoadState::Error => status_line(
                    format!(
                        "Could not load commit details: {}",
                        self.selected_commit.error
                    ),
                    cx,
                )
                .into_any_element(),
                LoadState::Idle => status_line("Select a commit to inspect its changes.", cx),
            }
        };

        div().size_full().child(
            h_resizable(("helm-workspace-commits", cx.entity().entity_id()))
                .child(
                    resizable_panel()
                        .size(px(330.))
                        .size_range(px(230.)..px(520.))
                        .child(list),
                )
                .child(resizable_panel().child(detail)),
        )
    }

    fn render_compare(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let base = self.compare_base.clone();
        let head = self.compare_head.clone();
        let compare_result = self.compare_result.value.clone();
        let commits = compare_result
            .as_ref()
            .map(|comparison| {
                comparison
                    .commits
                    .iter()
                    .enumerate()
                    .map(|(ix, commit)| {
                        let sha = commit.sha.clone();
                        helm_ui::commit_row(ix, commit, cx).on_click(cx.listener(
                            move |this, _, _, cx| {
                                this.section = WorkspaceSection::Commits;
                                this.select_commit(sha.clone(), cx);
                            },
                        ))
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let files = compare_result
            .as_ref()
            .map(|comparison| {
                let selected_file = self.selected_commit_file.clone();
                comparison
                    .files
                    .iter()
                    .map(|file| {
                        let path = file.path.clone();
                        let patch = file.patch.clone();
                        let selected = selected_file.as_deref() == Some(&path);
                        changed_file_row(
                            format!("helm-workspace-compare-file-{path}"),
                            &file.status,
                            &path,
                            file.additions,
                            file.deletions,
                            selected,
                            cx,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.select_patch(path.clone(), patch.clone(), cx)
                        }))
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        v_flex()
            .size_full()
            .min_h_0()
            .gap_3()
            .p_3()
            .child(
                gpui_component::h_flex()
                    .gap_2()
                    .child(Input::new(&base).w(px(220.)))
                    .child(div().text_sm().child("…"))
                    .child(Input::new(&head).w(px(220.)))
                    .child(
                        Button::new("helm-workspace-run-compare")
                            .primary()
                            .label("Compare")
                            .on_click(cx.listener(|this, _, _, cx| this.compare_refs(cx))),
                    ),
            )
            .when(self.compare_result.state == LoadState::Loading, |this| {
                this.child(status_line("Comparing refs…", cx))
            })
            .when(self.compare_result.state == LoadState::Error, |this| {
                this.child(status_line(
                    format!("Could not compare refs: {}", self.compare_result.error),
                    cx,
                ))
            })
            .when_some(compare_result.as_ref(), |this, result| {
                this.child(status_line(
                    format!(
                        "{} commits ahead · {} behind · {} changed files",
                        result.ahead_by,
                        result.behind_by,
                        result.files.len()
                    ),
                    cx,
                ))
                .child(Separator::horizontal())
                .child(div().text_xs().font_semibold().child("Commits"))
                .child(
                    v_flex()
                        .max_h(px(180.))
                        .overflow_y_scrollbar()
                        .children(commits),
                )
                .child(div().text_xs().font_semibold().child("Changed files"))
                .child(
                    v_flex()
                        .max_h(px(260.))
                        .overflow_y_scrollbar()
                        .children(files),
                )
                .child(Separator::horizontal())
                .child(self.render_patch(window, cx))
            })
            .into_any_element()
    }

    fn render_patch(&mut self, window: &mut Window, cx: &mut Context<Self>) -> gpui::AnyElement {
        let Some(buffer) = self.patch_buffer.clone() else {
            return status_line(
                self.selected_commit_file.as_ref().map_or_else(
                    || "Select a changed file to view GitHub's unified patch.".to_string(),
                    |_| {
                        "GitHub did not include a patch for this file (for example, it may be binary or too large)."
                            .to_string()
                    },
                ),
                cx,
            );
        };
        if self.patch_editor.is_none() {
            self.patch_editor = Some(cx.new(|cx| {
                let mut editor = Editor::for_buffer(buffer, None, window, cx);
                editor.set_read_only(true);
                editor
            }));
        }
        self.patch_editor
            .as_ref()
            .map(|editor| {
                editor.update(cx, |editor, cx| {
                    editor.render(window, cx).into_any_element()
                })
            })
            .unwrap_or_else(|| div().into_any_element())
    }

    fn render_readme(&self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.tree_empty {
            return v_flex()
                .flex_1()
                .items_center()
                .justify_center()
                .p_5()
                .child(status_line(
                    "This repository has no files or README because it has no commits yet.",
                    cx,
                ))
                .child(
                    div()
                        .max_w(px(620.))
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child(self.repo.clone_url.clone()),
                )
                .into_any_element();
        }
        let missing_message = self.tree_missing_path.as_ref().map(|path| {
            status_line(
                format!("{path} is not present in this commit; showing the README instead."),
                cx,
            )
        });
        if let Some(markdown) = &self.readme_markdown {
            return v_flex()
                .flex_1()
                .min_h_0()
                .overflow_y_scrollbar()
                .p_4()
                .children(missing_message)
                .child(MarkdownElement::new(
                    markdown.clone(),
                    MarkdownStyle::themed(MarkdownFont::Editor, window, cx),
                ))
                .into_any_element();
        }
        match self.readme.state {
            LoadState::Loading | LoadState::Idle => v_flex()
                .flex_1()
                .items_center()
                .justify_center()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .children(missing_message)
                .child("Loading README…")
                .into_any_element(),
            LoadState::Error => v_flex()
                .flex_1()
                .items_center()
                .justify_center()
                .p_5()
                .children(missing_message)
                .child(status_line(
                    format!("Could not load the README: {}", self.readme.error),
                    cx,
                ))
                .into_any_element(),
        }
    }

    fn render_blob(
        &mut self,
        path: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        if self.blob.state == LoadState::Loading {
            return v_flex()
                .flex_1()
                .items_center()
                .justify_center()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child(format!("Loading {path}…"))
                .into_any_element();
        }
        if let Some(error) = &self.blob_error {
            return v_flex()
                .flex_1()
                .items_center()
                .justify_center()
                .p_5()
                .child(status_line(error.clone(), cx))
                .into_any_element();
        }
        if self.blob.state == LoadState::Error {
            return v_flex()
                .flex_1()
                .items_center()
                .justify_center()
                .p_5()
                .child(status_line(
                    format!("Could not load {path}: {}", self.blob.error),
                    cx,
                ))
                .into_any_element();
        }
        let Some(blob) = self.blob.value.clone() else {
            return v_flex()
                .flex_1()
                .items_center()
                .justify_center()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child("Select a file to view it.")
                .into_any_element();
        };
        match blob.kind {
            BlobKind::Text => {
                if self.blob_editor.is_none() {
                    if let Some(buffer) = self.blob_buffer.clone() {
                        let editor = cx.new(|cx| {
                            let mut editor = Editor::for_buffer(buffer, None, window, cx);
                            editor.set_read_only(true);
                            editor
                        });
                        if let Some((_match_path, line)) = self
                            .search_result_line
                            .as_ref()
                            .filter(|(match_path, _)| match_path == path)
                        {
                            let row = line.saturating_sub(1);
                            editor.update(cx, |editor, cx| {
                                editor.change_selections(
                                    SelectionEffects::scroll(Autoscroll::fit()),
                                    window,
                                    cx,
                                    |selections| {
                                        selections
                                            .select_ranges([Point::new(row, 0)..Point::new(row, 0)])
                                    },
                                );
                            });
                        }
                        self.blob_editor = Some(editor);
                    }
                }
                if let Some(editor) = self.blob_editor.clone() {
                    v_flex()
                        .flex_1()
                        .min_h_0()
                        .when_some(self.blob_language_error.clone(), |this, error| {
                            this.child(status_line(error, cx))
                        })
                        .child(editor)
                        .into_any_element()
                } else {
                    v_flex()
                        .flex_1()
                        .items_center()
                        .justify_center()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child("Preparing highlighted file…")
                        .into_any_element()
                }
            }
            BlobKind::Image => {
                if let Some(image) = &self.blob_image {
                    v_flex()
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scrollbar()
                        .items_center()
                        .justify_center()
                        .p_4()
                        .child(
                            img(ImageSource::Image(image.clone()))
                                .max_w_full()
                                .max_h(px(800.))
                                .object_fit(ObjectFit::Contain),
                        )
                        .into_any_element()
                } else {
                    self.render_non_text_blob(
                        "This image format cannot be previewed here.",
                        blob.size,
                        cx,
                    )
                }
            }
            BlobKind::Binary => self.render_non_text_blob("Binary file", blob.size, cx),
            BlobKind::LfsPointer => self.render_non_text_blob(
                &blob
                    .lfs_size
                    .map(|size| format!("Stored in Git LFS (object size {})", format_bytes(size)))
                    .unwrap_or_else(|| "Stored in Git LFS".into()),
                blob.size,
                cx,
            ),
            BlobKind::TooLarge => {
                self.render_non_text_blob("File is too large to display inline", blob.size, cx)
            }
        }
    }

    fn render_non_text_blob(
        &self,
        description: &str,
        size: u64,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let url = github_permalink(
            &self.repo.html_url,
            &self.code_commit_sha,
            self.active_file_path.as_deref().unwrap_or_default(),
        );
        v_flex()
            .flex_1()
            .items_center()
            .justify_center()
            .gap_2()
            .p_5()
            .child(
                div()
                    .text_sm()
                    .font_semibold()
                    .child(description.to_string()),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child(format_bytes(size)),
            )
            .child(
                Button::new("helm-workspace-open-file-github")
                    .ghost()
                    .label("Open on GitHub")
                    .on_click(move |_, _, cx| cx.open_url(&url)),
            )
            .into_any_element()
    }

    fn render_overview(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let repo = &self.repo;
        let muted = cx.theme().muted_foreground;
        let issues_count = if !repo.has_issues {
            "Disabled".to_string()
        } else if let Some(counts) = &self.overview_counts.value {
            counts.open_issues.to_string()
        } else {
            match self.overview_counts.state {
                LoadState::Loading | LoadState::Idle => "Loading…".to_string(),
                LoadState::Error => "Unavailable".to_string(),
            }
        };
        let pulls_count = if let Some(counts) = &self.overview_counts.value {
            counts.open_pull_requests.to_string()
        } else {
            match self.overview_counts.state {
                LoadState::Loading | LoadState::Idle => "Loading…".to_string(),
                LoadState::Error => "Unavailable".to_string(),
            }
        };
        let last_commit_status = if repo.default_branch.is_empty() {
            "No commits yet".to_string()
        } else if let Some(commit) = &self.last_commit.value {
            let message = commit.message.lines().next().unwrap_or("No commit message");
            let short_sha = commit.sha.get(..7).unwrap_or(&commit.sha);
            format!("{short_sha} — {message}")
        } else {
            match self.last_commit.state {
                LoadState::Loading | LoadState::Idle => "Loading…".to_string(),
                LoadState::Error => "Unavailable".to_string(),
            }
        };
        v_flex()
            .flex_1()
            .overflow_y_scrollbar()
            .gap_4()
            .p_5()
            .child(
                div()
                    .text_lg()
                    .font_semibold()
                    .child(repo.full_name.clone()),
            )
            .child(
                div().text_sm().text_color(muted).child(
                    repo.description
                        .clone()
                        .unwrap_or_else(|| "No repository description.".to_string()),
                ),
            )
            .child(gpui_component::h_flex().flex_wrap().gap_6().children([
                fact("Default branch", &repo.default_branch, cx).into_any_element(),
                fact("Visibility", &repo.visibility, cx).into_any_element(),
                fact("Language", repo.language.as_deref().unwrap_or("—"), cx).into_any_element(),
                fact("Stars", &repo.stargazers_count.to_string(), cx).into_any_element(),
                fact("Forks", &repo.forks_count.to_string(), cx).into_any_element(),
                fact("Open issues", &issues_count, cx).into_any_element(),
                fact("Open pull requests", &pulls_count, cx).into_any_element(),
            ]))
            .child(fact("Last commit", &last_commit_status, cx))
            .when(self.overview_counts.state == LoadState::Error, |this| {
                this.child(status_line(
                    format!(
                        "Could not load open issue and pull-request counts: {}",
                        self.overview_counts.error
                    ),
                    cx,
                ))
            })
            .when(self.last_commit.state == LoadState::Error, |this| {
                this.child(status_line(
                    format!("Could not load last commit: {}", self.last_commit.error),
                    cx,
                ))
            })
            .when(!repo.topics.is_empty(), |this| {
                this.child(
                    gpui_component::h_flex().flex_wrap().gap_2().children(
                        repo.topics
                            .iter()
                            .map(|topic| Tag::secondary().outline().small().child(topic.clone())),
                    ),
                )
            })
            .when(repo.archived, |this| {
                this.child(
                    gpui_component::h_flex()
                        .gap_2()
                        .text_sm()
                        .text_color(muted)
                        .child(Icon::new(IconName::Info))
                        .child("This repository is archived and read-only on GitHub."),
                )
            })
            .when(repo.disabled, |this| {
                this.child(status_line("This repository is disabled on GitHub.", cx))
            })
    }
}

impl Item for WorkspaceTab {
    type Event = ();

    fn tab_content_text(&self, _detail: usize, _cx: &App) -> SharedString {
        self.repo.name.clone().into()
    }

    fn tab_tooltip_text(&self, _cx: &App) -> Option<SharedString> {
        Some(self.repo.full_name.clone().into())
    }
}

fn index_tree_directories(
    nodes: &[helm_backend::github::TreeNode],
    directories: &mut HashMap<String, ()>,
) {
    for node in nodes {
        if node.entry.kind == TreeEntryKind::Tree || !node.children.is_empty() {
            directories.insert(node.path.clone(), ());
        }
        index_tree_directories(&node.children, directories);
    }
}

fn nearest_tree_directory(path: &str, directories: &HashMap<String, ()>) -> Option<String> {
    let mut parent = path.rsplit_once('/')?.0;
    while !parent.is_empty() {
        if directories.contains_key(parent) {
            return Some(parent.to_string());
        }
        parent = parent.rsplit_once('/').map_or("", |(parent, _)| parent);
    }
    None
}

fn find_tree_node<'a>(
    nodes: &'a [helm_backend::github::TreeNode],
    path: &str,
) -> Option<&'a helm_backend::github::TreeNode> {
    for node in nodes {
        if node.path == path {
            return Some(node);
        }
        if let Some(found) = find_tree_node(&node.children, path) {
            return Some(found);
        }
    }
    None
}

fn filtered_tree_item(
    node: &helm_backend::github::TreeNode,
    needle: &str,
    selected_path: &str,
) -> Option<TreeItem> {
    let is_folder = node.entry.kind == TreeEntryKind::Tree || !node.children.is_empty();
    let name_matches = needle.is_empty()
        || node.name.to_lowercase().contains(needle)
        || node.path.to_lowercase().contains(needle);
    let child_needle = if is_folder && name_matches && !needle.is_empty() {
        ""
    } else {
        needle
    };
    let children = node
        .children
        .iter()
        .filter_map(|child| filtered_tree_item(child, child_needle, selected_path))
        .collect::<Vec<_>>();
    if !name_matches && children.is_empty() {
        return None;
    }
    let selected_descendant =
        !selected_path.is_empty() && selected_path.starts_with(&format!("{}/", node.path));
    Some(
        TreeItem::new(node.path.clone(), node.name.clone())
            .children(children)
            .expanded(!needle.is_empty() || selected_descendant),
    )
}

fn find_tree_item(items: &[TreeItem], path: &str) -> Option<TreeItem> {
    for item in items {
        if item.id.as_str() == path {
            return Some(item.clone());
        }
        if let Some(found) = find_tree_item(&item.children, path) {
            return Some(found);
        }
    }
    None
}

fn image_format_for_path(path: &str) -> Option<ImageFormat> {
    let extension = path.rsplit_once('.')?.1.to_ascii_lowercase();
    let mime = match extension.as_str() {
        "avif" => "image/avif",
        "bmp" => "image/bmp",
        "gif" => "image/gif",
        "ico" => "image/ico",
        "jpeg" | "jpg" => "image/jpeg",
        "png" => "image/png",
        "svg" => "image/svg+xml",
        "tif" | "tiff" => "image/tiff",
        "webp" => "image/webp",
        _ => return None,
    };
    ImageFormat::from_mime_type(mime)
}

fn github_permalink(repository_url: &str, commit_sha: &str, path: &str) -> String {
    github_path_permalink(repository_url, "blob", commit_sha, path)
}

fn github_tree_permalink(repository_url: &str, commit_sha: &str, path: &str) -> String {
    github_path_permalink(repository_url, "tree", commit_sha, path)
}

fn github_path_permalink(repository_url: &str, kind: &str, commit_sha: &str, path: &str) -> String {
    if repository_url.is_empty() || commit_sha.is_empty() || path.is_empty() {
        return String::new();
    }
    let encoded_path = path
        .split('/')
        .map(encode_url_path_segment)
        .collect::<Vec<_>>()
        .join("/");
    format!(
        "{}/{kind}/{commit_sha}/{encoded_path}",
        repository_url.trim_end_matches('/')
    )
}

fn encode_url_path_segment(segment: &str) -> String {
    let mut encoded = String::with_capacity(segment.len());
    for byte in segment.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(char::from(byte));
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

fn format_bytes(size: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut value = size as f64;
    let mut unit = 0;
    while value >= 1024. && unit < UNITS.len() - 1 {
        value /= 1024.;
        unit += 1;
    }

    if unit == 0 {
        format!("{size} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

fn short_sha(sha: &str) -> String {
    sha.chars().take(7).collect()
}

fn find_search_result_line(source: &str, query: &str, snippet: Option<&str>) -> Option<u32> {
    if let Some(snippet) = snippet.map(str::trim).filter(|snippet| !snippet.is_empty())
        && let Some(line) = source.lines().position(|line| line.contains(snippet))
    {
        return u32::try_from(line).ok().map(|line| line.saturating_add(1));
    }
    let term = query
        .split_whitespace()
        .filter(|term| !term.contains(':'))
        .map(|term| {
            term.trim_matches(|character: char| {
                matches!(character, '"' | '\'' | '(' | ')' | '*' | '?')
            })
        })
        .find(|term| !term.is_empty())?
        .to_lowercase();
    source
        .lines()
        .position(|line| line.to_lowercase().contains(&term))
        .and_then(|line| u32::try_from(line).ok())
        .map(|line| line.saturating_add(1))
}

fn local_clone_preference(
    local_clone: &LocalClone,
    ref_name: &str,
    ref_is_tag: bool,
    commit_sha: &str,
) -> (bool, bool, String, String) {
    let same_commit = local_clone.commit_sha.as_deref() == Some(commit_sha);
    let same_branch = !ref_is_tag && local_clone.branch.as_deref() == Some(ref_name);
    let root = local_clone.root.to_string_lossy().into_owned();
    (!same_commit, !same_branch, root.to_lowercase(), root)
}

fn safe_local_file_path(root: &std::path::Path, relative_path: &str) -> Option<std::path::PathBuf> {
    let relative_path = std::path::Path::new(relative_path);
    if relative_path.as_os_str().is_empty()
        || relative_path.is_absolute()
        || relative_path
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        return None;
    }
    Some(root.join(relative_path))
}

/// One row of a changed-files list: what happened to the file and its path
/// on the left, the lines added and removed on the right.
fn changed_file_row(
    id: String,
    status: &str,
    path: &str,
    additions: u64,
    deletions: u64,
    selected: bool,
    cx: &App,
) -> ListItem {
    let muted_foreground = cx.theme().muted_foreground;
    let success = cx.theme().success;
    let danger = cx.theme().danger;
    // GitHub's words for a change, as the letters Git uses.
    let letter = match status {
        "added" => "A",
        "removed" => "D",
        "renamed" => "R",
        "copied" => "C",
        _ => "M",
    };
    ListItem::new(id)
        .selected(selected)
        .child(
            gpui_component::h_flex()
                .min_w_0()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .flex_none()
                        .w(px(12.))
                        .text_xs()
                        .text_color(muted_foreground)
                        .child(letter),
                )
                .child(div().truncate().text_sm().child(path.to_string())),
        )
        .suffix(move |_, _| {
            gpui_component::h_flex()
                .flex_none()
                .gap_2()
                .text_xs()
                .child(div().text_color(success).child(format!("+{additions}")))
                .child(div().text_color(danger).child(format!("−{deletions}")))
        })
}

fn fact(label: &str, value: &str, cx: &Context<WorkspaceTab>) -> impl IntoElement {
    v_flex()
        .gap_1()
        .child(
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(label.to_string()),
        )
        .child(div().text_sm().child(value.to_string()))
}

fn status_line(message: impl Into<SharedString>, cx: &App) -> gpui::AnyElement {
    gpui_component::h_flex()
        .gap_2()
        .px_4()
        .py_2()
        .text_sm()
        .text_color(cx.theme().muted_foreground)
        .child(Icon::new(IconName::Info).xsmall())
        .child(message.into())
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permalink_uses_the_exact_commit_and_encodes_each_path_component() {
        assert_eq!(
            github_permalink(
                "https://github.com/example/project/",
                "abc123456789",
                "docs/a file#1.md"
            ),
            "https://github.com/example/project/blob/abc123456789/docs/a%20file%231.md"
        );
    }

    #[test]
    fn permalink_does_not_make_a_branch_based_link_without_a_commit_or_path() {
        assert!(github_permalink("https://github.com/example/project", "", "README.md").is_empty());
        assert!(github_permalink("https://github.com/example/project", "abc123", "").is_empty());
    }

    #[test]
    fn code_search_opens_on_the_snippet_line_or_first_query_term() {
        let source = "fn main() {\n    let needle = true;\n}\n";
        assert_eq!(
            find_search_result_line(source, "needle", Some("let needle = true;")),
            Some(2)
        );
        assert_eq!(
            find_search_result_line(source, "ref:main needle", None),
            Some(2)
        );
        assert_eq!(find_search_result_line(source, "absent", None), None);
    }

    #[test]
    fn local_file_path_rejects_absolute_and_parent_paths() {
        let root = std::path::Path::new("C:\\repos\\sample");
        assert!(safe_local_file_path(root, "src/main.rs").is_some());
        assert!(safe_local_file_path(root, "../outside.txt").is_none());
        assert!(safe_local_file_path(root, "src/../../outside.txt").is_none());
        assert!(safe_local_file_path(root, "C:\\outside.txt").is_none());
    }

    #[test]
    fn matching_local_clone_prefers_commit_then_branch_and_stable_path() {
        let stale_branch_match = LocalClone {
            root: std::path::PathBuf::from("C:\\repos\\a"),
            branch: Some("feature".into()),
            commit_sha: Some("def456".into()),
        };
        let matching_commit = LocalClone {
            root: std::path::PathBuf::from("C:\\repos\\z"),
            branch: Some("main".into()),
            commit_sha: Some("abc123".into()),
        };
        assert!(
            local_clone_preference(&matching_commit, "feature", false, "abc123")
                < local_clone_preference(&stale_branch_match, "feature", false, "abc123")
        );
    }

    #[test]
    fn filename_filter_keeps_matching_files_under_expanded_parent_folders() {
        let entries = [
            TreeEntry {
                path: "src/main.rs".into(),
                kind: TreeEntryKind::Blob,
                ..Default::default()
            },
            TreeEntry {
                path: "src/lib.rs".into(),
                kind: TreeEntryKind::Blob,
                ..Default::default()
            },
            TreeEntry {
                path: "README.md".into(),
                kind: TreeEntryKind::Blob,
                ..Default::default()
            },
        ];
        let nodes = build_tree(&entries);
        let items = nodes
            .iter()
            .filter_map(|node| filtered_tree_item(node, "main.rs", ""))
            .collect::<Vec<_>>();

        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id.as_str(), "src");
        assert!(items[0].is_expanded());
        assert_eq!(items[0].children.len(), 1);
        assert_eq!(items[0].children[0].id.as_str(), "src/main.rs");
    }

    #[test]
    fn missing_path_falls_back_to_its_nearest_existing_directory() {
        let directories = HashMap::from([("src".to_string(), ()), ("src/ui".to_string(), ())]);
        assert_eq!(
            nearest_tree_directory("src/ui/new.rs", &directories),
            Some("src/ui".into())
        );
        assert_eq!(
            nearest_tree_directory("src/missing.rs", &directories),
            Some("src".into())
        );
        assert_eq!(nearest_tree_directory("README.md", &directories), None);
    }
}
