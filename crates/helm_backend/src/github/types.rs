//! GitHub data types and shared state.

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AuthInfo {
    pub account: String,
    pub scopes: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Owner {
    pub login: String,
    pub id: u64,
}

/// The `author`/`committer` sub-object on a `/repos/{owner}/{repo}/commits`
/// entry — only present when the commit's git author/committer email is
/// linked to a GitHub account (`null` otherwise, e.g. commits from an email
/// with no matching account).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CommitAuthorInfo {
    pub login: String,
    pub avatar_url: String,
}

/// One entry from `/repos/{owner}/{repo}/commits` — just enough to map a
/// commit SHA to its GitHub author's avatar for the History tab; the actual
/// commit message/diff/etc. is read locally via `git log`, not this API.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CommitSummary {
    pub sha: String,
    pub author: Option<CommitAuthorInfo>,
    pub committer: Option<CommitAuthorInfo>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Repo {
    pub id: u64,
    pub name: String,
    pub full_name: String,
    pub owner: Owner,
    pub private: bool,
    pub description: Option<String>,
    pub clone_url: String,
    pub visibility: String,
    pub archived: bool,
    pub pushed_at: String,
    pub has_issues: bool,
    pub has_wiki: bool,
    pub has_projects: bool,
    #[serde(default)]
    pub has_discussions: bool,
    #[serde(default)]
    pub homepage: Option<String>,
    #[serde(default)]
    pub topics: Vec<String>,
    #[serde(default)]
    pub ssh_url: String,
    #[serde(default)]
    pub default_branch: String,
    #[serde(default)]
    pub language: Option<String>,
    #[serde(default)]
    pub stargazers_count: u64,
    #[serde(default)]
    pub forks_count: u64,
    #[serde(default)]
    pub open_issues_count: u64,
    #[serde(default)]
    pub updated_at: String,
    #[serde(default)]
    pub fork: bool,
    #[serde(default)]
    pub disabled: bool,
    pub permissions: Option<serde_json::Value>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Branch {
    pub name: String,
    #[serde(default)]
    pub protected: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RepoInvitation {
    pub id: u64,
    pub repository: RepoInvitationRepo,
    pub inviter: RepoInvitationInviter,
    pub permissions: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RepoInvitationRepo {
    pub full_name: String,
    #[serde(default)]
    pub private: bool,
    #[serde(default)]
    pub html_url: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RepoInvitationInviter {
    pub login: String,
    #[serde(default)]
    pub avatar_url: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GitHubUser {
    pub login: String,
    pub id: u64,
    pub name: Option<String>,
    pub bio: Option<String>,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub company: Option<String>,
    #[serde(default)]
    pub location: Option<String>,
    #[serde(default)]
    pub blog: Option<String>,
    #[serde(default)]
    pub twitter_username: Option<String>,
    pub avatar_url: String,
    pub public_repos: u64,
    pub total_private_repos: Option<u64>,
    pub followers: u64,
    pub following: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OrgDetail {
    pub login: String,
    pub name: Option<String>,
    pub description: Option<String>,
    pub avatar_url: String,
    pub blog: Option<String>,
    pub location: Option<String>,
    pub email: Option<String>,
    pub twitter_username: Option<String>,
    #[serde(default)]
    pub is_verified: bool,
    pub public_repos: u64,
    #[serde(default)]
    pub total_private_repos: Option<u64>,
    pub followers: u64,
    pub html_url: String,
    pub created_at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GitHubUserDetail {
    pub login: String,
    pub id: u64,
    pub avatar_url: String,
    pub html_url: String,
    pub name: Option<String>,
    pub company: Option<String>,
    pub blog: Option<String>,
    pub location: Option<String>,
    pub email: Option<String>,
    pub bio: Option<String>,
    pub twitter_username: Option<String>,
    pub public_repos: u64,
    pub followers: u64,
    pub following: u64,
    pub created_at: String,
    pub updated_at: String,
    #[serde(rename = "type")]
    pub user_type: String,
    #[serde(default)]
    pub hireable: Option<bool>,
    #[serde(default)]
    pub total_private_repos: Option<u64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Collaborator {
    pub login: String,
    pub id: u64,
    pub avatar_url: String,
    pub role_name: String,
    pub permissions: Option<serde_json::Value>,
}

/// Sub-object shared by issue/PR/release list items (`user`/`author`): just
/// enough login/avatar for a list row.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GhListUser {
    pub login: String,
    #[serde(default)]
    pub avatar_url: String,
}

/// One label on an issue/PR.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GhListLabel {
    pub name: String,
}

/// One item from `/repos/{owner}/{repo}/issues`. Issues and PRs share that
/// endpoint — a non-null `pull_request` marks an item as a PR, which the
/// Issues tab filters out (the PRs tab uses `/pulls` directly instead).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Issue {
    pub number: u64,
    pub title: String,
    pub state: String,
    #[serde(default)]
    pub html_url: String,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub comments: u64,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub updated_at: String,
    #[serde(default)]
    pub user: Option<GhListUser>,
    #[serde(default)]
    pub labels: Vec<GhListLabel>,
    #[serde(default)]
    pub pull_request: Option<serde_json::Value>,
}

/// `head`/`base` branch info on a pull request.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PullRef {
    #[serde(default)]
    pub label: String,
    #[serde(rename = "ref", default)]
    pub r#ref: String,
    #[serde(default)]
    pub sha: String,
}

/// One item from `/repos/{owner}/{repo}/pulls`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Pull {
    pub number: u64,
    pub title: String,
    pub state: String,
    #[serde(default)]
    pub html_url: String,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub merged: bool,
    #[serde(default)]
    pub merged_at: Option<String>,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub updated_at: String,
    #[serde(default)]
    pub user: Option<GhListUser>,
    #[serde(default)]
    pub head: PullRef,
    #[serde(default)]
    pub base: PullRef,
}

/// One comment on an issue or PR — `/repos/{owner}/{repo}/issues/{number}/comments`,
/// which (per GitHub's API) serves both: a PR's "conversation" comments live
/// on the same endpoint as an issue's, since every PR *is* an issue under
/// the hood. PR review comments (inline on a diff) are a separate endpoint
/// this doesn't cover.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Comment {
    #[serde(default)]
    pub id: u64,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub user: Option<GhListUser>,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub html_url: String,
}

/// One downloadable file attached to a release.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReleaseAsset {
    pub name: String,
    #[serde(default)]
    pub size: u64,
    #[serde(default)]
    pub download_count: u64,
    #[serde(default)]
    pub browser_download_url: String,
}

/// One item from `/repos/{owner}/{repo}/releases`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Release {
    pub tag_name: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub html_url: String,
    #[serde(default)]
    pub draft: bool,
    #[serde(default)]
    pub prerelease: bool,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub published_at: Option<String>,
    #[serde(default)]
    pub author: Option<GhListUser>,
    #[serde(default)]
    pub assets: Vec<ReleaseAsset>,
}

/// One row from `/users/{owner}/packages` or `/orgs/{owner}/packages`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Package {
    pub name: String,
    #[serde(rename = "package_type", default)]
    pub package_type: String,
    #[serde(default)]
    pub visibility: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub html_url: String,
}

/// One version of a package, from the package versions endpoint.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PackageVersion {
    pub id: u64,
    pub name: String,
    #[serde(default)]
    pub created_at: String,
}

/// One day of view/clone counts from the traffic endpoints.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TrafficDay {
    pub timestamp: String,
    pub count: u64,
    pub uniques: u64,
}

/// `/repos/{owner}/{repo}/traffic/views`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TrafficViews {
    pub count: u64,
    pub uniques: u64,
    #[serde(default)]
    pub views: Vec<TrafficDay>,
}

/// `/repos/{owner}/{repo}/traffic/clones`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TrafficClones {
    pub count: u64,
    pub uniques: u64,
    #[serde(default)]
    pub clones: Vec<TrafficDay>,
}

/// `/repos/{owner}/{repo}/traffic/popular/referrers`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TrafficReferrer {
    pub referrer: String,
    pub count: u64,
    pub uniques: u64,
}

/// `/repos/{owner}/{repo}/traffic/popular/paths`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TrafficPath {
    pub path: String,
    #[serde(default)]
    pub title: String,
    pub count: u64,
    pub uniques: u64,
}

/// Aggregated traffic snapshot for the Traffic tab: views/clones are `None`
/// when the repo has no traffic data yet (GitHub returns 202/404 for those
/// endpoints); referrers/paths default to empty.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct RepoTraffic {
    pub views: Option<TrafficViews>,
    pub clones: Option<TrafficClones>,
    pub referrers: Vec<TrafficReferrer>,
    pub paths: Vec<TrafficPath>,
}

/// One item from `/repos/{owner}/{repo}/deployments`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Deployment {
    pub id: u64,
    #[serde(default)]
    pub environment: String,
    #[serde(rename = "ref", default)]
    pub r#ref: String,
    #[serde(default)]
    pub sha: String,
    #[serde(default)]
    pub created_at: String,
    /// Not present on the deployment object itself (GitHub only exposes
    /// status via the separate deployment-statuses endpoint) — defaults to
    /// "unknown" rather than making a second call per deployment.
    #[serde(default = "default_deployment_status")]
    pub status: String,
}

fn default_deployment_status() -> String {
    "unknown".to_string()
}

/// One item from `/repos/{owner}/{repo}/tags`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Tag {
    pub name: String,
    #[serde(default)]
    pub commit: TagCommit,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TagCommit {
    #[serde(default)]
    pub sha: String,
}

/// A pending (not yet accepted) org membership, from
/// `/user/memberships/orgs` filtered to `state == "pending"`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OrgInvitation {
    pub role: String,
    pub organization: OrgInvitationOrg,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OrgInvitationOrg {
    pub login: String,
    #[serde(default)]
    pub avatar_url: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorkflowRun {
    pub id: u64,
    pub name: String,
    pub head_branch: Option<String>,
    pub head_sha: String,
    pub run_number: u64,
    pub event: String,
    pub status: String,
    pub conclusion: Option<String>,
    pub html_url: String,
    pub created_at: String,
    pub updated_at: String,
}

/// One step within a `WorkflowJob`, from the "List jobs for a workflow run"
/// endpoint's `steps[]` — this is the actual "which step is it on right
/// now" detail the plain run-list endpoint (`WorkflowRun` above) doesn't
/// carry at all.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorkflowStep {
    pub name: String,
    pub status: String,
    pub conclusion: Option<String>,
    pub number: u64,
}

/// One job within a workflow run (`GET .../actions/runs/{id}/jobs`) — e.g.
/// `build_windows`/`build_linux`/`build_macos` for `build_installers.yml`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WorkflowJob {
    pub id: u64,
    pub name: String,
    pub status: String,
    pub conclusion: Option<String>,
    pub html_url: String,
    #[serde(default)]
    pub steps: Vec<WorkflowStep>,
}

/// A line of `gh auth login`/`gh auth refresh` output, or its final result.
/// Replaces the Tauri `gh-auth-line`/`gh-auth-done` window events: panels
/// subscribe via `GhState::auth_tx.subscribe()` while a login is in flight.
/// `Done` carries no payload — the success/error result is returned directly
/// from `gh_login`/`gh_ensure_*_scope`'s own awaited call, which every
/// consumer already reads; this event only marks "stop listening for lines".
#[derive(Clone, Debug)]
pub enum GhAuthEvent {
    Line(String),
    Done,
}

/// A line of `gh repo clone`/`npm install` output, the npm phase starting, or
/// the whole clone's final result. Replaces the Tauri `gh-clone-line` window
/// event: panels subscribe via `GhState::clone_tx.subscribe()` while a clone
/// is in flight. `Done` carries no payload — see `GhAuthEvent::Done`.
#[derive(Clone, Debug)]
pub enum CloneEvent {
    Line(String),
    NpmStart,
    Done,
}

/// What the API functions need: the host's HTTP client, the cached token,
/// the API base URL, and the channels sign-in and clone report progress on.
/// Shared via `Arc`.
pub struct GhState {
    /// The host application's HTTP client. Requests go through it so that
    /// they use the same proxy settings as the rest of the app.
    pub http: std::sync::Arc<dyn http_client::HttpClient>,
    pub token: tokio::sync::RwLock<Option<String>>,
    pub base_url: tokio::sync::RwLock<String>,
    pub auth_tx: tokio::sync::broadcast::Sender<GhAuthEvent>,
    pub clone_tx: tokio::sync::broadcast::Sender<CloneEvent>,
}

impl GhState {
    pub fn new(http: std::sync::Arc<dyn http_client::HttpClient>) -> Self {
        let (auth_tx, _) = tokio::sync::broadcast::channel(16);
        let (clone_tx, _) = tokio::sync::broadcast::channel(256);
        Self {
            http,
            token: tokio::sync::RwLock::new(None),
            base_url: tokio::sync::RwLock::new("https://api.github.com".to_string()),
            auth_tx,
            clone_tx,
        }
    }
}
