//! GitHub REST API commands (was part of `github.rs`).
//!
//! `send` resolves the token (cached or via `gh auth token`) and makes the
//! request; `error::interpret` turns the answer into a value or a
//! [`GhError`]. All endpoint functions are thin wrappers over those two.

use serde::de::DeserializeOwned;

use super::error::{GhError, RawResponse, interpret, interpret_empty};
use super::gh_cmd;
use super::requests::{self, ApiRequest};
use super::types::{
    Branch, Collaborator, Comment, CommitSummary, Deployment, GhState, GitHubUser,
    GitHubUserDetail, Issue, OrgDetail, OrgInvitation, Package, PackageVersion, Pull, Release,
    Repo, RepoInvitation, Tag, TrafficClones, TrafficPath, TrafficReferrer, TrafficViews,
    WorkflowJob, WorkflowRun,
};

/// The access token: the cached one, or a fresh one from `gh auth token`.
async fn token(state: &GhState) -> Result<String, GhError> {
    if let Some(token) = &*state.token.read().await {
        return Ok(token.clone());
    }
    match gh_cmd().arg("auth").arg("token").output().await {
        Ok(out) if out.status.success() => {
            let token = String::from_utf8_lossy(&out.stdout).trim().to_string();
            *state.token.write().await = Some(token.clone());
            Ok(token)
        }
        Ok(out) => Err(GhError::Cli(format!(
            "gh auth token failed: {}",
            String::from_utf8_lossy(&out.stderr)
        ))),
        Err(e) => Err(GhError::Cli(format!("Failed to run gh: {e}"))),
    }
}

/// Sends one request and returns what came back, whatever its status. This
/// is the only function in the crate that touches the network. What to ask
/// for is `requests.rs`; what the answer means is `error::interpret`.
async fn send(state: &GhState, request: ApiRequest) -> Result<RawResponse, GhError> {
    let token = token(state).await?;
    let base = state.base_url.read().await.clone();
    let url = format!("{base}{}", request.path);

    let mut builder = state
        .client
        .request(request.method.parse().unwrap_or(reqwest::Method::GET), &url)
        .header("Authorization", format!("Bearer {token}"))
        .header("Accept", "application/vnd.github.v3+json")
        .header("User-Agent", "mentenaz-forge");
    if let Some(body) = request.body {
        builder = builder.json(&body);
    }

    let response = builder
        .send()
        .await
        .map_err(|e| GhError::Network(e.to_string()))?;
    let number = |name: &str| -> Option<u64> {
        response
            .headers()
            .get(name)?
            .to_str()
            .ok()?
            .trim()
            .parse()
            .ok()
    };
    let status = response.status().as_u16();
    let rate_remaining = number("x-ratelimit-remaining");
    let rate_reset = number("x-ratelimit-reset");
    let retry_after = number("retry-after");
    let body = response
        .text()
        .await
        .map_err(|e| GhError::Network(e.to_string()))?;
    Ok(RawResponse {
        status,
        body,
        rate_remaining,
        rate_reset,
        retry_after,
    })
}

/// Sends a request and reads its answer as `T`.
async fn fetch<T: DeserializeOwned>(state: &GhState, request: ApiRequest) -> Result<T, GhError> {
    interpret(&send(state, request).await?)
}

/// Sends a request whose answer carries nothing worth reading. GitHub
/// replies 204 with an empty body to most deletes and some updates.
async fn perform(state: &GhState, request: ApiRequest) -> Result<(), GhError> {
    interpret_empty(&send(state, request).await?)
}

/// The list GitHub wraps in an object, as it does for workflow runs
/// (`{"total_count": 3, "workflow_runs": [...]}`). Empty when the key is
/// missing or holds something else.
fn list_under<T: DeserializeOwned>(answer: &serde_json::Value, key: &str) -> Vec<T> {
    answer
        .get(key)
        .and_then(|list| serde_json::from_value(list.clone()).ok())
        .unwrap_or_default()
}

/// The logins of the organisations the user is an active member of.
fn active_org_logins(memberships: &[serde_json::Value]) -> Vec<String> {
    memberships
        .iter()
        .filter(|membership| membership.get("state").and_then(|s| s.as_str()) == Some("active"))
        .filter_map(|membership| {
            membership
                .get("organization")
                .and_then(|organization| organization.get("login"))
                .and_then(|login| login.as_str())
                .map(str::to_string)
        })
        .collect()
}

/// The memberships the user has been offered and not yet answered.
fn pending_org_invitations(memberships: Vec<serde_json::Value>) -> Vec<OrgInvitation> {
    memberships
        .into_iter()
        .filter(|membership| membership.get("state").and_then(|s| s.as_str()) == Some("pending"))
        .filter_map(|membership| serde_json::from_value(membership).ok())
        .collect()
}

pub async fn gh_get_current_user(state: &GhState) -> Result<GitHubUser, GhError> {
    fetch(state, requests::current_user()).await
}

pub async fn gh_get_repos(owner: String, state: &GhState) -> Result<Vec<Repo>, GhError> {
    fetch(state, requests::repos(&owner)).await
}

pub async fn gh_get_repo(owner: String, name: String, state: &GhState) -> Result<Repo, GhError> {
    fetch(state, requests::repo(&owner, &name)).await
}

/// Recent commits with their GitHub author/committer info (login + avatar
/// URL), for mapping a local `git log` SHA to a real avatar in the Git Ops
/// History tab. One list call covers up to 100 commits, instead of hitting
/// `/repos/{owner}/{repo}/commits/{sha}` once per commit.
pub async fn gh_list_recent_commits(
    owner: String,
    name: String,
    state: &GhState,
) -> Result<Vec<CommitSummary>, GhError> {
    fetch(state, requests::recent_commits(&owner, &name)).await
}

pub async fn gh_create_repo(opts: serde_json::Value, state: &GhState) -> Result<Repo, GhError> {
    fetch(state, requests::create_repo(&opts)).await
}

pub async fn gh_get_branches(
    owner: String,
    name: String,
    state: &GhState,
) -> Result<Vec<Branch>, GhError> {
    fetch(state, requests::branches(&owner, &name)).await
}

pub async fn gh_update_repo(
    owner: String,
    name: String,
    changes: serde_json::Value,
    state: &GhState,
) -> Result<Repo, GhError> {
    fetch(state, requests::update_repo(&owner, &name, changes)).await
}

pub async fn gh_get_org_detail(org: String, state: &GhState) -> Result<OrgDetail, GhError> {
    fetch(state, requests::org_detail(&org)).await
}

pub async fn gh_get_collaborators(
    owner: String,
    name: String,
    state: &GhState,
) -> Result<Vec<Collaborator>, GhError> {
    fetch(state, requests::collaborators(&owner, &name)).await
}

pub async fn gh_get_org_logins(state: &GhState) -> Result<Vec<String>, GhError> {
    let memberships: Vec<serde_json::Value> = fetch(state, requests::org_memberships()).await?;
    Ok(active_org_logins(&memberships))
}

/// Same endpoint/shape as [`gh_get_org_logins`], filtered to `state ==
/// "pending"` instead of `"active"` — the memberships GitHub hasn't been
/// accepted or declined yet.
pub async fn gh_list_org_invitations(state: &GhState) -> Result<Vec<OrgInvitation>, GhError> {
    let memberships: Vec<serde_json::Value> = fetch(state, requests::org_memberships()).await?;
    Ok(pending_org_invitations(memberships))
}

pub async fn gh_get_user(username: String, state: &GhState) -> Result<GitHubUserDetail, GhError> {
    fetch(state, requests::user(&username)).await
}

pub async fn gh_list_workflow_runs(
    owner: String,
    repo: String,
    state: &GhState,
) -> Result<Vec<WorkflowRun>, GhError> {
    let answer: serde_json::Value = fetch(state, requests::workflow_runs(&owner, &repo)).await?;
    Ok(list_under(&answer, "workflow_runs"))
}

/// A single run's own current status/conclusion — used to refresh the
/// header of the run-detail screen on each poll tick, since `html_url` etc.
/// never change but `status`/`conclusion` do while it's running.
pub async fn gh_get_workflow_run(
    owner: String,
    repo: String,
    run_id: u64,
    state: &GhState,
) -> Result<WorkflowRun, GhError> {
    fetch(state, requests::workflow_run(&owner, &repo, run_id)).await
}

/// Per-job (and per-step within each job) status for a run — the detail
/// `gh_list_workflow_runs`/`WorkflowRun` doesn't carry at all. This is what
/// actually answers "what is it doing right now": each step's own
/// status/conclusion, not just the run as a whole.
pub async fn gh_get_workflow_run_jobs(
    owner: String,
    repo: String,
    run_id: u64,
    state: &GhState,
) -> Result<Vec<WorkflowJob>, GhError> {
    let answer: serde_json::Value =
        fetch(state, requests::workflow_run_jobs(&owner, &repo, run_id)).await?;
    Ok(list_under(&answer, "jobs"))
}

pub async fn gh_list_deployments(
    owner: String,
    repo: String,
    state: &GhState,
) -> Result<Vec<Deployment>, GhError> {
    fetch(state, requests::deployments(&owner, &repo)).await
}

pub async fn gh_list_issues(
    owner: String,
    repo: String,
    state_filter: String,
    state: &GhState,
) -> Result<Vec<Issue>, GhError> {
    fetch(state, requests::issues(&owner, &repo, &state_filter)).await
}

pub async fn gh_list_pulls(
    owner: String,
    repo: String,
    state_filter: String,
    state: &GhState,
) -> Result<Vec<Pull>, GhError> {
    fetch(state, requests::pulls(&owner, &repo, &state_filter)).await
}

/// Comments on an issue or PR (see [`Comment`]'s doc comment for why one
/// endpoint covers both) — the in-panel detail viewer's discussion thread.
/// `number` is the issue/PR number, same one already shown on its list row.
pub async fn gh_list_issue_comments(
    owner: String,
    repo: String,
    number: u64,
    state: &GhState,
) -> Result<Vec<Comment>, GhError> {
    fetch(state, requests::issue_comments(&owner, &repo, number)).await
}

pub async fn gh_list_releases(
    owner: String,
    repo: String,
    state: &GhState,
) -> Result<Vec<Release>, GhError> {
    fetch(state, requests::releases(&owner, &repo)).await
}

pub async fn gh_list_tags(
    owner: String,
    repo: String,
    state: &GhState,
) -> Result<Vec<Tag>, GhError> {
    fetch(state, requests::tags(&owner, &repo)).await
}

pub async fn gh_create_pull(
    owner: String,
    repo: String,
    head: String,
    base: String,
    title: String,
    body: Option<String>,
    state: &GhState,
) -> Result<serde_json::Value, GhError> {
    fetch(
        state,
        requests::create_pull(&owner, &repo, &head, &base, &title, body.as_deref()),
    )
    .await
}

pub async fn gh_create_release(
    owner: String,
    repo: String,
    tag_name: String,
    name: Option<String>,
    body: Option<String>,
    draft: Option<bool>,
    prerelease: Option<bool>,
    state: &GhState,
) -> Result<serde_json::Value, GhError> {
    fetch(
        state,
        requests::create_release(
            &owner,
            &repo,
            &tag_name,
            name.as_deref(),
            body.as_deref(),
            draft.unwrap_or(false),
            prerelease.unwrap_or(false),
        ),
    )
    .await
}

pub async fn gh_list_dependabot_alerts(
    owner: String,
    repo: String,
    state: &GhState,
) -> Result<Vec<serde_json::Value>, GhError> {
    fetch(state, requests::dependabot_alerts(&owner, &repo)).await
}

pub async fn gh_list_secret_scanning_alerts(
    owner: String,
    repo: String,
    state: &GhState,
) -> Result<Vec<serde_json::Value>, GhError> {
    fetch(state, requests::secret_scanning_alerts(&owner, &repo)).await
}

pub async fn gh_get_traffic_views(
    owner: String,
    repo: String,
    state: &GhState,
) -> Result<TrafficViews, GhError> {
    fetch(state, requests::traffic_views(&owner, &repo)).await
}

pub async fn gh_get_traffic_clones(
    owner: String,
    repo: String,
    state: &GhState,
) -> Result<TrafficClones, GhError> {
    fetch(state, requests::traffic_clones(&owner, &repo)).await
}

pub async fn gh_get_traffic_referrers(
    owner: String,
    repo: String,
    state: &GhState,
) -> Result<Vec<TrafficReferrer>, GhError> {
    fetch(state, requests::traffic_referrers(&owner, &repo)).await
}

pub async fn gh_get_traffic_paths(
    owner: String,
    repo: String,
    state: &GhState,
) -> Result<Vec<TrafficPath>, GhError> {
    fetch(state, requests::traffic_paths(&owner, &repo)).await
}

pub async fn gh_list_packages(owner: String, state: &GhState) -> Result<Vec<Package>, GhError> {
    // The API only accepts one `package_type` per request, so run one call
    // per type and merge. The org path is tried first (org-owner repos), the
    // user path as fallback (same login, individual account).
    let mut combined: Vec<Package> = Vec::new();
    let mut last_err: Option<GhError> = None;
    for package_type in requests::PACKAGE_TYPES {
        let from_org = fetch::<Vec<Package>>(state, requests::org_packages(&owner, package_type));
        let result = match from_org.await {
            Ok(packages) => Ok(packages),
            Err(org_err) => {
                match fetch::<Vec<Package>>(state, requests::user_packages(&owner, package_type))
                    .await
                {
                    Ok(packages) => Ok(packages),
                    Err(_) => Err(org_err),
                }
            }
        };
        match result {
            Ok(packages) => combined.extend(packages),
            Err(e) => last_err = Some(e),
        }
    }
    if combined.is_empty() {
        if let Some(e) = last_err {
            return Err(e);
        }
    }
    Ok(combined)
}

pub async fn gh_list_package_versions(
    owner: String,
    pkg_type: String,
    pkg_name: String,
    state: &GhState,
) -> Result<Vec<PackageVersion>, GhError> {
    // The versions endpoint lives under a different prefix depending on who
    // owns the package (org, the authenticated user, or another user) — try
    // each in turn, first success wins.
    let mut last_err = None;
    for request in requests::package_versions(&owner, &pkg_type, &pkg_name) {
        match fetch::<Vec<PackageVersion>>(state, request).await {
            Ok(versions) => return Ok(versions),
            Err(e) => last_err = Some(e),
        }
    }
    // There are always three places to try, so there is a last error.
    Err(last_err.unwrap_or(GhError::NotFound {
        message: "no package versions found".to_string(),
    }))
}

pub async fn gh_update_user(
    changes: serde_json::Value,
    state: &GhState,
) -> Result<serde_json::Value, GhError> {
    fetch(state, requests::update_user(changes)).await
}

pub async fn gh_get_repo_invitations(state: &GhState) -> Result<Vec<RepoInvitation>, GhError> {
    fetch(state, requests::repo_invitations()).await
}

pub async fn gh_accept_repo_invitation(invitation_id: u64, state: &GhState) -> Result<(), GhError> {
    perform(state, requests::accept_repo_invitation(invitation_id)).await
}

pub async fn gh_decline_repo_invitation(invitation_id: u64, state: &GhState) -> Result<(), GhError> {
    perform(state, requests::decline_repo_invitation(invitation_id)).await
}

pub async fn gh_accept_org_invitation(org: String, state: &GhState) -> Result<(), GhError> {
    perform(state, requests::accept_org_invitation(&org)).await
}

pub async fn gh_decline_org_invitation(org: String, state: &GhState) -> Result<(), GhError> {
    perform(state, requests::decline_org_invitation(&org)).await
}

pub async fn gh_add_collaborator(
    owner: String,
    repo: String,
    username: String,
    permission: String,
    state: &GhState,
) -> Result<(), GhError> {
    perform(
        state,
        requests::add_collaborator(&owner, &repo, &username, &permission),
    )
    .await
}

pub async fn gh_remove_collaborator(
    owner: String,
    repo: String,
    username: String,
    state: &GhState,
) -> Result<(), GhError> {
    perform(state, requests::remove_collaborator(&owner, &repo, &username)).await
}

pub async fn gh_update_topics(
    owner: String,
    repo: String,
    topics: Vec<String>,
    state: &GhState,
) -> Result<serde_json::Value, GhError> {
    fetch(state, requests::update_topics(&owner, &repo, &topics)).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn memberships() -> Vec<serde_json::Value> {
        vec![
            json!({ "state": "active", "role": "admin", "organization": { "login": "mentenaz" } }),
            json!({ "state": "pending", "role": "member", "organization": { "login": "invited" } }),
            json!({ "state": "active", "role": "member", "organization": { "login": "other" } }),
            // Malformed entries are skipped, not fatal.
            json!({ "state": "active" }),
            json!({ "state": "pending" }),
        ]
    }

    #[test]
    fn active_memberships_are_the_users_organisations() {
        assert_eq!(active_org_logins(&memberships()), vec!["mentenaz", "other"]);
        assert!(active_org_logins(&[]).is_empty());
    }

    #[test]
    fn pending_memberships_are_invitations() {
        let invitations = pending_org_invitations(memberships());
        assert_eq!(invitations.len(), 1);
        assert_eq!(invitations[0].organization.login, "invited");
        assert_eq!(invitations[0].role, "member");
    }

    #[test]
    fn a_wrapped_list_is_unwrapped() {
        let answer = json!({ "total_count": 2, "items": [1, 2] });
        assert_eq!(list_under::<u32>(&answer, "items"), vec![1, 2]);
        // A missing key, or one holding something else, is an empty list.
        assert!(list_under::<u32>(&answer, "missing").is_empty());
        assert!(list_under::<u32>(&json!({ "items": "nope" }), "items").is_empty());
    }
}
