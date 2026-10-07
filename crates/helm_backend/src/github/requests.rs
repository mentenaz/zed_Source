//! What Helm asks GitHub for, as data.
//!
//! Each function here builds one [`ApiRequest`]: a method, a path and an
//! optional JSON body. Nothing is sent. `api.rs` sends them, and the tests
//! below check them, which is how the endpoints are covered without a
//! network.
//!
//! Every value placed in a path or a query string goes through [`seg`]
//! first, so a name containing `/`, `?` or `#` cannot change which endpoint
//! is called. That matters for more than safety: a container package is
//! commonly named `owner/image`, and its `/` has to be sent as `%2F`.

use serde_json::{Value, json};

#[derive(Clone, Debug, PartialEq)]
pub struct ApiRequest {
    pub method: &'static str,
    /// Path and query, starting with `/`. The base URL is added on sending.
    pub path: String,
    pub body: Option<Value>,
}

impl ApiRequest {
    /// The same request for page `page` (counting from 1) at `per_page`
    /// items a page, replacing any paging the request already asked for.
    pub fn page(mut self, page: u32, per_page: u32) -> ApiRequest {
        let (path, query) = match self.path.split_once('?') {
            Some((path, query)) => (path.to_string(), query.to_string()),
            None => (self.path.clone(), String::new()),
        };
        let mut params: Vec<String> = query
            .split('&')
            .filter(|param| !param.is_empty())
            .filter(|param| !param.starts_with("per_page=") && !param.starts_with("page="))
            .map(str::to_string)
            .collect();
        params.push(format!("per_page={per_page}"));
        params.push(format!("page={}", page.max(1)));
        self.path = format!("{path}?{}", params.join("&"));
        self
    }
}

fn get(path: String) -> ApiRequest {
    ApiRequest {
        method: "GET",
        path,
        body: None,
    }
}

fn delete(path: String) -> ApiRequest {
    ApiRequest {
        method: "DELETE",
        path,
        body: None,
    }
}

fn with_body(method: &'static str, path: String, body: Value) -> ApiRequest {
    ApiRequest {
        method,
        path,
        body: Some(body),
    }
}

/// One value, made safe to place in a path or a query string.
fn seg(value: &str) -> String {
    urlencoding::encode(value).into_owned()
}

/// `/repos/{owner}/{name}` followed by `rest`.
fn repo_path(owner: &str, name: &str, rest: &str) -> String {
    format!("/repos/{}/{}{rest}", seg(owner), seg(name))
}

// ── Account ────────────────────────────────────────────────────────────

pub fn current_user() -> ApiRequest {
    get("/user".to_string())
}

pub fn update_user(changes: Value) -> ApiRequest {
    with_body("PATCH", "/user".to_string(), changes)
}

pub fn user(username: &str) -> ApiRequest {
    get(format!("/users/{}", seg(username)))
}

/// The signed-in user's organisation memberships, active and pending.
pub fn org_memberships() -> ApiRequest {
    get("/user/memberships/orgs?per_page=100".to_string())
}

pub fn org_detail(org: &str) -> ApiRequest {
    get(format!("/orgs/{}", seg(org)))
}

// ── Repositories ───────────────────────────────────────────────────────

/// The repositories of `owner`. `"self"` means the signed-in user's own.
pub fn repos(owner: &str) -> ApiRequest {
    if owner == "self" {
        get("/user/repos?per_page=100&sort=updated&affiliation=owner".to_string())
    } else {
        get(format!("/orgs/{}/repos?per_page=100&sort=updated", seg(owner)))
    }
}

pub fn repo(owner: &str, name: &str) -> ApiRequest {
    get(repo_path(owner, name, ""))
}

/// Creates a repository from `opts`. An `owner` in `opts` names the
/// organisation to create it in, and is not sent as part of the body;
/// without one the repository is the signed-in user's.
pub fn create_repo(opts: &Value) -> ApiRequest {
    match opts.get("owner").and_then(|owner| owner.as_str()) {
        Some(org) => {
            let mut body = opts.clone();
            if let Some(fields) = body.as_object_mut() {
                fields.remove("owner");
            }
            with_body("POST", format!("/orgs/{}/repos", seg(org)), body)
        }
        None => with_body("POST", "/user/repos".to_string(), opts.clone()),
    }
}

pub fn update_repo(owner: &str, name: &str, changes: Value) -> ApiRequest {
    with_body("PATCH", repo_path(owner, name, ""), changes)
}

pub fn update_topics(owner: &str, name: &str, topics: &[String]) -> ApiRequest {
    with_body(
        "PUT",
        repo_path(owner, name, "/topics"),
        json!({ "names": topics }),
    )
}

pub fn branches(owner: &str, name: &str) -> ApiRequest {
    get(repo_path(owner, name, "/branches?per_page=100"))
}

pub fn tags(owner: &str, name: &str) -> ApiRequest {
    get(repo_path(owner, name, "/tags?per_page=100"))
}

pub fn recent_commits(owner: &str, name: &str) -> ApiRequest {
    get(repo_path(owner, name, "/commits?per_page=100"))
}

// ── Collaboration ──────────────────────────────────────────────────────

pub fn issues(owner: &str, name: &str, state: &str) -> ApiRequest {
    get(repo_path(
        owner,
        name,
        &format!(
            "/issues?state={}&per_page=100&sort=updated&direction=desc",
            seg(state)
        ),
    ))
}

pub fn pulls(owner: &str, name: &str, state: &str) -> ApiRequest {
    get(repo_path(
        owner,
        name,
        &format!(
            "/pulls?state={}&per_page=100&sort=updated&direction=desc",
            seg(state)
        ),
    ))
}

/// The conversation on an issue or a pull request; GitHub serves both from
/// the issues endpoint.
pub fn issue_comments(owner: &str, name: &str, number: u64) -> ApiRequest {
    get(repo_path(
        owner,
        name,
        &format!("/issues/{number}/comments?per_page=100"),
    ))
}

pub fn create_pull(
    owner: &str,
    name: &str,
    head: &str,
    base: &str,
    title: &str,
    body: Option<&str>,
) -> ApiRequest {
    with_body(
        "POST",
        repo_path(owner, name, "/pulls"),
        json!({ "title": title, "head": head, "base": base, "body": body }),
    )
}

pub fn collaborators(owner: &str, name: &str) -> ApiRequest {
    get(repo_path(owner, name, "/collaborators?per_page=100"))
}

pub fn add_collaborator(owner: &str, name: &str, username: &str, permission: &str) -> ApiRequest {
    with_body(
        "PUT",
        repo_path(owner, name, &format!("/collaborators/{}", seg(username))),
        json!({ "permission": permission }),
    )
}

pub fn remove_collaborator(owner: &str, name: &str, username: &str) -> ApiRequest {
    delete(repo_path(
        owner,
        name,
        &format!("/collaborators/{}", seg(username)),
    ))
}

// ── Invitations ────────────────────────────────────────────────────────

pub fn repo_invitations() -> ApiRequest {
    get("/user/repository_invitations?per_page=100".to_string())
}

pub fn accept_repo_invitation(invitation_id: u64) -> ApiRequest {
    ApiRequest {
        method: "PATCH",
        path: format!("/user/repository_invitations/{invitation_id}"),
        body: None,
    }
}

pub fn decline_repo_invitation(invitation_id: u64) -> ApiRequest {
    delete(format!("/user/repository_invitations/{invitation_id}"))
}

pub fn accept_org_invitation(org: &str) -> ApiRequest {
    with_body(
        "PATCH",
        format!("/user/memberships/orgs/{}", seg(org)),
        json!({ "state": "active" }),
    )
}

pub fn decline_org_invitation(org: &str) -> ApiRequest {
    delete(format!("/user/memberships/orgs/{}", seg(org)))
}

// ── Releases and packages ──────────────────────────────────────────────

pub fn releases(owner: &str, name: &str) -> ApiRequest {
    get(repo_path(owner, name, "/releases?per_page=50"))
}

pub fn create_release(
    owner: &str,
    name: &str,
    tag_name: &str,
    title: Option<&str>,
    body: Option<&str>,
    draft: bool,
    prerelease: bool,
) -> ApiRequest {
    with_body(
        "POST",
        repo_path(owner, name, "/releases"),
        json!({
            "tag_name": tag_name,
            "name": title,
            "body": body,
            "draft": draft,
            "prerelease": prerelease,
        }),
    )
}

/// The package types GitHub Packages has. Its API lists one type per
/// request.
pub const PACKAGE_TYPES: [&str; 6] = ["container", "npm", "maven", "rubygems", "nuget", "pip"];

pub fn org_packages(org: &str, package_type: &str) -> ApiRequest {
    get(format!(
        "/orgs/{}/packages?per_page=100&package_type={}",
        seg(org),
        seg(package_type)
    ))
}

pub fn user_packages(user: &str, package_type: &str) -> ApiRequest {
    get(format!(
        "/users/{}/packages?per_page=100&package_type={}",
        seg(user),
        seg(package_type)
    ))
}

/// The three places a package's versions can live, depending on who owns
/// it: an organisation, the signed-in user, or another user. In the order
/// they are tried.
pub fn package_versions(owner: &str, package_type: &str, package: &str) -> [ApiRequest; 3] {
    let tail = format!(
        "/packages/{}/{}/versions?per_page=100",
        seg(package_type),
        seg(package)
    );
    [
        get(format!("/orgs/{}{tail}", seg(owner))),
        get(format!("/user{tail}")),
        get(format!("/users/{}{tail}", seg(owner))),
    ]
}

// ── Actions and deployments ────────────────────────────────────────────

pub fn workflow_runs(owner: &str, name: &str) -> ApiRequest {
    get(repo_path(owner, name, "/actions/runs?per_page=50&page=1"))
}

pub fn workflow_run(owner: &str, name: &str, run_id: u64) -> ApiRequest {
    get(repo_path(owner, name, &format!("/actions/runs/{run_id}")))
}

pub fn workflow_run_jobs(owner: &str, name: &str, run_id: u64) -> ApiRequest {
    get(repo_path(
        owner,
        name,
        &format!("/actions/runs/{run_id}/jobs?per_page=100"),
    ))
}

pub fn deployments(owner: &str, name: &str) -> ApiRequest {
    get(repo_path(owner, name, "/deployments?per_page=50"))
}

// ── Insights ───────────────────────────────────────────────────────────

pub fn traffic_views(owner: &str, name: &str) -> ApiRequest {
    get(repo_path(owner, name, "/traffic/views"))
}

pub fn traffic_clones(owner: &str, name: &str) -> ApiRequest {
    get(repo_path(owner, name, "/traffic/clones"))
}

pub fn traffic_referrers(owner: &str, name: &str) -> ApiRequest {
    get(repo_path(owner, name, "/traffic/popular/referrers"))
}

pub fn traffic_paths(owner: &str, name: &str) -> ApiRequest {
    get(repo_path(owner, name, "/traffic/popular/paths"))
}

pub fn dependabot_alerts(owner: &str, name: &str) -> ApiRequest {
    get(repo_path(owner, name, "/dependabot/alerts?per_page=100"))
}

pub fn secret_scanning_alerts(owner: &str, name: &str) -> ApiRequest {
    get(repo_path(
        owner,
        name,
        "/secret-scanning/alerts?per_page=100",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(request: &ApiRequest) -> String {
        format!("{} {}", request.method, request.path)
    }

    #[test]
    fn a_request_can_be_asked_for_one_page() {
        // Paging already in the request is replaced, other parameters kept.
        assert_eq!(
            line(&issues("o", "r", "open").page(3, 10)),
            "GET /repos/o/r/issues?state=open&sort=updated&direction=desc&per_page=10&page=3"
        );
        assert_eq!(
            line(&workflow_runs("o", "r").page(2, 10)),
            "GET /repos/o/r/actions/runs?per_page=10&page=2"
        );
        // A request with no query string gets one.
        assert_eq!(line(&repo("o", "r").page(1, 10)), "GET /repos/o/r?per_page=10&page=1");
        // Pages count from 1.
        assert_eq!(line(&tags("o", "r").page(0, 10)), "GET /repos/o/r/tags?per_page=10&page=1");
        // The body and method are untouched.
        let paged = update_user(json!({ "bio": "x" })).page(2, 5);
        assert_eq!(paged.method, "PATCH");
        assert_eq!(paged.body, Some(json!({ "bio": "x" })));
    }

    #[test]
    fn account_requests() {
        assert_eq!(line(&current_user()), "GET /user");
        assert_eq!(line(&user("octocat")), "GET /users/octocat");
        assert_eq!(line(&org_memberships()), "GET /user/memberships/orgs?per_page=100");
        assert_eq!(line(&org_detail("mentenaz")), "GET /orgs/mentenaz");

        let update = update_user(json!({ "bio": "hello" }));
        assert_eq!(line(&update), "PATCH /user");
        assert_eq!(update.body, Some(json!({ "bio": "hello" })));
    }

    #[test]
    fn your_own_repositories_and_an_organisations_are_different_endpoints() {
        assert_eq!(
            line(&repos("self")),
            "GET /user/repos?per_page=100&sort=updated&affiliation=owner"
        );
        assert_eq!(
            line(&repos("mentenaz")),
            "GET /orgs/mentenaz/repos?per_page=100&sort=updated"
        );
    }

    #[test]
    fn repository_requests() {
        assert_eq!(line(&repo("o", "r")), "GET /repos/o/r");
        assert_eq!(line(&branches("o", "r")), "GET /repos/o/r/branches?per_page=100");
        assert_eq!(line(&tags("o", "r")), "GET /repos/o/r/tags?per_page=100");
        assert_eq!(line(&recent_commits("o", "r")), "GET /repos/o/r/commits?per_page=100");

        let update = update_repo("o", "r", json!({ "description": "d" }));
        assert_eq!(line(&update), "PATCH /repos/o/r");
        assert_eq!(update.body, Some(json!({ "description": "d" })));

        let topics = update_topics("o", "r", &["rust".to_string(), "gpui".to_string()]);
        assert_eq!(line(&topics), "PUT /repos/o/r/topics");
        assert_eq!(topics.body, Some(json!({ "names": ["rust", "gpui"] })));
    }

    #[test]
    fn creating_a_repository_goes_to_the_owner_named_in_the_options() {
        let mine = create_repo(&json!({ "name": "new", "private": true }));
        assert_eq!(line(&mine), "POST /user/repos");
        assert_eq!(mine.body, Some(json!({ "name": "new", "private": true })));

        // `owner` picks the organisation and is not part of what is sent.
        let orgs = create_repo(&json!({ "name": "new", "owner": "mentenaz" }));
        assert_eq!(line(&orgs), "POST /orgs/mentenaz/repos");
        assert_eq!(orgs.body, Some(json!({ "name": "new" })));
    }

    #[test]
    fn collaboration_requests() {
        assert_eq!(
            line(&issues("o", "r", "open")),
            "GET /repos/o/r/issues?state=open&per_page=100&sort=updated&direction=desc"
        );
        assert_eq!(
            line(&pulls("o", "r", "all")),
            "GET /repos/o/r/pulls?state=all&per_page=100&sort=updated&direction=desc"
        );
        assert_eq!(
            line(&issue_comments("o", "r", 42)),
            "GET /repos/o/r/issues/42/comments?per_page=100"
        );
        assert_eq!(
            line(&collaborators("o", "r")),
            "GET /repos/o/r/collaborators?per_page=100"
        );

        let pull = create_pull("o", "r", "feature", "main", "Add it", None);
        assert_eq!(line(&pull), "POST /repos/o/r/pulls");
        assert_eq!(
            pull.body,
            Some(json!({ "title": "Add it", "head": "feature", "base": "main", "body": null }))
        );

        let add = add_collaborator("o", "r", "octocat", "push");
        assert_eq!(line(&add), "PUT /repos/o/r/collaborators/octocat");
        assert_eq!(add.body, Some(json!({ "permission": "push" })));

        let remove = remove_collaborator("o", "r", "octocat");
        assert_eq!(line(&remove), "DELETE /repos/o/r/collaborators/octocat");
        assert_eq!(remove.body, None);
    }

    #[test]
    fn invitation_requests() {
        assert_eq!(
            line(&repo_invitations()),
            "GET /user/repository_invitations?per_page=100"
        );
        assert_eq!(
            line(&accept_repo_invitation(7)),
            "PATCH /user/repository_invitations/7"
        );
        assert_eq!(
            line(&decline_repo_invitation(7)),
            "DELETE /user/repository_invitations/7"
        );

        let accept = accept_org_invitation("mentenaz");
        assert_eq!(line(&accept), "PATCH /user/memberships/orgs/mentenaz");
        assert_eq!(accept.body, Some(json!({ "state": "active" })));
        assert_eq!(
            line(&decline_org_invitation("mentenaz")),
            "DELETE /user/memberships/orgs/mentenaz"
        );
    }

    #[test]
    fn release_requests() {
        assert_eq!(line(&releases("o", "r")), "GET /repos/o/r/releases?per_page=50");

        let release = create_release("o", "r", "v1.0.0", Some("One"), None, false, true);
        assert_eq!(line(&release), "POST /repos/o/r/releases");
        assert_eq!(
            release.body,
            Some(json!({
                "tag_name": "v1.0.0",
                "name": "One",
                "body": null,
                "draft": false,
                "prerelease": true,
            }))
        );
    }

    #[test]
    fn package_requests() {
        assert_eq!(
            line(&org_packages("mentenaz", "npm")),
            "GET /orgs/mentenaz/packages?per_page=100&package_type=npm"
        );
        assert_eq!(
            line(&user_packages("octocat", "container")),
            "GET /users/octocat/packages?per_page=100&package_type=container"
        );

        let versions = package_versions("mentenaz", "npm", "left-pad");
        assert_eq!(
            versions.iter().map(line).collect::<Vec<_>>(),
            vec![
                "GET /orgs/mentenaz/packages/npm/left-pad/versions?per_page=100",
                "GET /user/packages/npm/left-pad/versions?per_page=100",
                "GET /users/mentenaz/packages/npm/left-pad/versions?per_page=100",
            ]
        );
    }

    #[test]
    fn actions_and_insight_requests() {
        assert_eq!(
            line(&workflow_runs("o", "r")),
            "GET /repos/o/r/actions/runs?per_page=50&page=1"
        );
        assert_eq!(line(&workflow_run("o", "r", 9)), "GET /repos/o/r/actions/runs/9");
        assert_eq!(
            line(&workflow_run_jobs("o", "r", 9)),
            "GET /repos/o/r/actions/runs/9/jobs?per_page=100"
        );
        assert_eq!(line(&deployments("o", "r")), "GET /repos/o/r/deployments?per_page=50");
        assert_eq!(line(&traffic_views("o", "r")), "GET /repos/o/r/traffic/views");
        assert_eq!(line(&traffic_clones("o", "r")), "GET /repos/o/r/traffic/clones");
        assert_eq!(
            line(&traffic_referrers("o", "r")),
            "GET /repos/o/r/traffic/popular/referrers"
        );
        assert_eq!(line(&traffic_paths("o", "r")), "GET /repos/o/r/traffic/popular/paths");
        assert_eq!(
            line(&dependabot_alerts("o", "r")),
            "GET /repos/o/r/dependabot/alerts?per_page=100"
        );
        assert_eq!(
            line(&secret_scanning_alerts("o", "r")),
            "GET /repos/o/r/secret-scanning/alerts?per_page=100"
        );
    }

    #[test]
    fn a_name_cannot_change_which_endpoint_is_called() {
        // A value with path or query syntax in it stays one segment.
        assert_eq!(line(&user("a/b?c=d#e")), "GET /users/a%2Fb%3Fc%3Dd%23e");
        assert_eq!(line(&repo("o", "../../user")), "GET /repos/o/..%2F..%2Fuser");
        assert_eq!(
            line(&issues("o", "r", "open&per_page=1")),
            "GET /repos/o/r/issues?state=open%26per_page%3D1&per_page=100&sort=updated&direction=desc"
        );
        assert_eq!(
            line(&remove_collaborator("o", "r", "x/../../y")),
            "DELETE /repos/o/r/collaborators/x%2F..%2F..%2Fy"
        );
        // Ordinary names are untouched.
        assert_eq!(line(&repo("mentenaz", "zed_Source")), "GET /repos/mentenaz/zed_Source");
        assert_eq!(line(&repo("o", "Forge.Scaffold.SDK")), "GET /repos/o/Forge.Scaffold.SDK");
    }

    #[test]
    fn a_container_package_keeps_its_slash_as_one_segment() {
        // Container images are commonly named `owner/image`.
        let versions = package_versions("mentenaz", "container", "forge/runner");
        assert_eq!(
            line(&versions[0]),
            "GET /orgs/mentenaz/packages/container/forge%2Frunner/versions?per_page=100"
        );
    }
}
