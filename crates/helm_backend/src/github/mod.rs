//! GitHub (Helm) backend — ported from `github.rs`.
//!
//! Split into single-purpose submodules:
//! - `mod`: the public surface
//! - `oauth`: signing in with the OAuth device flow
//! - `gh_cli`: importing a token from the GitHub CLI (optional sign-in)
//! - `api`: REST API fetch helpers + endpoint functions
//! - `requests`: what each endpoint asks for, as data (method, path, body)
//! - `cache`: remembered answers (`ETag`s) and the rate limit
//! - `paging`: one page of a list, and reading the `Link` header
//! - `error`: `GhError`, and turning an HTTP answer into a value or an error
//! - `clone`: `git clone` with the account's token, streaming its output

mod api;
mod cache;
mod clone;
mod error;
mod gh_cli;
mod oauth;
mod paging;
pub mod requests;
mod types;

pub use api::{
    compare_ref_movement, fetch_all, fetch_blob, fetch_blob_at_path, fetch_commit_detail,
    fetch_compare, fetch_page, fetch_page_under, fetch_readme, fetch_readme_at_path,
    fetch_repo_tree, fetch_tree_dir, peek_page, peek_page_under, resolve_ref, search_code,
    search_code_page, search_repositories, search_repositories_page,
};
pub use api::{
    gh_accept_org_invitation, gh_accept_repo_invitation, gh_add_collaborator, gh_create_pull,
    gh_create_release, gh_create_repo, gh_decline_org_invitation, gh_decline_repo_invitation,
    gh_get_branches, gh_get_collaborators, gh_get_combined_status, gh_get_current_user,
    gh_get_org_detail, gh_get_org_logins, gh_get_org_role, gh_get_repo, gh_get_repo_invitations, gh_get_repos,
    gh_get_traffic_clones, gh_get_traffic_paths, gh_get_traffic_referrers, gh_get_traffic_views,
    gh_get_user, gh_get_workflow_run, gh_get_workflow_run_jobs, gh_list_dependabot_alerts,
    gh_list_deployments, gh_list_issue_comments, gh_list_issues, gh_list_org_invitations,
    gh_list_package_versions, gh_list_packages, gh_list_pulls, gh_list_recent_commits,
    gh_list_releases, gh_list_secret_scanning_alerts, gh_list_tags, gh_list_workflow_runs,
    gh_remove_collaborator, gh_update_repo, gh_update_topics, gh_update_user,
};
pub use cache::{CachedResponse, MAX_CACHED_RESPONSES, RateLimit, ResponseCache};
pub use clone::{CloneCancelSignal, CloneCanceller, CloneFailure, clone_canceller, clone_repo};
pub use error::{GhError, RawResponse, interpret, interpret_empty, rate_limit_message};
pub use gh_cli::{GhCliAccount, gh_cli_accounts, gh_cli_token};
pub use oauth::{
    DeviceFlow, HELM_OAUTH_CLIENT_ID, HELM_OAUTH_SCOPES, PollOutcome, poll_for_token,
    request_device_code,
};
pub use paging::{Page, PageLinks, interpret_page, interpret_page_under, last_page, parse_link};

pub use types::{
    BlobData, BlobKind, Branch, CloneEvent, Collaborator, CombinedCommitStatus, Comment,
    CommitDetail, CommitFile, CommitSummary, CompareFile, CompareResult, Deployment,
    FileClassification, GhState, GitHubUser, GitHubUserDetail, Issue,
    MAX_CONCURRENT_REQUESTS, MAX_INLINE_IMAGE_BYTES, MAX_INLINE_TEXT_BYTES, OrgDetail,
    OrgInvitation, OrgMember, Package, PackageVersion, Pull, Readme, RefMovement, Release, Repo,
    RepoInvitation, RepoPermission, RepoPermissions, RepoRef, RepoTraffic, RepoTree, ResolvedRef,
    SearchCodeMatch, SearchCodeMatchFragment, SearchRepoResult, Tag, TreeEntry, TreeEntryKind,
    TreeLoadResult, TreeNode, WorkflowJob, WorkflowRun, build_tree, classify_file,
    classify_ref_movement,
};
