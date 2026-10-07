//! Asks GitHub who you are, using the `gh` CLI's existing sign-in — a way to
//! try the backend against the real API from a terminal, with no editor
//! involved. Read-only: it changes nothing on GitHub.
//!
//!     cargo run -p helm_backend --example whoami

use std::sync::Arc;

use helm_backend::github::{
    GhError, GhState, Page, fetch_page, gh_get_current_user, gh_get_org_logins, gh_get_repo,
    gh_get_repos, requests,
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
