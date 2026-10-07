//! The "Helm" (GitHub) panel — Gate (CLI check) → Auth (login) → Menu, now
//! backed by the real `crate::backend::github` module instead of a UI-only
//! screen flip. Ported from `Forge_Old/panels/github/{gate,auth,menu}.rs`,
//! using `crate::backend::on_tokio` to bridge `github`'s tokio-native
//! `reqwest`/`gh` CLI calls onto GPUI's own `cx.spawn`, and
//! `gpui_component` elements (`Button`, `list::ListItem`, `Icon`,
//! `Spinner`) instead of the old `forge_ui` crate.

mod backend;
mod navigation;
mod profile;
mod orgs;
mod repos;
mod clone;
mod invitations;
mod collaborators;
mod branches;
mod pulls;
mod issues;
mod releases_packages;
mod insights;
mod activity;
mod repository_modal;
mod workflow_run_tab;
mod state;
mod widgets;

use std::sync::Arc;
use std::time::Duration;

use gpui::{
    Action, App, AppContext, AsyncWindowContext, ClipboardItem, Context, DismissEvent, Entity,
    EventEmitter, FocusHandle, Focusable, InteractiveElement, IntoElement, KeyBinding, MouseButton,
    ParentElement, PathPromptOptions, Render, SharedString, StatefulInteractiveElement, Styled,
    Subscription, Task, TaskExt, WeakEntity, Window, actions, div, prelude::FluentBuilder as _, px,
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
use gpui_flow::{Controls, FlowGraph, FlowNode, FlowState, NodeId};
use serde_json::json;

use crate::backend::github::{
    Branch, CloneEvent, Collaborator, Comment, CommitSummary, Deployment, GhAuthEvent, GhState,
    GitHubUser, GitHubUserDetail, Issue, OrgDetail, OrgInvitation, Package, PackageVersion, Pull,
    Release, Repo, RepoInvitation, RepoTraffic, Tag, WorkflowJob, WorkflowRun,
    gh_accept_org_invitation, gh_accept_repo_invitation, gh_add_collaborator, gh_auth_status,
    gh_check_cli, gh_clone_repo, gh_create_pull, gh_create_release, gh_create_repo,
    gh_decline_org_invitation, gh_decline_repo_invitation, gh_ensure_scope, gh_get_branches,
    gh_get_collaborators, gh_get_current_user, gh_get_org_detail, gh_get_org_logins,
    gh_get_repo_invitations, gh_get_repos, gh_get_traffic_clones, gh_get_traffic_paths,
    gh_get_traffic_referrers, gh_get_traffic_views, gh_get_user, gh_get_workflow_run,
    gh_get_workflow_run_jobs, gh_list_dependabot_alerts, gh_list_deployments,
    gh_list_issue_comments, gh_list_issues, gh_list_org_invitations, gh_list_package_versions,
    gh_list_packages, gh_list_pulls, gh_list_recent_commits, gh_list_releases,
    gh_list_secret_scanning_alerts, gh_list_tags, gh_list_workflow_runs, gh_login, gh_logout,
    gh_remove_collaborator, gh_update_repo, gh_update_topics, gh_update_user,
};
use crate::backend::on_tokio;
use collaborators::*;
use repository_modal::*;
use workflow_run_tab::*;
use state::*;
use widgets::*;
use workspace::{
    Item, ModalView, Toast, Workspace,
    dock::{DockPosition, Panel, PanelEvent},
    notifications::NotificationId,
};

actions!(
    helm_panel,
    [
        ToggleFocus,
        /// Moves the selection down one row in whichever list (repos,
        /// issues) currently has focus.
        SelectNextRow,
        /// Moves the selection up one row, same scope as `SelectNextRow`.
        SelectPrevRow,
        /// Opens the selected row — the keyboard equivalent of clicking it
        /// (drills into the repo / opens the issue detail).
        OpenSelectedRow,
        /// The Invitations screen's secondary per-row action (Decline) —
        /// `OpenSelectedRow`/Enter there is Accept. No other list currently
        /// uses this; everywhere else Enter is the row's only action.
        ActSelectedRow
    ]
);

pub fn init(cx: &mut App) {
    cx.observe_new(|workspace: &mut Workspace, _, _| {
        workspace.register_action(|workspace, _: &ToggleFocus, window, cx| {
            workspace.toggle_panel_focus::<HelmPanel>(window, cx);
        });
    })
    .detach();

    // Every list screen in Helm — repos, issues, and (not yet ported to
    // this pattern) everything else — was mouse-only before this: no way to
    // even reach a row without clicking it, let alone drill into one. Scoped
    // to `HelmRowList` (each list container sets that key context), matching
    // how `gpui_component::table::data_table` scopes its own row-navigation
    // bindings to `DataTable`, and identically to
    // `npm_manager_panel`/`nuget_manager_panel`/`python_manager_panel`'s own
    // per-crate `PACKAGE_LIST_CONTEXT`.
    cx.bind_keys([
        KeyBinding::new("down", SelectNextRow, Some("HelmRowList")),
        KeyBinding::new("up", SelectPrevRow, Some("HelmRowList")),
        KeyBinding::new("enter", OpenSelectedRow, Some("HelmRowList")),
        KeyBinding::new("space", ActSelectedRow, Some("HelmRowList")),
    ]);
}

/// A change Helm sends to GitHub on the user's behalf. Every mutating
/// handler builds one of these and hands it to [`HelmPanel::run_action`], so
/// they all share one failure path — including being sent through the auth
/// gate and re-sent when the token turns out to lack a scope.
#[derive(Clone)]
enum HelmAction {
    CreateRepo {
        opts: serde_json::Value,
    },
    EditRepo {
        changes: serde_json::Value,
        topics: Vec<String>,
    },
    CreatePull {
        title: String,
        body: String,
        head: String,
        base: String,
    },
    CreateRelease {
        tag_name: String,
        title: String,
        body: String,
        draft: bool,
        prerelease: bool,
    },
    UpdateProfile {
        changes: serde_json::Value,
    },
    AcceptRepoInvitation(u64),
    DeclineRepoInvitation(u64),
    AcceptOrgInvitation(String),
    DeclineOrgInvitation(String),
    SetCollaboratorPermission {
        login: String,
        permission: String,
    },
    RemoveCollaborator(String),
}

impl HelmAction {
    /// The OAuth scope GitHub wants for this action — what the auth gate
    /// asks for when the action is rejected and the token doesn't have it.
    fn required_scope(&self) -> &'static str {
        match self {
            HelmAction::UpdateProfile { .. } => "user",
            HelmAction::AcceptOrgInvitation(_) | HelmAction::DeclineOrgInvitation(_) => {
                "write:org"
            }
            _ => "repo",
        }
    }

    /// Completes "Failed to …" in the failure notification.
    fn failure_label(&self) -> &'static str {
        match self {
            HelmAction::CreateRepo { .. } => "create repository",
            HelmAction::EditRepo { .. } => "update repository",
            HelmAction::CreatePull { .. } => "create pull request",
            HelmAction::CreateRelease { .. } => "create release",
            HelmAction::UpdateProfile { .. } => "update profile",
            HelmAction::AcceptRepoInvitation(_) | HelmAction::AcceptOrgInvitation(_) => {
                "accept invitation"
            }
            HelmAction::DeclineRepoInvitation(_) | HelmAction::DeclineOrgInvitation(_) => {
                "decline invitation"
            }
            HelmAction::SetCollaboratorPermission { .. } => "update collaborator",
            HelmAction::RemoveCollaborator(_) => "remove collaborator",
        }
    }

    /// Whether this acts on `HelmPanel::selected_repo`.
    fn needs_repo(&self) -> bool {
        matches!(
            self,
            HelmAction::EditRepo { .. }
                | HelmAction::CreatePull { .. }
                | HelmAction::CreateRelease { .. }
                | HelmAction::SetCollaboratorPermission { .. }
                | HelmAction::RemoveCollaborator(_)
        )
    }

    /// Makes the API call(s). Returns the resulting repository for the two
    /// actions that produce one (create and edit), `None` otherwise.
    async fn perform(self, repo: Option<Repo>, gh_state: &GhState) -> Result<Option<Repo>, String> {
        let selected = || repo.clone().ok_or_else(|| "No repository selected".to_string());
        match self {
            HelmAction::CreateRepo { opts } => gh_create_repo(opts, gh_state).await.map(Some),
            HelmAction::EditRepo { changes, topics } => {
                let repo = selected()?;
                let owner = repo.owner.login;
                let updated = gh_update_repo(owner.clone(), repo.name, changes, gh_state).await?;
                // The topics endpoint 404s on a pre-rename name, so it has to
                // use the name from the PATCH response.
                gh_update_topics(owner, updated.name.clone(), topics, gh_state).await?;
                Ok(Some(updated))
            }
            HelmAction::CreatePull {
                title,
                body,
                head,
                base,
            } => {
                let repo = selected()?;
                gh_create_pull(
                    repo.owner.login,
                    repo.name,
                    head,
                    base,
                    title,
                    (!body.is_empty()).then_some(body),
                    gh_state,
                )
                .await
                .map(|_| None)
            }
            HelmAction::CreateRelease {
                tag_name,
                title,
                body,
                draft,
                prerelease,
            } => {
                let repo = selected()?;
                gh_create_release(
                    repo.owner.login,
                    repo.name,
                    tag_name,
                    (!title.is_empty()).then_some(title),
                    (!body.is_empty()).then_some(body),
                    Some(draft),
                    Some(prerelease),
                    gh_state,
                )
                .await
                .map(|_| None)
            }
            HelmAction::UpdateProfile { changes } => {
                gh_update_user(changes, gh_state).await.map(|_| None)
            }
            HelmAction::AcceptRepoInvitation(id) => {
                gh_accept_repo_invitation(id, gh_state).await.map(|_| None)
            }
            HelmAction::DeclineRepoInvitation(id) => {
                gh_decline_repo_invitation(id, gh_state).await.map(|_| None)
            }
            HelmAction::AcceptOrgInvitation(org) => {
                gh_accept_org_invitation(org, gh_state).await.map(|_| None)
            }
            HelmAction::DeclineOrgInvitation(org) => {
                gh_decline_org_invitation(org, gh_state).await.map(|_| None)
            }
            HelmAction::SetCollaboratorPermission { login, permission } => {
                let repo = selected()?;
                gh_add_collaborator(repo.owner.login, repo.name, login, permission, gh_state)
                    .await
                    .map(|_| None)
            }
            HelmAction::RemoveCollaborator(login) => {
                let repo = selected()?;
                gh_remove_collaborator(repo.owner.login, repo.name, login, gh_state)
                    .await
                    .map(|_| None)
            }
        }
    }
}

/// An action GitHub rejected because the token lacked a scope, kept so it
/// can be re-sent once the auth gate has granted it.
struct PendingAction {
    action: HelmAction,
    /// Where the user was when the action failed, restored after re-auth
    /// (which otherwise always lands on the menu).
    resume_screen: HelmScreen,
}

/// What `gh auth status` says about the token after a permission-shaped API
/// failure — decides whether re-authorizing could fix it at all.
enum TokenScopeCheck {
    /// The token has the scope; the failure is about the account's rights
    /// (or org policy), which re-auth can't change.
    HasScope,
    MissingScope,
    NotLoggedIn,
}

/// The `error_msg` that puts the auth screen into its "authorize this scope"
/// state. One function so the code that sets it and `render_auth`'s check
/// for it can't drift apart.
fn missing_scope_message(scope: &str) -> String {
    format!("Missing '{scope}' scope.")
}

/// Whether a `gh_api_fetch` error is one a missing scope produces. GitHub
/// answers 403 for an insufficient scope, 404 when the scope is so
/// insufficient the resource isn't visible to the token at all, and 401 for
/// a token it no longer accepts.
fn is_permission_error(error: &str) -> bool {
    ["GitHub API 401", "GitHub API 403", "GitHub API 404"]
        .iter()
        .any(|prefix| error.starts_with(prefix))
}

/// Whether a token carrying `granted` satisfies `needed`, counting the one
/// parent scope that implies it (`admin:org` includes `write:org`).
fn token_has_scope(granted: &[String], needed: &str) -> bool {
    granted.iter().any(|scope| {
        scope == needed || (needed == "write:org" && scope == "admin:org")
    })
}

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
    /// Set when an action was bounced to the auth gate for a missing scope;
    /// re-sent by `do_auth` once the scope is granted.
    pending_action: Option<PendingAction>,
    /// The scope the auth gate's "Authorize" button requests. `repo` for
    /// the startup check; an action's own scope when it sent us there.
    scope_to_authorize: &'static str,
    device_code: String,
    device_url: String,
    /// True for 2s after the device code is copied, flips the copy icon to
    /// a checkmark.
    code_copied: bool,

    // Session (populated once authenticated)
    account: String,
    scopes: Vec<String>,
    org_logins: Vec<String>,
    org_list_cursor: Option<usize>,
    org_list_focus: FocusHandle,
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
    /// Row `up`/`down`/`enter` act on, within the Profile screen's own nav
    /// menu (Repositories/Organizations/Invitations/Account Security) —
    /// built fresh each render, not a stored `Vec`, so this indexes
    /// whatever `render_profile` computed that same pass.
    profile_menu_cursor: Option<usize>,
    profile_menu_focus: FocusHandle,

    // Repos
    repos: Vec<Repo>,
    repo_search: Entity<InputState>,
    /// Row `up`/`down`/`enter` act on, within the filtered repo list
    /// `render_repo_list` computes from `repos` + `repo_search` — an index
    /// into that filtered order, not into `repos` itself, since the two can
    /// disagree once a search narrows the list.
    repo_list_cursor: Option<usize>,
    repo_list_focus: FocusHandle,

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
    branches_list_cursor: Option<usize>,
    branches_list_focus: FocusHandle,
    /// Populated by [`Self::load_collaborators`] for the `Collaborators`
    /// screen.
    collaborators: Vec<Collaborator>,
    collaborators_list_cursor: Option<usize>,
    collaborators_list_focus: FocusHandle,

    // Repo-detail tab caches (Issues/Pulls/Releases/Packages/Traffic) —
    // loaded on screen entry and kept while drilling; `set_screen` clears
    // them along with `selected_repo` when leaving the repo-drilled screens.
    issues: Vec<Issue>,
    issues_filter: String,
    /// Row `up`/`down`/`enter` act on, within `issues`.
    issues_list_cursor: Option<usize>,
    issues_list_focus: FocusHandle,
    pulls: Vec<Pull>,
    pulls_filter: String,
    pulls_list_cursor: Option<usize>,
    pulls_list_focus: FocusHandle,
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
    releases_list_cursor: Option<usize>,
    releases_list_focus: FocusHandle,
    packages: Vec<Package>,
    packages_list_cursor: Option<usize>,
    packages_list_focus: FocusHandle,
    package_versions: Vec<PackageVersion>,
    package_versions_error: Option<String>,
    expanded_package: Option<String>,
    traffic: Option<RepoTraffic>,
    commits: Vec<CommitSummary>,
    commits_list_cursor: Option<usize>,
    commits_list_focus: FocusHandle,
    workflow_runs: Vec<WorkflowRun>,
    workflow_runs_list_cursor: Option<usize>,
    workflow_runs_list_focus: FocusHandle,
    deployments: Vec<Deployment>,
    deployments_list_cursor: Option<usize>,
    deployments_list_focus: FocusHandle,
    tags: Vec<Tag>,
    tags_list_cursor: Option<usize>,
    tags_list_focus: FocusHandle,
    dependabot_alerts: Vec<serde_json::Value>,
    dependabot_list_cursor: Option<usize>,
    dependabot_list_focus: FocusHandle,
    secret_scanning_alerts: Vec<serde_json::Value>,
    secret_scanning_list_cursor: Option<usize>,
    secret_scanning_list_focus: FocusHandle,

    // Pending repo invitations shown on the Profile screen's Invitations
    // screen — `repo_invitation_count` above is just the badge for that row.
    invitations: Vec<RepoInvitation>,
    invitations_list_cursor: Option<usize>,
    invitations_list_focus: FocusHandle,
    /// Pending org invitations, shown on the same Invitations screen — listed
    /// above `invitations` with no visual separator, so `invitations_list_cursor`
    /// covers both as one combined, index-shared list (org invitations first,
    /// matching display order) rather than each getting its own cursor.
    org_invitations: Vec<OrgInvitation>,

    /// Another user's public profile, viewed via [`Self::open_user_profile`]
    /// (e.g. clicking a collaborator) — cleared whenever `set_screen` lands
    /// anywhere but `UserProfile`. Distinct from `user`, which is always the
    /// signed-in account.
    viewed_user: Option<GitHubUserDetail>,
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
                pending_action: None,
                scope_to_authorize: "repo",
                device_code: String::new(),
                device_url: String::new(),
                code_copied: false,
                account: String::new(),
                scopes: Vec::new(),
                org_logins: Vec::new(),
                org_list_cursor: None,
                org_list_focus: cx.focus_handle(),
                user: None,
                repo_invitation_count: 0,
                selected_org: None,
                org_detail: None,
                profile_menu_cursor: None,
                profile_menu_focus: cx.focus_handle(),
                repos: Vec::new(),
                repo_search,
                repo_list_cursor: None,
                repo_list_focus: cx.focus_handle(),
                selected_repo: None,
                clone_url_copied: false,
                cloning: false,
                clone_lines: Vec::new(),
                clone_error: None,
                clone_succeeded_path: None,
                clone_target_dir: None,
                workspace,
                branches: Vec::new(),
                branches_list_cursor: None,
                branches_list_focus: cx.focus_handle(),
                collaborators: Vec::new(),
                collaborators_list_cursor: None,
                collaborators_list_focus: cx.focus_handle(),
                issues: Vec::new(),
                issues_filter: "open".into(),
                issues_list_cursor: None,
                issues_list_focus: cx.focus_handle(),
                pulls: Vec::new(),
                pulls_filter: "open".into(),
                pulls_list_cursor: None,
                pulls_list_focus: cx.focus_handle(),
                selected_issue: None,
                selected_pr: None,
                detail_comments: Vec::new(),
                detail_comments_state: LoadState::Idle,
                releases: Vec::new(),
                releases_list_cursor: None,
                releases_list_focus: cx.focus_handle(),
                packages: Vec::new(),
                packages_list_cursor: None,
                packages_list_focus: cx.focus_handle(),
                package_versions: Vec::new(),
                package_versions_error: None,
                expanded_package: None,
                traffic: None,
                commits: Vec::new(),
                commits_list_cursor: None,
                commits_list_focus: cx.focus_handle(),
                workflow_runs: Vec::new(),
                workflow_runs_list_cursor: None,
                workflow_runs_list_focus: cx.focus_handle(),
                deployments: Vec::new(),
                deployments_list_cursor: None,
                deployments_list_focus: cx.focus_handle(),
                tags: Vec::new(),
                tags_list_cursor: None,
                tags_list_focus: cx.focus_handle(),
                dependabot_alerts: Vec::new(),
                dependabot_list_cursor: None,
                dependabot_list_focus: cx.focus_handle(),
                secret_scanning_alerts: Vec::new(),
                secret_scanning_list_cursor: None,
                secret_scanning_list_focus: cx.focus_handle(),
                invitations: Vec::new(),
                invitations_list_cursor: None,
                invitations_list_focus: cx.focus_handle(),
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
                        // An action that was bounced here for a missing
                        // scope: put the user back where they were and
                        // re-send it. `may_reauthorize: false` so a second
                        // rejection reports the failure instead of looping
                        // the gate.
                        if let Some(pending) = this.pending_action.take() {
                            this.screen = pending.resume_screen;
                            this.run_action(pending.action, false, cx);
                        }
                    }
                    AuthOutcome::MissingRepoScope { account, scopes } => {
                        this.account = account;
                        this.scopes = scopes;
                        this.load_state = LoadState::Idle;
                        this.scope_to_authorize = "repo";
                        this.error_msg = missing_scope_message("repo");
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
        self.listen_for_device_code(cx);

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

    /// Mirrors the device code and verification URL that `gh auth login` /
    /// `gh auth refresh` print onto the panel, until the command reports
    /// `Done`. Call before spawning the command.
    fn listen_for_device_code(&mut self, cx: &mut Context<Self>) {
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
    }

    /// Runs `gh auth refresh -s <scope> --hostname github.com` for
    /// `self.scope_to_authorize` and shows its device code the same way a
    /// first login does — `login_started` is what switches `render_auth`
    /// over to the code/URL view, without which the refresh would sit
    /// waiting on a code the user was never shown.
    fn handle_authorize_scope(&mut self, cx: &mut Context<Self>) {
        self.login_started = true;
        self.device_code.clear();
        self.device_url.clear();
        self.code_copied = false;
        self.load_state = LoadState::Loading;
        self.error_msg.clear();
        cx.notify();

        self.listen_for_device_code(cx);

        let scope = self.scope_to_authorize;
        let gh_state = self.gh_state.clone();
        cx.spawn(async move |this, cx| {
            let result = on_tokio(async move { gh_ensure_scope(scope, &gh_state).await }).await;
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
                    this.pending_action = None;
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

    /// Sends `action` to GitHub and applies its result to the panel.
    ///
    /// When GitHub rejects it with a permission-shaped error and
    /// `may_reauthorize` is set, `gh auth status` is consulted to tell a
    /// token that lacks the action's scope apart from an account that simply
    /// lacks the rights. Only the former is sent to the auth gate (with the
    /// action parked in `pending_action` for `do_auth` to re-send) —
    /// re-authorizing can't fix the latter, so that just reports the
    /// failure.
    fn run_action(&mut self, action: HelmAction, may_reauthorize: bool, cx: &mut Context<Self>) {
        let repo = self.selected_repo.clone();
        if action.needs_repo() && repo.is_none() {
            return;
        }
        let scope = action.required_scope();
        let performed = action.clone();

        let gh_state = self.gh_state.clone();
        cx.spawn(async move |this, cx| {
            let (result, scope_check) = on_tokio(async move {
                let result = performed.perform(repo, &gh_state).await;
                let scope_check = match &result {
                    Err(error) if may_reauthorize && is_permission_error(error) => {
                        match gh_auth_status().await {
                            Ok(Some(info)) if token_has_scope(&info.scopes, scope) => {
                                TokenScopeCheck::HasScope
                            }
                            Ok(Some(_)) => TokenScopeCheck::MissingScope,
                            Ok(None) => TokenScopeCheck::NotLoggedIn,
                            // Couldn't ask `gh`; fall through to reporting
                            // the original failure rather than guessing.
                            Err(_) => TokenScopeCheck::HasScope,
                        }
                    }
                    _ => TokenScopeCheck::HasScope,
                };
                (result, scope_check)
            })
            .await;
            this.update(cx, |this, cx| match (result, scope_check) {
                (Ok(repo), _) => {
                    this.action_succeeded(&action, repo, cx);
                    this.action_settled(&action, cx);
                }
                (Err(error), TokenScopeCheck::HasScope) => {
                    this.notify(format!("Failed to {}: {error}", action.failure_label()), cx);
                    this.action_settled(&action, cx);
                }
                (Err(_), TokenScopeCheck::MissingScope) => {
                    this.notify(
                        format!(
                            "GitHub rejected the request: your login is missing the '{scope}' \
                             scope. Authorize it and Helm will try again."
                        ),
                        cx,
                    );
                    this.pending_action = Some(PendingAction {
                        action,
                        resume_screen: this.screen,
                    });
                    // The same state `do_auth` leaves behind for a token
                    // without `repo`, so `render_auth` shows its "Authorize"
                    // prompt — for this action's scope.
                    this.screen = HelmScreen::Auth;
                    this.login_started = false;
                    this.load_state = LoadState::Idle;
                    this.scope_to_authorize = scope;
                    this.error_msg = missing_scope_message(scope);
                    cx.notify();
                }
                (Err(_), TokenScopeCheck::NotLoggedIn) => {
                    this.notify(
                        "GitHub rejected the request: you are no longer signed in. \
                         Sign in and Helm will try again.",
                        cx,
                    );
                    this.pending_action = Some(PendingAction {
                        action,
                        resume_screen: this.screen,
                    });
                    this.login_started = false;
                    // Re-runs the status check, which lands on the login
                    // prompt.
                    this.do_auth(cx);
                }
            })
            .ok();
        })
        .detach();
    }

    /// Applies a successful action to panel state. `repo` is the repository
    /// GitHub returned, for the actions that return one.
    fn action_succeeded(&mut self, action: &HelmAction, repo: Option<Repo>, cx: &mut Context<Self>) {
        match action {
            HelmAction::CreateRepo { .. } => {
                if let Some(repo) = repo {
                    self.repos.push(repo.clone());
                    self.select_repo(repo, cx);
                }
            }
            HelmAction::EditRepo { .. } => {
                if let Some(updated) = repo {
                    if let Some(existing) = self.repos.iter_mut().find(|r| r.id == updated.id) {
                        *existing = updated.clone();
                    }
                    self.selected_repo = Some(updated);
                }
            }
            HelmAction::UpdateProfile { .. } => {
                self.notify("Profile updated", cx);
                let gh_state = self.gh_state.clone();
                cx.spawn(async move |this, cx| {
                    let updated = on_tokio(async move { gh_get_current_user(&gh_state).await }).await;
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
            HelmAction::AcceptRepoInvitation(id) => {
                self.invitations.retain(|inv| inv.id != *id);
                self.notify("Invitation accepted", cx);
            }
            HelmAction::DeclineRepoInvitation(id) => {
                self.invitations.retain(|inv| inv.id != *id);
                self.notify("Invitation declined", cx);
            }
            HelmAction::AcceptOrgInvitation(org) => {
                self.org_invitations
                    .retain(|inv| &inv.organization.login != org);
                self.notify("Invitation accepted", cx);
            }
            HelmAction::DeclineOrgInvitation(org) => {
                self.org_invitations
                    .retain(|inv| &inv.organization.login != org);
                self.notify("Invitation declined", cx);
            }
            HelmAction::CreatePull { .. }
            | HelmAction::CreateRelease { .. }
            | HelmAction::SetCollaboratorPermission { .. }
            | HelmAction::RemoveCollaborator(_) => {}
        }
        self.repo_invitation_count = self.invitations.len() + self.org_invitations.len();
        cx.notify();
    }

    /// Runs once an action has finished either way (but not when it was
    /// parked for re-auth): reloads the list the action edits, so the UI
    /// reflects what GitHub actually has rather than what was attempted.
    fn action_settled(&mut self, action: &HelmAction, cx: &mut Context<Self>) {
        match action {
            HelmAction::CreatePull { .. } => self.load_pulls(cx),
            HelmAction::CreateRelease { .. } => self.load_releases(cx),
            HelmAction::SetCollaboratorPermission { .. } | HelmAction::RemoveCollaborator(_) => {
                self.load_collaborators(cx)
            }
            _ => {}
        }
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

        // Missing scope — `repo` from the startup check, or whichever scope
        // a rejected action needs.
        if self.error_msg == missing_scope_message(self.scope_to_authorize) && !self.login_started
        {
            let loading = self.load_state == LoadState::Loading;
            let scope = self.scope_to_authorize;
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
                        .child(format!("⚠ Missing '{scope}' scope")),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(muted_foreground)
                        .child(if self.pending_action.is_some() {
                            format!(
                                "GitHub rejected your last change because this login lacks \
                                 the {scope} scope. Authorize it and Helm will send the \
                                 change again."
                            )
                        } else {
                            format!("Helm needs the {scope} scope to manage repositories.")
                        }),
                )
                .child(
                    Button::new("auth-authorize-scope")
                        .primary()
                        .label(format!("Authorize {scope} scope"))
                        .disabled(loading)
                        .on_click(cx.listener(|this, _, _, cx| this.handle_authorize_scope(cx))),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn scopes(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn permission_errors_are_recognized_by_status() {
        assert!(is_permission_error("GitHub API 401: {\"message\":\"Bad credentials\"}"));
        assert!(is_permission_error("GitHub API 403: {\"message\":\"Forbidden\"}"));
        assert!(is_permission_error("GitHub API 404: {\"message\":\"Not Found\"}"));

        // Validation, server and transport failures are not about the token.
        assert!(!is_permission_error("GitHub API 422: {\"message\":\"Validation Failed\"}"));
        assert!(!is_permission_error("GitHub API 500: oops"));
        assert!(!is_permission_error("Network error: timed out"));
        assert!(!is_permission_error("gh auth token failed: GitHub API 403"));
    }

    #[test]
    fn token_scope_matching() {
        let granted = scopes(&["gist", "read:org", "repo"]);
        assert!(token_has_scope(&granted, "repo"));
        assert!(!token_has_scope(&granted, "user"));
        // `read:org` is not enough to change an org membership.
        assert!(!token_has_scope(&granted, "write:org"));
        assert!(token_has_scope(&scopes(&["write:org"]), "write:org"));
        assert!(token_has_scope(&scopes(&["admin:org"]), "write:org"));
        // A narrower repo scope is not the full one.
        assert!(!token_has_scope(&scopes(&["public_repo"]), "repo"));
        assert!(!token_has_scope(&[], "repo"));
    }

    #[test]
    fn actions_ask_for_the_scope_they_need() {
        let json = serde_json::Value::Null;
        assert_eq!(HelmAction::CreateRepo { opts: json.clone() }.required_scope(), "repo");
        assert_eq!(
            HelmAction::EditRepo { changes: json.clone(), topics: Vec::new() }.required_scope(),
            "repo"
        );
        assert_eq!(HelmAction::RemoveCollaborator("x".into()).required_scope(), "repo");
        assert_eq!(HelmAction::AcceptRepoInvitation(1).required_scope(), "repo");
        assert_eq!(HelmAction::UpdateProfile { changes: json }.required_scope(), "user");
        assert_eq!(HelmAction::AcceptOrgInvitation("o".into()).required_scope(), "write:org");
        assert_eq!(HelmAction::DeclineOrgInvitation("o".into()).required_scope(), "write:org");
    }

    #[test]
    fn only_repository_actions_need_a_selected_repo() {
        let json = serde_json::Value::Null;
        assert!(HelmAction::EditRepo { changes: json.clone(), topics: Vec::new() }.needs_repo());
        assert!(HelmAction::RemoveCollaborator("x".into()).needs_repo());
        assert!(!HelmAction::CreateRepo { opts: json.clone() }.needs_repo());
        assert!(!HelmAction::UpdateProfile { changes: json }.needs_repo());
        assert!(!HelmAction::AcceptRepoInvitation(1).needs_repo());
    }

    #[test]
    fn step_selected_starts_at_the_nearest_end() {
        assert_eq!(step_selected(None, 3, true), Some(0));
        assert_eq!(step_selected(None, 3, false), Some(2));
    }

    #[test]
    fn step_selected_wraps_at_both_ends() {
        assert_eq!(step_selected(Some(0), 3, true), Some(1));
        assert_eq!(step_selected(Some(2), 3, true), Some(0));
        assert_eq!(step_selected(Some(0), 3, false), Some(2));
        assert_eq!(step_selected(Some(1), 3, false), Some(0));
        assert_eq!(step_selected(Some(0), 1, true), Some(0));
        assert_eq!(step_selected(Some(0), 1, false), Some(0));
    }

    #[test]
    fn step_selected_handles_empty_and_shrunken_lists() {
        assert_eq!(step_selected(None, 0, true), None);
        assert_eq!(step_selected(Some(4), 0, false), None);
        // A selection left over from a longer list still lands in range.
        assert!(step_selected(Some(9), 3, true).is_some_and(|ix| ix < 3));
        assert!(step_selected(Some(9), 3, false).is_some_and(|ix| ix < 3));
    }

    #[test]
    fn missing_scope_message_names_the_scope() {
        assert_eq!(missing_scope_message("repo"), "Missing 'repo' scope.");
        assert_ne!(missing_scope_message("repo"), missing_scope_message("user"));
    }
}
