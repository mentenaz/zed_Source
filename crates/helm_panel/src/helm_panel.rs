//! The "Helm" (GitHub) panel — Gate (CLI check) → Auth (login) → Menu, now
//! backed by the real `crate::backend::github` module instead of a UI-only
//! screen flip. Ported from `Forge_Old/panels/github/{gate,auth,menu}.rs`,
//! using `crate::backend::on_tokio` to bridge `github`'s tokio-native
//! `reqwest`/`gh` CLI calls onto GPUI's own `cx.spawn`, and
//! `gpui_component` elements (`Button`, `list::ListItem`, `Icon`,
//! `Spinner`) instead of the old `forge_ui` crate.

mod backend;

use std::sync::Arc;
use std::time::Duration;

use gpui::{
    Action, App, AppContext, AsyncWindowContext, ClipboardItem, Context, DismissEvent, Entity,
    EventEmitter, FocusHandle, Focusable, InteractiveElement, IntoElement, ParentElement,
    PathPromptOptions, Render, StatefulInteractiveElement, Styled, Subscription, TaskExt,
    WeakEntity, Window, actions, div, prelude::FluentBuilder as _, px,
};
use gpui_component::{
    ActiveTheme, Disableable, Icon, IconName, Sizable as _, StyledExt,
    avatar::Avatar,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{Input, InputEvent, InputState},
    list::ListItem,
    menu::{DropdownMenu as _, PopupMenuItem},
    scroll::ScrollableElement as _,
    spinner::Spinner,
    switch::Switch,
    text::markdown,
    v_flex,
};
use serde_json::json;

use crate::backend::github::{
    Branch, CloneEvent, Collaborator, Comment, CommitSummary, Deployment, GhAuthEvent, GhState,
    GitHubUser, GitHubUserDetail, Issue, OrgDetail, OrgInvitation, Package, PackageVersion, Pull,
    Release, Repo, RepoInvitation, RepoTraffic, Tag, WorkflowRun, gh_accept_org_invitation,
    gh_accept_repo_invitation, gh_add_collaborator, gh_auth_status, gh_check_cli, gh_clone_repo,
    gh_create_pull, gh_create_release, gh_create_repo, gh_decline_org_invitation,
    gh_decline_repo_invitation, gh_ensure_repo_scope, gh_get_branches,
    gh_get_collaborators, gh_get_current_user, gh_get_org_detail, gh_get_org_logins,
    gh_get_repo_invitations, gh_get_repos, gh_get_traffic_clones, gh_get_traffic_paths,
    gh_get_traffic_referrers, gh_get_traffic_views, gh_get_user, gh_list_dependabot_alerts,
    gh_list_deployments, gh_list_issue_comments, gh_list_issues, gh_list_org_invitations,
    gh_list_package_versions, gh_list_packages, gh_list_pulls, gh_list_recent_commits,
    gh_list_releases, gh_list_secret_scanning_alerts, gh_list_tags, gh_list_workflow_runs,
    gh_login, gh_logout, gh_remove_collaborator, gh_update_repo, gh_update_topics, gh_update_user,
};
use crate::backend::on_tokio;
use workspace::{
    ModalView, Toast, Workspace,
    dock::{DockPosition, Panel, PanelEvent},
    notifications::NotificationId,
};

actions!(helm_panel, [ToggleFocus]);

pub fn init(cx: &mut App) {
    cx.observe_new(|workspace: &mut Workspace, _, _| {
        workspace.register_action(|workspace, _: &ToggleFocus, window, cx| {
            workspace.toggle_panel_focus::<HelmPanel>(window, cx);
        });
    })
    .detach();
}

/// Which screen the panel is currently showing.
#[derive(Clone, Copy, PartialEq, Eq)]
enum HelmScreen {
    Gate,
    Auth,
    Menu,
    Profile,
    OrgList,
    OrgDetail,
    RepoList,
    RepoDetail,
    Branches,
    Collaborators,
    Issues,
    /// One issue's own view (title/state/author/labels, body, and comment
    /// thread) — reached from `Issues` by clicking a row. See
    /// `Self::open_issue_detail`.
    IssueDetail,
    Pulls,
    /// A PR's own view — same shape as `IssueDetail`, reached from `Pulls`.
    /// See `Self::open_pr_detail`.
    PrDetail,
    Releases,
    Packages,
    Traffic,
    Invitations,
    /// A public profile view for someone other than the signed-in user —
    /// see [`Self::open_user_profile`]. `Profile` stays the signed-in user's
    /// own screen.
    UserProfile,
    Commits,
    WorkflowRuns,
    Deployments,
    Tags,
    Security,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum LoadState {
    Idle,
    Loading,
    Error,
}

/// Result of re-checking `gh auth status`, mirroring the old TS store's
/// `doAuth`.
enum AuthOutcome {
    NotLoggedIn,
    MissingRepoScope {
        account: String,
        scopes: Vec<String>,
    },
    Ready {
        account: String,
        scopes: Vec<String>,
        orgs: Vec<String>,
        user: GitHubUser,
    },
    Failed(String),
}

/// One row in the main menu.
struct MenuItem {
    id: &'static str,
    label: &'static str,
    danger: bool,
}

/// One nav row on the Profile screen.
struct NavRow {
    id: &'static str,
    label: &'static str,
    hint: Option<String>,
}

/// GitHub's collaborator permission levels, in ascending order — used both
/// as the API's `permission` value and as the label shown in the dropdown.
const COLLABORATOR_PERMISSIONS: [&str; 5] = ["pull", "triage", "push", "maintain", "admin"];

pub struct HelmPanel {
    focus_handle: FocusHandle,
    gh_state: Arc<GhState>,
    screen: HelmScreen,
    /// Screens navigated away from via [`Self::navigate_to`], most recent
    /// last. Internal state-machine transitions (Gate finishing, auth
    /// succeeding, logging out) assign `self.screen`/call `set_screen`
    /// directly and don't touch this — you can't usefully "go back" to the
    /// login screen.
    back_stack: Vec<HelmScreen>,
    /// Screens popped off `back_stack` by [`Self::go_back`], available to
    /// [`Self::go_forward`]. Cleared whenever [`Self::navigate_to`] is used.
    forward_stack: Vec<HelmScreen>,
    load_state: LoadState,
    error_msg: String,

    // Auth
    auth_initialized: bool,
    login_started: bool,
    device_code: String,
    device_url: String,
    /// True for 2s after the device code is copied, flips the copy icon to
    /// a checkmark.
    code_copied: bool,

    // Session (populated once authenticated)
    account: String,
    scopes: Vec<String>,
    org_logins: Vec<String>,
    user: Option<GitHubUser>,
    /// Pending repo invitations the signed-in user hasn't accepted yet —
    /// just a count, surfaced as the hint badge on the Profile screen's
    /// Invitations row (loaded by [`Self::load_repo_invitations`]).
    repo_invitation_count: usize,

    /// Which org's details the identity header should show instead of the
    /// user's own profile. Set by [`Self::select_org`], cleared whenever
    /// [`Self::set_screen`] lands on a screen that isn't `OrgDetail`.
    selected_org: Option<String>,
    org_detail: Option<OrgDetail>,

    // Repos
    repos: Vec<Repo>,
    repo_search: Entity<InputState>,

    /// The repo the user drilled into from `RepoList`. Cleared whenever
    /// `set_screen` lands anywhere but `RepoDetail`.
    selected_repo: Option<Repo>,
    /// True for 2s after the clone URL is copied, flips the copy icon to a
    /// checkmark — separate from `code_copied` (device-flow code) since both
    /// concepts, while never shown together, are otherwise unrelated.
    clone_url_copied: bool,
    cloning: bool,
    clone_lines: Vec<String>,
    clone_error: Option<String>,
    /// Set on a successful clone, to the path it landed at — drives the
    /// `HelmModalKind::CloneRepo` modal's "Would you like to open that
    /// workspace?" stage instead of opening it automatically. Reset whenever
    /// [`Self::open_clone_modal`] starts a fresh attempt.
    clone_succeeded_path: Option<String>,
    /// Parent directory the Clone action targets — `None` means "the current
    /// workspace root", the pre-picker default. Chosen via [`Self::pick_clone_dir`]
    /// and consumed by [`Self::handle_clone`].
    clone_target_dir: Option<String>,
    /// The `AppState` this panel needs for the workspace-root reads/writes
    /// around cloning — read to compute where a clone lands, written on a
    /// successful clone so the freshly-cloned repo becomes the open folder.
    workspace: WeakEntity<Workspace>,

    /// Populated by [`Self::load_branches`] for the `Branches` screen.
    branches: Vec<Branch>,
    /// Populated by [`Self::load_collaborators`] for the `Collaborators`
    /// screen.
    collaborators: Vec<Collaborator>,

    // Repo-detail tab caches (Issues/Pulls/Releases/Packages/Traffic) —
    // loaded on screen entry and kept while drilling; `set_screen` clears
    // them along with `selected_repo` when leaving the repo-drilled screens.
    issues: Vec<Issue>,
    issues_filter: String,
    pulls: Vec<Pull>,
    pulls_filter: String,
    /// The issue/PR drilled into from `Issues`/`Pulls` — mutually exclusive
    /// (only one of the two is ever `Some` at a time), cleared whenever
    /// `set_screen` lands anywhere but `IssueDetail`/`PrDetail`. Same
    /// pattern as `selected_repo`.
    selected_issue: Option<Issue>,
    selected_pr: Option<Pull>,
    /// The open item's comment thread, loaded by
    /// [`Self::load_detail_comments`] — shared between `IssueDetail` and
    /// `PrDetail` since GitHub serves both from the same endpoint (see
    /// `Comment`'s doc comment).
    detail_comments: Vec<Comment>,
    detail_comments_state: LoadState,
    releases: Vec<Release>,
    packages: Vec<Package>,
    package_versions: Vec<PackageVersion>,
    package_versions_error: Option<String>,
    expanded_package: Option<String>,
    traffic: Option<RepoTraffic>,
    commits: Vec<CommitSummary>,
    workflow_runs: Vec<WorkflowRun>,
    deployments: Vec<Deployment>,
    tags: Vec<Tag>,
    dependabot_alerts: Vec<serde_json::Value>,
    secret_scanning_alerts: Vec<serde_json::Value>,

    // Pending repo invitations shown on the Profile screen's Invitations
    // screen — `repo_invitation_count` above is just the badge for that row.
    invitations: Vec<RepoInvitation>,
    /// Pending org invitations, shown on the same Invitations screen.
    org_invitations: Vec<OrgInvitation>,

    /// Another user's public profile, viewed via [`Self::open_user_profile`]
    /// (e.g. clicking a collaborator) — cleared whenever `set_screen` lands
    /// anywhere but `UserProfile`. Distinct from `user`, which is always the
    /// signed-in account.
    viewed_user: Option<GitHubUserDetail>,
}

#[derive(Clone)]
enum HelmModalKind {
    CreateRepo,
    EditRepo(Repo),
    AddCollaborator,
    RemoveCollaborator(String),
    /// Carries the repo's default branch, prefilled as the base.
    CreatePull(String),
    CreateRelease,
    /// Carries the signed-in user's current profile, prefilled into the form.
    EditProfile(GitHubUser),
    /// No form fields of its own — reads `parent`'s `cloning`/`clone_lines`/
    /// `clone_error`/`clone_succeeded_path` directly and walks through
    /// picking a folder, showing progress, and offering to open the result,
    /// rather than a single submit like every other kind.
    CloneRepo,
}

struct HelmRepositoryModal {
    kind: HelmModalKind,
    parent: Entity<HelmPanel>,
    /// Re-renders this modal whenever `parent` does — needed only for
    /// `CloneRepo`, which reads `parent`'s clone-progress fields live rather
    /// than owning its own form state. `None` for every other kind.
    _clone_progress_sub: Option<Subscription>,
    focus_handle: FocusHandle,
    name: Entity<InputState>,
    description: Entity<InputState>,
    homepage: Entity<InputState>,
    topics: Entity<InputState>,
    organization: Entity<InputState>,
    username: Entity<InputState>,
    tag_name: Entity<InputState>,
    head_branch: Entity<InputState>,
    base_branch: Entity<InputState>,
    company: Entity<InputState>,
    location: Entity<InputState>,
    private: bool,
    has_issues: bool,
    has_wiki: bool,
    has_projects: bool,
    has_discussions: bool,
    draft: bool,
    prerelease: bool,
    permission: usize,
}

impl HelmRepositoryModal {
    fn new(
        kind: HelmModalKind,
        parent: Entity<HelmPanel>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let (name_value, description_value, homepage_value, topics_value) = match &kind {
            HelmModalKind::EditRepo(repo) => (
                repo.name.clone(),
                repo.description.clone().unwrap_or_default(),
                repo.homepage.clone().unwrap_or_default(),
                repo.topics.join(", "),
            ),
            HelmModalKind::EditProfile(user) => (
                user.name.clone().unwrap_or_default(),
                user.bio.clone().unwrap_or_default(),
                user.blog.clone().unwrap_or_default(),
                String::new(),
            ),
            _ => (String::new(), String::new(), String::new(), String::new()),
        };
        let (has_issues, has_wiki, has_projects, has_discussions) = match &kind {
            HelmModalKind::EditRepo(repo) => (
                repo.has_issues,
                repo.has_wiki,
                repo.has_projects,
                repo.has_discussions,
            ),
            _ => (true, true, true, true),
        };
        let (company_value, location_value) = match &kind {
            HelmModalKind::EditProfile(user) => (
                user.company.clone().unwrap_or_default(),
                user.location.clone().unwrap_or_default(),
            ),
            _ => (String::new(), String::new()),
        };
        let base_branch_value = match &kind {
            HelmModalKind::CreatePull(default_base) => default_base.clone(),
            _ => String::new(),
        };

        let clone_progress_sub = matches!(kind, HelmModalKind::CloneRepo)
            .then(|| cx.observe(&parent, |_, _, cx| cx.notify()));

        Self {
            kind,
            parent,
            _clone_progress_sub: clone_progress_sub,
            focus_handle: cx.focus_handle(),
            name: cx.new(|cx| InputState::new(window, cx).default_value(name_value)),
            description: cx.new(|cx| InputState::new(window, cx).default_value(description_value)),
            homepage: cx.new(|cx| InputState::new(window, cx).default_value(homepage_value)),
            topics: cx.new(|cx| {
                InputState::new(window, cx)
                    .default_value(topics_value)
                    .placeholder("comma, separated, topics")
            }),
            organization: cx.new(|cx| {
                InputState::new(window, cx).placeholder("Organization (blank = your account)")
            }),
            username: cx.new(|cx| InputState::new(window, cx).placeholder("GitHub username")),
            tag_name: cx.new(|cx| InputState::new(window, cx).placeholder("v1.0.0")),
            head_branch: cx.new(|cx| InputState::new(window, cx).placeholder("feature-branch")),
            base_branch: cx.new(|cx| InputState::new(window, cx).default_value(base_branch_value)),
            company: cx.new(|cx| InputState::new(window, cx).default_value(company_value)),
            location: cx.new(|cx| InputState::new(window, cx).default_value(location_value)),
            private: true,
            has_issues,
            has_wiki,
            has_projects,
            has_discussions,
            draft: false,
            prerelease: false,
            permission: 0,
        }
    }

    fn title(&self) -> &'static str {
        match self.kind {
            HelmModalKind::CreateRepo => "Create repository",
            HelmModalKind::EditRepo(_) => "Edit repository",
            HelmModalKind::AddCollaborator => "Add collaborator",
            HelmModalKind::RemoveCollaborator(_) => "Remove collaborator?",
            HelmModalKind::CreatePull(_) => "Create pull request",
            HelmModalKind::CreateRelease => "Create release",
            HelmModalKind::EditProfile(_) => "Edit profile",
            HelmModalKind::CloneRepo => "Clone repository",
        }
    }
}

impl Focusable for HelmRepositoryModal {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<DismissEvent> for HelmRepositoryModal {}
impl ModalView for HelmRepositoryModal {}

impl Render for HelmRepositoryModal {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let foreground = cx.theme().foreground;
        let muted = cx.theme().muted_foreground;
        let parent = self.parent.clone();
        let title = self.title();
        let kind = self.kind.clone();
        let confirm_kind = kind.clone();
        let show_footer = !matches!(confirm_kind, HelmModalKind::CloneRepo);

        let content = match &kind {
            HelmModalKind::CreateRepo => v_flex()
                .gap_3()
                .child(Input::new(&self.name))
                .child(Input::new(&self.description))
                .child(Input::new(&self.organization))
                .child(
                    Switch::new("helm-modal-private")
                        .label("Private")
                        .checked(self.private)
                        .on_click(cx.listener(|this, checked: &bool, _, cx| {
                            this.private = *checked;
                            cx.notify();
                        })),
                )
                .into_any_element(),
            HelmModalKind::EditRepo(_) => v_flex()
                .gap_3()
                .child(Input::new(&self.name))
                .child(Input::new(&self.description))
                .child(Input::new(&self.homepage))
                .child(Input::new(&self.topics))
                .child(
                    Switch::new("helm-modal-issues")
                        .label("Issues")
                        .checked(self.has_issues)
                        .on_click(cx.listener(|this, checked: &bool, _, cx| {
                            this.has_issues = *checked;
                            cx.notify();
                        })),
                )
                .child(
                    Switch::new("helm-modal-projects")
                        .label("Projects")
                        .checked(self.has_projects)
                        .on_click(cx.listener(|this, checked: &bool, _, cx| {
                            this.has_projects = *checked;
                            cx.notify();
                        })),
                )
                .child(
                    Switch::new("helm-modal-wiki")
                        .label("Wiki")
                        .checked(self.has_wiki)
                        .on_click(cx.listener(|this, checked: &bool, _, cx| {
                            this.has_wiki = *checked;
                            cx.notify();
                        })),
                )
                .child(
                    Switch::new("helm-modal-discussions")
                        .label("Discussions")
                        .checked(self.has_discussions)
                        .on_click(cx.listener(|this, checked: &bool, _, cx| {
                            this.has_discussions = *checked;
                            cx.notify();
                        })),
                )
                .into_any_element(),
            HelmModalKind::AddCollaborator => v_flex()
                .gap_3()
                .child(Input::new(&self.username))
                .child(
                    Button::new("helm-modal-permission")
                        .outline()
                        .label(COLLABORATOR_PERMISSIONS[self.permission])
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.permission =
                                (this.permission + 1) % COLLABORATOR_PERMISSIONS.len();
                            cx.notify();
                        })),
                )
                .into_any_element(),
            HelmModalKind::RemoveCollaborator(login) => v_flex()
                .gap_2()
                .child(div().text_color(foreground).child(format!(
                    "{login} will lose access to this repository immediately."
                )))
                .into_any_element(),
            HelmModalKind::CreatePull(_) => v_flex()
                .gap_3()
                .child(labeled_field("Title", Input::new(&self.name), muted))
                .child(labeled_field(
                    "Description",
                    Input::new(&self.description),
                    muted,
                ))
                .child(labeled_field(
                    "Head branch",
                    Input::new(&self.head_branch),
                    muted,
                ))
                .child(labeled_field(
                    "Base branch",
                    Input::new(&self.base_branch),
                    muted,
                ))
                .into_any_element(),
            HelmModalKind::CreateRelease => v_flex()
                .gap_3()
                .child(labeled_field("Tag", Input::new(&self.tag_name), muted))
                .child(labeled_field("Title", Input::new(&self.name), muted))
                .child(labeled_field(
                    "Description",
                    Input::new(&self.description),
                    muted,
                ))
                .child(
                    Switch::new("helm-modal-draft")
                        .label("Draft")
                        .checked(self.draft)
                        .on_click(cx.listener(|this, checked: &bool, _, cx| {
                            this.draft = *checked;
                            cx.notify();
                        })),
                )
                .child(
                    Switch::new("helm-modal-prerelease")
                        .label("Pre-release")
                        .checked(self.prerelease)
                        .on_click(cx.listener(|this, checked: &bool, _, cx| {
                            this.prerelease = *checked;
                            cx.notify();
                        })),
                )
                .into_any_element(),
            HelmModalKind::EditProfile(_) => v_flex()
                .gap_3()
                .child(labeled_field("Name", Input::new(&self.name), muted))
                .child(labeled_field("Bio", Input::new(&self.description), muted))
                .child(labeled_field("Company", Input::new(&self.company), muted))
                .child(labeled_field("Location", Input::new(&self.location), muted))
                .child(labeled_field(
                    "Blog / website",
                    Input::new(&self.homepage),
                    muted,
                ))
                .into_any_element(),
            HelmModalKind::CloneRepo => {
                let panel = parent.read(cx);
                if let Some(succeeded_path) = panel.clone_succeeded_path.clone() {
                    let repo_name = panel
                        .selected_repo
                        .as_ref()
                        .map(|repo| repo.name.clone())
                        .unwrap_or_default();
                    v_flex()
                        .gap_3()
                        .items_center()
                        .child(
                            Icon::new(IconName::CircleCheck)
                                .size(px(32.))
                                .text_color(cx.theme().success),
                        )
                        .child(
                            div()
                                .text_color(foreground)
                                .child(format!("You have successfully cloned {repo_name}.")),
                        )
                        .child(
                            div()
                                .text_sm()
                                .text_color(muted)
                                .child("Would you like to open that workspace?"),
                        )
                        .child(
                            h_flex()
                                .justify_end()
                                .gap_2()
                                .child(
                                    Button::new("helm-clone-open-no")
                                        .outline()
                                        .label("No")
                                        .on_click(cx.listener(|_, _, _, cx| {
                                            cx.emit(DismissEvent);
                                        })),
                                )
                                .child(Button::new("helm-clone-open-yes").primary().label("Yes").on_click({
                                    let parent = parent.clone();
                                    cx.listener(move |_, _, window, cx| {
                                        parent.update(cx, |panel, cx| {
                                            panel.handle_clone_open_workspace(
                                                succeeded_path.clone(),
                                                window,
                                                cx,
                                            );
                                        });
                                        cx.emit(DismissEvent);
                                    })
                                })),
                        )
                        .into_any_element()
                } else if panel.cloning {
                    let recent_lines: Vec<String> = panel
                        .clone_lines
                        .iter()
                        .rev()
                        .take(20)
                        .rev()
                        .cloned()
                        .collect();
                    v_flex()
                        .gap_2()
                        .child(
                            h_flex()
                                .gap_2()
                                .items_center()
                                .child(Spinner::new().small())
                                .child(div().text_sm().text_color(muted).child("Cloning…")),
                        )
                        .children(recent_lines.into_iter().map(|line| {
                            div()
                                .font_family("Cascadia Mono")
                                .text_xs()
                                .text_color(muted)
                                .child(line)
                        }))
                        .into_any_element()
                } else if let Some(err) = panel.clone_error.clone() {
                    v_flex()
                        .gap_2()
                        .child(div().text_sm().text_color(cx.theme().danger).child(format!("✗ {err}")))
                        .child(
                            h_flex().justify_end().gap_2().child(
                                Button::new("helm-clone-retry").outline().label("Retry").on_click({
                                    let parent = parent.clone();
                                    cx.listener(move |_, _, window, cx| {
                                        parent.update(cx, |panel, cx| panel.handle_clone(window, cx));
                                    })
                                }),
                            ),
                        )
                        .into_any_element()
                } else {
                    let target = panel.clone_target_path(cx).unwrap_or_default();
                    v_flex()
                        .gap_2()
                        .child(
                            h_flex()
                                .items_center()
                                .justify_between()
                                .gap_2()
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(muted)
                                        .child(format!("Clones into {target}")),
                                )
                                .child(
                                    Button::new("helm-clone-choose-dir")
                                        .ghost()
                                        .xsmall()
                                        .label("Choose folder…")
                                        .on_click({
                                            let parent = parent.clone();
                                            cx.listener(move |_, _, _, cx| {
                                                parent.update(cx, |panel, cx| panel.pick_clone_dir(cx));
                                            })
                                        }),
                                ),
                        )
                        .child(
                            Button::new("helm-clone-start")
                                .primary()
                                .icon(IconName::Github)
                                .label("Clone repository")
                                .on_click({
                                    let parent = parent.clone();
                                    cx.listener(move |_, _, window, cx| {
                                        parent.update(cx, |panel, cx| panel.handle_clone(window, cx));
                                    })
                                }),
                        )
                        .into_any_element()
                }
            }
        };

        let confirm = cx.listener(move |this, _, window, cx| {
            let parent = parent.clone();
            let kind = kind.clone();
            let name = this.name.read(cx).value().to_string();
            let description = this.description.read(cx).value().to_string();
            let homepage = this.homepage.read(cx).value().to_string();
            let topics = this
                .topics
                .read(cx)
                .value()
                .split(',')
                .map(|topic| topic.trim().to_string())
                .filter(|topic| !topic.is_empty())
                .collect::<Vec<_>>();
            let organization = this.organization.read(cx).value().trim().to_string();
            let username = this.username.read(cx).value().trim().to_string();
            let permission = COLLABORATOR_PERMISSIONS[this.permission].to_string();
            let private = this.private;
            let has_issues = this.has_issues;
            let has_wiki = this.has_wiki;
            let has_projects = this.has_projects;
            let has_discussions = this.has_discussions;
            let tag_name = this.tag_name.read(cx).value().to_string();
            let head_branch = this.head_branch.read(cx).value().trim().to_string();
            let base_branch = this.base_branch.read(cx).value().trim().to_string();
            let company = this.company.read(cx).value().to_string();
            let location = this.location.read(cx).value().to_string();
            let draft = this.draft;
            let prerelease = this.prerelease;

            match kind {
                HelmModalKind::CreateRepo => {
                    parent.update(cx, |this, cx| {
                        this.handle_create_repo(
                            name,
                            description,
                            private,
                            (!organization.is_empty()).then_some(organization),
                            window,
                            cx,
                        );
                    });
                }
                HelmModalKind::EditRepo(_) => {
                    parent.update(cx, |this, cx| {
                        this.handle_edit_repo(
                            name,
                            description,
                            homepage,
                            topics,
                            has_issues,
                            has_wiki,
                            has_projects,
                            has_discussions,
                            window,
                            cx,
                        );
                    });
                }
                HelmModalKind::AddCollaborator => {
                    if username.is_empty() {
                        return;
                    }
                    parent.update(cx, |this, cx| {
                        this.handle_set_collaborator_permission(username, permission, window, cx);
                    });
                }
                HelmModalKind::RemoveCollaborator(login) => {
                    parent.update(cx, |this, cx| {
                        this.handle_remove_collaborator(login, window, cx);
                    });
                }
                HelmModalKind::CreatePull(_) => {
                    if head_branch.is_empty() || base_branch.is_empty() || name.is_empty() {
                        return;
                    }
                    parent.update(cx, |this, cx| {
                        this.handle_create_pull(name, description, head_branch, base_branch, window, cx);
                    });
                }
                HelmModalKind::CreateRelease => {
                    if tag_name.is_empty() {
                        return;
                    }
                    parent.update(cx, |this, cx| {
                        this.handle_create_release(
                            tag_name, name, description, draft, prerelease, window, cx,
                        );
                    });
                }
                HelmModalKind::EditProfile(_) => {
                    parent.update(cx, |this, cx| {
                        this.handle_update_profile(
                            name, description, company, location, homepage, window, cx,
                        );
                    });
                }
                // The footer's generic Confirm button is hidden for this kind
                // (see below) — its own content has its own buttons/handlers,
                // each dismissing explicitly where appropriate.
                HelmModalKind::CloneRepo => return,
            }
            cx.emit(DismissEvent);
        });

        v_flex()
            .w(px(520.))
            .max_h(px(720.))
            .gap_3()
            .p_4()
            .bg(cx.theme().background)
            .border_1()
            .border_color(cx.theme().border)
            .rounded_lg()
            .child(div().font_semibold().text_color(foreground).child(title))
            .child(div().text_sm().text_color(muted).child(content))
            .when(show_footer, |el| {
                el.child(
                    h_flex()
                        .justify_end()
                        .gap_2()
                        .child(
                            Button::new("helm-modal-cancel")
                                .outline()
                                .label("Cancel")
                                .on_click(cx.listener(|_, _, _, cx| cx.emit(DismissEvent))),
                        )
                        .child(
                            Button::new("helm-modal-confirm")
                                .primary()
                                .label(match confirm_kind {
                                    HelmModalKind::CreateRepo => "Create",
                                    HelmModalKind::EditRepo(_) => "Save",
                                    HelmModalKind::AddCollaborator => "Add",
                                    HelmModalKind::RemoveCollaborator(_) => "Remove",
                                    HelmModalKind::CreatePull(_) => "Create",
                                    HelmModalKind::CreateRelease => "Create",
                                    HelmModalKind::EditProfile(_) => "Save",
                                    HelmModalKind::CloneRepo => "Clone",
                                })
                                .on_click(confirm),
                        ),
                )
            })
    }
}

impl HelmPanel {
    pub async fn load(
        workspace: WeakEntity<Workspace>,
        mut cx: AsyncWindowContext,
    ) -> anyhow::Result<Entity<Self>> {
        workspace.update_in(&mut cx, |_, window, cx| {
            Self::view(workspace.clone(), window, cx)
        })
    }

    pub fn view(
        workspace: WeakEntity<Workspace>,
        window: &mut Window,
        cx: &mut App,
    ) -> Entity<Self> {
        cx.new(|cx| {
            let repo_search =
                cx.new(|cx| InputState::new(window, cx).placeholder("Search repositories…"));
            cx.subscribe(&repo_search, |_this, _, event: &InputEvent, cx| {
                if let InputEvent::Change = event {
                    cx.notify();
                }
            })
            .detach();

            let mut this = Self {
                focus_handle: cx.focus_handle(),
                gh_state: Arc::new(GhState::default()),
                screen: HelmScreen::Gate,
                back_stack: Vec::new(),
                forward_stack: Vec::new(),
                load_state: LoadState::Idle,
                error_msg: String::new(),
                auth_initialized: false,
                login_started: false,
                device_code: String::new(),
                device_url: String::new(),
                code_copied: false,
                account: String::new(),
                scopes: Vec::new(),
                org_logins: Vec::new(),
                user: None,
                repo_invitation_count: 0,
                selected_org: None,
                org_detail: None,
                repos: Vec::new(),
                repo_search,
                selected_repo: None,
                clone_url_copied: false,
                cloning: false,
                clone_lines: Vec::new(),
                clone_error: None,
                clone_succeeded_path: None,
                clone_target_dir: None,
                workspace,
                branches: Vec::new(),
                collaborators: Vec::new(),
                issues: Vec::new(),
                issues_filter: "open".into(),
                pulls: Vec::new(),
                pulls_filter: "open".into(),
                selected_issue: None,
                selected_pr: None,
                detail_comments: Vec::new(),
                detail_comments_state: LoadState::Idle,
                releases: Vec::new(),
                packages: Vec::new(),
                package_versions: Vec::new(),
                package_versions_error: None,
                expanded_package: None,
                traffic: None,
                commits: Vec::new(),
                workflow_runs: Vec::new(),
                deployments: Vec::new(),
                tags: Vec::new(),
                dependabot_alerts: Vec::new(),
                secret_scanning_alerts: Vec::new(),
                invitations: Vec::new(),
                org_invitations: Vec::new(),
                viewed_user: None,
            };
            this.check_cli(cx);
            this
        })
    }

    fn check_cli(&mut self, cx: &mut Context<Self>) {
        self.screen = HelmScreen::Gate;
        self.load_state = LoadState::Loading;
        self.error_msg.clear();
        cx.notify();

        cx.spawn(async move |this, cx| {
            let result = on_tokio(gh_check_cli()).await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(()) => {
                        this.load_state = LoadState::Idle;
                        this.error_msg.clear();
                        this.do_auth(cx);
                    }
                    Err(_) => {
                        this.load_state = LoadState::Idle;
                        this.error_msg = "GitHub CLI (gh) is not installed or not on PATH.".into();
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn do_auth(&mut self, cx: &mut Context<Self>) {
        self.screen = HelmScreen::Auth;
        self.load_state = LoadState::Loading;
        self.error_msg.clear();
        cx.notify();

        let gh_state = self.gh_state.clone();
        cx.spawn(async move |this, cx| {
            let outcome = on_tokio(async move {
                let info = match gh_auth_status().await {
                    Ok(info) => info,
                    Err(e) => return AuthOutcome::Failed(e),
                };
                let Some(info) = info else {
                    return AuthOutcome::NotLoggedIn;
                };
                if !info.scopes.iter().any(|s| s == "repo") {
                    return AuthOutcome::MissingRepoScope {
                        account: info.account,
                        scopes: info.scopes,
                    };
                }
                let orgs = match gh_get_org_logins(&gh_state).await {
                    Ok(o) => o,
                    Err(e) => return AuthOutcome::Failed(e),
                };
                let user = match gh_get_current_user(&gh_state).await {
                    Ok(u) => u,
                    Err(e) => return AuthOutcome::Failed(e),
                };
                AuthOutcome::Ready {
                    account: info.account,
                    scopes: info.scopes,
                    orgs,
                    user,
                }
            })
            .await;

            this.update(cx, |this, cx| {
                this.auth_initialized = true;
                match outcome {
                    AuthOutcome::Ready {
                        account,
                        scopes,
                        orgs,
                        user,
                    } => {
                        this.account = account;
                        this.scopes = scopes;
                        this.org_logins = orgs;
                        this.user = Some(user);
                        this.load_state = LoadState::Idle;
                        this.error_msg.clear();
                        this.screen = HelmScreen::Menu;
                        this.load_repo_invitations(cx);
                    }
                    AuthOutcome::MissingRepoScope { account, scopes } => {
                        this.account = account;
                        this.scopes = scopes;
                        this.load_state = LoadState::Idle;
                        this.error_msg = "Missing 'repo' scope.".into();
                    }
                    AuthOutcome::NotLoggedIn => {
                        this.load_state = LoadState::Idle;
                    }
                    AuthOutcome::Failed(e) => {
                        this.load_state = LoadState::Error;
                        this.error_msg = e;
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Loads the pending repo- and org-invitation lists for the Profile
    /// screen's Invitations row (badge count + the Invitations screen
    /// itself). Fire-and-forget: a failure on either just leaves the
    /// previous state for that list in place.
    fn load_repo_invitations(&mut self, cx: &mut Context<Self>) {
        let gh_state = self.gh_state.clone();
        cx.spawn(async move |this, cx| {
            let (repo_result, org_result) = on_tokio(async move {
                let repo_result = gh_get_repo_invitations(&gh_state).await;
                let org_result = gh_list_org_invitations(&gh_state).await;
                (repo_result, org_result)
            })
            .await;
            this.update(cx, |this, cx| {
                if let Ok(invitations) = repo_result {
                    this.invitations = invitations;
                }
                if let Ok(org_invitations) = org_result {
                    this.org_invitations = org_invitations;
                }
                this.repo_invitation_count = this.invitations.len() + this.org_invitations.len();
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn handle_login(&mut self, cx: &mut Context<Self>) {
        self.login_started = true;
        self.device_code.clear();
        self.device_url.clear();
        self.code_copied = false;
        self.load_state = LoadState::Loading;
        self.error_msg.clear();
        cx.notify();

        // Subscribe before starting the login process so no early lines are
        // missed.
        let mut rx = self.gh_state.auth_tx.subscribe();
        cx.spawn(async move |this, cx| {
            loop {
                let (event, rx2) = on_tokio(async move {
                    let event = rx.recv().await;
                    (event, rx)
                })
                .await;
                rx = rx2;
                match event {
                    Ok(GhAuthEvent::Line(line)) => {
                        let (code, url) = parse_device_line(&line);
                        if code.is_none() && url.is_none() {
                            continue;
                        }
                        let alive = this
                            .update(cx, |this, cx| {
                                if let Some(c) = code {
                                    this.device_code = c;
                                }
                                if let Some(u) = url {
                                    this.device_url = u;
                                }
                                cx.notify();
                            })
                            .is_ok();
                        if !alive {
                            break;
                        }
                    }
                    Ok(GhAuthEvent::Done) => break,
                    // `Lagged` means missed events, not a dead channel —
                    // resync and keep listening; only `Closed` ends this loop.
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        })
        .detach();

        let gh_state = self.gh_state.clone();
        cx.spawn(async move |this, cx| {
            let result = on_tokio(async move { gh_login(&gh_state).await }).await;
            this.update(cx, |this, cx| match result {
                Ok(()) => this.do_auth(cx),
                Err(e) => {
                    this.load_state = LoadState::Error;
                    this.error_msg = e;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    fn handle_ensure_repo_scope(&mut self, cx: &mut Context<Self>) {
        self.load_state = LoadState::Loading;
        self.error_msg.clear();
        cx.notify();

        let gh_state = self.gh_state.clone();
        cx.spawn(async move |this, cx| {
            let result = on_tokio(async move { gh_ensure_repo_scope(&gh_state).await }).await;
            this.update(cx, |this, cx| match result {
                Ok(()) => this.do_auth(cx),
                Err(e) => {
                    this.load_state = LoadState::Error;
                    this.error_msg = e;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Copies the device code and flips the icon to a checkmark for 2s.
    fn handle_copy_code(&mut self, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(self.device_code.clone()));
        self.code_copied = true;
        cx.notify();

        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_secs(2)).await;
            this.update(cx, |this, cx| {
                this.code_copied = false;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn handle_logout(&mut self, cx: &mut Context<Self>) {
        self.load_state = LoadState::Loading;
        cx.notify();

        let gh_state = self.gh_state.clone();
        cx.spawn(async move |this, cx| {
            let result = on_tokio(async move { gh_logout(&gh_state).await }).await;
            this.update(cx, |this, cx| match result {
                Ok(()) => {
                    this.account.clear();
                    this.scopes.clear();
                    this.org_logins.clear();
                    this.user = None;
                    this.auth_initialized = false;
                    this.login_started = false;
                    this.device_code.clear();
                    this.device_url.clear();
                    this.code_copied = false;
                    this.back_stack.clear();
                    this.forward_stack.clear();
                    this.check_cli(cx);
                }
                Err(e) => {
                    this.load_state = LoadState::Error;
                    this.error_msg = e;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Lands on `screen` without touching `back_stack`/`forward_stack` —
    /// for internal state-machine transitions. User-initiated navigation
    /// should go through [`Self::navigate_to`] instead.
    fn set_screen(&mut self, screen: HelmScreen, cx: &mut Context<Self>) {
        self.screen = screen;
        self.error_msg.clear();
        // Landing on a screen that isn't org-scoped drops whichever org was
        // being browsed — `OrgList` itself doesn't need `selected_org`, only
        // `OrgDetail` does. `RepoList`/`RepoDetail` and the screens drilled
        // into from it (Branches/Collaborators/the repo tabs) are also kept:
        // they can be showing an org's own repos (reached via
        // `open_repo_list` from `OrgDetail`), and clearing
        // `selected_org`/`org_detail` here meant going back to `OrgDetail`
        // afterwards found no org data left to render — it fell into the
        // "Failed to load" empty state, whose Retry button then no-op'd too
        // since `selected_org` was already gone.
        const ORG_SCOPED: &[HelmScreen] = &[
            HelmScreen::OrgDetail,
            HelmScreen::RepoList,
            HelmScreen::RepoDetail,
            HelmScreen::Branches,
            HelmScreen::Collaborators,
            HelmScreen::Issues,
            HelmScreen::IssueDetail,
            HelmScreen::Pulls,
            HelmScreen::PrDetail,
            HelmScreen::Releases,
            HelmScreen::Packages,
            HelmScreen::Traffic,
            HelmScreen::Commits,
            HelmScreen::WorkflowRuns,
            HelmScreen::Deployments,
            HelmScreen::Tags,
            HelmScreen::Security,
        ];
        const REPO_DRIVEN: &[HelmScreen] = &[
            HelmScreen::RepoDetail,
            HelmScreen::Branches,
            HelmScreen::Collaborators,
            HelmScreen::Issues,
            HelmScreen::IssueDetail,
            HelmScreen::Pulls,
            HelmScreen::PrDetail,
            HelmScreen::Releases,
            HelmScreen::Packages,
            HelmScreen::Traffic,
            HelmScreen::Commits,
            HelmScreen::WorkflowRuns,
            HelmScreen::Deployments,
            HelmScreen::Tags,
            HelmScreen::Security,
        ];
        if !ORG_SCOPED.contains(&screen) {
            self.selected_org = None;
            self.org_detail = None;
        }
        // `selected_repo`/clone state is only meaningful on `RepoDetail` and
        // the screens drilled into from it; clearing here also drops the tab
        // caches so going back to `RepoDetail` doesn't resurrect stale data.
        if !REPO_DRIVEN.contains(&screen) {
            self.selected_repo = None;
            self.cloning = false;
            self.clone_lines.clear();
            self.clone_error = None;
            self.issues.clear();
            self.pulls.clear();
            self.releases.clear();
            self.packages.clear();
            self.package_versions.clear();
            self.expanded_package = None;
            self.traffic = None;
            self.commits.clear();
            self.workflow_runs.clear();
            self.deployments.clear();
            self.tags.clear();
            self.dependabot_alerts.clear();
            self.secret_scanning_alerts.clear();
        }
        if screen != HelmScreen::UserProfile {
            self.viewed_user = None;
        }
        // Refresh the invitations badge every time the Profile screen (or the
        // Invitations screen itself) is landed on, including via back/forward.
        if matches!(screen, HelmScreen::Profile | HelmScreen::Invitations) {
            self.load_repo_invitations(cx);
        }
        cx.notify();
    }

    /// User picked an org from the list: sets `selected_org` (which also
    /// switches the identity header to the org's own avatar/name, see
    /// `render_identity_header`), navigates to `OrgDetail`, and kicks off
    /// the detail fetch.
    fn select_org(&mut self, org: String, cx: &mut Context<Self>) {
        self.selected_org = Some(org.clone());
        self.org_detail = None;
        self.navigate_to(HelmScreen::OrgDetail, cx);
        self.load_org(org, cx);
    }

    fn load_org(&mut self, org: String, cx: &mut Context<Self>) {
        self.load_state = LoadState::Loading;
        self.error_msg.clear();
        cx.notify();

        let gh_state = self.gh_state.clone();
        cx.spawn(async move |this, cx| {
            let result = on_tokio(async move { gh_get_org_detail(org, &gh_state).await }).await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(detail) => {
                        this.org_detail = Some(detail);
                        this.load_state = LoadState::Idle;
                    }
                    Err(e) => {
                        this.load_state = LoadState::Error;
                        this.error_msg = e;
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Navigate to the repo list, loading either the selected org's repos
    /// or the user's own (`"self"`) — mirrors the old TS
    /// `loadRepos(selectedOrg || "self")`.
    fn open_repo_list(&mut self, cx: &mut Context<Self>) {
        let owner = self
            .selected_org
            .clone()
            .unwrap_or_else(|| "self".to_string());
        self.navigate_to(HelmScreen::RepoList, cx);
        self.load_repos(owner, cx);
    }

    fn load_repos(&mut self, owner: String, cx: &mut Context<Self>) {
        self.load_state = LoadState::Loading;
        self.error_msg.clear();
        cx.notify();

        let gh_state = self.gh_state.clone();
        cx.spawn(async move |this, cx| {
            let result = on_tokio(async move { gh_get_repos(owner, &gh_state).await }).await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(repos) => {
                        this.repos = repos;
                        this.load_state = LoadState::Idle;
                    }
                    Err(e) => {
                        this.load_state = LoadState::Error;
                        this.error_msg = e;
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// User picked a repo from `RepoList` — mirrors the old TS
    /// `setSelectedRepo` + `setScreen("repo-detail")` pair. No extra fetch
    /// needed, the list response already has everything the detail screen
    /// shows.
    fn select_repo(&mut self, repo: Repo, cx: &mut Context<Self>) {
        self.selected_repo = Some(repo);
        self.navigate_to(HelmScreen::RepoDetail, cx);
    }

    /// Copies the repo's clone URL and flips the icon to a checkmark for 2s
    /// — same pattern as `handle_copy_code` for the device-flow code.
    fn handle_copy_clone_url(&mut self, cx: &mut Context<Self>) {
        let Some(repo) = self.selected_repo.as_ref() else {
            return;
        };
        cx.write_to_clipboard(ClipboardItem::new_string(repo.clone_url.clone()));
        self.clone_url_copied = true;
        cx.notify();

        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_secs(2)).await;
            this.update(cx, |this, cx| {
                this.clone_url_copied = false;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The exact path a clone of `self.selected_repo` would land at — the
    /// current workspace's first worktree root (or the user-picked
    /// `clone_target_dir`) joined with the repo's name. `None` when no
    /// folder is open yet or no repo is selected.
    fn clone_target_path(&self, cx: &App) -> Option<String> {
        let repo = self.selected_repo.as_ref()?;
        let parent = match self.clone_target_dir.clone() {
            Some(dir) => dir,
            None => self
                .workspace
                .upgrade()?
                .read(cx)
                .worktrees(cx)
                .next()?
                .read(cx)
                .abs_path()
                .to_string_lossy()
                .into_owned(),
        };
        Some(
            std::path::Path::new(&parent)
                .join(&repo.name)
                .to_string_lossy()
                .into_owned(),
        )
    }

    /// Opens the Clone-repository overlay, resetting any previous
    /// attempt's progress/error/success state so it always starts fresh.
    fn open_clone_modal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.cloning = false;
        self.clone_lines.clear();
        self.clone_error = None;
        self.clone_succeeded_path = None;
        self.open_workspace_modal(HelmModalKind::CloneRepo, window, cx);
    }

    /// Opens `path` (a just-completed clone) as a new workspace window — the
    /// Clone overlay's "Yes" button, called instead of doing this
    /// automatically so the user can decline and clone elsewhere without a
    /// second window popping up unasked.
    fn handle_clone_open_workspace(&mut self, path: String, window: &mut Window, cx: &mut Context<Self>) {
        self.workspace
            .update(cx, |workspace, cx| {
                workspace
                    .open_workspace_for_paths(
                        workspace::OpenMode::NewWindow,
                        vec![std::path::PathBuf::from(&path)],
                        window,
                        cx,
                    )
                    .detach_and_log_err(cx);
            })
            .ok();
    }

    /// Clones `self.selected_repo` into [`Self::clone_target_path`]. On
    /// success, stores the landing path in `clone_succeeded_path` instead of
    /// opening it automatically — see `HelmModalKind::CloneRepo`'s render,
    /// which then offers to open it. No-ops if no folder is open yet (see
    /// `render_repo_detail`, which disables the Clone button in that case).
    fn handle_clone(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(repo) = self.selected_repo.clone() else {
            return;
        };
        let Some(target_path) = self.clone_target_path(cx) else {
            return;
        };

        self.cloning = true;
        self.clone_lines.clear();
        self.clone_error = None;
        cx.notify();

        // Subscribe before starting the clone so no early lines are missed
        // — same reasoning as `handle_login`'s `auth_tx` subscription.
        let mut rx = self.gh_state.clone_tx.subscribe();
        cx.spawn_in(window, async move |this, cx| {
            loop {
                let (event, rx2) = on_tokio(async move {
                    let event = rx.recv().await;
                    (event, rx)
                })
                .await;
                rx = rx2;
                let line = match event {
                    Ok(CloneEvent::Line(line)) => line,
                    Ok(CloneEvent::NpmStart) => "Installing npm dependencies…".to_string(),
                    Ok(CloneEvent::Done) => break,
                    // `Lagged` means missed events, not a dead channel —
                    // resync and keep listening; only `Closed` ends this loop.
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                };
                let alive = this
                    .update(cx, |this, cx| {
                        this.clone_lines.push(line);
                        cx.notify();
                    })
                    .is_ok();
                if !alive {
                    break;
                }
            }
        })
        .detach();

        let gh_state = self.gh_state.clone();
        let full_name = repo.full_name.clone();
        let target_for_gh = target_path.clone();
        cx.spawn_in(window, async move |this, cx| {
            let result =
                on_tokio(
                    async move { gh_clone_repo(full_name, target_for_gh, false, &gh_state).await },
                )
                .await;
            this.update_in(cx, |this, window, cx| {
                this.cloning = false;
                match result {
                    Ok(()) => {
                        this.clone_lines
                            .push(format!("Cloned repository to {target_path}"));
                        this.workspace
                            .update(cx, |workspace, cx| {
                                workspace
                                    .open_workspace_for_paths(
                                        workspace::OpenMode::NewWindow,
                                        vec![std::path::PathBuf::from(&target_path)],
                                        window,
                                        cx,
                                    )
                                    .detach_and_log_err(cx);
                            })
                            .ok();
                    }
                    Err(e) => this.clone_error = Some(e),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Native folder picker for the Clone destination — stores the chosen
    /// parent directory in `clone_target_dir`; leaving it unset drags along
    /// the current workspace root. Cancelling the dialog keeps the previous
    /// choice (or the default).
    fn pick_clone_dir(&mut self, cx: &mut Context<Self>) {
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Choose Clone Destination".into()),
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = receiver.await {
                if let Some(path) = paths.first() {
                    this.update(cx, |this, cx| {
                        this.clone_target_dir = Some(path.to_string_lossy().into_owned());
                        cx.notify();
                    })
                    .ok();
                }
            }
        })
        .detach();
    }

    /// Creates a repo under the authenticated user's own account, or under the
    /// given org (`Some(owner)`) via `gh_create_repo`'s `"owner"` key — the
    /// key is stripped from the request body and rerouted to
    /// `POST /orgs/{org}/repos` under the hood. On success, jumps straight to
    /// the new repo's detail screen; on failure, surfaces the error via a
    /// notification since the Menu screen (where this is triggered from) has
    /// no dedicated place to show it.
    fn handle_create_repo(
        &mut self,
        name: String,
        description: String,
        private: bool,
        owner: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut opts = json!({
            "name": name,
            "description": description,
            "private": private,
        });
        if let Some(owner) = owner {
            opts["owner"] = json!(owner);
        }

        let gh_state = self.gh_state.clone();
        cx.spawn_in(window, async move |this, cx| {
            let result = on_tokio(async move { gh_create_repo(opts, &gh_state).await }).await;
            this.update_in(cx, |this, _window, cx| match result {
                Ok(repo) => {
                    this.repos.push(repo.clone());
                    this.select_repo(repo, cx);
                }
                Err(e) => {
                    this.notify(format!("Failed to create repository: {e}"), cx);
                }
            })
            .ok();
        })
        .detach();
    }

    /// Renames/updates description+homepage, toggles feature switches
    /// (issues/wiki/projects/discussions) and updates topics for
    /// `self.selected_repo`, in that order — the topics endpoint 404s on the
    /// pre-rename name, so it must use the renamed repo's name from the
    /// first response.
    fn handle_edit_repo(
        &mut self,
        name: String,
        description: String,
        homepage: String,
        topics: Vec<String>,
        has_issues: bool,
        has_wiki: bool,
        has_projects: bool,
        has_discussions: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(repo) = self.selected_repo.clone() else {
            return;
        };
        let owner = repo.owner.login.clone();

        let changes = json!({
            "name": name,
            "description": description,
            "homepage": homepage,
            "has_issues": has_issues,
            "has_wiki": has_wiki,
            "has_projects": has_projects,
            "has_discussions": has_discussions,
        });

        let gh_state = self.gh_state.clone();
        cx.spawn_in(window, async move |this, cx| {
            let result = on_tokio(async move {
                let updated =
                    gh_update_repo(owner.clone(), repo.name.clone(), changes, &gh_state).await?;
                let topics_result =
                    gh_update_topics(owner, updated.name.clone(), topics, &gh_state).await;
                match topics_result {
                    Ok(_) => Ok(updated),
                    Err(e) => Err(e),
                }
            })
            .await;
            this.update_in(cx, |this, _window, cx| match result {
                Ok(mut updated) => {
                    // `gh_update_repo`'s response reflects the PATCH body, not the
                    // just-applied topics PUT — fold the topics we sent back in so
                    // `selected_repo` doesn't briefly show stale ones.
                    if let Some(idx) = this.repos.iter().position(|r| r.id == updated.id) {
                        std::mem::swap(&mut this.repos[idx], &mut updated);
                        updated = this.repos[idx].clone();
                    }
                    this.selected_repo = Some(updated);
                    cx.notify();
                }
                Err(e) => {
                    this.notify(format!("Failed to update repository: {e}"), cx);
                }
            })
            .ok();
        })
        .detach();
    }

    /// Loads the branch list for `self.selected_repo` — mirrors `load_repos`'s
    /// shape.
    fn load_branches(&mut self, cx: &mut Context<Self>) {
        let Some(repo) = self.selected_repo.clone() else {
            return;
        };
        self.load_state = LoadState::Loading;
        self.error_msg.clear();
        self.branches.clear();
        cx.notify();

        let gh_state = self.gh_state.clone();
        cx.spawn(async move |this, cx| {
            let result =
                on_tokio(
                    async move { gh_get_branches(repo.owner.login, repo.name, &gh_state).await },
                )
                .await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(branches) => {
                        this.branches = branches;
                        this.load_state = LoadState::Idle;
                    }
                    Err(e) => {
                        this.load_state = LoadState::Error;
                        this.error_msg = e;
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Loads the collaborator list for `self.selected_repo` — mirrors
    /// `load_repos`'s shape.
    fn load_collaborators(&mut self, cx: &mut Context<Self>) {
        let Some(repo) = self.selected_repo.clone() else {
            return;
        };
        self.load_state = LoadState::Loading;
        self.error_msg.clear();
        cx.notify();

        let gh_state = self.gh_state.clone();
        cx.spawn(async move |this, cx| {
            let result = on_tokio(async move {
                gh_get_collaborators(repo.owner.login, repo.name, &gh_state).await
            })
            .await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(collaborators) => {
                        this.collaborators = collaborators;
                        this.load_state = LoadState::Idle;
                    }
                    Err(e) => {
                        this.load_state = LoadState::Error;
                        this.error_msg = e;
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Loads `self.selected_repo`'s issue list for the current
    /// `issues_filter`. The Issues tab drops PR-shaped items (the `/issues`
    /// endpoint mixes issues and PRs).
    fn load_issues(&mut self, cx: &mut Context<Self>) {
        let Some(repo) = self.selected_repo.clone() else {
            return;
        };
        self.load_state = LoadState::Loading;
        self.error_msg.clear();
        cx.notify();
        let filter = self.issues_filter.clone();
        let gh_state = self.gh_state.clone();
        cx.spawn(async move |this, cx| {
            let result = on_tokio(async move {
                gh_list_issues(repo.owner.login, repo.name, filter, &gh_state).await
            })
            .await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(mut issues) => {
                        issues.retain(|issue| issue.pull_request.is_none());
                        this.issues = issues;
                        this.load_state = LoadState::Idle;
                    }
                    Err(e) => {
                        this.load_state = LoadState::Error;
                        this.error_msg = e;
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Loads `self.selected_repo`'s pull requests for the current
    /// `pulls_filter`.
    fn load_pulls(&mut self, cx: &mut Context<Self>) {
        let Some(repo) = self.selected_repo.clone() else {
            return;
        };
        self.load_state = LoadState::Loading;
        self.error_msg.clear();
        cx.notify();
        let filter = self.pulls_filter.clone();
        let gh_state = self.gh_state.clone();
        cx.spawn(async move |this, cx| {
            let result = on_tokio(async move {
                gh_list_pulls(repo.owner.login, repo.name, filter, &gh_state).await
            })
            .await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(pulls) => {
                        this.pulls = pulls;
                        this.load_state = LoadState::Idle;
                    }
                    Err(e) => {
                        this.load_state = LoadState::Error;
                        this.error_msg = e;
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// User clicked an issue row: shows it in-panel instead of opening
    /// GitHub in the browser (`Issues`'s list `Issue` already has the full
    /// body — the list endpoint returns it — so this needs no fetch of its
    /// own beyond the comment thread).
    fn open_issue_detail(&mut self, issue: Issue, cx: &mut Context<Self>) {
        let number = issue.number;
        self.selected_issue = Some(issue);
        self.navigate_to(HelmScreen::IssueDetail, cx);
        self.load_detail_comments(number, cx);
    }

    /// Same as [`Self::open_issue_detail`], for a PR row.
    fn open_pr_detail(&mut self, pr: Pull, cx: &mut Context<Self>) {
        let number = pr.number;
        self.selected_pr = Some(pr);
        self.navigate_to(HelmScreen::PrDetail, cx);
        self.load_detail_comments(number, cx);
    }

    /// Loads the comment thread for whichever issue/PR is now open —
    /// shared between both, since GitHub serves PR "conversation" comments
    /// from the same `/issues/{number}/comments` endpoint (see `Comment`'s
    /// doc comment).
    fn load_detail_comments(&mut self, number: u64, cx: &mut Context<Self>) {
        let Some(repo) = self.selected_repo.clone() else {
            return;
        };
        self.detail_comments_state = LoadState::Loading;
        self.detail_comments.clear();
        cx.notify();
        let gh_state = self.gh_state.clone();
        cx.spawn(async move |this, cx| {
            let result = on_tokio(async move {
                gh_list_issue_comments(repo.owner.login, repo.name, number, &gh_state).await
            })
            .await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(comments) => {
                        this.detail_comments = comments;
                        this.detail_comments_state = LoadState::Idle;
                    }
                    Err(e) => {
                        this.detail_comments_state = LoadState::Error;
                        this.error_msg = e;
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Loads `self.selected_repo`'s releases.
    fn load_releases(&mut self, cx: &mut Context<Self>) {
        let Some(repo) = self.selected_repo.clone() else {
            return;
        };
        self.load_state = LoadState::Loading;
        self.error_msg.clear();
        cx.notify();
        let gh_state = self.gh_state.clone();
        cx.spawn(async move |this, cx| {
            let result =
                on_tokio(
                    async move { gh_list_releases(repo.owner.login, repo.name, &gh_state).await },
                )
                .await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(releases) => {
                        this.releases = releases;
                        this.load_state = LoadState::Idle;
                    }
                    Err(e) => {
                        this.load_state = LoadState::Error;
                        this.error_msg = e;
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Loads `self.selected_repo`'s packages (owner-scoped: either the repo's
    /// org or the user's own account).
    fn load_packages(&mut self, cx: &mut Context<Self>) {
        let Some(repo) = self.selected_repo.clone() else {
            return;
        };
        self.load_state = LoadState::Loading;
        self.error_msg.clear();
        self.package_versions.clear();
        self.package_versions_error = None;
        self.expanded_package = None;
        cx.notify();
        let gh_state = self.gh_state.clone();
        cx.spawn(async move |this, cx| {
            let result =
                on_tokio(async move { gh_list_packages(repo.owner.login, &gh_state).await }).await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(packages) => {
                        this.packages = packages;
                        this.load_state = LoadState::Idle;
                    }
                    Err(e) => {
                        this.load_state = LoadState::Error;
                        this.error_msg = e;
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Expands `pkg` to show its versions (loading them on first tap) — a
    /// second tap on the same package collapses it again.
    fn toggle_package_versions(&mut self, owner: String, pkg: Package, cx: &mut Context<Self>) {
        if let Some(expanded) = self.expanded_package.as_deref() {
            if expanded == pkg.name {
                self.expanded_package = None;
                self.package_versions.clear();
                self.package_versions_error = None;
                cx.notify();
                return;
            }
        }
        self.expanded_package = Some(pkg.name.clone());
        self.package_versions.clear();
        self.package_versions_error = None;
        cx.notify();
        let gh_state = self.gh_state.clone();
        cx.spawn(async move |this, cx| {
            let result = on_tokio(async move {
                gh_list_package_versions(owner, pkg.package_type, pkg.name.clone(), &gh_state).await
            })
            .await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(versions) => this.package_versions = versions,
                    Err(e) => this.package_versions_error = Some(e),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Loads the four traffic endpoints for `self.selected_repo` into one
    /// [`RepoTraffic`]. Views/clones come back `None` when GitHub has no
    /// traffic data yet (202/404); referrers/paths just stay empty on
    /// failure.
    fn load_traffic(&mut self, cx: &mut Context<Self>) {
        let Some(repo) = self.selected_repo.clone() else {
            return;
        };
        self.load_state = LoadState::Loading;
        self.error_msg.clear();
        cx.notify();
        let gh_state = self.gh_state.clone();
        let owner = repo.owner.login;
        let name = repo.name;
        cx.spawn(async move |this, cx| {
            let result = on_tokio(async move {
                let views = gh_get_traffic_views(owner.clone(), name.clone(), &gh_state)
                    .await
                    .ok();
                let clones = gh_get_traffic_clones(owner.clone(), name.clone(), &gh_state)
                    .await
                    .ok();
                let referrers = gh_get_traffic_referrers(owner.clone(), name.clone(), &gh_state)
                    .await
                    .unwrap_or_default();
                let paths = gh_get_traffic_paths(owner.clone(), name.clone(), &gh_state)
                    .await
                    .unwrap_or_default();
                RepoTraffic {
                    views,
                    clones,
                    referrers,
                    paths,
                }
            })
            .await;
            this.update(cx, |this, cx| {
                this.traffic = Some(result);
                this.load_state = LoadState::Idle;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Loads `self.selected_repo`'s recent commits (Commits screen).
    fn load_commits(&mut self, cx: &mut Context<Self>) {
        let Some(repo) = self.selected_repo.clone() else {
            return;
        };
        self.load_state = LoadState::Loading;
        self.error_msg.clear();
        cx.notify();
        let gh_state = self.gh_state.clone();
        cx.spawn(async move |this, cx| {
            let result = on_tokio(async move {
                gh_list_recent_commits(repo.owner.login, repo.name, &gh_state).await
            })
            .await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(commits) => {
                        this.commits = commits;
                        this.load_state = LoadState::Idle;
                    }
                    Err(e) => {
                        this.load_state = LoadState::Error;
                        this.error_msg = e;
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Loads `self.selected_repo`'s recent Actions/CI workflow runs.
    fn load_workflow_runs(&mut self, cx: &mut Context<Self>) {
        let Some(repo) = self.selected_repo.clone() else {
            return;
        };
        self.load_state = LoadState::Loading;
        self.error_msg.clear();
        cx.notify();
        let gh_state = self.gh_state.clone();
        cx.spawn(async move |this, cx| {
            let result = on_tokio(async move {
                gh_list_workflow_runs(repo.owner.login, repo.name, &gh_state).await
            })
            .await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(runs) => {
                        this.workflow_runs = runs;
                        this.load_state = LoadState::Idle;
                    }
                    Err(e) => {
                        this.load_state = LoadState::Error;
                        this.error_msg = e;
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Loads `self.selected_repo`'s deployments.
    fn load_deployments(&mut self, cx: &mut Context<Self>) {
        let Some(repo) = self.selected_repo.clone() else {
            return;
        };
        self.load_state = LoadState::Loading;
        self.error_msg.clear();
        cx.notify();
        let gh_state = self.gh_state.clone();
        cx.spawn(async move |this, cx| {
            let result = on_tokio(async move {
                gh_list_deployments(repo.owner.login, repo.name, &gh_state).await
            })
            .await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(deployments) => {
                        this.deployments = deployments;
                        this.load_state = LoadState::Idle;
                    }
                    Err(e) => {
                        this.load_state = LoadState::Error;
                        this.error_msg = e;
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Loads `self.selected_repo`'s tags.
    fn load_tags(&mut self, cx: &mut Context<Self>) {
        let Some(repo) = self.selected_repo.clone() else {
            return;
        };
        self.load_state = LoadState::Loading;
        self.error_msg.clear();
        cx.notify();
        let gh_state = self.gh_state.clone();
        cx.spawn(async move |this, cx| {
            let result =
                on_tokio(async move { gh_list_tags(repo.owner.login, repo.name, &gh_state).await })
                    .await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(tags) => {
                        this.tags = tags;
                        this.load_state = LoadState::Idle;
                    }
                    Err(e) => {
                        this.load_state = LoadState::Error;
                        this.error_msg = e;
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Loads `self.selected_repo`'s dependabot and secret-scanning alerts for
    /// the Security screen — like `load_traffic`, each endpoint's failure is
    /// independent (a repo can have one feature enabled and not the other).
    fn load_security(&mut self, cx: &mut Context<Self>) {
        let Some(repo) = self.selected_repo.clone() else {
            return;
        };
        self.load_state = LoadState::Loading;
        self.error_msg.clear();
        cx.notify();
        let gh_state = self.gh_state.clone();
        let owner = repo.owner.login;
        let name = repo.name;
        cx.spawn(async move |this, cx| {
            let (dependabot, secret_scanning) = on_tokio(async move {
                let dependabot = gh_list_dependabot_alerts(owner.clone(), name.clone(), &gh_state)
                    .await
                    .unwrap_or_default();
                let secret_scanning =
                    gh_list_secret_scanning_alerts(owner, name, &gh_state)
                        .await
                        .unwrap_or_default();
                (dependabot, secret_scanning)
            })
            .await;
            this.update(cx, |this, cx| {
                this.dependabot_alerts = dependabot;
                this.secret_scanning_alerts = secret_scanning;
                this.load_state = LoadState::Idle;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// User clicked a username shown elsewhere in the panel (e.g. a
    /// collaborator row) — opens their public profile.
    fn open_user_profile(&mut self, username: String, cx: &mut Context<Self>) {
        self.navigate_to(HelmScreen::UserProfile, cx);
        self.load_user_profile(username, cx);
    }

    fn load_user_profile(&mut self, username: String, cx: &mut Context<Self>) {
        self.load_state = LoadState::Loading;
        self.error_msg.clear();
        self.viewed_user = None;
        cx.notify();
        let gh_state = self.gh_state.clone();
        cx.spawn(async move |this, cx| {
            let result = on_tokio(async move { gh_get_user(username, &gh_state).await }).await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(user) => {
                        this.viewed_user = Some(user);
                        this.load_state = LoadState::Idle;
                    }
                    Err(e) => {
                        this.load_state = LoadState::Error;
                        this.error_msg = e;
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Opens the "Create pull request" modal, prefilling the base branch
    /// with `self.selected_repo`'s default branch.
    fn open_create_pull_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let default_base = self
            .selected_repo
            .as_ref()
            .map(|r| r.default_branch.clone())
            .unwrap_or_default();
        self.open_workspace_modal(HelmModalKind::CreatePull(default_base), window, cx);
    }

    fn handle_create_pull(
        &mut self,
        title: String,
        body: String,
        head: String,
        base: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(repo) = self.selected_repo.clone() else {
            return;
        };
        let gh_state = self.gh_state.clone();
        cx.spawn_in(window, async move |this, cx| {
            let result = on_tokio(async move {
                gh_create_pull(
                    repo.owner.login,
                    repo.name,
                    head,
                    base,
                    title,
                    (!body.is_empty()).then_some(body),
                    &gh_state,
                )
                .await
            })
            .await;
            this.update_in(cx, |this, _window, cx| {
                if let Err(e) = result {
                    this.notify(format!("Failed to create pull request: {e}"), cx);
                }
                this.load_pulls(cx);
            })
            .ok();
        })
        .detach();
    }

    /// Opens the "Create release" modal.
    fn open_create_release_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_workspace_modal(HelmModalKind::CreateRelease, window, cx);
    }

    fn handle_create_release(
        &mut self,
        tag_name: String,
        title: String,
        body: String,
        draft: bool,
        prerelease: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(repo) = self.selected_repo.clone() else {
            return;
        };
        let gh_state = self.gh_state.clone();
        cx.spawn_in(window, async move |this, cx| {
            let result = on_tokio(async move {
                gh_create_release(
                    repo.owner.login,
                    repo.name,
                    tag_name,
                    (!title.is_empty()).then_some(title),
                    (!body.is_empty()).then_some(body),
                    Some(draft),
                    Some(prerelease),
                    &gh_state,
                )
                .await
            })
            .await;
            this.update_in(cx, |this, _window, cx| {
                if let Err(e) = result {
                    this.notify(format!("Failed to create release: {e}"), cx);
                }
                this.load_releases(cx);
            })
            .ok();
        })
        .detach();
    }

    /// Opens the "Edit profile" modal, prefilled from `self.user`.
    fn open_edit_profile_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(user) = self.user.clone() else {
            return;
        };
        self.open_workspace_modal(HelmModalKind::EditProfile(user), window, cx);
    }

    /// Updates the signed-in user's profile. GitHub's `/user` PATCH endpoint
    /// needs the `user` OAuth scope — already granted at login time
    /// (`gh_login` requests `repo,read:org,user` up front, see cli.rs), so
    /// unlike `gh_ensure_repo_scope` (an explicit, user-visible re-auth
    /// screen for a scope that's genuinely sometimes missing) this doesn't
    /// call `gh_ensure_user_scope` here: that runs `gh auth refresh`, an
    /// interactive device-code re-auth, with no UI surfacing the
    /// code/URL — it would silently hang Save waiting on a browser flow the
    /// user was never shown.
    fn handle_update_profile(
        &mut self,
        name: String,
        bio: String,
        company: String,
        location: String,
        blog: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let gh_state = self.gh_state.clone();
        cx.spawn_in(window, async move |this, cx| {
            let result = on_tokio(async move {
                let changes = json!({
                    "name": name,
                    "bio": bio,
                    "company": company,
                    "location": location,
                    "blog": blog,
                });
                gh_update_user(changes, &gh_state).await
            })
            .await;
            this.update_in(cx, |this, window, cx| match result {
                Ok(_) => {
                    this.notify("Profile updated", cx);
                    let gh_state = this.gh_state.clone();
                    cx.spawn_in(window, async move |this, cx| {
                        let updated =
                            on_tokio(async move { gh_get_current_user(&gh_state).await }).await;
                        this.update(cx, |this, cx| {
                            if let Ok(user) = updated {
                                this.user = Some(user);
                            }
                            cx.notify();
                        })
                        .ok();
                    })
                    .detach();
                }
                Err(e) => {
                    this.notify(format!("Failed to update profile: {e}"), cx);
                }
            })
            .ok();
        })
        .detach();
    }

    /// Accepts a pending repo invitation and drops it from the list + badge.
    fn handle_accept_invitation(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        let gh_state = self.gh_state.clone();
        cx.spawn_in(window, async move |this, cx| {
            let result =
                on_tokio(async move { gh_accept_repo_invitation(id, &gh_state).await }).await;
            this.update_in(cx, |this, _window, cx| match result {
                Ok(()) => {
                    this.invitations.retain(|inv| inv.id != id);
                    this.repo_invitation_count = this.invitations.len();
                    this.notify("Invitation accepted", cx);
                }
                Err(e) => {
                    this.notify(format!("Failed to accept invitation: {e}"), cx);
                }
            })
            .ok();
        })
        .detach();
    }

    /// Declines a pending repo invitation and drops it from the list + badge.
    fn handle_decline_invitation(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        let gh_state = self.gh_state.clone();
        cx.spawn_in(window, async move |this, cx| {
            let result =
                on_tokio(async move { gh_decline_repo_invitation(id, &gh_state).await }).await;
            this.update_in(cx, |this, _window, cx| match result {
                Ok(()) => {
                    this.invitations.retain(|inv| inv.id != id);
                    this.repo_invitation_count = this.invitations.len() + this.org_invitations.len();
                    this.notify("Invitation declined", cx);
                }
                Err(e) => {
                    this.notify(format!("Failed to decline invitation: {e}"), cx);
                }
            })
            .ok();
        })
        .detach();
    }

    /// Accepts a pending org invitation and drops it from the list + badge.
    fn handle_accept_org_invitation(
        &mut self,
        org: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let gh_state = self.gh_state.clone();
        let org_for_call = org.clone();
        cx.spawn_in(window, async move |this, cx| {
            let result =
                on_tokio(async move { gh_accept_org_invitation(org_for_call, &gh_state).await })
                    .await;
            this.update_in(cx, |this, _window, cx| match result {
                Ok(()) => {
                    this.org_invitations
                        .retain(|inv| inv.organization.login != org);
                    this.repo_invitation_count = this.invitations.len() + this.org_invitations.len();
                    this.notify("Invitation accepted", cx);
                }
                Err(e) => {
                    this.notify(format!("Failed to accept invitation: {e}"), cx);
                }
            })
            .ok();
        })
        .detach();
    }

    /// Declines a pending org invitation and drops it from the list + badge.
    fn handle_decline_org_invitation(
        &mut self,
        org: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let gh_state = self.gh_state.clone();
        let org_for_call = org.clone();
        cx.spawn_in(window, async move |this, cx| {
            let result =
                on_tokio(async move { gh_decline_org_invitation(org_for_call, &gh_state).await })
                    .await;
            this.update_in(cx, |this, _window, cx| match result {
                Ok(()) => {
                    this.org_invitations.retain(|inv| inv.organization.login != org);
                    this.repo_invitation_count = this.invitations.len() + this.org_invitations.len();
                    this.notify("Invitation declined", cx);
                }
                Err(e) => {
                    this.notify(format!("Failed to decline invitation: {e}"), cx);
                }
            })
            .ok();
        })
        .detach();
    }

    /// Sets `login`'s permission on `self.selected_repo` — GitHub's
    /// add-collaborator endpoint doubles as the update-permission endpoint,
    /// so this is also how an existing collaborator's role is changed —
    /// then refreshes the list.
    fn handle_set_collaborator_permission(
        &mut self,
        login: String,
        permission: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(repo) = self.selected_repo.clone() else {
            return;
        };
        let gh_state = self.gh_state.clone();
        cx.spawn_in(window, async move |this, cx| {
            let result = on_tokio(async move {
                gh_add_collaborator(repo.owner.login, repo.name, login, permission, &gh_state).await
            })
            .await;
            this.update_in(cx, |this, _window, cx| {
                if let Err(e) = result {
                    this.notify(format!("Failed to update collaborator: {e}"), cx);
                }
                this.load_collaborators(cx);
            })
            .ok();
        })
        .detach();
    }

    /// Removes `login` as a collaborator on `self.selected_repo`, then
    /// refreshes the list.
    fn handle_remove_collaborator(
        &mut self,
        login: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(repo) = self.selected_repo.clone() else {
            return;
        };
        let gh_state = self.gh_state.clone();
        cx.spawn_in(window, async move |this, cx| {
            let result = on_tokio(async move {
                gh_remove_collaborator(repo.owner.login, repo.name, login, &gh_state).await
            })
            .await;
            this.update_in(cx, |this, _window, cx| {
                if let Err(e) = result {
                    this.notify(format!("Failed to remove collaborator: {e}"), cx);
                }
                this.load_collaborators(cx);
            })
            .ok();
        })
        .detach();
    }

    /// Navigate to `screen` as a user-initiated action: push the current
    /// screen onto the back stack and clear the forward stack (same as
    /// following a link in a browser).
    fn navigate_to(&mut self, screen: HelmScreen, cx: &mut Context<Self>) {
        self.back_stack.push(self.screen);
        self.forward_stack.clear();
        self.set_screen(screen, cx);
    }

    fn go_back(&mut self, cx: &mut Context<Self>) {
        if let Some(prev) = self.back_stack.pop() {
            self.forward_stack.push(self.screen);
            self.set_screen(prev, cx);
        }
    }

    fn go_forward(&mut self, cx: &mut Context<Self>) {
        if let Some(next) = self.forward_stack.pop() {
            self.back_stack.push(self.screen);
            self.set_screen(next, cx);
        }
    }

    fn go_home(&mut self, cx: &mut Context<Self>) {
        if self.screen != HelmScreen::Menu {
            self.navigate_to(HelmScreen::Menu, cx);
        }
    }

    /// Home/Back/Forward controls, shown in the panel header once
    /// authenticated.
    fn render_nav_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let buttons: [(&'static str, IconName, &'static str, bool); 3] = [
            (
                "helm-nav-home",
                IconName::LayoutDashboard,
                "Home",
                self.screen != HelmScreen::Menu,
            ),
            (
                "helm-nav-back",
                IconName::ArrowLeft,
                "Back",
                !self.back_stack.is_empty(),
            ),
            (
                "helm-nav-forward",
                IconName::ArrowRight,
                "Forward",
                !self.forward_stack.is_empty(),
            ),
        ];

        h_flex()
            .items_center()
            .gap_1()
            .children(buttons.into_iter().map(|(id, icon, tooltip, enabled)| {
                Button::new(id)
                    .ghost()
                    .xsmall()
                    .icon(icon)
                    .disabled(!enabled)
                    .tooltip(tooltip)
                    .on_click(cx.listener(move |this, _, _, cx| match id {
                        "helm-nav-home" => this.go_home(cx),
                        "helm-nav-back" => this.go_back(cx),
                        "helm-nav-forward" => this.go_forward(cx),
                        _ => {}
                    }))
            }))
    }

    /// "View Profile" — stats row plus nav rows into Edit
    /// Profile/Repositories/Organizations/Invitations/Account Security.
    /// Organizations and Repositories show real counts; Invitations shows
    /// its count only when there are pending invites to surface
    /// (`repo_invitation_count`, loaded by [`Self::load_repo_invitations`]).
    /// OrgList/RepoList land on real screens; the remaining rows are
    /// informational for now.
    fn render_profile(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let muted_foreground = cx.theme().muted_foreground;
        let foreground = cx.theme().foreground;
        let border = cx.theme().border;

        let Some(user) = self.user.clone() else {
            return v_flex()
                .flex_1()
                .items_center()
                .justify_center()
                .p_4()
                .child(
                    div()
                        .text_sm()
                        .text_color(muted_foreground)
                        .child("No profile data."),
                )
                .into_any_element();
        };

        let total_repos = user.public_repos + user.total_private_repos.unwrap_or(0);

        let stats_row = h_flex()
            .items_center()
            .gap_4()
            .px_3()
            .py_2()
            .text_sm()
            .text_color(muted_foreground)
            .child(format!("Followers {}", fmt_num(user.followers)))
            .child(format!("Following {}", fmt_num(user.following)))
            .child(format!("Repos {}", fmt_num(total_repos)));

        let rows: Vec<NavRow> = [
            NavRow {
                id: "edit-profile",
                label: "Edit Profile",
                hint: None,
            },
            NavRow {
                id: "repos",
                label: "Repositories",
                hint: Some(total_repos.to_string()),
            },
        ]
        .into_iter()
        .chain(if !self.org_logins.is_empty() {
            Some(NavRow {
                id: "orgs",
                label: "Organizations",
                hint: Some(self.org_logins.len().to_string()),
            })
        } else {
            None
        })
        .chain(if self.repo_invitation_count > 0 {
            Some(NavRow {
                id: "invitations",
                label: "Invitations",
                hint: Some(self.repo_invitation_count.to_string()),
            })
        } else {
            None
        })
        .chain([NavRow {
            id: "account-security",
            label: "Account Security",
            hint: None,
        }])
        .collect();

        v_flex()
            .child(stats_row)
            .child(div().h_px().w_full().bg(border))
            .child(v_flex().py_1().children(rows.into_iter().map(|row| {
                let hint = row.hint;
                let id = row.id;
                ListItem::new(format!("helm-profile-{}", row.id))
                    .child(div().text_color(foreground).child(row.label))
                    .suffix(move |_, _| {
                        h_flex()
                            .items_center()
                            .gap_2()
                            .children(
                                hint.clone()
                                    .map(|hint| div().text_color(muted_foreground).child(hint)),
                            )
                            .child(
                                Icon::new(IconName::ChevronRight)
                                    .xsmall()
                                    .text_color(muted_foreground),
                            )
                    })
                    .on_click(cx.listener(move |this, _, window, cx| match id {
                        "orgs" => this.navigate_to(HelmScreen::OrgList, cx),
                        "repos" => this.open_repo_list(cx),
                        "invitations" => this.navigate_to(HelmScreen::Invitations, cx),
                        "edit-profile" => this.open_edit_profile_dialog(window, cx),
                        // No per-account security API is wired up (Dependabot
                        // /secret-scanning alerts, elsewhere in this panel,
                        // are repo-scoped, not account-scoped) — opens
                        // GitHub's own settings page instead of a dead click.
                        "account-security" => cx.open_url("https://github.com/settings/security"),
                        _ => {}
                    }))
            })))
            .into_any_element()
    }

    /// The identity strip shown between the panel header and the current
    /// screen on every authenticated screen (mirrors the old app's
    /// `render_identity_header`, with the org-identity swap since `OrgDetail`
    /// landed). `None` before `self.user` has loaded, which in practice only
    /// happens on Gate/Auth (excluded by `render`'s `show_nav` condition
    /// anyway).
    fn render_identity_header(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let muted_foreground = cx.theme().muted_foreground;
        let border = cx.theme().border;

        // `OrgDetail` swaps in the org's own avatar/name instead of the
        // user's — matches the old app's `PanelHeader`/`OrgHeader` split.
        // `selected_org` being set but `org_detail` still loading just
        // hides the header for a moment, rather than flashing stale user
        // identity.
        let (avatar_url, name, login, description) = if self.selected_org.is_some() {
            let org = self.org_detail.as_ref()?;
            (
                org.avatar_url.clone(),
                org.name.clone().unwrap_or_else(|| org.login.clone()),
                org.login.clone(),
                org.description.clone(),
            )
        } else {
            let user = self.user.as_ref()?;
            (
                user.avatar_url.clone(),
                user.name.clone().unwrap_or_else(|| user.login.clone()),
                user.login.clone(),
                user.bio.clone(),
            )
        };

        Some(
            v_flex()
                .flex_shrink_0()
                .child(
                    h_flex()
                        .gap_3()
                        .items_center()
                        .px_3()
                        .py_2()
                        .child(
                            Avatar::new()
                                .src(avatar_url)
                                .name(name.clone())
                                .with_size(px(40.)),
                        )
                        .child(
                            v_flex()
                                .gap_0()
                                .min_w_0()
                                .child(div().font_semibold().child(name))
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(muted_foreground)
                                        .child(format!("@{login}")),
                                )
                                .when_some(
                                    description.filter(|bio| !bio.trim().is_empty()),
                                    |el, bio| {
                                        el.child(
                                            div()
                                                .text_xs()
                                                .text_color(muted_foreground)
                                                .child(bio),
                                        )
                                    },
                                ),
                        ),
                )
                .child(div().h_px().w_full().bg(border)),
        )
    }

    /// The "gh CLI required" gate — shown until `gh --version` succeeds.
    fn render_gate(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let muted_foreground = cx.theme().muted_foreground;
        let warning = cx.theme().warning;

        if self.load_state == LoadState::Loading {
            return v_flex()
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
                                .child("Checking for GitHub CLI…"),
                        ),
                )
                .into_any_element();
        }

        v_flex()
            .gap_3()
            .p_4()
            .child(
                div()
                    .px_3()
                    .py_2()
                    .rounded_md()
                    .bg(warning.opacity(0.12))
                    .text_color(warning)
                    .text_sm()
                    .child("⚠ GitHub CLI (gh) is required"),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(muted_foreground)
                    .child("Helm runs on top of the GitHub CLI."),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(muted_foreground)
                    .child("Install from: https://cli.github.com"),
            )
            .child(
                Button::new("gate-retry")
                    .outline()
                    .label("Retry")
                    .on_click(cx.listener(|this, _, _, cx| this.check_cli(cx))),
            )
            .into_any_element()
    }

    /// Login / scope-authorization screen.
    fn render_auth(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let border = cx.theme().border;
        let muted_foreground = cx.theme().muted_foreground;
        let foreground = cx.theme().foreground;
        let warning = cx.theme().warning;
        let danger = cx.theme().danger;

        // Initial `do_auth()` hasn't resolved yet — never flash the login
        // button.
        if !self.auth_initialized && !self.login_started {
            return v_flex()
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
                                .child("Checking GitHub login…"),
                        ),
                )
                .into_any_element();
        }

        // Missing 'repo' scope.
        if self.error_msg.contains("repo") && !self.login_started {
            let loading = self.load_state == LoadState::Loading;
            return v_flex()
                .gap_3()
                .p_4()
                .child(
                    div()
                        .px_3()
                        .py_2()
                        .rounded_md()
                        .bg(warning.opacity(0.12))
                        .text_color(warning)
                        .text_sm()
                        .child("⚠ Missing 'repo' scope"),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(muted_foreground)
                        .child("Helm needs the repo scope to manage repositories."),
                )
                .child(
                    Button::new("auth-ensure-repo-scope")
                        .primary()
                        .label("Authorize repo scope")
                        .disabled(loading)
                        .on_click(cx.listener(|this, _, _, cx| this.handle_ensure_repo_scope(cx))),
                )
                .into_any_element();
        }

        // Login in progress — device code + URL.
        if self.login_started && self.load_state == LoadState::Loading {
            let mut col = v_flex()
                .gap_3()
                .p_4()
                .child(div().text_color(foreground).child("Connect to GitHub"))
                .child(div().h_px().w_full().bg(border));

            col = if !self.device_code.is_empty() {
                let copied = self.code_copied;
                col.child(
                    v_flex()
                        .gap_1()
                        .child(
                            div()
                                .text_xs()
                                .text_color(muted_foreground)
                                .child("Your one-time code"),
                        )
                        .child(
                            h_flex()
                                .items_center()
                                .gap_2()
                                .child(
                                    div()
                                        .font_family("Cascadia Mono")
                                        .text_lg()
                                        .text_color(foreground)
                                        .child(self.device_code.clone()),
                                )
                                .child(
                                    Button::new("auth-copy-code")
                                        .ghost()
                                        .xsmall()
                                        .icon(if copied {
                                            IconName::Check
                                        } else {
                                            IconName::Copy
                                        })
                                        .tooltip(if copied { "Copied" } else { "Copy code" })
                                        .on_click(
                                            cx.listener(|this, _, _, cx| this.handle_copy_code(cx)),
                                        ),
                                ),
                        ),
                )
            } else {
                col.child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(Spinner::new().small())
                        .child(
                            div()
                                .text_sm()
                                .text_color(muted_foreground)
                                .child("Starting device flow…"),
                        ),
                )
            };

            if !self.device_url.is_empty() {
                let url = self.device_url.clone();
                col = col.child(
                    Button::new("auth-open-device-url")
                        .outline()
                        .icon(IconName::Github)
                        .label("Open")
                        .child(Icon::new(IconName::ExternalLink).xsmall())
                        .tooltip(url.clone())
                        .on_click(move |_, _, cx| {
                            cx.open_url(&url);
                        }),
                );
            }

            col = col.child(div().text_sm().text_color(muted_foreground).child(
                if self.device_url.is_empty() {
                    "Waiting for GitHub…"
                } else {
                    "Open the link, enter the code, then return here."
                },
            ));

            return col.into_any_element();
        }

        // Login failed.
        if self.login_started && self.load_state == LoadState::Error {
            return v_flex()
                .gap_3()
                .p_4()
                .child(
                    div()
                        .px_3()
                        .py_2()
                        .rounded_md()
                        .bg(danger.opacity(0.12))
                        .text_color(danger)
                        .text_sm()
                        .child(format!("✗ {}", self.error_msg)),
                )
                .child(
                    Button::new("auth-retry")
                        .outline()
                        .label("Try again")
                        .on_click(cx.listener(|this, _, _, cx| this.handle_login(cx))),
                )
                .into_any_element();
        }

        // Not logged in — show the login button.
        v_flex()
            .gap_3()
            .p_4()
            .child(div().text_color(foreground).child("GitHub"))
            .child(div().h_px().w_full().bg(border))
            .child(
                div()
                    .text_sm()
                    .text_color(muted_foreground)
                    .child("Connect your GitHub account to manage repositories and organizations."),
            )
            .child(
                Button::new("helm-login")
                    .primary()
                    .icon(IconName::Github)
                    .label("Login with GitHub")
                    .on_click(cx.listener(|this, _, _, cx| this.handle_login(cx))),
            )
            .into_any_element()
    }

    /// Main menu — mirrors the old `MenuScreen`. Every row is wired to a real
    /// destination (Profile/OrgList/RepoList/Create Repository/Logout);
    /// Organizations only shows once real org data confirms the account
    /// actually belongs to any.
    fn render_menu(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let items: Vec<MenuItem> = [
            MenuItem {
                id: "profile",
                label: "View Profile",
                danger: false,
            },
            MenuItem {
                id: "orgs",
                label: "Organizations",
                danger: false,
            },
            MenuItem {
                id: "repos",
                label: "Repositories",
                danger: false,
            },
            MenuItem {
                id: "create",
                label: "Create Repository",
                danger: false,
            },
            MenuItem {
                id: "logout",
                label: "Logout",
                danger: true,
            },
        ]
        .into_iter()
        .filter(|item| item.id != "orgs" || !self.org_logins.is_empty())
        .collect();

        let foreground = cx.theme().foreground;
        let danger_color = cx.theme().danger;
        let muted_foreground = cx.theme().muted_foreground;

        v_flex()
            .child(v_flex().py_1().children(items.into_iter().map(|item| {
                let id = item.id;
                let label_color = if item.danger {
                    danger_color
                } else {
                    foreground
                };

                ListItem::new(id)
                    .child(div().text_color(label_color).child(item.label))
                    .suffix(move |_, _| {
                        Icon::new(IconName::ChevronRight)
                            .xsmall()
                            .text_color(muted_foreground)
                    })
                    .on_click(cx.listener(move |this, _, window, cx| match id {
                        "logout" => this.handle_logout(cx),
                        "profile" => this.navigate_to(HelmScreen::Profile, cx),
                        "orgs" => this.navigate_to(HelmScreen::OrgList, cx),
                        "repos" => this.open_repo_list(cx),
                        "create" => this.open_create_repo_dialog(window, cx),
                        _ => {}
                    }))
            })))
            .into_any_element()
    }

    /// Opens the "Create repository" dialog — targets the authenticated
    /// user's own account by default; typing an org login in the
    /// "Organization" field creates it org-scoped instead (`gh_create_repo`
    /// reroutes the `"owner"` key to `POST /orgs/{org}/repos`).
    fn open_create_repo_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_workspace_modal(HelmModalKind::CreateRepo, window, cx);
        return;

        /*
        let name = cx.new(|cx| InputState::new(window, cx).placeholder("my-new-repo"));
        let description =
            cx.new(|cx| InputState::new(window, cx).placeholder("Description (optional)"));
        let org = cx.new(|cx| {
            InputState::new(window, cx).placeholder("Organization (blank = your account)")
        });
        let private = Rc::new(Cell::new(true));
        let view = cx.entity();

        window.open_dialog(cx, move |dialog, _, _| {
            let name = name.clone();
            let description = description.clone();
            let org = org.clone();
            let private = private.clone();
            let view = view.clone();

            dialog
                .title("Create repository")
                .child(
                    v_flex()
                        .gap_3()
                        .child(Input::new(&name))
                        .child(Input::new(&description))
                        .child(Input::new(&org))
                        .child(
                            Switch::new("create-repo-private")
                                .label("Private")
                                .checked(private.get())
                                .on_click({
                                    let private = private.clone();
                                    move |checked: &bool, _, _| private.set(*checked)
                                }),
                        ),
                )
                .footer(
                    DialogFooter::new()
                        .child(
                            DialogClose::new()
                                .child(Button::new("cancel").outline().label("Cancel")),
                        )
                        .child(
                            DialogAction::new().child(
                                Button::new("create-repo-confirm").primary().label("Create"),
                            ),
                        ),
                )
                .on_ok(move |_, window, cx| {
                    let name_val = name.read(cx).value().to_string();
                    let description_val = description.read(cx).value().to_string();
                    let private_val = private.get();
                    let org_val = org.read(cx).value().to_string();
                    let org_val = (!org_val.trim().is_empty()).then(|| org_val.trim().to_string());
                    view.update(cx, |this, cx| {
                        this.handle_create_repo(
                            name_val,
                            description_val,
                            private_val,
                            org_val,
                            window,
                            cx,
                        );
                    });
                    true
                })
        });
        */
    }

    /// The org list — mirrors the old `OrgListScreen`.
    fn render_org_list(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let muted_foreground = cx.theme().muted_foreground;
        let foreground = cx.theme().foreground;

        if self.org_logins.is_empty() {
            return v_flex()
                .flex_1()
                .items_center()
                .justify_center()
                .p_4()
                .child(
                    div()
                        .text_sm()
                        .text_color(muted_foreground)
                        .child("No organizations"),
                )
                .into_any_element();
        }

        v_flex()
            .py_1()
            .children(self.org_logins.clone().into_iter().map(|org| {
                let click_org = org.clone();
                ListItem::new(format!("helm-org-{org}"))
                    .child(div().text_color(foreground).child(org))
                    .suffix(move |_, _| {
                        Icon::new(IconName::ChevronRight)
                            .xsmall()
                            .text_color(muted_foreground)
                    })
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.select_org(click_org.clone(), cx)),
                    )
            }))
            .into_any_element()
    }

    /// The selected org's detail — mirrors the old `OrgDetailScreen`.
    /// "Repositories" shows the org's real repo count and routes into
    /// `RepoList` via [`Self::open_repo_list`].
    fn render_org_detail(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let muted_foreground = cx.theme().muted_foreground;
        let foreground = cx.theme().foreground;
        let border = cx.theme().border;

        if self.load_state == LoadState::Loading {
            return v_flex()
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
                                .child("Loading organization…"),
                        ),
                )
                .into_any_element();
        }

        let Some(org) = self.org_detail.clone() else {
            return v_flex()
                .gap_3()
                .p_4()
                .child(
                    div()
                        .text_sm()
                        .text_color(muted_foreground)
                        .child("Failed to load"),
                )
                .child(
                    Button::new("org-retry")
                        .outline()
                        .label("Retry")
                        .on_click(cx.listener(|this, _, _, cx| {
                            if let Some(org) = this.selected_org.clone() {
                                this.load_org(org, cx);
                            }
                        })),
                )
                .into_any_element();
        };

        let total_repos = org.public_repos + org.total_private_repos.unwrap_or(0);

        let stats_row = h_flex()
            .items_center()
            .gap_4()
            .px_3()
            .py_2()
            .text_sm()
            .text_color(muted_foreground)
            .child(format!("Repos {}", fmt_num(total_repos)))
            .child(format!("Followers {}", fmt_num(org.followers)));

        let info_row = |icon: IconName, value: String| {
            h_flex()
                .items_center()
                .gap_2()
                .px_3()
                .py_1()
                .text_sm()
                .child(Icon::new(icon).xsmall().text_color(muted_foreground))
                .child(div().flex_1().min_w_0().text_color(foreground).child(value))
        };

        let mut info_rows = v_flex();
        if let Some(location) = org.location.clone() {
            info_rows = info_rows.child(info_row(IconName::Globe, location));
        }
        if let Some(email) = org.email.clone() {
            info_rows = info_rows.child(info_row(IconName::Inbox, email));
        }
        if let Some(blog) = org.blog.clone() {
            let url = blog.clone();
            info_rows = info_rows.child(
                info_row(IconName::ExternalLink, blog)
                    .id("helm-org-blog")
                    .cursor_pointer()
                    .on_click(move |_, _, cx| cx.open_url(&url)),
            );
        }
        if let Some(twitter) = org.twitter_username.clone() {
            info_rows = info_rows.child(
                info_row(IconName::ExternalLink, format!("@{twitter}"))
                    .id("helm-org-twitter")
                    .cursor_pointer()
                    .on_click(move |_, _, cx| {
                        cx.open_url(&format!("https://x.com/{twitter}"));
                    }),
            );
        }

        v_flex()
            .child(stats_row)
            .child(div().h_px().w_full().bg(border))
            .child(info_rows)
            .child(div().h_px().w_full().bg(border))
            .child(
                ListItem::new("helm-org-repositories")
                    .child(div().text_color(foreground).child("Repositories"))
                    .suffix(move |_, _| {
                        h_flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .text_color(muted_foreground)
                                    .child(total_repos.to_string()),
                            )
                            .child(
                                Icon::new(IconName::ChevronRight)
                                    .xsmall()
                                    .text_color(muted_foreground),
                            )
                    })
                    .on_click(cx.listener(|this, _, _, cx| this.open_repo_list(cx))),
            )
            .into_any_element()
    }

    /// The repo list — mirrors the old `RepoListScreen`. Loads either the
    /// selected org's repos or the user's own, filterable by name. Rows
    /// route into `RepoDetail` via [`Self::select_repo`].
    fn render_repo_list(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let muted_foreground = cx.theme().muted_foreground;
        let foreground = cx.theme().foreground;
        let border = cx.theme().border;

        if self.load_state == LoadState::Loading {
            return v_flex()
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
                                .child("Loading repositories…"),
                        ),
                )
                .into_any_element();
        }

        if self.load_state == LoadState::Error {
            let owner = self
                .selected_org
                .clone()
                .unwrap_or_else(|| "self".to_string());
            return v_flex()
                .gap_3()
                .p_4()
                .child(
                    div()
                        .text_sm()
                        .text_color(muted_foreground)
                        .child("Failed to load repositories"),
                )
                .child(
                    Button::new("repo-list-retry")
                        .outline()
                        .label("Retry")
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.load_repos(owner.clone(), cx);
                        })),
                )
                .into_any_element();
        }

        if self.repos.is_empty() {
            return v_flex()
                .flex_1()
                .items_center()
                .justify_center()
                .p_4()
                .child(
                    div()
                        .text_sm()
                        .text_color(muted_foreground)
                        .child("No repositories found"),
                )
                .into_any_element();
        }

        let query = self.repo_search.read(cx).value().trim().to_lowercase();
        let filtered: Vec<Repo> = if query.is_empty() {
            self.repos.clone()
        } else {
            self.repos
                .iter()
                .filter(|r| r.name.to_lowercase().contains(&query))
                .cloned()
                .collect()
        };

        let search_row = div().px_3().py_2().child(Input::new(&self.repo_search));

        let list = if filtered.is_empty() {
            v_flex()
                .flex_1()
                .items_center()
                .justify_center()
                .p_4()
                .child(
                    div()
                        .text_sm()
                        .text_color(muted_foreground)
                        .child("No repositories match your search"),
                )
                .into_any_element()
        } else {
            v_flex()
                .py_1()
                .children(filtered.into_iter().map(|repo| {
                    let vis = repo_vis_label(&repo);
                    let click_repo = repo.clone();
                    ListItem::new(format!("helm-repo-{}", repo.id))
                        .child(div().text_color(foreground).child(repo.name.clone()))
                        .suffix(move |_, _| {
                            h_flex()
                                .items_center()
                                .gap_2()
                                .child(div().text_color(muted_foreground).child(vis))
                                .child(
                                    Icon::new(IconName::ChevronRight)
                                        .xsmall()
                                        .text_color(muted_foreground),
                                )
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.select_repo(click_repo.clone(), cx)
                        }))
                }))
                .into_any_element()
        };

        v_flex()
            .child(search_row)
            .child(div().h_px().w_full().bg(border))
            .child(list)
            .into_any_element()
    }

    /// Repo detail: name/visibility, description, homepage, topics, clone
    /// URL + a Clone action that streams `gh repo clone` output and, on
    /// success, sets the clone as the new workspace root. The old app's
    /// inline-editable description/homepage is now the Settings/Edit dialog,
    /// there's no feature-toggle port, and Branches/Collaborators drill into
    /// real screens — so the read side plus Clone (with an optional custom
    /// target folder via [`Self::pick_clone_dir`]) is fully covered.
    fn render_repo_detail(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let muted_foreground = cx.theme().muted_foreground;
        let foreground = cx.theme().foreground;
        let border = cx.theme().border;

        let Some(repo) = self.selected_repo.clone() else {
            return v_flex()
                .flex_1()
                .items_center()
                .justify_center()
                .p_4()
                .child(
                    div()
                        .text_sm()
                        .text_color(muted_foreground)
                        .child("No repo selected"),
                )
                .into_any_element();
        };

        let header_row = h_flex()
            .items_center()
            .justify_between()
            .gap_2()
            .px_3()
            .py_2()
            .child(
                h_flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .font_semibold()
                            .text_color(foreground)
                            .child(repo.name.clone()),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(muted_foreground)
                            .child(repo_vis_label(&repo)),
                    ),
            )
            .child(
                Button::new("helm-repo-edit")
                    .ghost()
                    .xsmall()
                    .icon(IconName::Settings)
                    .tooltip("Edit repository")
                    .on_click(
                        cx.listener(|this, _, window, cx| this.open_edit_repo_dialog(window, cx)),
                    ),
            );

        let topics_row = (!repo.topics.is_empty()).then(|| {
            h_flex()
                .flex_wrap()
                .gap_1()
                .px_3()
                .py_2()
                .children(repo.topics.iter().map(|t| {
                    div()
                        .px_2()
                        .py_0p5()
                        .rounded_full()
                        .text_xs()
                        .bg(cx.theme().muted)
                        .text_color(muted_foreground)
                        .child(t.clone())
                }))
        });

        let info_row = |label: &'static str, value: String| {
            v_flex()
                .gap_0p5()
                .px_3()
                .py_2()
                .child(div().text_xs().text_color(muted_foreground).child(label))
                .child(div().text_sm().text_color(foreground).child(value))
        };

        let description_row = info_row(
            "Description",
            repo.description.clone().unwrap_or_else(|| "—".to_string()),
        );
        let homepage_row = info_row(
            "Homepage",
            repo.homepage.clone().unwrap_or_else(|| "—".to_string()),
        );

        let copied = self.clone_url_copied;
        let clone_url_row = h_flex()
            .items_center()
            .justify_between()
            .gap_2()
            .px_3()
            .py_2()
            .child(
                v_flex()
                    .gap_0p5()
                    .min_w_0()
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted_foreground)
                            .child("Clone URL"),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(foreground)
                            .child(repo.clone_url.clone()),
                    ),
            )
            .child(
                Button::new("helm-repo-copy-clone-url")
                    .ghost()
                    .xsmall()
                    .icon(if copied {
                        IconName::Check
                    } else {
                        IconName::Copy
                    })
                    .tooltip(if copied { "Copied" } else { "Copy clone URL" })
                    .on_click(cx.listener(|this, _, _, cx| this.handle_copy_clone_url(cx))),
            );

        let workspace_root = self
            .workspace
            .update(cx, |workspace, cx| {
                workspace
                    .worktrees(cx)
                    .next()
                    .map(|worktree| worktree.read(cx).abs_path().to_path_buf())
            })
            .ok()
            .flatten();

        let clone_section = if workspace_root.is_some() {
            v_flex()
                .gap_2()
                .px_3()
                .py_2()
                .child(
                    Button::new("helm-repo-clone")
                        .primary()
                        .icon(IconName::Github)
                        .label("Clone repository")
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.open_clone_modal(window, cx)
                        })),
                )
                .into_any_element()
        } else {
            v_flex()
                .gap_1()
                .px_3()
                .py_2()
                .child(
                    div()
                        .text_sm()
                        .text_color(muted_foreground)
                        .child("Open a folder first (File → Open Folder) to clone into it."),
                )
                .child(
                    Button::new("helm-repo-clone")
                        .primary()
                        .icon(IconName::Github)
                        .label("Clone repository")
                        .disabled(true),
                )
                .into_any_element()
        };

        let nav_row = |id: &'static str, icon: IconName, label: &'static str| {
            ListItem::new(id)
                .child(
                    h_flex()
                        .items_center()
                        .gap_2()
                        .child(Icon::new(icon).xsmall().text_color(muted_foreground))
                        .child(div().text_color(foreground).child(label)),
                )
                .suffix(move |_, _| {
                    Icon::new(IconName::ChevronRight)
                        .xsmall()
                        .text_color(muted_foreground)
                })
        };

        v_flex()
            .child(header_row)
            .child(div().h_px().w_full().bg(border))
            .children(topics_row)
            .child(description_row)
            .child(homepage_row)
            .child(div().h_px().w_full().bg(border))
            .child(clone_url_row)
            .child(clone_section)
            .child(div().h_px().w_full().bg(border))
            .child(
                nav_row("helm-repo-branches", IconName::Git, "Branches").on_click(cx.listener(
                    |this, _, _, cx| {
                        this.navigate_to(HelmScreen::Branches, cx);
                        this.load_branches(cx);
                    },
                )),
            )
            .child(
                nav_row("helm-repo-collaborators", IconName::User, "Collaborators").on_click(
                    cx.listener(|this, _, _, cx| {
                        this.navigate_to(HelmScreen::Collaborators, cx);
                        this.load_collaborators(cx);
                    }),
                ),
            )
            .child(
                nav_row("helm-repo-issues", IconName::Inbox, "Issues").on_click(cx.listener(
                    |this, _, _, cx| {
                        this.navigate_to(HelmScreen::Issues, cx);
                        this.load_issues(cx);
                    },
                )),
            )
            .child(
                nav_row("helm-repo-pulls", IconName::Redo, "Pull requests").on_click(cx.listener(
                    |this, _, _, cx| {
                        this.navigate_to(HelmScreen::Pulls, cx);
                        this.load_pulls(cx);
                    },
                )),
            )
            .child(
                nav_row(
                    "helm-repo-releases",
                    IconName::GalleryVerticalEnd,
                    "Releases",
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.navigate_to(HelmScreen::Releases, cx);
                    this.load_releases(cx);
                })),
            )
            .child(
                nav_row("helm-repo-packages", IconName::HardDrive, "Packages").on_click(
                    cx.listener(|this, _, _, cx| {
                        this.navigate_to(HelmScreen::Packages, cx);
                        this.load_packages(cx);
                    }),
                ),
            )
            .child(
                nav_row("helm-repo-traffic", IconName::ChartPie, "Traffic").on_click(cx.listener(
                    |this, _, _, cx| {
                        this.navigate_to(HelmScreen::Traffic, cx);
                        this.load_traffic(cx);
                    },
                )),
            )
            .child(
                nav_row("helm-repo-commits", IconName::Git, "Commits").on_click(cx.listener(
                    |this, _, _, cx| {
                        this.navigate_to(HelmScreen::Commits, cx);
                        this.load_commits(cx);
                    },
                )),
            )
            .child(
                nav_row("helm-repo-actions", IconName::SquareTerminal, "Actions").on_click(
                    cx.listener(|this, _, _, cx| {
                        this.navigate_to(HelmScreen::WorkflowRuns, cx);
                        this.load_workflow_runs(cx);
                    }),
                ),
            )
            .child(
                nav_row("helm-repo-deployments", IconName::Globe, "Deployments").on_click(
                    cx.listener(|this, _, _, cx| {
                        this.navigate_to(HelmScreen::Deployments, cx);
                        this.load_deployments(cx);
                    }),
                ),
            )
            .child(
                nav_row("helm-repo-tags", IconName::Asterisk, "Tags").on_click(cx.listener(
                    |this, _, _, cx| {
                        this.navigate_to(HelmScreen::Tags, cx);
                        this.load_tags(cx);
                    },
                )),
            )
            .child(
                nav_row("helm-repo-security", IconName::TriangleAlert, "Security").on_click(
                    cx.listener(|this, _, _, cx| {
                        this.navigate_to(HelmScreen::Security, cx);
                        this.load_security(cx);
                    }),
                ),
            )
            .into_any_element()
    }

    /// Opens the "Edit repository" dialog, prefilled from `self.selected_repo`.
    fn open_edit_repo_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(repo) = self.selected_repo.clone() else {
            return;
        };

        self.open_workspace_modal(HelmModalKind::EditRepo(repo), window, cx);
        return;

        /*
        let name = cx.new(|cx| InputState::new(window, cx).default_value(repo.name.clone()));
        let description = cx.new(|cx| {
            InputState::new(window, cx).default_value(repo.description.clone().unwrap_or_default())
        });
        let homepage = cx.new(|cx| {
            InputState::new(window, cx).default_value(repo.homepage.clone().unwrap_or_default())
        });
        let topics = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(repo.topics.join(", "))
                .placeholder("comma, separated, topics")
        });
        let has_issues = Rc::new(Cell::new(repo.has_issues));
        let has_wiki = Rc::new(Cell::new(repo.has_wiki));
        let has_projects = Rc::new(Cell::new(repo.has_projects));
        let has_discussions = Rc::new(Cell::new(repo.has_discussions));
        let view = cx.entity();

        window.open_dialog(cx, move |dialog, _, _| {
            let name = name.clone();
            let description = description.clone();
            let homepage = homepage.clone();
            let topics = topics.clone();
            let has_issues = has_issues.clone();
            let has_wiki = has_wiki.clone();
            let has_projects = has_projects.clone();
            let has_discussions = has_discussions.clone();
            let view = view.clone();

            dialog
                .title("Edit repository")
                .child(
                    v_flex()
                        .gap_3()
                        .child(Input::new(&name))
                        .child(Input::new(&description))
                        .child(Input::new(&homepage))
                        .child(Input::new(&topics))
                        .child(
                            Switch::new("edit-repo-has-issues")
                                .label("Issues")
                                .checked(has_issues.get())
                                .on_click({
                                    let has_issues = has_issues.clone();
                                    move |checked: &bool, _, _| has_issues.set(*checked)
                                }),
                        )
                        .child(
                            Switch::new("edit-repo-has-projects")
                                .label("Projects")
                                .checked(has_projects.get())
                                .on_click({
                                    let has_projects = has_projects.clone();
                                    move |checked: &bool, _, _| has_projects.set(*checked)
                                }),
                        )
                        .child(
                            Switch::new("edit-repo-has-wiki")
                                .label("Wiki")
                                .checked(has_wiki.get())
                                .on_click({
                                    let has_wiki = has_wiki.clone();
                                    move |checked: &bool, _, _| has_wiki.set(*checked)
                                }),
                        )
                        .child(
                            Switch::new("edit-repo-has-discussions")
                                .label("Discussions")
                                .checked(has_discussions.get())
                                .on_click({
                                    let has_discussions = has_discussions.clone();
                                    move |checked: &bool, _, _| has_discussions.set(*checked)
                                }),
                        ),
                )
                .footer(
                    DialogFooter::new()
                        .child(
                            DialogClose::new()
                                .child(Button::new("cancel").outline().label("Cancel")),
                        )
                        .child(
                            DialogAction::new()
                                .child(Button::new("edit-repo-confirm").primary().label("Save")),
                        ),
                )
                .on_ok(move |_, window, cx| {
                    let name_val = name.read(cx).value().to_string();
                    let description_val = description.read(cx).value().to_string();
                    let homepage_val = homepage.read(cx).value().to_string();
                    let topics_val: Vec<String> = topics
                        .read(cx)
                        .value()
                        .split(',')
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect();
                    let has_issues_val = has_issues.get();
                    let has_wiki_val = has_wiki.get();
                    let has_projects_val = has_projects.get();
                    let has_discussions_val = has_discussions.get();
                    view.update(cx, |this, cx| {
                        this.handle_edit_repo(
                            name_val,
                            description_val,
                            homepage_val,
                            topics_val,
                            has_issues_val,
                            has_wiki_val,
                            has_projects_val,
                            has_discussions_val,
                            window,
                            cx,
                        );
                    });
                    true
                })
        });
        */
    }

    fn open_workspace_modal(
        &mut self,
        kind: HelmModalKind,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(workspace) = self.workspace.upgrade() else {
            // No workspace to open a modal *in*, so there's nowhere to show
            // a toast either — this is a "should never happen" guard, not a
            // user-facing error state.
            log::warn!("helm_panel: open_workspace_modal called with no attached workspace");
            return;
        };
        let parent = cx.entity();
        workspace.update(cx, |workspace, cx| {
            workspace.toggle_modal(window, cx, move |window, cx| {
                HelmRepositoryModal::new(kind, parent, window, cx)
            });
        });
    }

    /// Shows `message` as a toast on the workspace this panel is attached
    /// to. `gpui_component`'s own `window.push_notification` (this crate's
    /// earlier approach) requires the window to be wrapped in a
    /// `gpui_component::Root`, which the main Zed workspace window never is
    /// — calling it here panics with "window first layer should be a
    /// gpui_component::Root". `workspace::Toast` is the mechanism Zed's own
    /// windows actually support.
    fn notify(&self, message: impl Into<std::borrow::Cow<'static, str>>, cx: &mut App) {
        let message = message.into();
        self.workspace
            .update(cx, |workspace, cx| {
                workspace.show_toast(Toast::new(NotificationId::unique::<HelmPanel>(), message), cx);
            })
            .ok();
    }

    /// The branches list — read-only, mirrors `render_repo_list`'s
    /// loading/error/empty states.
    fn render_branches(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let muted_foreground = cx.theme().muted_foreground;
        let foreground = cx.theme().foreground;

        if self.load_state == LoadState::Loading {
            return v_flex()
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
                                .child("Loading branches…"),
                        ),
                )
                .into_any_element();
        }

        if self.load_state == LoadState::Error {
            return v_flex()
                .gap_3()
                .p_4()
                .child(
                    div()
                        .text_sm()
                        .text_color(muted_foreground)
                        .child("Failed to load branches"),
                )
                .child(
                    Button::new("branches-retry")
                        .outline()
                        .label("Retry")
                        .on_click(cx.listener(|this, _, _, cx| this.load_branches(cx))),
                )
                .into_any_element();
        }

        if self.branches.is_empty() {
            return v_flex()
                .flex_1()
                .items_center()
                .justify_center()
                .p_4()
                .child(
                    div()
                        .text_sm()
                        .text_color(muted_foreground)
                        .child("No branches found"),
                )
                .into_any_element();
        }

        let default_branch = self
            .selected_repo
            .as_ref()
            .map(|r| r.default_branch.clone())
            .unwrap_or_default();

        v_flex()
            .py_1()
            .children(self.branches.iter().map(|branch| {
                let is_default = branch.name == default_branch;
                let protected = branch.protected;
                ListItem::new(format!("helm-branch-{}", branch.name))
                    .child(div().text_color(foreground).child(branch.name.clone()))
                    .suffix(move |_, _| {
                        h_flex()
                            .items_center()
                            .gap_2()
                            .when(is_default, |row| {
                                row.child(
                                    div()
                                        .px_2()
                                        .py_0p5()
                                        .rounded_full()
                                        .text_xs()
                                        .bg(muted_foreground.opacity(0.15))
                                        .text_color(muted_foreground)
                                        .child("default"),
                                )
                            })
                            .when(protected, |row| {
                                row.child(
                                    div()
                                        .text_xs()
                                        .text_color(muted_foreground)
                                        .child("protected"),
                                )
                            })
                    })
            }))
            .into_any_element()
    }

    /// The collaborators list — add/remove and per-row permission changes.
    fn render_collaborators(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let muted_foreground = cx.theme().muted_foreground;
        let foreground = cx.theme().foreground;
        let border = cx.theme().border;

        let header = h_flex()
            .items_center()
            .justify_between()
            .px_3()
            .py_2()
            .child(
                div()
                    .text_sm()
                    .font_semibold()
                    .text_color(foreground)
                    .child("Collaborators"),
            )
            .child(
                Button::new("collaborators-add")
                    .ghost()
                    .xsmall()
                    .icon(IconName::Plus)
                    .label("Add collaborator")
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_add_collaborator_dialog(window, cx)
                    })),
            );

        if self.load_state == LoadState::Loading {
            return v_flex()
                .child(header)
                .child(div().h_px().w_full().bg(border))
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
                                        .child("Loading collaborators…"),
                                ),
                        ),
                )
                .into_any_element();
        }

        if self.load_state == LoadState::Error {
            return v_flex()
                .child(header)
                .child(div().h_px().w_full().bg(border))
                .child(
                    v_flex()
                        .gap_3()
                        .p_4()
                        .child(
                            div()
                                .text_sm()
                                .text_color(muted_foreground)
                                .child("Failed to load collaborators"),
                        )
                        .child(
                            Button::new("collaborators-retry")
                                .outline()
                                .label("Retry")
                                .on_click(
                                    cx.listener(|this, _, _, cx| this.load_collaborators(cx)),
                                ),
                        ),
                )
                .into_any_element();
        }

        if self.collaborators.is_empty() {
            return v_flex()
                .child(header)
                .child(div().h_px().w_full().bg(border))
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
                                .child("No collaborators"),
                        ),
                )
                .into_any_element();
        }

        let view = cx.entity();

        v_flex()
            .child(header)
            .child(div().h_px().w_full().bg(border))
            .child(v_flex().py_1().children(self.collaborators.iter().map(|collab| {
                let login = collab.login.clone();
                let role_name = collab.role_name.clone();

                ListItem::new(format!("helm-collaborator-{}", collab.id))
                    .child(
                        h_flex()
                            .items_center()
                            .gap_2()
                            .child(
                                Avatar::new()
                                    .src(collab.avatar_url.clone())
                                    .name(login.clone())
                                    .with_size(px(24.)),
                            )
                            .child(div().text_color(foreground).child(login.clone())),
                    )
                    .suffix({
                        let view = view.clone();
                        let role_login = login.clone();
                        let remove_login = login.clone();
                        let collab_id = collab.id;
                        move |_, _| {
                            let role_view = view.clone();
                            let role_login = role_login.clone();
                            let remove_view = view.clone();
                            let remove_login = remove_login.clone();

                            h_flex()
                                .items_center()
                                .gap_2()
                                .child(
                                    Button::new(("helm-collaborator-role", collab_id))
                                        .ghost()
                                        .xsmall()
                                        .label(role_name.clone())
                                        .dropdown_menu(move |menu, _, _| {
                                            COLLABORATOR_PERMISSIONS.iter().fold(menu, |menu, perm| {
                                                let role_view = role_view.clone();
                                                let role_login = role_login.clone();
                                                let perm = perm.to_string();
                                                menu.item(PopupMenuItem::new(perm.clone()).on_click(
                                                    move |_, window, cx| {
                                                        role_view.update(cx, |this, cx| {
                                                            this.handle_set_collaborator_permission(
                                                                role_login.clone(),
                                                                perm.clone(),
                                                                window,
                                                                cx,
                                                            );
                                                        });
                                                    },
                                                ))
                                            })
                                        }),
                                )
                                .child(
                                    Button::new(("helm-collaborator-remove", collab_id))
                                        .ghost()
                                        .xsmall()
                                        .icon(IconName::Delete)
                                        .tooltip("Remove collaborator")
                                        .on_click(move |_, window, cx| {
                                            remove_view.update(cx, |this, cx| {
                                                this.open_remove_collaborator_confirm(
                                                    remove_login.clone(),
                                                    window,
                                                    cx,
                                                );
                                            });
                                        }),
                                )
                        }
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.open_user_profile(login.clone(), cx);
                    }))
            })))
            .into_any_element()
    }

    /// Small Open/Closed/All toggle for the Issues and Pulls screens — the
    /// active state is a primary button, the others ghost.
    fn state_filter_button(
        &self,
        id: &'static str,
        label: &'static str,
        filter: &'static str,
        active: bool,
        screen: HelmScreen,
        cx: &mut Context<Self>,
    ) -> Button {
        Button::new(id)
            .small()
            .when(active, |b| b.primary())
            .when(!active, |b| b.ghost())
            .label(label)
            .on_click(cx.listener(move |this, _, _, cx| {
                this.set_tab_filter(screen, filter, cx);
            }))
    }

    /// Applies a filter to whichever of Issues/Pulls owns it and reloads the
    /// list.
    fn set_tab_filter(&mut self, screen: HelmScreen, filter: &str, cx: &mut Context<Self>) {
        match screen {
            HelmScreen::Issues => {
                self.issues_filter = filter.to_string();
                self.load_issues(cx);
            }
            HelmScreen::Pulls => {
                self.pulls_filter = filter.to_string();
                self.load_pulls(cx);
            }
            _ => {}
        }
    }

    /// The Issues tab — filterable list of `self.selected_repo`'s issues
    /// (PRs filtered out), each row opening its page in the browser.
    fn render_issues(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let muted_foreground = cx.theme().muted_foreground;
        let foreground = cx.theme().foreground;
        let success = cx.theme().success;
        let danger = cx.theme().danger;

        let filter_row = h_flex()
            .items_center()
            .gap_1()
            .px_3()
            .py_2()
            .child(self.state_filter_button(
                "helm-issues-open",
                "Open",
                "open",
                self.issues_filter == "open",
                HelmScreen::Issues,
                cx,
            ))
            .child(self.state_filter_button(
                "helm-issues-closed",
                "Closed",
                "closed",
                self.issues_filter == "closed",
                HelmScreen::Issues,
                cx,
            ))
            .child(self.state_filter_button(
                "helm-issues-all",
                "All",
                "all",
                self.issues_filter == "all",
                HelmScreen::Issues,
                cx,
            ));

        if self.load_state == LoadState::Loading {
            return v_flex()
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
                                .child("Loading issues…"),
                        ),
                )
                .into_any_element();
        }

        if self.load_state == LoadState::Error {
            return v_flex()
                .gap_3()
                .p_4()
                .child(
                    div()
                        .text_sm()
                        .text_color(muted_foreground)
                        .child("Failed to load issues"),
                )
                .child(
                    Button::new("issues-retry")
                        .outline()
                        .label("Retry")
                        .on_click(cx.listener(|this, _, _, cx| this.load_issues(cx))),
                )
                .into_any_element();
        }

        if self.issues.is_empty() {
            return v_flex()
                .flex_1()
                .items_center()
                .justify_center()
                .p_4()
                .child(
                    div()
                        .text_sm()
                        .text_color(muted_foreground)
                        .child("No issues found"),
                )
                .into_any_element();
        }

        v_flex()
            .child(filter_row)
            .child(div().h_px().w_full().bg(cx.theme().border))
            .child(v_flex().py_1().children(self.issues.iter().map(|issue| {
                let number = issue.number;
                let title = issue.title.clone();
                let state = if issue.state == "closed" {
                    "closed"
                } else {
                    "open"
                };
                let author = issue
                    .user
                    .as_ref()
                    .map(|u| u.login.clone())
                    .unwrap_or_default();
                let comments = issue.comments;
                let labels = issue.labels.clone();
                let issue_for_click = issue.clone();
                ListItem::new(format!("helm-issue-{number}"))
                    .child(
                        v_flex()
                            .gap_0p5()
                            .min_w_0()
                            .child(
                                div()
                                    .truncate()
                                    .text_sm()
                                    .font_semibold()
                                    .text_color(if state == "closed" {
                                        muted_foreground
                                    } else {
                                        foreground
                                    })
                                    .child(title),
                            )
                            .child(
                                h_flex()
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(muted_foreground)
                                            .child(format!("#{number} · {author}")),
                                    )
                                    .when(comments > 0, |row| {
                                        row.child(
                                            div()
                                                .text_xs()
                                                .text_color(muted_foreground)
                                                .child(format!("{comments} comments")),
                                        )
                                    }),
                            ),
                    )
                    .suffix(move |_, _| {
                        h_flex()
                            .items_center()
                            .gap_2()
                            .child(div().text_xs().text_color(if state == "closed" {
                                danger
                            } else {
                                success
                            }))
                            .children(labels.iter().map(|label| {
                                div()
                                    .px_1p5()
                                    .rounded_full()
                                    .text_xs()
                                    .bg(muted_foreground.opacity(0.15))
                                    .text_color(muted_foreground)
                                    .child(label.name.clone())
                            }))
                            .child(
                                Icon::new(IconName::ChevronRight)
                                    .xsmall()
                                    .text_color(muted_foreground),
                            )
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.open_issue_detail(issue_for_click.clone(), cx);
                    }))
            })))
            .into_any_element()
    }

    /// The Pull requests tab — filterable list showing head→base branches,
    /// each row opening its page in the browser.
    fn render_pulls(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let muted_foreground = cx.theme().muted_foreground;
        let foreground = cx.theme().foreground;
        let success = cx.theme().success;
        let danger = cx.theme().danger;

        let filter_row = h_flex()
            .items_center()
            .justify_between()
            .gap_1()
            .px_3()
            .py_2()
            .child(
                h_flex()
                    .items_center()
                    .gap_1()
                    .child(self.state_filter_button(
                        "helm-pulls-open",
                        "Open",
                        "open",
                        self.pulls_filter == "open",
                        HelmScreen::Pulls,
                        cx,
                    ))
                    .child(self.state_filter_button(
                        "helm-pulls-closed",
                        "Closed",
                        "closed",
                        self.pulls_filter == "closed",
                        HelmScreen::Pulls,
                        cx,
                    ))
                    .child(self.state_filter_button(
                        "helm-pulls-all",
                        "All",
                        "all",
                        self.pulls_filter == "all",
                        HelmScreen::Pulls,
                        cx,
                    )),
            )
            .child(
                Button::new("pulls-create")
                    .ghost()
                    .xsmall()
                    .icon(IconName::Plus)
                    .label("New pull request")
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_create_pull_dialog(window, cx)
                    })),
            );

        if self.load_state == LoadState::Loading {
            return v_flex()
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
                                .child("Loading pull requests…"),
                        ),
                )
                .into_any_element();
        }

        if self.load_state == LoadState::Error {
            return v_flex()
                .gap_3()
                .p_4()
                .child(
                    div()
                        .text_sm()
                        .text_color(muted_foreground)
                        .child("Failed to load pull requests"),
                )
                .child(
                    Button::new("pulls-retry")
                        .outline()
                        .label("Retry")
                        .on_click(cx.listener(|this, _, _, cx| this.load_pulls(cx))),
                )
                .into_any_element();
        }

        if self.pulls.is_empty() {
            return v_flex()
                .flex_1()
                .items_center()
                .justify_center()
                .p_4()
                .child(
                    div()
                        .text_sm()
                        .text_color(muted_foreground)
                        .child("No pull requests found"),
                )
                .into_any_element();
        }

        v_flex()
            .child(filter_row)
            .child(div().h_px().w_full().bg(cx.theme().border))
            .child(v_flex().py_1().children(self.pulls.iter().map(|pr| {
                let number = pr.number;
                let title = pr.title.clone();
                let merged = pr.merged;
                let status = if merged {
                    "merged"
                } else if pr.state == "closed" {
                    "closed"
                } else {
                    "open"
                };
                let head_label = if pr.head.label.is_empty() {
                    pr.head.r#ref.clone()
                } else {
                    pr.head.label.clone()
                };
                let base_label = if pr.base.label.is_empty() {
                    pr.base.r#ref.clone()
                } else {
                    pr.base.label.clone()
                };
                let pr_for_click = pr.clone();
                ListItem::new(format!("helm-pr-{number}"))
                    .child(
                        v_flex()
                            .gap_0p5()
                            .min_w_0()
                            .child(
                                div()
                                    .truncate()
                                    .text_sm()
                                    .font_semibold()
                                    .text_color(if status == "open" {
                                        foreground
                                    } else {
                                        muted_foreground
                                    })
                                    .child(title),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(muted_foreground)
                                    .child(format!("#{number}")),
                            ),
                    )
                    .suffix(move |_, _| {
                        h_flex()
                            .items_center()
                            .gap_2()
                            .child(div().text_xs().text_color(if status == "open" {
                                success
                            } else {
                                danger
                            }))
                            .child(
                                div()
                                    .text_xs()
                                    .font_family("Cascadia Mono")
                                    .text_color(muted_foreground)
                                    .child(format!("{head_label} → {base_label}")),
                            )
                            .child(
                                Icon::new(IconName::ChevronRight)
                                    .xsmall()
                                    .text_color(muted_foreground),
                            )
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.open_pr_detail(pr_for_click.clone(), cx);
                    }))
            })))
            .into_any_element()
    }

    /// The open issue's own view: title/number/state/author/labels, full
    /// body (rendered as markdown — already present on `selected_issue`,
    /// since the `/issues` list endpoint returns it, so no extra fetch was
    /// needed just for this), and the comment thread
    /// `load_detail_comments` loaded on entry. Back/forward navigation
    /// comes for free from the shared nav bar (`render`'s `show_nav`).
    fn render_issue_detail(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let muted_foreground = cx.theme().muted_foreground;
        let foreground = cx.theme().foreground;
        let success = cx.theme().success;
        let danger = cx.theme().danger;
        let border = cx.theme().border;

        let Some(issue) = self.selected_issue.clone() else {
            return v_flex()
                .p_4()
                .child(
                    div()
                        .text_sm()
                        .text_color(muted_foreground)
                        .child("No issue selected"),
                )
                .into_any_element();
        };

        let state_open = issue.state != "closed";
        let author = issue
            .user
            .as_ref()
            .map(|u| u.login.clone())
            .unwrap_or_default();
        let url = issue.html_url.clone();

        v_flex()
            .child(
                v_flex()
                    .gap_2()
                    .p_3()
                    .child(
                        h_flex()
                            .items_center()
                            .justify_between()
                            .child(
                                h_flex()
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        div()
                                            .px_1p5()
                                            .py_0p5()
                                            .rounded_full()
                                            .text_xs()
                                            .bg((if state_open { success } else { danger })
                                                .opacity(0.15))
                                            .text_color(if state_open { success } else { danger })
                                            .child(if state_open { "Open" } else { "Closed" }),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(muted_foreground)
                                            .child(format!("#{} opened by {author}", issue.number)),
                                    ),
                            )
                            .child(
                                Button::new("helm-issue-open-browser")
                                    .ghost()
                                    .xsmall()
                                    .icon(IconName::ExternalLink)
                                    .tooltip("Open in Browser")
                                    .on_click(move |_, _, cx| cx.open_url(&url)),
                            ),
                    )
                    .child(
                        div()
                            .text_base()
                            .font_semibold()
                            .text_color(foreground)
                            .child(issue.title.clone()),
                    )
                    .when(!issue.labels.is_empty(), |col| {
                        col.child(
                            h_flex()
                                .flex_wrap()
                                .gap_1()
                                .children(issue.labels.iter().map(|label| {
                                    div()
                                        .px_1p5()
                                        .rounded_full()
                                        .text_xs()
                                        .bg(muted_foreground.opacity(0.15))
                                        .text_color(muted_foreground)
                                        .child(label.name.clone())
                                })),
                        )
                    }),
            )
            .child(div().h_px().w_full().bg(border))
            .child(
                div().p_3().text_sm().child(markdown(
                    issue
                        .body
                        .clone()
                        .filter(|b| !b.trim().is_empty())
                        .unwrap_or_else(|| "_No description provided._".to_string()),
                )),
            )
            .child(div().h_px().w_full().bg(border))
            .child(self.render_comment_thread(cx))
            .into_any_element()
    }

    /// Same as [`Self::render_issue_detail`], for a PR — head→base branch
    /// info and a merged/closed/open status take the place of labels,
    /// everything else (body, comment thread, browser link) is identical.
    fn render_pr_detail(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let muted_foreground = cx.theme().muted_foreground;
        let foreground = cx.theme().foreground;
        let success = cx.theme().success;
        let danger = cx.theme().danger;
        let border = cx.theme().border;

        let Some(pr) = self.selected_pr.clone() else {
            return v_flex()
                .p_4()
                .child(
                    div()
                        .text_sm()
                        .text_color(muted_foreground)
                        .child("No pull request selected"),
                )
                .into_any_element();
        };

        let (status_label, status_color) = if pr.merged {
            ("Merged", success)
        } else if pr.state == "closed" {
            ("Closed", danger)
        } else {
            ("Open", success)
        };
        let author = pr
            .user
            .as_ref()
            .map(|u| u.login.clone())
            .unwrap_or_default();
        let head_label = if pr.head.label.is_empty() {
            pr.head.r#ref.clone()
        } else {
            pr.head.label.clone()
        };
        let base_label = if pr.base.label.is_empty() {
            pr.base.r#ref.clone()
        } else {
            pr.base.label.clone()
        };
        let url = pr.html_url.clone();

        v_flex()
            .child(
                v_flex()
                    .gap_2()
                    .p_3()
                    .child(
                        h_flex()
                            .items_center()
                            .justify_between()
                            .child(
                                h_flex()
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        div()
                                            .px_1p5()
                                            .py_0p5()
                                            .rounded_full()
                                            .text_xs()
                                            .bg(status_color.opacity(0.15))
                                            .text_color(status_color)
                                            .child(status_label),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(muted_foreground)
                                            .child(format!("#{} opened by {author}", pr.number)),
                                    ),
                            )
                            .child(
                                Button::new("helm-pr-open-browser")
                                    .ghost()
                                    .xsmall()
                                    .icon(IconName::ExternalLink)
                                    .tooltip("Open in Browser")
                                    .on_click(move |_, _, cx| cx.open_url(&url)),
                            ),
                    )
                    .child(
                        div()
                            .text_base()
                            .font_semibold()
                            .text_color(foreground)
                            .child(pr.title.clone()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .font_family("Cascadia Mono")
                            .text_color(muted_foreground)
                            .child(format!("{head_label} → {base_label}")),
                    ),
            )
            .child(div().h_px().w_full().bg(border))
            .child(
                div().p_3().text_sm().child(markdown(
                    pr.body
                        .clone()
                        .filter(|b| !b.trim().is_empty())
                        .unwrap_or_else(|| "_No description provided._".to_string()),
                )),
            )
            .child(div().h_px().w_full().bg(border))
            .child(self.render_comment_thread(cx))
            .into_any_element()
    }

    /// The comment thread for whichever issue/PR is open — shared by both
    /// detail views since they're backed by the same `detail_comments`
    /// (see that field's doc comment). Each comment's body renders as
    /// markdown too, same as the issue/PR body above it.
    fn render_comment_thread(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let muted_foreground = cx.theme().muted_foreground;
        let foreground = cx.theme().foreground;
        let border = cx.theme().border;

        if self.detail_comments_state == LoadState::Loading {
            return v_flex()
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
                                .child("Loading comments…"),
                        ),
                )
                .into_any_element();
        }

        if self.detail_comments_state == LoadState::Error {
            return v_flex()
                .gap_3()
                .p_4()
                .child(
                    div()
                        .text_sm()
                        .text_color(muted_foreground)
                        .child("Failed to load comments"),
                )
                .into_any_element();
        }

        if self.detail_comments.is_empty() {
            return v_flex()
                .p_4()
                .child(
                    div()
                        .text_sm()
                        .text_color(muted_foreground)
                        .child("No comments yet"),
                )
                .into_any_element();
        }

        v_flex()
            .gap_3()
            .p_3()
            .children(self.detail_comments.iter().map(|comment| {
                let author = comment
                    .user
                    .as_ref()
                    .map(|u| u.login.clone())
                    .unwrap_or_default();
                v_flex()
                    .gap_1()
                    .pb_3()
                    .border_b_1()
                    .border_color(border)
                    .child(
                        div()
                            .text_xs()
                            .font_semibold()
                            .text_color(foreground)
                            .child(author),
                    )
                    .child(
                        div()
                            .text_sm()
                            .child(markdown(comment.body.clone().unwrap_or_default())),
                    )
            }))
            .into_any_element()
    }

    /// The Releases tab — list of tags with name/body preview, draft and
    /// prerelease badges, and asset download summary; rows open in the
    /// browser.
    fn render_releases(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let muted_foreground = cx.theme().muted_foreground;
        let foreground = cx.theme().foreground;
        let warning = cx.theme().warning;

        let header = h_flex()
            .items_center()
            .justify_between()
            .px_3()
            .py_2()
            .child(
                div()
                    .text_sm()
                    .font_semibold()
                    .text_color(foreground)
                    .child("Releases"),
            )
            .child(
                Button::new("releases-create")
                    .ghost()
                    .xsmall()
                    .icon(IconName::Plus)
                    .label("New release")
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_create_release_dialog(window, cx)
                    })),
            );

        if self.load_state == LoadState::Loading {
            return v_flex()
                .child(header)
                .child(div().h_px().w_full().bg(cx.theme().border))
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
                                        .child("Loading releases…"),
                                ),
                        ),
                )
                .into_any_element();
        }

        if self.load_state == LoadState::Error {
            return v_flex()
                .child(header)
                .child(div().h_px().w_full().bg(cx.theme().border))
                .child(
                    v_flex()
                        .gap_3()
                        .p_4()
                        .child(
                            div()
                                .text_sm()
                                .text_color(muted_foreground)
                                .child("Failed to load releases"),
                        )
                        .child(
                            Button::new("releases-retry")
                                .outline()
                                .label("Retry")
                                .on_click(cx.listener(|this, _, _, cx| this.load_releases(cx))),
                        ),
                )
                .into_any_element();
        }

        if self.releases.is_empty() {
            return v_flex()
                .child(header)
                .child(div().h_px().w_full().bg(cx.theme().border))
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
                                .child("No releases yet"),
                        ),
                )
                .into_any_element();
        }

        v_flex()
            .child(header)
            .child(div().h_px().w_full().bg(cx.theme().border))
            .child(v_flex().py_1().children(self.releases.iter().map(|release| {
                let tag = release.tag_name.clone();
                let title = release
                    .name
                    .clone()
                    .filter(|n| !n.is_empty())
                    .unwrap_or_else(|| tag.clone());
                let draft = release.draft;
                let prerelease = release.prerelease;
                let body_preview = release.body.as_deref().map(|b| {
                    let mut trimmed: String = b.chars().take(120).collect();
                    if b.chars().count() > 120 {
                        trimmed.push('…');
                    }
                    trimmed
                });
                let asset_count = release.assets.len();
                let url = release.html_url.clone();
                ListItem::new(format!("helm-release-{}", release.tag_name))
                    .child(
                        v_flex()
                            .gap_0p5()
                            .min_w_0()
                            .child(
                                h_flex()
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        div()
                                            .text_sm()
                                            .font_semibold()
                                            .text_color(foreground)
                                            .child(title),
                                    )
                                    .when(draft, |row| {
                                        row.child(
                                            div()
                                                .px_1p5()
                                                .py_0p5()
                                                .rounded_full()
                                                .text_xs()
                                                .bg(muted_foreground.opacity(0.15))
                                                .text_color(muted_foreground)
                                                .child("draft"),
                                        )
                                    })
                                    .when(prerelease, |row| {
                                        row.child(
                                            div()
                                                .px_1p5()
                                                .py_0p5()
                                                .rounded_full()
                                                .text_xs()
                                                .bg(warning.opacity(0.15))
                                                .text_color(warning)
                                                .child("pre-release"),
                                        )
                                    }),
                            )
                            .when_some(body_preview, |col, body| {
                                col.child(
                                    div()
                                        .truncate()
                                        .text_xs()
                                        .text_color(muted_foreground)
                                        .child(body),
                                )
                            })
                            .when(asset_count > 0, |col| {
                                col.child(div().text_xs().text_color(muted_foreground).child(
                                    format!(
                                        "{asset_count} asset{}",
                                        if asset_count == 1 { "" } else { "s" }
                                    ),
                                ))
                            }),
                    )
                    .suffix(move |_, _| {
                        Icon::new(IconName::ExternalLink)
                            .xsmall()
                            .text_color(muted_foreground)
                    })
                    .on_click(move |_, _, cx| cx.open_url(&url))
            })))
            .into_any_element()
    }

    /// The Packages tab — owner-scoped package list; tapping a package
    /// expands its versions inline.
    fn render_packages(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let muted_foreground = cx.theme().muted_foreground;
        let foreground = cx.theme().foreground;
        let border = cx.theme().border;

        if self.load_state == LoadState::Loading {
            return v_flex()
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
                                .child("Loading packages…"),
                        ),
                )
                .into_any_element();
        }

        if self.load_state == LoadState::Error {
            return v_flex()
                .gap_3()
                .p_4()
                .child(
                    div()
                        .text_sm()
                        .text_color(muted_foreground)
                        .child("Failed to load packages"),
                )
                .child(
                    Button::new("packages-retry")
                        .outline()
                        .label("Retry")
                        .on_click(cx.listener(|this, _, _, cx| this.load_packages(cx))),
                )
                .into_any_element();
        }

        if self.packages.is_empty() {
            return v_flex()
                .flex_1()
                .items_center()
                .justify_center()
                .p_4()
                .child(
                    div()
                        .text_sm()
                        .text_color(muted_foreground)
                        .child("No packages found"),
                )
                .into_any_element();
        }

        let owner = self
            .selected_repo
            .as_ref()
            .map(|r| r.owner.login.clone())
            .unwrap_or_default();
        let expanded = self.expanded_package.clone();
        let version_error = self.package_versions_error.clone();
        let view = cx.entity();

        v_flex()
            .py_1()
            .children(self.packages.iter().map(|pkg| {
                let pkg_name = pkg.name.clone();
                let is_expanded = expanded.as_deref() == Some(pkg_name.as_str());
                let pkg_type = pkg.package_type.clone();
                let pkg_vis = pkg.visibility.clone();
                let pkg_desc = pkg.description.clone();
                let package_clone = pkg.clone();
                let owner_clone = owner.clone();
                let view = view.clone();

                let row = ListItem::new(format!("helm-package-{pkg_name}"))
                    .child(
                        v_flex()
                            .gap_0p5()
                            .min_w_0()
                            .child(
                                h_flex()
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        div()
                                            .text_sm()
                                            .font_semibold()
                                            .text_color(foreground)
                                            .child(pkg_name.clone()),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(muted_foreground)
                                            .child(pkg_type.clone()),
                                    ),
                            )
                            .when_some(pkg_desc, |col, desc| {
                                col.child(
                                    div()
                                        .truncate()
                                        .text_xs()
                                        .text_color(muted_foreground)
                                        .child(desc),
                                )
                            }),
                    )
                    .suffix({
                        let pkg_vis = pkg_vis.clone();
                        move |_, _| {
                            h_flex()
                                .items_center()
                                .gap_2()
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(muted_foreground)
                                        .child(pkg_vis.clone()),
                                )
                                .child(
                                    Icon::new(if is_expanded {
                                        IconName::ChevronDown
                                    } else {
                                        IconName::ChevronRight
                                    })
                                    .xsmall()
                                    .text_color(muted_foreground),
                                )
                        }
                    })
                    .on_click({
                        let owner_clone = owner_clone.clone();
                        let package_clone = package_clone.clone();
                        move |_, _window, cx| {
                            view.update(cx, |this, cx| {
                                this.toggle_package_versions(
                                    owner_clone.clone(),
                                    package_clone.clone(),
                                    cx,
                                );
                            });
                        }
                    });

                if is_expanded {
                    let versions = if self.package_versions.is_empty() && version_error.is_some() {
                        v_flex()
                            .px_4()
                            .py_1()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(muted_foreground)
                                    .child("Failed to load versions"),
                            )
                            .into_any_element()
                    } else if self.package_versions.is_empty() {
                        v_flex()
                            .px_4()
                            .py_1()
                            .child(
                                h_flex()
                                    .gap_2()
                                    .items_center()
                                    .child(Spinner::new().small())
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(muted_foreground)
                                            .child("Loading versions…"),
                                    ),
                            )
                            .into_any_element()
                    } else {
                        v_flex()
                            .children(self.package_versions.iter().map(|version| {
                                h_flex()
                                    .items_center()
                                    .justify_between()
                                    .gap_2()
                                    .px_4()
                                    .py_1()
                                    .child(
                                        div()
                                            .text_sm()
                                            .text_color(foreground)
                                            .child(version.name.clone()),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(muted_foreground)
                                            .child(short_date(&version.created_at)),
                                    )
                            }))
                            .into_any_element()
                    };
                    v_flex()
                        .child(row)
                        .child(div().h_px().w_full().bg(border))
                        .child(versions)
                        .into_any_element()
                } else {
                    row.into_any_element()
                }
            }))
            .into_any_element()
    }

    /// The Traffic tab — view/clone totals plus the last week of daily data,
    /// then the top referrers and paths. An empty repo shows the "no traffic
    /// data yet" state (views/clones are `None` for 202/404 repos).
    fn render_traffic(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let muted_foreground = cx.theme().muted_foreground;
        let foreground = cx.theme().foreground;
        let border = cx.theme().border;

        if self.load_state == LoadState::Loading {
            return v_flex()
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
                                .child("Loading traffic…"),
                        ),
                )
                .into_any_element();
        }

        if self.load_state == LoadState::Error {
            return v_flex()
                .gap_3()
                .p_4()
                .child(
                    div()
                        .text_sm()
                        .text_color(muted_foreground)
                        .child("Failed to load traffic"),
                )
                .child(
                    Button::new("traffic-retry")
                        .outline()
                        .label("Retry")
                        .on_click(cx.listener(|this, _, _, cx| this.load_traffic(cx))),
                )
                .into_any_element();
        }

        let traffic = self.traffic.clone().unwrap_or_default();
        let has_any = traffic.views.is_some()
            || traffic.clones.is_some()
            || !traffic.referrers.is_empty()
            || !traffic.paths.is_empty();
        if !has_any {
            return v_flex()
                .flex_1()
                .items_center()
                .justify_center()
                .p_4()
                .child(div().text_sm().text_color(muted_foreground).child(
                    "No traffic data yet — GitHub releases it for repositories with enough views.",
                ))
                .into_any_element();
        }

        let views_total = traffic.views.as_ref().map(|v| (v.count, v.uniques));
        let clones_total = traffic.clones.as_ref().map(|c| (c.count, c.uniques));

        let totals_row = h_flex()
            .items_center()
            .gap_4()
            .px_3()
            .py_2()
            .text_sm()
            .text_color(muted_foreground)
            .when_some(views_total, |row, (count, uniques)| {
                row.child(format!(
                    "Views {} · {} unique",
                    fmt_num(count),
                    fmt_num(uniques)
                ))
            })
            .when_some(clones_total, |row, (count, uniques)| {
                row.child(format!(
                    "Clones {} · {} unique",
                    fmt_num(count),
                    fmt_num(uniques)
                ))
            });

        let daily_label = |title: &str| {
            div()
                .px_3()
                .pt_2()
                .pb_1()
                .text_xs()
                .font_semibold()
                .text_color(muted_foreground)
                .child(title.to_string())
        };

        let mut col = v_flex()
            .child(totals_row)
            .child(div().h_px().w_full().bg(border));

        if let Some(views) = traffic.views.clone() {
            col = col.child(daily_label("Views (last 7 days)")).child(
                v_flex().children(
                    views
                        .views
                        .iter()
                        .rev()
                        .skip(views.views.len().saturating_sub(7))
                        .map(|day| {
                            h_flex()
                                .items_center()
                                .justify_between()
                                .gap_2()
                                .px_3()
                                .py_1()
                                .child(
                                    div()
                                        .text_sm()
                                        .text_color(foreground)
                                        .child(short_date(&day.timestamp)),
                                )
                                .child(div().text_xs().text_color(muted_foreground).child(format!(
                                    "{} · {} unique",
                                    fmt_num(day.count),
                                    fmt_num(day.uniques)
                                )))
                        }),
                ),
            );
        }

        if !traffic.referrers.is_empty() {
            col = col
                .child(daily_label("Top referrers"))
                .child(
                    v_flex().children(traffic.referrers.iter().take(10).map(|r| {
                        h_flex()
                            .items_center()
                            .justify_between()
                            .gap_2()
                            .px_3()
                            .py_1()
                            .child(
                                div()
                                    .truncate()
                                    .text_sm()
                                    .text_color(foreground)
                                    .child(r.referrer.clone()),
                            )
                            .child(div().text_xs().text_color(muted_foreground).child(format!(
                                "{} · {} unique",
                                fmt_num(r.count),
                                fmt_num(r.uniques)
                            )))
                    })),
                );
        }

        if !traffic.paths.is_empty() {
            col = col.child(daily_label("Top paths")).child(v_flex().children(
                traffic.paths.iter().take(10).map(|p| {
                    h_flex()
                        .items_center()
                        .justify_between()
                        .gap_2()
                        .px_3()
                        .py_1()
                        .child(
                            div()
                                .truncate()
                                .text_sm()
                                .font_family("Cascadia Mono")
                                .text_color(foreground)
                                .child(p.path.clone()),
                        )
                        .child(div().text_xs().text_color(muted_foreground).child(format!(
                            "{} · {} unique",
                            fmt_num(p.count),
                            fmt_num(p.uniques)
                        )))
                }),
            ));
        }

        col.into_any_element()
    }

    /// Shared loading/error/empty states for the read-only repo-activity
    /// lists below (Commits/Actions/Deployments/Tags) — same shape as
    /// `render_invitations`/`render_branches`, factored out since there are
    /// four of them.
    fn activity_list_states(
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
    fn render_commits(&self, cx: &mut Context<Self>) -> impl IntoElement {
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

        v_flex()
            .py_1()
            .children(self.commits.iter().map(|commit| {
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
    fn render_workflow_runs(&self, cx: &mut Context<Self>) -> impl IntoElement {
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

        v_flex()
            .py_1()
            .children(self.workflow_runs.iter().map(|run| {
                let status_label = run.conclusion.clone().unwrap_or_else(|| run.status.clone());
                let color = match status_label.as_str() {
                    "success" => success,
                    "failure" | "cancelled" | "timed_out" => danger,
                    _ => muted_foreground,
                };
                let url = run.html_url.clone();

                ListItem::new(format!("helm-run-{}", run.id))
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
                    .on_click(move |_, _, cx| cx.open_url(&url))
                    .into_any_element()
            }))
            .into_any_element()
    }

    /// The Deployments screen.
    fn render_deployments(&self, cx: &mut Context<Self>) -> impl IntoElement {
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

        v_flex()
            .py_1()
            .children(self.deployments.iter().map(|dep| {
                let short_sha: String = dep.sha.chars().take(7).collect();
                ListItem::new(format!("helm-deployment-{}", dep.id))
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

    /// The Tags screen.
    fn render_tags(&self, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(el) = self.activity_list_states(
            "Loading tags…",
            "Failed to load tags",
            "No tags found",
            self.tags.is_empty(),
            |this, cx| this.load_tags(cx),
            cx,
        ) {
            return el;
        }

        let foreground = cx.theme().foreground;
        let muted_foreground = cx.theme().muted_foreground;

        v_flex()
            .py_1()
            .children(self.tags.iter().map(|tag| {
                let short_sha: String = tag.commit.sha.chars().take(7).collect();
                ListItem::new(format!("helm-tag-{}", tag.name))
                    .child(
                        div()
                            .text_sm()
                            .font_family("Cascadia Mono")
                            .text_color(foreground)
                            .child(tag.name.clone()),
                    )
                    .suffix(move |_, _| {
                        div().text_xs().text_color(muted_foreground).child(short_sha.clone())
                    })
                    .into_any_element()
            }))
            .into_any_element()
    }

    /// The Security screen — dependabot and secret-scanning alerts, in two
    /// sections. Both endpoints return raw JSON (no dedicated repo feature
    /// flag check is done here — a 404/disabled response just yields an
    /// empty list, same as `load_security`'s `unwrap_or_default`).
    fn render_security(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let muted_foreground = cx.theme().muted_foreground;
        let foreground = cx.theme().foreground;
        let border = cx.theme().border;

        if self.load_state == LoadState::Loading {
            return v_flex()
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
                                .child("Loading security alerts…"),
                        ),
                )
                .into_any_element();
        }

        let section_label = |title: &str| {
            div()
                .px_3()
                .pt_2()
                .pb_1()
                .text_xs()
                .font_semibold()
                .text_color(muted_foreground)
                .child(title.to_string())
        };

        let dependabot_rows = self.dependabot_alerts.iter().enumerate().map(|(i, alert)| {
            let package = alert
                .pointer("/dependency/package/name")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown package")
                .to_string();
            let severity = alert
                .pointer("/security_advisory/severity")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string();
            let state = alert
                .get("state")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            ListItem::new(format!("helm-dependabot-{i}"))
                .child(div().text_sm().text_color(foreground).child(package))
                .suffix(move |_, _| {
                    div()
                        .text_xs()
                        .text_color(muted_foreground)
                        .child(format!("{severity} · {state}"))
                })
        });

        let secret_rows = self.secret_scanning_alerts.iter().enumerate().map(|(i, alert)| {
            let secret_type = alert
                .get("secret_type_display_name")
                .or_else(|| alert.get("secret_type"))
                .and_then(|v| v.as_str())
                .unwrap_or("unknown secret")
                .to_string();
            let state = alert
                .get("state")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            ListItem::new(format!("helm-secret-scanning-{i}"))
                .child(div().text_sm().text_color(foreground).child(secret_type))
                .suffix(move |_, _| div().text_xs().text_color(muted_foreground).child(state.clone()))
        });

        v_flex()
            .child(section_label("Dependabot alerts"))
            .child(if self.dependabot_alerts.is_empty() {
                div()
                    .px_3()
                    .py_2()
                    .text_sm()
                    .text_color(muted_foreground)
                    .child("No open Dependabot alerts")
                    .into_any_element()
            } else {
                v_flex().children(dependabot_rows).into_any_element()
            })
            .child(div().h_px().w_full().bg(border))
            .child(section_label("Secret scanning alerts"))
            .child(if self.secret_scanning_alerts.is_empty() {
                div()
                    .px_3()
                    .py_2()
                    .text_sm()
                    .text_color(muted_foreground)
                    .child("No open secret scanning alerts")
                    .into_any_element()
            } else {
                v_flex().children(secret_rows).into_any_element()
            })
            .into_any_element()
    }

    /// A public profile view for someone other than the signed-in user,
    /// reached via [`Self::open_user_profile`] (e.g. clicking a
    /// collaborator). Read-only — editing is only available on `Profile`,
    /// the signed-in user's own screen.
    fn render_user_profile(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let muted_foreground = cx.theme().muted_foreground;
        let foreground = cx.theme().foreground;
        let border = cx.theme().border;

        if self.load_state == LoadState::Loading {
            return v_flex()
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
                                .child("Loading profile…"),
                        ),
                )
                .into_any_element();
        }

        let Some(user) = self.viewed_user.clone() else {
            return v_flex()
                .gap_3()
                .p_4()
                .child(
                    div()
                        .text_sm()
                        .text_color(muted_foreground)
                        .child("Failed to load profile"),
                )
                .into_any_element();
        };

        let url = user.html_url.clone();

        v_flex()
            .child(
                h_flex()
                    .items_center()
                    .gap_3()
                    .px_3()
                    .py_2()
                    .child(
                        Avatar::new()
                            .src(user.avatar_url.clone())
                            .name(user.login.clone())
                            .with_size(px(40.)),
                    )
                    .child(
                        v_flex()
                            .gap_0()
                            .min_w_0()
                            .child(
                                div()
                                    .font_semibold()
                                    .text_color(foreground)
                                    .child(user.name.clone().unwrap_or_else(|| user.login.clone())),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(muted_foreground)
                                    .child(format!("@{}", user.login)),
                            ),
                    )
                    .child(
                        Button::new("helm-user-profile-open-browser")
                            .ghost()
                            .xsmall()
                            .icon(IconName::ExternalLink)
                            .tooltip("Open in Browser")
                            .on_click(move |_, _, cx| cx.open_url(&url)),
                    ),
            )
            .child(div().h_px().w_full().bg(border))
            .when_some(user.bio.clone(), |col, bio| {
                col.child(div().p_3().text_sm().text_color(foreground).child(bio))
            })
            .child(
                h_flex()
                    .items_center()
                    .gap_4()
                    .px_3()
                    .py_2()
                    .text_sm()
                    .text_color(muted_foreground)
                    .child(format!("Repos {}", fmt_num(user.public_repos)))
                    .child(format!("Followers {}", fmt_num(user.followers)))
                    .child(format!("Following {}", fmt_num(user.following))),
            )
            .when_some(user.company.clone(), |col, company| {
                col.child(
                    div()
                        .px_3()
                        .py_1()
                        .text_xs()
                        .text_color(muted_foreground)
                        .child(format!("Company: {company}")),
                )
            })
            .when_some(user.location.clone(), |col, location| {
                col.child(
                    div()
                        .px_3()
                        .py_1()
                        .text_xs()
                        .text_color(muted_foreground)
                        .child(format!("Location: {location}")),
                )
            })
            .into_any_element()
    }

    /// The Invitations screen — pending repo invitations with Accept/Decline
    /// actions that round-trip against the GitHub API.
    fn render_invitations(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let muted_foreground = cx.theme().muted_foreground;
        let foreground = cx.theme().foreground;

        if self.invitations.is_empty() && self.org_invitations.is_empty() {
            return v_flex()
                .flex_1()
                .items_center()
                .justify_center()
                .p_4()
                .child(
                    div()
                        .text_sm()
                        .text_color(muted_foreground)
                        .child("No pending invitations"),
                )
                .into_any_element();
        }

        let view = cx.entity();

        let org_rows = self.org_invitations.iter().map(|inv| {
            let org_login = inv.organization.login.clone();
            let role = inv.role.clone();
            let org_accept = org_login.clone();
            let org_decline = org_login.clone();
            let view_accept = view.clone();
            let view_decline = view.clone();

            ListItem::new(format!("helm-org-invitation-{}", inv.organization.login))
                .child(
                    v_flex()
                        .gap_0p5()
                        .min_w_0()
                        .child(
                            div()
                                .text_sm()
                                .font_semibold()
                                .text_color(foreground)
                                .child(inv.organization.login.clone()),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(muted_foreground)
                                .child(format!("Organization · role: {role}")),
                        ),
                )
                .suffix({
                    move |_, _| {
                        h_flex()
                            .items_center()
                            .gap_2()
                            .child(
                                Button::new(format!("helm-org-invitation-accept-{org_login}"))
                                    .primary()
                                    .xsmall()
                                    .label("Accept")
                                    .on_click({
                                        let view = view_accept.clone();
                                        let org = org_accept.clone();
                                        move |_, window, cx| {
                                            view.update(cx, |this, cx| {
                                                this.handle_accept_org_invitation(
                                                    org.clone(),
                                                    window,
                                                    cx,
                                                );
                                            });
                                        }
                                    }),
                            )
                            .child(
                                Button::new(format!("helm-org-invitation-decline-{org_decline}"))
                                    .ghost()
                                    .xsmall()
                                    .icon(IconName::Delete)
                                    .tooltip("Decline invitation")
                                    .on_click({
                                        let view = view_decline.clone();
                                        let org = org_decline.clone();
                                        move |_, window, cx| {
                                            view.update(cx, |this, cx| {
                                                this.handle_decline_org_invitation(
                                                    org.clone(),
                                                    window,
                                                    cx,
                                                );
                                            });
                                        }
                                    }),
                            )
                    }
                })
        });

        v_flex()
            .py_1()
            .children(org_rows)
            .children(self.invitations.iter().map(|inv| {
                let full_name = inv.repository.full_name.clone();
                let private = inv.repository.private;
                let inviter = inv.inviter.login.clone();
                let permissions = inv.permissions.clone();
                let id_accept = inv.id;
                let id_decline = inv.id;
                let view_accept = view.clone();
                let view_decline = view.clone();

                ListItem::new(format!("helm-invitation-{}", inv.id))
                    .child(
                        v_flex()
                            .gap_0p5()
                            .min_w_0()
                            .child(
                                div()
                                    .text_sm()
                                    .font_semibold()
                                    .text_color(foreground)
                                    .child(full_name),
                            )
                            .child(
                                h_flex()
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(muted_foreground)
                                            .child(if private { "private" } else { "public" }),
                                    )
                                    .child(
                                        div().text_xs().text_color(muted_foreground).child(
                                            format!("{permissions} · invited by @{inviter}"),
                                        ),
                                    ),
                            ),
                    )
                    .suffix({
                        move |_, _| {
                            h_flex()
                                .items_center()
                                .gap_2()
                                .child(
                                    Button::new(("helm-invitation-accept", id_accept))
                                        .primary()
                                        .xsmall()
                                        .label("Accept")
                                        .on_click({
                                            let view = view_accept.clone();
                                            move |_, window, cx| {
                                                view.update(cx, |this, cx| {
                                                    this.handle_accept_invitation(
                                                        id_accept, window, cx,
                                                    );
                                                });
                                            }
                                        }),
                                )
                                .child(
                                    Button::new(("helm-invitation-decline", id_decline))
                                        .ghost()
                                        .xsmall()
                                        .icon(IconName::Delete)
                                        .tooltip("Decline invitation")
                                        .on_click({
                                            let view = view_decline.clone();
                                            move |_, window, cx| {
                                                view.update(cx, |this, cx| {
                                                    this.handle_decline_invitation(
                                                        id_decline, window, cx,
                                                    );
                                                });
                                            }
                                        }),
                                )
                        }
                    })
            }))
            .into_any_element()
    }

    /// Opens the "Add collaborator" dialog: a username input plus a
    /// permission dropdown, matching GitHub's own permission levels.
    fn open_add_collaborator_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_workspace_modal(HelmModalKind::AddCollaborator, window, cx);
        return;

        /*
        let username = cx.new(|cx| InputState::new(window, cx).placeholder("GitHub username"));
        let permission = Rc::new(Cell::new(0usize));
        let view = cx.entity();

        window.open_dialog(cx, move |dialog, _, _| {
            let username = username.clone();
            let permission = permission.clone();
            let view = view.clone();
            let permission_label = COLLABORATOR_PERMISSIONS[permission.get()];

            dialog
                .title("Add collaborator")
                .child(
                    v_flex().gap_3().child(Input::new(&username)).child(
                        Button::new("add-collaborator-permission")
                            .outline()
                            .label(permission_label)
                            .dropdown_menu({
                                let permission = permission.clone();
                                move |menu, _, _| {
                                    COLLABORATOR_PERMISSIONS.iter().enumerate().fold(
                                        menu,
                                        |menu, (ix, perm)| {
                                            let permission = permission.clone();
                                            menu.item(
                                                PopupMenuItem::new(*perm)
                                                    .checked(ix == permission.get())
                                                    .on_click(move |_, window, _| {
                                                        permission.set(ix);
                                                        window.refresh();
                                                    }),
                                            )
                                        },
                                    )
                                }
                            }),
                    ),
                )
                .footer(
                    DialogFooter::new()
                        .child(
                            DialogClose::new()
                                .child(Button::new("cancel").outline().label("Cancel")),
                        )
                        .child(
                            DialogAction::new().child(
                                Button::new("add-collaborator-confirm")
                                    .primary()
                                    .label("Add"),
                            ),
                        ),
                )
                .on_ok(move |_, window, cx| {
                    let login = username.read(cx).value().trim().to_string();
                    let perm = COLLABORATOR_PERMISSIONS[permission.get()].to_string();
                    if login.is_empty() {
                        return false;
                    }
                    view.update(cx, |this, cx| {
                        this.handle_set_collaborator_permission(login, perm, window, cx);
                    });
                    true
                })
        });
        */
    }

    /// Confirms removing `login` from `self.selected_repo`'s collaborators —
    /// same danger-variant `AlertDialog` shape as the editor's "Disregard
    /// changes" confirm.
    fn open_remove_collaborator_confirm(
        &mut self,
        login: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_workspace_modal(HelmModalKind::RemoveCollaborator(login), window, cx);
        return;

        /*
        let view = cx.entity();

        window.open_alert_dialog(cx, move |alert, _, _| {
            let view = view.clone();
            let login = login.clone();

            alert
                .title("Remove collaborator?")
                .description(format!(
                    "{login} will lose access to this repository immediately."
                ))
                .button_props(
                    DialogButtonProps::default()
                        .ok_variant(ButtonVariant::Danger)
                        .ok_text("Remove")
                        .cancel_text("Cancel")
                        .show_cancel(true),
                )
                .on_ok(move |_, window, cx| {
                    let login = login.clone();
                    view.update(cx, |this, cx| {
                        this.handle_remove_collaborator(login, window, cx);
                    });
                    true
                })
        });
        */
    }
}

/// A small label above an `Input` — for the modal forms whose fields (unlike
/// `CreateRepo`'s) have no distinguishing placeholder text of their own.
fn labeled_field(
    label: &'static str,
    input: Input,
    muted: gpui::Hsla,
) -> impl IntoElement {
    v_flex()
        .gap_1()
        .child(div().text_xs().text_color(muted).child(label))
        .child(input)
}

/// Mirrors the old TS `visLabel`: archived beats internal beats
/// private/public.
fn repo_vis_label(repo: &Repo) -> &'static str {
    if repo.archived {
        "archived"
    } else if repo.visibility == "internal" {
        "internal"
    } else if repo.private {
        "private"
    } else {
        "public"
    }
}

/// Pulls a `XXXX-XXXX` device code and a `https://github.com/login/device...`
/// URL out of a line of `gh auth login` output, if present.
fn parse_device_line(line: &str) -> (Option<String>, Option<String>) {
    let code = line
        .split(|c: char| !c.is_ascii_alphanumeric() && c != '-')
        .find(|tok| {
            let bytes = tok.as_bytes();
            bytes.len() == 9
                && bytes[4] == b'-'
                && bytes[..4].iter().all(|b| b.is_ascii_alphanumeric())
                && bytes[5..].iter().all(|b| b.is_ascii_alphanumeric())
                && tok
                    .chars()
                    .any(|c| c.is_ascii_digit() || c.is_ascii_uppercase())
        })
        .map(|s| s.to_string());

    let url = line.find("https://github.com/login/device").map(|start| {
        line[start..]
            .split(|c: char| c.is_whitespace())
            .next()
            .unwrap_or("")
            .trim_end_matches(['.', ')'])
            .to_string()
    });

    (code, url)
}

/// Formats a count as `1.2k` above 1000, plain otherwise.
fn fmt_num(n: u64) -> String {
    if n >= 1000 {
        format!("{:.1}k", n as f64 / 1000.0)
    } else {
        n.to_string()
    }
}

/// Trims an ISO-8601 date from the GitHub API down to its `YYYY-MM-DD` part.
fn short_date(iso: &str) -> String {
    iso.get(..10)
        .map(|s| s.to_string())
        .unwrap_or_else(|| iso.to_string())
}

impl Focusable for HelmPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<PanelEvent> for HelmPanel {}

impl Panel for HelmPanel {
    fn persistent_name() -> &'static str {
        "Helm Panel"
    }

    fn panel_key() -> &'static str {
        "HelmPanel"
    }

    fn position(&self, _window: &Window, _cx: &App) -> DockPosition {
        DockPosition::Left
    }

    fn position_is_valid(&self, position: DockPosition) -> bool {
        matches!(position, DockPosition::Left)
    }

    fn set_position(
        &mut self,
        _position: DockPosition,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
    }

    fn default_size(&self, _window: &Window, _cx: &App) -> gpui::Pixels {
        px(320.)
    }

    fn icon(&self, _window: &Window, _cx: &App) -> Option<ui::IconName> {
        Some(ui::IconName::Github)
    }

    fn icon_tooltip(&self, _window: &Window, _cx: &App) -> Option<&'static str> {
        Some("Helm")
    }

    fn toggle_action(&self) -> Box<dyn Action> {
        Box::new(ToggleFocus)
    }

    fn activation_priority(&self) -> u32 {
        5
    }
}

impl Render for HelmPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let show_nav = !matches!(self.screen, HelmScreen::Gate | HelmScreen::Auth);

        v_flex()
            .size_full()
            .bg(cx.theme().sidebar)
            .border_r_1()
            .border_color(cx.theme().border)
            .child(
                h_flex()
                    .flex_shrink_0()
                    .items_center()
                    .justify_between()
                    .gap_2()
                    .px_3()
                    .py_2()
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(Icon::new(IconName::Helm).text_color(cx.theme().foreground))
                            .child(
                                div()
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .text_sm()
                                    .text_color(cx.theme().foreground)
                                    .child("Helm"),
                            ),
                    )
                    .when(show_nav, |row| row.child(self.render_nav_bar(cx))),
            )
            .child(div().h_px().w_full().bg(cx.theme().border))
            .children(self.render_identity_header(cx))
            .child(
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scrollbar()
                    .child(match self.screen {
                        HelmScreen::Gate => self.render_gate(cx).into_any_element(),
                        HelmScreen::Auth => self.render_auth(cx).into_any_element(),
                        HelmScreen::Menu => self.render_menu(cx).into_any_element(),
                        HelmScreen::Profile => self.render_profile(cx).into_any_element(),
                        HelmScreen::OrgList => self.render_org_list(cx).into_any_element(),
                        HelmScreen::OrgDetail => self.render_org_detail(cx).into_any_element(),
                        HelmScreen::RepoList => self.render_repo_list(cx).into_any_element(),
                        HelmScreen::RepoDetail => self.render_repo_detail(cx).into_any_element(),
                        HelmScreen::Branches => self.render_branches(cx).into_any_element(),
                        HelmScreen::Collaborators => {
                            self.render_collaborators(cx).into_any_element()
                        }
                        HelmScreen::Issues => self.render_issues(cx).into_any_element(),
                        HelmScreen::IssueDetail => self.render_issue_detail(cx).into_any_element(),
                        HelmScreen::Pulls => self.render_pulls(cx).into_any_element(),
                        HelmScreen::PrDetail => self.render_pr_detail(cx).into_any_element(),
                        HelmScreen::Releases => self.render_releases(cx).into_any_element(),
                        HelmScreen::Packages => self.render_packages(cx).into_any_element(),
                        HelmScreen::Traffic => self.render_traffic(cx).into_any_element(),
                        HelmScreen::Invitations => self.render_invitations(cx).into_any_element(),
                        HelmScreen::UserProfile => self.render_user_profile(cx).into_any_element(),
                        HelmScreen::Commits => self.render_commits(cx).into_any_element(),
                        HelmScreen::WorkflowRuns => {
                            self.render_workflow_runs(cx).into_any_element()
                        }
                        HelmScreen::Deployments => self.render_deployments(cx).into_any_element(),
                        HelmScreen::Tags => self.render_tags(cx).into_any_element(),
                        HelmScreen::Security => self.render_security(cx).into_any_element(),
                    }),
            )
    }
}
