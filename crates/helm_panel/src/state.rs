//! The Helm panel's small state types: which screen is showing, whether a
//! load is in flight, and the rows of its menus.

use super::*;

/// Which screen the panel is currently showing.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum HelmScreen {
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
    /// The run list; clicking a row opens that run's live job-status flow
    /// graph as its own workspace tab — see [`HelmPanel::select_workflow_run`].
    WorkflowRuns,
    Deployments,
    Tags,
    Security,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum LoadState {
    Idle,
    Loading,
    Error,
}

/// Result of re-checking `gh auth status`, mirroring the old TS store's
/// `doAuth`.
pub(super) enum AuthOutcome {
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
pub(super) struct MenuItem {
    pub(super) id: &'static str,
    pub(super) label: &'static str,
    pub(super) danger: bool,
}

/// One nav row on the Profile screen.
pub(super) struct NavRow {
    pub(super) id: &'static str,
    pub(super) label: &'static str,
    pub(super) hint: Option<String>,
}
