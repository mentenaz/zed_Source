//! GitHub (Helm) backend — ported from `github.rs`.
//!
//! Split into single-purpose submodules:
//! - `mod`: types, `GhState`, `gh_cmd` helper
//! - `cli`: gh CLI auth (login/logout/token/status/scopes)
//! - `api`: REST API fetch helpers + endpoint functions
//! - `clone`: `gh repo clone` with streaming (event-log) output

mod api;
mod cli;
mod clone;
mod types;

pub use api::{
    gh_accept_org_invitation, gh_accept_repo_invitation, gh_add_collaborator, gh_create_pull,
    gh_create_release, gh_create_repo, gh_decline_org_invitation, gh_decline_repo_invitation,
    gh_get_branches, gh_get_collaborators, gh_get_current_user, gh_get_org_detail,
    gh_get_org_logins, gh_get_repo_invitations, gh_get_repos, gh_get_traffic_clones,
    gh_get_traffic_paths, gh_get_traffic_referrers, gh_get_traffic_views, gh_get_user,
    gh_get_workflow_run, gh_get_workflow_run_jobs, gh_list_dependabot_alerts, gh_list_deployments,
    gh_list_issue_comments, gh_list_issues, gh_list_org_invitations, gh_list_package_versions,
    gh_list_packages, gh_list_pulls, gh_list_recent_commits, gh_list_releases,
    gh_list_secret_scanning_alerts, gh_list_tags, gh_list_workflow_runs, gh_remove_collaborator,
    gh_update_repo, gh_update_topics, gh_update_user,
};
pub use cli::{
    gh_auth_status, gh_check_cli, gh_ensure_scope, gh_login, gh_logout,
};
pub use clone::gh_clone_repo;

pub use types::{
    Branch, CloneEvent, Collaborator, Comment, CommitSummary, Deployment, GhAuthEvent, GhState,
    GitHubUser, GitHubUserDetail, Issue, OrgDetail, OrgInvitation, Package, PackageVersion, Pull,
    Release, Repo, RepoInvitation, RepoTraffic, Tag, WorkflowJob, WorkflowRun,
};

/// Returns a `gh` Command with CREATE_NO_WINDOW set on Windows so no external
/// console flashes for GitHub CLI calls.
pub fn gh_cmd() -> tokio::process::Command {
    let mut c = tokio::process::Command::new("gh");
    #[cfg(target_os = "windows")]
    c.creation_flags(0x0800_0000);
    c
}
