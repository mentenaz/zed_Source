//! The "Helm" (GitHub) panel — Gate (CLI check) → Auth (login) → Menu, now
//! backed by the real `crate::backend::github` module instead of a UI-only
//! screen flip. Ported from `Forge_Old/panels/github/{gate,auth,menu}.rs`,
//! using `crate::backend::on_tokio` to bridge `github`'s tokio-native
//! `reqwest`/`gh` CLI calls onto GPUI's own `cx.spawn`, and
//! `gpui_component` elements (`Button`, `list::ListItem`, `Icon`,
//! `Spinner`) instead of the old `forge_ui` crate.

mod backend;
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
        let repo_name = repo.name.clone();
        cx.spawn_in(window, async move |this, cx| {
            // `run_npm: true` — installs dependencies as Phase 2 of the same
            // clone, streamed into this same progress view (`CloneEvent::NpmStart`
            // switches the "Cloning…" label, see the modal's render) instead of
            // leaving it to `npm_bootstrap`'s separate post-open prompt. By the
            // time the new workspace opens, `node_modules` already exists, so
            // that prompt's own `check_worktree` guard skips it there.
            let result =
                on_tokio(
                    async move { gh_clone_repo(full_name, target_for_gh, true, &gh_state).await },
                )
                .await;
            this.update_in(cx, |this, window, cx| {
                this.cloning = false;
                match result {
                    Ok(()) => {
                        this.clone_lines
                            .push(format!("Cloned repository to {target_path}"));
                        this.notify(format!("Cloned {repo_name} and installed dependencies"), cx);
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
        _window: &mut Window,
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
        self.run_action(HelmAction::CreateRepo { opts }, true, cx);
    }

    /// Renames/updates description+homepage, toggles feature switches
    /// (issues/wiki/projects/discussions), optionally changes visibility,
    /// and updates topics for `self.selected_repo`.
    fn handle_edit_repo(
        &mut self,
        name: String,
        description: String,
        homepage: String,
        topics: Vec<String>,
        private: bool,
        has_issues: bool,
        has_wiki: bool,
        has_projects: bool,
        has_discussions: bool,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(repo) = self.selected_repo.as_ref() else {
            return;
        };

        let mut changes = json!({
            "name": name,
            "description": description,
            "homepage": homepage,
            "has_issues": has_issues,
            "has_wiki": has_wiki,
            "has_projects": has_projects,
            "has_discussions": has_discussions,
        });
        // Only sent when the switch was actually flipped: a visibility change
        // is the one consequential field here, so an unrelated edit (or an
        // `internal` repo, which also reports `private: true`) must not
        // restate it.
        if private != repo.private {
            changes["private"] = json!(private);
        }

        self.run_action(HelmAction::EditRepo { changes, topics }, true, cx);
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

    /// Opens the "Edit profile" modal, prefilled from `self.user`.
    fn open_edit_profile_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(user) = self.user.clone() else {
            return;
        };
        self.open_workspace_modal(HelmModalKind::EditProfile(user), window, cx);
    }

    /// Updates the signed-in user's profile. GitHub's `/user` PATCH endpoint
    /// needs the `user` OAuth scope, which `gh_login` requests up front
    /// (`repo,read:org,user`, see cli.rs). A login made outside Helm may not
    /// have it; in that case `run_action` sends this through the auth gate
    /// for `user`, where the device code is shown, and re-sends it.
    fn handle_update_profile(
        &mut self,
        name: String,
        bio: String,
        company: String,
        location: String,
        blog: String,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let changes = json!({
            "name": name,
            "bio": bio,
            "company": company,
            "location": location,
            "blog": blog,
        });
        self.run_action(HelmAction::UpdateProfile { changes }, true, cx);
    }

    /// Accepts a pending repo invitation and drops it from the list + badge.
    fn handle_accept_invitation(&mut self, id: u64, _window: &mut Window, cx: &mut Context<Self>) {
        self.run_action(HelmAction::AcceptRepoInvitation(id), true, cx);
    }

    /// Declines a pending repo invitation and drops it from the list + badge.
    fn handle_decline_invitation(&mut self, id: u64, _window: &mut Window, cx: &mut Context<Self>) {
        self.run_action(HelmAction::DeclineRepoInvitation(id), true, cx);
    }

    /// Accepts a pending org invitation and drops it from the list + badge.
    fn handle_accept_org_invitation(
        &mut self,
        org: String,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.run_action(HelmAction::AcceptOrgInvitation(org), true, cx);
    }

    /// Declines a pending org invitation and drops it from the list + badge.
    fn handle_decline_org_invitation(
        &mut self,
        org: String,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.run_action(HelmAction::DeclineOrgInvitation(org), true, cx);
    }

    /// Sets `login`'s permission on `self.selected_repo` — GitHub's
    /// add-collaborator endpoint doubles as the update-permission endpoint,
    /// so this is also how an existing collaborator's role is changed —
    /// then refreshes the list.
    fn handle_set_collaborator_permission(
        &mut self,
        login: String,
        permission: String,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.run_action(
            HelmAction::SetCollaboratorPermission { login, permission },
            true,
            cx,
        );
    }

    /// Removes `login` as a collaborator on `self.selected_repo`, then
    /// refreshes the list.
    fn handle_remove_collaborator(
        &mut self,
        login: String,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.run_action(HelmAction::RemoveCollaborator(login), true, cx);
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

        let len = rows.len();
        let cursor = self.profile_menu_cursor;
        let row_ids: Vec<&'static str> = rows.iter().map(|row| row.id).collect();
        v_flex()
            .child(stats_row)
            .child(div().h_px().w_full().bg(border))
            .child(
                v_flex()
                    .id("helm-profile-menu")
                    .track_focus(&self.profile_menu_focus)
                    .on_mouse_down(MouseButton::Left, cx.listener(|this, _, window, cx| {
                        window.focus(&this.profile_menu_focus, cx);
                    }))
                    .key_context("HelmRowList")
                    .on_action(cx.listener(move |this, _: &SelectNextRow, _, cx| {
                        this.profile_menu_cursor = step_selected(this.profile_menu_cursor, len, true);
                        cx.notify();
                    }))
                    .on_action(cx.listener(move |this, _: &SelectPrevRow, _, cx| {
                        this.profile_menu_cursor = step_selected(this.profile_menu_cursor, len, false);
                        cx.notify();
                    }))
                    .on_action(cx.listener(move |this, _: &OpenSelectedRow, window, cx| {
                        let Some(&id) = this.profile_menu_cursor.and_then(|ix| row_ids.get(ix)) else {
                            return;
                        };
                        match id {
                            "orgs" => this.navigate_to(HelmScreen::OrgList, cx),
                            "repos" => this.open_repo_list(cx),
                            "invitations" => this.navigate_to(HelmScreen::Invitations, cx),
                            "edit-profile" => this.open_edit_profile_dialog(window, cx),
                            "account-security" => {
                                cx.open_url("https://github.com/settings/security")
                            }
                            _ => {}
                        }
                    }))
                    .py_1()
                    .children(rows.into_iter().enumerate().map(|(ix, row)| {
                        let hint = row.hint;
                        let id = row.id;
                        ListItem::new(format!("helm-profile-{}", row.id))
                            .selected(cursor == Some(ix))
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
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.profile_menu_cursor = Some(ix);
                                match id {
                                    "orgs" => this.navigate_to(HelmScreen::OrgList, cx),
                                    "repos" => this.open_repo_list(cx),
                                    "invitations" => this.navigate_to(HelmScreen::Invitations, cx),
                                    "edit-profile" => this.open_edit_profile_dialog(window, cx),
                                    // No per-account security API is wired up
                                    // (Dependabot/secret-scanning alerts,
                                    // elsewhere in this panel, are
                                    // repo-scoped, not account-scoped) —
                                    // opens GitHub's own settings page
                                    // instead of a dead click.
                                    "account-security" => {
                                        cx.open_url("https://github.com/settings/security")
                                    }
                                    _ => {}
                                }
                            }))
                    })),
            )
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

        let len = self.org_logins.len();
        let cursor = self.org_list_cursor;
        v_flex()
            .id("helm-org-list")
            .track_focus(&self.org_list_focus)
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, window, cx| {
                window.focus(&this.org_list_focus, cx);
            }))
            .key_context("HelmRowList")
            .on_action(cx.listener(move |this, _: &SelectNextRow, _, cx| {
                this.org_list_cursor = step_selected(this.org_list_cursor, len, true);
                cx.notify();
            }))
            .on_action(cx.listener(move |this, _: &SelectPrevRow, _, cx| {
                this.org_list_cursor = step_selected(this.org_list_cursor, len, false);
                cx.notify();
            }))
            .on_action(cx.listener(move |this, _: &OpenSelectedRow, _, cx| {
                let Some(org) = this.org_list_cursor.and_then(|ix| this.org_logins.get(ix)).cloned()
                else {
                    return;
                };
                this.select_org(org, cx);
            }))
            .py_1()
            .children(self.org_logins.clone().into_iter().enumerate().map(|(ix, org)| {
                let click_org = org.clone();
                ListItem::new(format!("helm-org-{org}"))
                    .selected(cursor == Some(ix))
                    .child(div().text_color(foreground).child(org))
                    .suffix(move |_, _| {
                        Icon::new(IconName::ChevronRight)
                            .xsmall()
                            .text_color(muted_foreground)
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.org_list_cursor = Some(ix);
                        this.select_org(click_org.clone(), cx)
                    }))
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
            let len = filtered.len();
            let cursor = self.repo_list_cursor;
            // Captured for `OpenSelectedRow` below rather than re-reading
            // `self.repos`: `repo_list_cursor` indexes this filtered order
            // (per its own doc comment), and re-filtering `self.repos` by a
            // *stale* `self.repo_search` value inside the action handler —
            // run on a later keypress, against whatever the search box says
            // *then* — would disagree with what's actually on screen now.
            let filtered_for_open = filtered.clone();
            v_flex()
                .id("helm-repo-list")
                .track_focus(&self.repo_list_focus)
                .on_mouse_down(MouseButton::Left, cx.listener(|this, _, window, cx| {
                    window.focus(&this.repo_list_focus, cx);
                }))
                .key_context("HelmRowList")
                .on_action(cx.listener(move |this, _: &SelectNextRow, _, cx| {
                    this.repo_list_cursor = step_selected(this.repo_list_cursor, len, true);
                    cx.notify();
                }))
                .on_action(cx.listener(move |this, _: &SelectPrevRow, _, cx| {
                    this.repo_list_cursor = step_selected(this.repo_list_cursor, len, false);
                    cx.notify();
                }))
                .on_action(cx.listener(move |this, _: &OpenSelectedRow, _, cx| {
                    let Some(repo) =
                        this.repo_list_cursor.and_then(|ix| filtered_for_open.get(ix)).cloned()
                    else {
                        return;
                    };
                    this.select_repo(repo, cx);
                }))
                .py_1()
                .children(filtered.into_iter().enumerate().map(|(ix, repo)| {
                    let vis = repo_vis_label(&repo);
                    let click_repo = repo.clone();
                    ListItem::new(format!("helm-repo-{}", repo.id))
                        .selected(cursor == Some(ix))
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
                            this.repo_list_cursor = Some(ix);
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
        let collab_len = self.collaborators.len();
        let collab_cursor = self.collaborators_list_cursor;

        v_flex()
            .child(header)
            .child(div().h_px().w_full().bg(border))
            .child(
                v_flex()
                    .id("helm-collaborators-list")
                    .track_focus(&self.collaborators_list_focus)
                    .on_mouse_down(MouseButton::Left, cx.listener(|this, _, window, cx| {
                        window.focus(&this.collaborators_list_focus, cx);
                    }))
                    .key_context("HelmRowList")
                    .on_action(cx.listener(move |this, _: &SelectNextRow, _, cx| {
                        this.collaborators_list_cursor =
                            step_selected(this.collaborators_list_cursor, collab_len, true);
                        cx.notify();
                    }))
                    .on_action(cx.listener(move |this, _: &SelectPrevRow, _, cx| {
                        this.collaborators_list_cursor =
                            step_selected(this.collaborators_list_cursor, collab_len, false);
                        cx.notify();
                    }))
                    .on_action(cx.listener(move |this, _: &OpenSelectedRow, _, cx| {
                        let Some(login) = this
                            .collaborators_list_cursor
                            .and_then(|ix| this.collaborators.get(ix))
                            .map(|c| c.login.clone())
                        else {
                            return;
                        };
                        this.open_user_profile(login, cx);
                    }))
                    .py_1()
                    .children(self.collaborators.iter().enumerate().map(|(ix, collab)| {
                        let login = collab.login.clone();
                        let role_name = collab.role_name.clone();

                        ListItem::new(format!("helm-collaborator-{}", collab.id))
                            .selected(collab_cursor == Some(ix))
                            .child(
                        h_flex()
                            .items_center()
                            .gap_2()
                            .child(
                                Avatar::new()
                                    .src(collab.avatar_url.clone())
                                    .name(login.clone())
                                    .with_size(px(48.)),
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
                        this.collaborators_list_cursor = Some(ix);
                        this.open_user_profile(login.clone(), cx);
                    }))
                    }))
            )
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

        // Org invitations are listed before repo invitations with no visual
        // separator between them, so `invitations_list_cursor` indexes this
        // one combined, display-order list rather than either `Vec` alone.
        enum Invite {
            Org(String),
            Repo(u64),
        }
        let combined: Vec<Invite> = self
            .org_invitations
            .iter()
            .map(|inv| Invite::Org(inv.organization.login.clone()))
            .chain(self.invitations.iter().map(|inv| Invite::Repo(inv.id)))
            .collect();
        let combined_len = combined.len();
        let cursor = self.invitations_list_cursor;

        let org_rows = self.org_invitations.iter().enumerate().map(|(ix, inv)| {
            let org_login = inv.organization.login.clone();
            let role = inv.role.clone();
            let org_accept = org_login.clone();
            let org_decline = org_login.clone();
            let view_accept = view.clone();
            let view_decline = view.clone();

            ListItem::new(format!("helm-org-invitation-{}", inv.organization.login))
                .selected(cursor == Some(ix))
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

        let org_count = self.org_invitations.len();
        let combined_for_open = combined;
        let combined_for_act: Vec<(bool, String)> = self
            .org_invitations
            .iter()
            .map(|inv| (true, inv.organization.login.clone()))
            .chain(self.invitations.iter().map(|inv| (false, inv.id.to_string())))
            .collect();

        v_flex()
            .id("helm-invitations-list")
            .track_focus(&self.invitations_list_focus)
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, window, cx| {
                window.focus(&this.invitations_list_focus, cx);
            }))
            .key_context("HelmRowList")
            .on_action(cx.listener(move |this, _: &SelectNextRow, _, cx| {
                this.invitations_list_cursor =
                    step_selected(this.invitations_list_cursor, combined_len, true);
                cx.notify();
            }))
            .on_action(cx.listener(move |this, _: &SelectPrevRow, _, cx| {
                this.invitations_list_cursor =
                    step_selected(this.invitations_list_cursor, combined_len, false);
                cx.notify();
            }))
            // Enter accepts the selected row's invitation...
            .on_action(cx.listener(move |this, _: &OpenSelectedRow, window, cx| {
                let Some(invite) = this.invitations_list_cursor.and_then(|ix| combined_for_open.get(ix))
                else {
                    return;
                };
                match invite {
                    Invite::Org(login) => this.handle_accept_org_invitation(login.clone(), window, cx),
                    Invite::Repo(id) => this.handle_accept_invitation(*id, window, cx),
                }
            }))
            // ...Space declines it — the Invitations screen's rows have no
            // separate "view" action for Enter to be the safe default of, so
            // Accept/Decline (its only two actions) split Enter/Space instead.
            .on_action(cx.listener(move |this, _: &ActSelectedRow, window, cx| {
                let Some((is_org, key)) =
                    this.invitations_list_cursor.and_then(|ix| combined_for_act.get(ix))
                else {
                    return;
                };
                if *is_org {
                    this.handle_decline_org_invitation(key.clone(), window, cx);
                } else if let Ok(id) = key.parse() {
                    this.handle_decline_invitation(id, window, cx);
                }
            }))
            .py_1()
            .children(org_rows)
            .children(self.invitations.iter().enumerate().map(|(ix, inv)| {
                let full_name = inv.repository.full_name.clone();
                let private = inv.repository.private;
                let inviter = inv.inviter.login.clone();
                let permissions = inv.permissions.clone();
                let id_accept = inv.id;
                let id_decline = inv.id;
                let view_accept = view.clone();
                let view_decline = view.clone();

                ListItem::new(format!("helm-invitation-{}", inv.id))
                    .selected(cursor == Some(org_count + ix))
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
