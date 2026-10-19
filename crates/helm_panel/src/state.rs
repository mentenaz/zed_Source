//! The Helm panel's small state types: which screen is showing, whether a
//! load is in flight, and the rows of its menus.

use super::*;

/// Which screen the panel is currently showing.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum HelmScreen {
    Auth,
    Menu,
    Profile,
    OrgList,
    OrgDetail,
    /// The members of the organisation in view, reached from `OrgDetail`.
    OrgMembers,
    RepoList,
    GitHubSearch,
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

/// What checking the stored sign-in found.
pub(super) enum AuthOutcome {
    /// No token is stored for the account.
    NotLoggedIn,
    /// GitHub no longer accepts the stored token (401). It has been deleted.
    Revoked,
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

/// What a sign-in is for. It decides which scopes are asked for and which
/// account the new token must belong to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum SignInFor {
    /// Any account. Whichever one signs in becomes this panel's.
    AnyAccount,
    /// Signing in again as this login, whose token is missing or revoked.
    Account(String),
    /// Adding `scope` to this login's token.
    Scope { login: String, scope: &'static str },
}

impl SignInFor {
    /// The login the new token must belong to, if any.
    pub(super) fn expected_login(&self) -> Option<&str> {
        match self {
            SignInFor::AnyAccount => None,
            SignInFor::Account(login) | SignInFor::Scope { login, .. } => Some(login),
        }
    }
}

/// A sign-in in progress on the auth screen.
pub(super) enum SignIn {
    Idle,
    /// Asking GitHub for a one-time code.
    Starting,
    /// The code is on screen; GitHub is asked every few seconds whether the
    /// user has entered it.
    Waiting(DeviceFlow),
    /// GitHub handed out a token: finding out whose it is, and storing it.
    Finishing,
    Failed(String),
    /// The token belongs to `actual`, not to the `expected` account the
    /// sign-in was for. Nothing has been stored.
    WrongAccount {
        expected: String,
        actual: String,
        token: String,
    },
}

impl SignIn {
    pub(super) fn is_idle(&self) -> bool {
        matches!(self, SignIn::Idle)
    }
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
