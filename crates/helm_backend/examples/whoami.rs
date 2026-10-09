//! Asks GitHub who you are, using the `gh` CLI's existing sign-in — a way to
//! try the backend against the real API from a terminal, with no editor
//! involved. Read-only: it changes nothing on GitHub.
//!
//!     cargo run -p helm_backend --example whoami

use std::sync::Arc;

use helm_backend::github::{
    GhError, GhState, Page, TreeLoadResult, fetch_page, fetch_readme, fetch_repo_tree,
    gh_get_current_user, gh_get_org_logins, gh_get_repo, gh_get_repos, requests,
};

fn main() -> Result<(), String> {
    let http = reqwest_client::ReqwestClient::user_agent("helm-backend-example")
        .map_err(|error| error.to_string())?;
    let state = GhState::new(Arc::new(http));

    reqwest_client::runtime()
        .block_on(async {
            let user = gh_get_current_user(&state).await?;
            println!("signed in as {}", user.login);

            let orgs = gh_get_org_logins(&state).await?;
            println!("{} organisation(s)", orgs.len());

            let repos = gh_get_repos("self".to_string(), &state).await?;
            println!("{} repositories in all", repos.len());

            // One page of ten, to see paging against a real list.
            if let Some(repo) = repos.first() {
                let branch = if repo.default_branch.is_empty() {
                    return Err(GhError::Other(format!(
                        "{} has no default branch to read",
                        repo.full_name
                    )));
                } else {
                    repo.default_branch.as_str()
                };
                println!("{} tree at {branch}:", repo.full_name);
                match fetch_repo_tree(&state, &repo.owner.login, &repo.name, branch).await? {
                    TreeLoadResult::EmptyRepository => {
                        println!("  (empty repository; no README or tree to show)");
                    }
                    TreeLoadResult::Tree(tree) => {
                        for entry in tree.entries.iter().take(20) {
                            println!("  {:?} {}", entry.kind, entry.path);
                        }
                        if tree.entries.len() > 20 {
                            println!("  ... and {} more entries", tree.entries.len() - 20);
                        }

                        match fetch_readme(&state, &repo.owner.login, &repo.name, branch).await {
                            Ok(readme) => {
                                println!("{} ({}):", readme.name, readme.path);
                                for line in readme.content.lines().take(12) {
                                    println!("  {line}");
                                }
                                if readme.content.lines().count() > 12 {
                                    println!("  ...");
                                }
                            }
                            Err(GhError::NotFound { .. }) => {
                                println!("README is not available on this ref");
                            }
                            Err(error) => return Err(error),
                        }
                    }
                }

                let request = requests::recent_commits(&repo.owner.login, &repo.name);
                let commits: Page<serde_json::Value> = fetch_page(&state, request, 1, 10).await?;
                println!(
                    "{}: {} commits on page {} of {}",
                    repo.name,
                    commits.items.len(),
                    commits.page,
                    commits.last_page
                );
            }

            // The same question twice: the second answer comes from memory
            // (GitHub replies "not modified"), so it costs nothing.
            let before = state.rate_limit().map(|rate| rate.remaining);
            gh_get_current_user(&state).await?;
            let after = state.rate_limit().map(|rate| rate.remaining);
            println!("asking again: requests left {before:?} -> {after:?}");
            if let Some(rate) = state.rate_limit() {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_or(0, |elapsed| elapsed.as_secs());
                println!("{}", rate.summary(now));
            }

            // A request that must fail, to see an error arrive as its type.
            let missing = "this-repository-does-not-exist-helm-backend".to_string();
            match gh_get_repo(user.login.clone(), missing, &state).await {
                Ok(_) => println!("unexpectedly found a repository that should not exist"),
                Err(error) => println!(
                    "a missing repository: {error} (permission-like: {})",
                    error.is_permission()
                ),
            }
            Ok::<_, GhError>(())
        })
        .map_err(|error| error.to_string())
}
