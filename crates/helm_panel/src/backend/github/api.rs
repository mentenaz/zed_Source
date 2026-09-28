//! GitHub REST API commands (was part of `github.rs`).
//!
//! `gh_api_fetch` resolves the token (cached or via `gh auth token`) and hits
//! the REST endpoint, returning deserialized data. All endpoint functions are
//! thin wrappers over it.

use serde::de::DeserializeOwned;

use super::gh_cmd;
use super::types::{
    Branch, Collaborator, Comment, CommitSummary, Deployment, GhState, GitHubUser,
    GitHubUserDetail, Issue, OrgDetail, OrgInvitation, Package, PackageVersion, Pull, Release,
    Repo, RepoInvitation, Tag, TrafficClones, TrafficPath, TrafficReferrer, TrafficViews,
    WorkflowRun,
};

async fn gh_api_fetch<T: DeserializeOwned>(
    state: &GhState,
    path: &str,
    method: &str,
    body: Option<serde_json::Value>,
) -> Result<T, String> {
    let token = {
        let read_guard = state.token.read().await;
        if let Some(t) = &*read_guard {
            t.clone()
        } else {
            drop(read_guard);
            match gh_cmd().arg("auth").arg("token").output().await {
                Ok(out) if out.status.success() => {
                    let token = String::from_utf8_lossy(&out.stdout).trim().to_string();
                    *state.token.write().await = Some(token.clone());
                    token
                }
                Ok(out) => {
                    let err = String::from_utf8_lossy(&out.stderr);
                    return Err(format!("gh auth token failed: {}", err));
                }
                Err(e) => return Err(format!("Failed to run gh: {}", e)),
            }
        }
    };

    let base = state.base_url.read().await.clone();
    let url = format!("{}{}", base, path);

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| format!("Failed to build HTTP client: {}", e))?;

    let mut req = client
        .request(method.parse().unwrap_or(reqwest::Method::GET), &url)
        .header("Authorization", format!("Bearer {}", token))
        .header("Accept", "application/vnd.github.v3+json")
        .header("User-Agent", "mentenaz-forge");

    if let Some(b) = body {
        req = req.json(&b);
    }

    let res = req
        .send()
        .await
        .map_err(|e| format!("Network error: {}", e))?;
    let status = res.status();
    if !status.is_success() {
        let txt = res.text().await.unwrap_or_else(|_| "<no body>".to_string());
        return Err(format!("GitHub API {}: {}", status.as_u16(), txt));
    }
    res.json::<T>()
        .await
        .map_err(|e| format!("Failed to parse JSON: {}", e))
}

/// No-content variant that ignores an empty (200/201/204) response body.
async fn gh_api_no_content(
    state: &GhState,
    path: &str,
    method: &str,
    body: Option<serde_json::Value>,
) -> Result<(), String> {
    let _: serde_json::Value = gh_api_fetch(state, path, method, body).await?;
    Ok(())
}

pub async fn gh_get_current_user(state: &GhState) -> Result<GitHubUser, String> {
    gh_api_fetch(state, "/user", "GET", None).await
}

pub async fn gh_get_repos(owner: String, state: &GhState) -> Result<Vec<Repo>, String> {
    let path = if owner == "self" {
        "/user/repos?per_page=100&sort=updated&affiliation=owner".to_string()
    } else {
        format!("/orgs/{}/repos?per_page=100&sort=updated", owner)
    };
    gh_api_fetch(state, &path, "GET", None).await
}

pub async fn gh_get_repo(owner: String, name: String, state: &GhState) -> Result<Repo, String> {
    gh_api_fetch(state, &format!("/repos/{}/{}", owner, name), "GET", None).await
}

/// Recent commits with their GitHub author/committer info (login + avatar
/// URL), for mapping a local `git log` SHA to a real avatar in the Git Ops
/// History tab. One list call covers up to 100 commits, instead of hitting
/// `/repos/{owner}/{repo}/commits/{sha}` once per commit.
pub async fn gh_list_recent_commits(
    owner: String,
    name: String,
    state: &GhState,
) -> Result<Vec<CommitSummary>, String> {
    gh_api_fetch(
        state,
        &format!("/repos/{}/{}/commits?per_page=100", owner, name),
        "GET",
        None,
    )
    .await
}

pub async fn gh_create_repo(opts: serde_json::Value, state: &GhState) -> Result<Repo, String> {
    let (path, body) = if let Some(org) = opts.get("owner").and_then(|o| o.as_str()) {
        let path = format!("/orgs/{}/repos", org);
        let mut b = opts.clone();
        if let Some(obj) = b.as_object_mut() {
            obj.remove("owner");
        }
        (path, b)
    } else {
        ("/user/repos".to_string(), opts.clone())
    };
    gh_api_fetch(state, &path, "POST", Some(body)).await
}

pub async fn gh_get_branches(
    owner: String,
    name: String,
    state: &GhState,
) -> Result<Vec<Branch>, String> {
    let path = format!("/repos/{}/{}/branches?per_page=100", owner, name);
    gh_api_fetch(state, &path, "GET", None).await
}

pub async fn gh_update_repo(
    owner: String,
    name: String,
    changes: serde_json::Value,
    state: &GhState,
) -> Result<Repo, String> {
    let path = format!("/repos/{}/{}", owner, name);
    gh_api_fetch(state, &path, "PATCH", Some(changes)).await
}

pub async fn gh_get_org_detail(org: String, state: &GhState) -> Result<OrgDetail, String> {
    gh_api_fetch(state, &format!("/orgs/{}", org), "GET", None).await
}

pub async fn gh_get_collaborators(
    owner: String,
    name: String,
    state: &GhState,
) -> Result<Vec<Collaborator>, String> {
    gh_api_fetch(
        state,
        &format!("/repos/{}/{}/collaborators?per_page=100", owner, name),
        "GET",
        None,
    )
    .await
}

pub async fn gh_get_org_logins(state: &GhState) -> Result<Vec<String>, String> {
    let memberships: Vec<serde_json::Value> =
        gh_api_fetch(state, "/user/memberships/orgs?per_page=100", "GET", None).await?;
    let mut out = Vec::new();
    for m in memberships {
        if m.get("state").and_then(|s| s.as_str()) == Some("active") {
            if let Some(org) = m
                .get("organization")
                .and_then(|o| o.get("login"))
                .and_then(|l| l.as_str())
            {
                out.push(org.to_string());
            }
        }
    }
    Ok(out)
}

/// Same endpoint/shape as [`gh_get_org_logins`], filtered to `state ==
/// "pending"` instead of `"active"` — the memberships GitHub hasn't been
/// accepted or declined yet.
pub async fn gh_list_org_invitations(state: &GhState) -> Result<Vec<OrgInvitation>, String> {
    let memberships: Vec<serde_json::Value> =
        gh_api_fetch(state, "/user/memberships/orgs?per_page=100", "GET", None).await?;
    let mut out = Vec::new();
    for m in memberships {
        if m.get("state").and_then(|s| s.as_str()) == Some("pending") {
            if let Ok(invitation) = serde_json::from_value::<OrgInvitation>(m) {
                out.push(invitation);
            }
        }
    }
    Ok(out)
}

pub async fn gh_get_user(username: String, state: &GhState) -> Result<GitHubUserDetail, String> {
    let path = format!("/users/{}", username);
    gh_api_fetch(state, &path, "GET", None).await
}

pub async fn gh_list_workflow_runs(
    owner: String,
    repo: String,
    state: &GhState,
) -> Result<Vec<WorkflowRun>, String> {
    let resp: serde_json::Value = gh_api_fetch(
        state,
        &format!("/repos/{}/{}/actions/runs?per_page=50&page=1", owner, repo),
        "GET",
        None,
    )
    .await?;
    let runs = resp
        .get("workflow_runs")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();
    Ok(runs)
}

pub async fn gh_list_deployments(
    owner: String,
    repo: String,
    state: &GhState,
) -> Result<Vec<Deployment>, String> {
    gh_api_fetch(
        state,
        &format!("/repos/{}/{}/deployments?per_page=50", owner, repo),
        "GET",
        None,
    )
    .await
}

pub async fn gh_list_issues(
    owner: String,
    repo: String,
    state_filter: String,
    state: &GhState,
) -> Result<Vec<Issue>, String> {
    gh_api_fetch(
        state,
        &format!(
            "/repos/{}/{}/issues?state={}&per_page=100&sort=updated&direction=desc",
            owner, repo, state_filter
        ),
        "GET",
        None,
    )
    .await
}

pub async fn gh_list_pulls(
    owner: String,
    repo: String,
    state_filter: String,
    state: &GhState,
) -> Result<Vec<Pull>, String> {
    gh_api_fetch(
        state,
        &format!(
            "/repos/{}/{}/pulls?state={}&per_page=100&sort=updated&direction=desc",
            owner, repo, state_filter
        ),
        "GET",
        None,
    )
    .await
}

/// Comments on an issue or PR (see [`Comment`]'s doc comment for why one
/// endpoint covers both) — the in-panel detail viewer's discussion thread.
/// `number` is the issue/PR number, same one already shown on its list row.
pub async fn gh_list_issue_comments(
    owner: String,
    repo: String,
    number: u64,
    state: &GhState,
) -> Result<Vec<Comment>, String> {
    gh_api_fetch(
        state,
        &format!(
            "/repos/{}/{}/issues/{}/comments?per_page=100",
            owner, repo, number
        ),
        "GET",
        None,
    )
    .await
}

pub async fn gh_list_releases(
    owner: String,
    repo: String,
    state: &GhState,
) -> Result<Vec<Release>, String> {
    gh_api_fetch(
        state,
        &format!("/repos/{}/{}/releases?per_page=50", owner, repo),
        "GET",
        None,
    )
    .await
}

pub async fn gh_list_tags(
    owner: String,
    repo: String,
    state: &GhState,
) -> Result<Vec<Tag>, String> {
    gh_api_fetch(
        state,
        &format!("/repos/{}/{}/tags?per_page=100", owner, repo),
        "GET",
        None,
    )
    .await
}

pub async fn gh_create_pull(
    owner: String,
    repo: String,
    head: String,
    base: String,
    title: String,
    body: Option<String>,
    state: &GhState,
) -> Result<serde_json::Value, String> {
    let payload = serde_json::json!({
        "title": title,
        "head": head,
        "base": base,
        "body": body,
    });
    gh_api_fetch(
        state,
        &format!("/repos/{}/{}/pulls", owner, repo),
        "POST",
        Some(payload),
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
) -> Result<serde_json::Value, String> {
    let payload = serde_json::json!({
        "tag_name": tag_name,
        "name": name,
        "body": body,
        "draft": draft.unwrap_or(false),
        "prerelease": prerelease.unwrap_or(false),
    });
    gh_api_fetch(
        state,
        &format!("/repos/{}/{}/releases", owner, repo),
        "POST",
        Some(payload),
    )
    .await
}

pub async fn gh_list_dependabot_alerts(
    owner: String,
    repo: String,
    state: &GhState,
) -> Result<Vec<serde_json::Value>, String> {
    gh_api_fetch(
        state,
        &format!("/repos/{}/{}/dependabot/alerts?per_page=100", owner, repo),
        "GET",
        None,
    )
    .await
}

pub async fn gh_list_secret_scanning_alerts(
    owner: String,
    repo: String,
    state: &GhState,
) -> Result<Vec<serde_json::Value>, String> {
    gh_api_fetch(
        state,
        &format!(
            "/repos/{}/{}/secret-scanning/alerts?per_page=100",
            owner, repo
        ),
        "GET",
        None,
    )
    .await
}

pub async fn gh_get_traffic_views(
    owner: String,
    repo: String,
    state: &GhState,
) -> Result<TrafficViews, String> {
    gh_api_fetch(
        state,
        &format!("/repos/{}/{}/traffic/views", owner, repo),
        "GET",
        None,
    )
    .await
}

pub async fn gh_get_traffic_clones(
    owner: String,
    repo: String,
    state: &GhState,
) -> Result<TrafficClones, String> {
    gh_api_fetch(
        state,
        &format!("/repos/{}/{}/traffic/clones", owner, repo),
        "GET",
        None,
    )
    .await
}

pub async fn gh_get_traffic_referrers(
    owner: String,
    repo: String,
    state: &GhState,
) -> Result<Vec<TrafficReferrer>, String> {
    gh_api_fetch(
        state,
        &format!("/repos/{}/{}/traffic/popular/referrers", owner, repo),
        "GET",
        None,
    )
    .await
}

pub async fn gh_get_traffic_paths(
    owner: String,
    repo: String,
    state: &GhState,
) -> Result<Vec<TrafficPath>, String> {
    gh_api_fetch(
        state,
        &format!("/repos/{}/{}/traffic/popular/paths", owner, repo),
        "GET",
        None,
    )
    .await
}

pub async fn gh_list_packages(owner: String, state: &GhState) -> Result<Vec<Package>, String> {
    // The API only accepts one `package_type` per request, so run one call
    // per type and merge. The org path is tried first (org-owner repos), the
    // user path as fallback (same login, individual account).
    let types = ["container", "npm", "maven", "rubygems", "nuget", "pip"];
    let mut combined: Vec<Package> = Vec::new();
    let mut last_err: Option<String> = None;
    for pkg_type in types {
        let org_path = format!(
            "/orgs/{}/packages?per_page=100&package_type={}",
            owner, pkg_type
        );
        let user_path = format!(
            "/users/{}/packages?per_page=100&package_type={}",
            owner, pkg_type
        );
        let result = match gh_api_fetch::<Vec<Package>>(state, &org_path, "GET", None).await {
            Ok(pkgs) => Ok(pkgs),
            Err(org_err) => {
                match gh_api_fetch::<Vec<Package>>(state, &user_path, "GET", None).await {
                    Ok(pkgs) => Ok(pkgs),
                    Err(_) => Err(org_err),
                }
            }
        };
        match result {
            Ok(pkgs) => combined.extend(pkgs),
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
) -> Result<Vec<PackageVersion>, String> {
    // The versions endpoint lives under a different prefix depending on who
    // owns the package (org, the authenticated user, or another user) — try
    // each in turn, first success wins.
    let paths = [
        format!(
            "/orgs/{}/packages/{}/{}/versions?per_page=100",
            owner, pkg_type, pkg_name
        ),
        format!(
            "/user/packages/{}/{}/versions?per_page=100",
            pkg_type, pkg_name
        ),
        format!(
            "/users/{}/packages/{}/{}/versions?per_page=100",
            owner, pkg_type, pkg_name
        ),
    ];
    let mut last_err = String::new();
    for path in paths {
        match gh_api_fetch::<Vec<PackageVersion>>(state, &path, "GET", None).await {
            Ok(versions) => return Ok(versions),
            Err(e) => last_err = e,
        }
    }
    Err(last_err)
}

pub async fn gh_update_user(
    changes: serde_json::Value,
    state: &GhState,
) -> Result<serde_json::Value, String> {
    gh_api_fetch(state, "/user", "PATCH", Some(changes)).await
}

pub async fn gh_get_repo_invitations(state: &GhState) -> Result<Vec<RepoInvitation>, String> {
    gh_api_fetch(
        state,
        "/user/repository_invitations?per_page=100",
        "GET",
        None,
    )
    .await
}

pub async fn gh_accept_repo_invitation(invitation_id: u64, state: &GhState) -> Result<(), String> {
    gh_api_no_content(
        state,
        &format!("/user/repository_invitations/{}", invitation_id),
        "PATCH",
        None,
    )
    .await
}

pub async fn gh_decline_repo_invitation(invitation_id: u64, state: &GhState) -> Result<(), String> {
    gh_api_no_content(
        state,
        &format!("/user/repository_invitations/{}", invitation_id),
        "DELETE",
        None,
    )
    .await
}

pub async fn gh_accept_org_invitation(org: String, state: &GhState) -> Result<(), String> {
    let body = serde_json::json!({ "state": "active" });
    gh_api_no_content(
        state,
        &format!("/user/memberships/orgs/{}", org),
        "PATCH",
        Some(body),
    )
    .await
}

pub async fn gh_decline_org_invitation(org: String, state: &GhState) -> Result<(), String> {
    gh_api_no_content(
        state,
        &format!("/user/memberships/orgs/{}", org),
        "DELETE",
        None,
    )
    .await
}

pub async fn gh_add_collaborator(
    owner: String,
    repo: String,
    username: String,
    permission: String,
    state: &GhState,
) -> Result<(), String> {
    let body = serde_json::json!({ "permission": permission });
    gh_api_no_content(
        state,
        &format!("/repos/{}/{}/collaborators/{}", owner, repo, username),
        "PUT",
        Some(body),
    )
    .await
}

pub async fn gh_remove_collaborator(
    owner: String,
    repo: String,
    username: String,
    state: &GhState,
) -> Result<(), String> {
    gh_api_no_content(
        state,
        &format!("/repos/{}/{}/collaborators/{}", owner, repo, username),
        "DELETE",
        None,
    )
    .await
}

pub async fn gh_update_topics(
    owner: String,
    repo: String,
    topics: Vec<String>,
    state: &GhState,
) -> Result<serde_json::Value, String> {
    let body = serde_json::json!({ "names": topics });
    gh_api_fetch(
        state,
        &format!("/repos/{}/{}/topics", owner, repo),
        "PUT",
        Some(body),
    )
    .await
}
