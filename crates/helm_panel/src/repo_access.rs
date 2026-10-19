//! Whether the account a workspace uses can push to the workspace's GitHub
//! repository, and if not, which other signed-in account can (section 11,
//! item 1a of `helm_backend/octocrab-helm-coverage.md`).
//!
//! The repository comes from the remote the active git repository pushes
//! to (`origin`, or `upstream` when the current branch tracks it). The workspace's account is asked first; only if it cannot
//! push are the other signed-in accounts asked, each with its own token in
//! a state of its own, so nothing is switched. When one can push, a toast
//! offers to switch to it. The result is kept on the panel for the git
//! panel to read.

use std::collections::HashSet;

use helm_backend::github::{GitHubRemote, RepoPermission, gh_get_repo, parse_github_remote};
use project::git_store::{GitStore, GitStoreEvent};

use super::accounts::read_token;
use super::*;

/// What the account a workspace uses may do with its repository.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RepoAccess {
    pub remote: GitHubRemote,
    /// The account that was checked: the workspace's.
    pub login: String,
    /// Whether that account can see the repository at all.
    pub can_read: bool,
    /// Whether it can push. `None` when GitHub did not say (a token that is
    /// not given permissions in the answer), which is treated as unknown.
    pub can_push: Option<bool>,
    /// Another signed-in account that can push, when this one cannot.
    pub suggested_login: Option<String>,
}

/// One check's subject: which account, which repository.
fn check_key(login: &str, remote: &GitHubRemote) -> String {
    format!("{}@{}", login.to_ascii_lowercase(), remote.full_name().to_ascii_lowercase())
}

/// What one account may do with a repository, as far as GitHub said.
enum Answer {
    /// It sees it; `Some(push)` when GitHub said whether it may push.
    Visible(Option<bool>),
    /// 404: it cannot see it (or the repository does not exist).
    NotVisible,
    /// Anything else (offline, signed out, rate limited): unknown.
    Unknown,
}

async fn ask(remote: &GitHubRemote, state: &GhState) -> Answer {
    match gh_get_repo(remote.owner.clone(), remote.name.clone(), state).await {
        Ok(repo) => Answer::Visible(
            repo.permissions
                .map(|permissions| permissions.allows(RepoPermission::Push)),
        ),
        Err(GhError::NotFound { .. }) => Answer::NotVisible,
        Err(_) => Answer::Unknown,
    }
}

impl HelmPanel {
    /// The workspace's repository access, once it has been checked.
    pub fn repo_access(&self) -> Option<&RepoAccess> {
        self.repo_access.as_ref()
    }

    /// Follows the workspace's git repositories, so the check runs again
    /// when the active repository or its remote changes. Called once the
    /// workspace can be read.
    pub(super) fn watch_git_store(&mut self, cx: &mut Context<Self>) {
        let Some(workspace) = self.workspace.upgrade() else {
            return;
        };
        let git_store = workspace.read(cx).project().read(cx).git_store().clone();
        self._git_store_subscription = Some(cx.subscribe(
            &git_store,
            |this, _: Entity<GitStore>, event: &GitStoreEvent, cx| match event {
                GitStoreEvent::ActiveRepositoryChanged(_)
                | GitStoreEvent::RepositoryAdded
                | GitStoreEvent::RepositoryRemoved(_)
                | GitStoreEvent::RepositoryUpdated(_, _, true) => this.check_repo_access(false, cx),
                _ => {}
            },
        ));
    }

    /// The github.com repository the workspace's active git repository
    /// pushes to: the remote its current branch tracks when that is
    /// `upstream`, else `origin`. In a fork, `upstream` is usually the
    /// original project, which the user cannot push to, so `origin` comes
    /// first unless the branch really tracks `upstream`.
    fn workspace_github_remote(&self, cx: &App) -> Option<GitHubRemote> {
        let workspace = self.workspace.upgrade()?;
        let project = workspace.read(cx).project().read(cx);
        let repository = project.git_store().read(cx).active_repository()?;
        let repository = repository.read(cx);
        let tracks_upstream = repository
            .branch
            .as_ref()
            .and_then(|branch| branch.upstream.as_ref())
            .and_then(|upstream| upstream.remote_name())
            == Some("upstream");
        let origin = repository.remote_origin_url.as_deref();
        let upstream = repository.remote_upstream_url.as_deref();
        let (first, second) = if tracks_upstream {
            (upstream, origin)
        } else {
            (origin, upstream)
        };
        first
            .and_then(parse_github_remote)
            .or_else(|| second.and_then(parse_github_remote))
    }

    /// Checks whether the workspace's account can push to the workspace's
    /// repository, unless that was already checked for this account and
    /// repository (`force` checks again, after a failure).
    pub(super) fn check_repo_access(&mut self, force: bool, cx: &mut Context<Self>) {
        let Some(remote) = self.workspace_github_remote(cx) else {
            if self.repo_access.take().is_some() {
                cx.notify();
            }
            self.repo_access_key = None;
            return;
        };
        // Signed in and ready: the menu is past the sign-in screen.
        if self.account.is_empty() || self.user.is_none() {
            return;
        }
        let login = self.account.clone();
        let key = check_key(&login, &remote);
        if !force && self.repo_access_key.as_deref() == Some(key.as_str()) {
            return;
        }
        self.repo_access_key = Some(key.clone());

        let gh_state = self.gh_state.clone();
        let others: Vec<String> = HelmPanelSettings::get_global(cx)
            .accounts
            .iter()
            .filter(|account| !account.login.eq_ignore_ascii_case(&login))
            .map(|account| account.login.clone())
            .collect();
        let http = gh_state.http.clone();

        cx.spawn(async move |this, cx| {
            let own = {
                let remote = remote.clone();
                let gh_state = gh_state.clone();
                on_tokio(async move { ask(&remote, &gh_state).await }).await
            };
            let (can_read, can_push) = match own {
                Answer::Visible(push) => (true, push),
                Answer::NotVisible => (false, Some(false)),
                // Nothing known: leave things as they are.
                Answer::Unknown => return,
            };

            let mut suggested_login = None;
            if can_push == Some(false) {
                for other in others {
                    let Ok(Some(token)) = read_token(&other, cx).await else {
                        continue;
                    };
                    let remote = remote.clone();
                    let http = http.clone();
                    let answer = on_tokio(async move {
                        // A state of its own: the panel's account and its
                        // remembered answers are not touched.
                        let probe = GhState::new(http);
                        probe.set_token(Some(token)).await;
                        ask(&remote, &probe).await
                    })
                    .await;
                    if matches!(answer, Answer::Visible(Some(true))) {
                        suggested_login = Some(other);
                        break;
                    }
                }
            }

            this.update(cx, |this, cx| {
                // Switched account, or the repository changed, meanwhile.
                if !this.is_current_state(&gh_state)
                    || this.repo_access_key.as_deref() != Some(key.as_str())
                {
                    return;
                }
                let access = RepoAccess {
                    remote,
                    login,
                    can_read,
                    can_push,
                    suggested_login,
                };
                this.offer_account_switch(&access, &key, cx);
                this.repo_access = Some(access);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Shows the "switch to the account that can push" toast, once per
    /// account and repository in this session.
    fn offer_account_switch(&mut self, access: &RepoAccess, key: &str, cx: &mut Context<Self>) {
        let Some(suggested) = access.suggested_login.clone() else {
            return;
        };
        if !self.repo_access_offered.insert(key.to_string()) {
            return;
        }
        let name = |login: &str| {
            HelmPanelSettings::get_global(cx)
                .account(login)
                .map_or_else(|| login.to_string(), |account| account.display_name().to_string())
        };
        let current = name(&access.login);
        let suggested_name = name(&suggested);
        let repository = access.remote.full_name();
        let message = if access.can_read {
            format!("{current} can't push to {repository}. {suggested_name} can.")
        } else {
            format!("{current} can't see {repository}. {suggested_name} can push to it.")
        };
        let panel = cx.entity().downgrade();
        self.workspace
            .update(cx, |workspace, cx| {
                workspace.show_toast(
                    Toast::new(NotificationId::unique::<RepoAccess>(), message).on_click(
                        format!("Switch to {suggested_name}"),
                        move |_, cx| {
                            panel
                                .update(cx, |this, cx| this.switch_account(suggested.clone(), cx))
                                .ok();
                        },
                    ),
                    cx,
                );
            })
            .ok();
    }
}

/// The checks already offered as a toast this session.
pub(super) type OfferedChecks = HashSet<String>;
