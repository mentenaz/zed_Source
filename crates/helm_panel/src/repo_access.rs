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

use std::path::PathBuf;
use std::sync::Mutex;

use helm_backend::github::{
    GitHubRemote, RepoPermission, gh_get_repo, git_auth_config, parse_github_remote,
};
use ide_env::GitCredential;

/// Which panel gave git which account's login for which folder: (folder,
/// panel, login). Only to warn when two windows on the same folder use
/// different accounts, since git can only log in as one of them there.
static FOLDER_LOGINS: Mutex<Vec<(PathBuf, u64, String)>> = Mutex::new(Vec::new());

/// Records that `owner` gives git `login` for `folder`, and returns the
/// login another panel already gives it for the same folder, when that is a
/// different account.
fn claim_folder(folder: &std::path::Path, owner: u64, login: &str) -> Option<String> {
    let Ok(mut claims) = FOLDER_LOGINS.lock() else {
        return None;
    };
    claims.retain(|(_, claimed_by, _)| *claimed_by != owner);
    let other = claims
        .iter()
        .find(|(claimed, _, claimed_login)| {
            claimed == folder && !claimed_login.eq_ignore_ascii_case(login)
        })
        .map(|(_, _, claimed_login)| claimed_login.clone());
    claims.push((folder.to_path_buf(), owner, login.to_string()));
    other
}

/// Forgets `owner`'s claims ([`claim_folder`]).
fn release_folders(owner: u64) {
    if let Ok(mut claims) = FOLDER_LOGINS.lock() {
        claims.retain(|(_, claimed_by, _)| *claimed_by != owner);
    }
}
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
    pub(super) fn workspace_github_remote(&self, cx: &App) -> Option<GitHubRemote> {
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
        // The commit identity and the push credentials follow the same
        // account and repository.
        self.refresh_commit_identity(false, cx);
        self.register_git_credentials(cx);
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
                // The account can push (or GitHub did not say): an offer
                // made for an account that could not is out of date.
                if access.can_push != Some(false) {
                    this.dismiss_access_offer(cx);
                }
                this.offer_account_switch(&access, &key, cx);
                this.repo_access = Some(access);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Makes the git panel's push, pull and fetch in the workspace's
    /// repository log in to github.com as the workspace's account (section
    /// 11, item 3): its token goes to git as an `Authorization` header in
    /// `GIT_CONFIG_*` environment variables, through `ide_env`. Removed when
    /// the workspace has no GitHub HTTPS repository or is signed out, so
    /// git's own login applies again.
    pub(super) fn register_git_credentials(&mut self, cx: &mut Context<Self>) {
        let dir = self.workspace_repository_dir(cx);
        self.workspace_remote_https = self
            .workspace_github_remote(cx)
            .is_some_and(|remote| remote.https);
        let signed_in = self.signed_in_login().is_some();
        let applies = signed_in && self.workspace_remote_https;
        // Signed out: git must not fall back to its own login, which may be
        // another account. Its helpers for github.com are cleared, and Zed's
        // terminal helper answers with a refusal.
        let refused = !signed_in && self.signed_out && self.workspace_remote_https;
        // Already given for this repository and account: repository updates
        // (any file change) come through here often.
        let already = dir.is_some() && self.git_credentials_dir == dir;
        if applies && already && !self.git_credentials_refused {
            if self.account_loading {
                self.account_loading = false;
                cx.notify();
            }
            return;
        }
        if refused && already && self.git_credentials_refused {
            return;
        }
        self.unregister_git_credentials();
        let owner = self.git_owner;
        if refused && let Some(dir) = dir {
            ide_env::set_git_remote_config(
                owner,
                &dir,
                vec![("credential.https://github.com.helper".to_string(), String::new())],
            );
            ide_env::set_git_credential(owner, &dir, Some(GitCredential::Refuse));
            self.git_credentials_dir = Some(dir);
            self.git_credentials_refused = true;
            cx.notify();
            return;
        }
        let Some(dir) = dir.filter(|_| applies) else {
            // Nothing to give: no GitHub HTTPS repository. (Not signed in
            // yet while loading keeps `account_loading` as it is.)
            if signed_in && self.account_loading {
                self.account_loading = false;
                cx.notify();
            }
            return;
        };
        self.git_credentials_dir = Some(dir.clone());
        let gh_state = self.gh_state.clone();
        cx.spawn(async move |this, cx| {
            let token = gh_state.token().await;
            this.update(cx, |this, cx| {
                // Switched account or repository meanwhile.
                if !this.is_current_state(&gh_state)
                    || this.git_credentials_dir.as_ref() != Some(&dir)
                {
                    return;
                }
                let entries = token.as_deref().map(git_auth_config).unwrap_or_default();
                ide_env::set_git_remote_config(owner, &dir, entries);
                // Git in a terminal asks Zed's credential helper, which
                // answers from this (section 11, item 4).
                ide_env::set_git_credential(
                    owner,
                    &dir,
                    token.map(|token| GitCredential::Login {
                        username: "x-access-token".to_string(),
                        password: token,
                    }),
                );
                this.account_loading = false;
                // Another window on this folder uses a different account:
                // git logs in as whichever window set it up last.
                if let Some(login) = this.signed_in_login().map(str::to_string)
                    && let Some(other) = claim_folder(&dir, owner, &login)
                {
                    this.notify(
                        format!(
                            "This folder is also open in another window as @{other}. Git uses one \
                             GitHub account per folder, so pushes from both windows now go out as \
                             @{login}. Use the same account in both, or close one."
                        ),
                        cx,
                    );
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The workspace's account, when it is signed out (GitHub no longer
    /// accepts its token, or there is none on this machine) and the
    /// workspace's repository is on github.com over HTTPS, where git would
    /// otherwise need that account's login. Not for a network failure.
    pub fn signed_out_login(&self) -> Option<&str> {
        if !self.signed_out || !self.workspace_remote_https || self.signed_in_login().is_some() {
            return None;
        }
        self.active_login.as_deref()
    }

    /// Whether the signed-out account was copied from the GitHub CLI, so
    /// signing in again means re-importing it from there.
    pub fn signed_out_from_gh_cli(&self, cx: &App) -> bool {
        self.signed_out_login().is_some_and(|login| {
            HelmPanelSettings::get_global(cx)
                .account(login)
                .is_some_and(|account| account.source == HelmAccountSource::GhCli)
        })
    }

    /// Signs the workspace's signed-out account in again, for the same
    /// login: re-imported from the GitHub CLI when it came from there,
    /// otherwise Helm's own sign-in, which checks the account that signs in
    /// is the expected one. Shows the Helm panel, where it happens.
    pub fn sign_in_again(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(login) = self.signed_out_login().map(str::to_string) else {
            return;
        };
        if self.signed_out_from_gh_cli(cx) {
            self.import_from_gh_cli(login, cx);
        } else {
            self.start_sign_in(SignInFor::Account(login), cx);
        }
        if let Some(workspace) = self.workspace.upgrade() {
            workspace.update(cx, |workspace, cx| {
                workspace.focus_panel::<HelmPanel>(window, cx);
            });
        }
    }

    /// Whether the git panel should hold a push, pull or fetch back: the
    /// workspace's GitHub account is still loading for a github.com HTTPS
    /// repository, so git would otherwise log in with its own login.
    pub fn is_loading_github_account(&self) -> bool {
        self.account_loading && self.workspace_remote_https
    }

    /// Stops git panel pushes in the workspace's repository from using the
    /// account's token (signed out, account switched or removed).
    pub(super) fn unregister_git_credentials(&mut self) {
        self.git_credentials_refused = false;
        release_folders(self.git_owner);
        if let Some(dir) = self.git_credentials_dir.take() {
            // Only this panel's: another window on the same folder keeps its
            // own, which then apply again.
            ide_env::set_git_remote_config(self.git_owner, &dir, Vec::new());
            ide_env::set_git_credential(self.git_owner, &dir, None);
        }
    }

    /// Shows the "switch to the account that can push" toast, once per
    /// account and repository in this session.
    /// Takes down the "switch to the account that can push" toast. Deferred,
    /// since an account switch can come from a toast or menu of the
    /// workspace while it is being updated.
    pub(super) fn dismiss_access_offer(&self, cx: &mut Context<Self>) {
        let workspace = self.workspace.clone();
        cx.defer(move |cx| {
            workspace
                .update(cx, |workspace, cx| {
                    workspace.dismiss_toast(&NotificationId::unique::<RepoAccess>(), cx)
                })
                .ok();
        });
    }

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
