//! Who commits in a workspace are authored as (section 11, item 2 of
//! `helm_backend/octocrab-helm-coverage.md`).
//!
//! Each account has a commit name and email in settings (`git_name`,
//! `git_email`). For a workspace whose repository is on github.com, they
//! are written into that repository's own `.git/config`, never the global
//! config, so commits from the git panel, a terminal or any other tool use
//! them, and a terminal that is already open follows an account switch at
//! once.
//!
//! An account without a chosen email gets one without asking when the
//! global git email is one of its verified addresses (the global name comes
//! with it). Otherwise the git panel asks once. A repository whose own
//! config names an email Helm set for one of the user's accounts follows
//! an account switch silently; one that names any other email (set by hand
//! or by another tool) is not overwritten without asking.

use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};

use helm_backend::github::{gh_get_current_user, gh_get_verified_emails};

use super::accounts::set_commit_identity;
use super::*;

/// Where the workspace's commit identity stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CommitIdentity {
    /// Not a GitHub repository, or not signed in.
    Unknown,
    /// Being worked out or written.
    Checking,
    /// The account has no commit email yet: the choices to offer.
    NeedsEmail {
        login: String,
        name: String,
        /// The account's verified addresses, primary first. Empty when the
        /// token may not read them.
        emails: Vec<String>,
        /// GitHub's private address for the account.
        noreply: String,
        /// What commits use until an email is chosen: the repository's own
        /// email, else the global one.
        current_email: Option<String>,
    },
    /// The repository's own config names another email.
    Conflict {
        login: String,
        name: String,
        email: String,
        current_email: String,
    },
    /// Commits are authored as this.
    Applied { name: String, email: String },
    Failed(String),
}

/// Runs `git` in `dir`, with no console window on Windows.
fn git(dir: &Path, args: &[&str]) -> std::io::Result<Output> {
    let mut command = std::process::Command::new("git");
    command
        .current_dir(dir)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt as _;
        command.creation_flags(0x0800_0000);
    }
    command.output()
}

/// A `git config` value, if it is set.
pub(super) fn git_config(dir: &Path, scope: &str, key: &str) -> Option<String> {
    let output = git(dir, &["config", scope, "--get", key]).ok()?;
    let value = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (output.status.success() && !value.is_empty()).then_some(value)
}

/// What writing an identity into a repository found.
enum Written {
    Done,
    /// The repository names another email; nothing was written.
    Conflict(String),
}

/// Writes `name` and `email` into the repository at `dir`. An email
/// already there is replaced when it is one of `own_emails` (set by Helm
/// for one of the user's accounts) or `overwrite` is set; any other email
/// is a conflict, and nothing is written.
fn write_identity(
    dir: &Path,
    name: &str,
    email: &str,
    own_emails: &[String],
    overwrite: bool,
) -> Result<Written, String> {
    if !overwrite
        && let Some(current) = git_config(dir, "--local", "user.email")
        && !current.eq_ignore_ascii_case(email)
        && !own_emails
            .iter()
            .any(|own| own.eq_ignore_ascii_case(&current))
    {
        return Ok(Written::Conflict(current));
    }
    for (key, value) in [("user.name", name), ("user.email", email)] {
        if git_config(dir, "--local", key).as_deref() == Some(value) {
            continue;
        }
        let output = git(dir, &["config", "--local", key, value])
            .map_err(|error| format!("Could not run git: {error}"))?;
        if !output.status.success() {
            return Err(format!(
                "git could not set {key}: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
    }
    Ok(Written::Done)
}

impl HelmPanel {
    /// Where the workspace's commit identity stands, for the git panel.
    pub fn commit_identity(&self) -> &CommitIdentity {
        &self.commit_identity
    }

    /// The folder of the workspace's active git repository.
    pub(super) fn workspace_repository_dir(&self, cx: &App) -> Option<PathBuf> {
        let workspace = self.workspace.upgrade()?;
        let project = workspace.read(cx).project().read(cx);
        let repository = project.git_store().read(cx).active_repository()?;
        Some(repository.read(cx).work_directory_abs_path.to_path_buf())
    }

    /// Works out and applies the commit identity for the workspace's
    /// account and repository, unless that was already done for them
    /// (`force` does it again).
    pub(super) fn refresh_commit_identity(&mut self, force: bool, cx: &mut Context<Self>) {
        let (Some(dir), Some(login), Some(_)) = (
            self.workspace_repository_dir(cx),
            self.signed_in_login().map(str::to_string),
            self.workspace_github_remote(cx),
        ) else {
            if self.commit_identity != CommitIdentity::Unknown {
                self.commit_identity = CommitIdentity::Unknown;
                self.commit_identity_key = None;
                cx.notify();
            }
            return;
        };
        let key = format!("{}@{}", login.to_ascii_lowercase(), dir.display());
        if !force && self.commit_identity_key.as_deref() == Some(key.as_str()) {
            return;
        }
        self.commit_identity_key = Some(key.clone());

        let account = HelmPanelSettings::get_global(cx).account(&login).cloned();
        if let Some(account) = &account
            && let Some(email) = &account.git_email
        {
            let name = account
                .git_name
                .clone()
                .unwrap_or_else(|| account.display_name().to_string());
            self.write_commit_identity(dir, name, email.clone(), false, cx);
            return;
        }

        self.commit_identity = CommitIdentity::Checking;
        cx.notify();
        let gh_state = self.gh_state.clone();
        cx.spawn(async move |this, cx| {
            let (user, emails) = {
                let gh_state = gh_state.clone();
                on_tokio(async move {
                    let user = gh_get_current_user(&gh_state).await;
                    // A token without the `user` scope cannot read them.
                    let emails = gh_get_verified_emails(&gh_state).await.unwrap_or_default();
                    (user, emails)
                })
                .await
            };
            let global = {
                let dir = dir.clone();
                cx.background_spawn(async move {
                    (
                        git_config(&dir, "--global", "user.name"),
                        git_config(&dir, "--global", "user.email"),
                        git_config(&dir, "--local", "user.email"),
                    )
                })
                .await
            };
            let (global_name, global_email, local_email) = global;
            this.update(cx, |this, cx| {
                if !this.is_current_state(&gh_state)
                    || this.commit_identity_key.as_deref() != Some(key.as_str())
                {
                    return;
                }
                let user = match user {
                    Ok(user) => user,
                    Err(error) => {
                        this.commit_identity = CommitIdentity::Failed(error.to_string());
                        cx.notify();
                        return;
                    }
                };
                let github_name = user.name.clone().unwrap_or_else(|| user.login.clone());
                let current_email = local_email.or_else(|| global_email.clone());
                // The global identity already belongs to this account: use it.
                if let Some(global_email) = global_email
                    && emails
                        .iter()
                        .any(|email| email.eq_ignore_ascii_case(&global_email))
                {
                    let name = global_name.unwrap_or(github_name);
                    set_commit_identity(&login, &name, &global_email, cx);
                    this.write_commit_identity(dir, name, global_email, false, cx);
                    return;
                }
                this.commit_identity = CommitIdentity::NeedsEmail {
                    noreply: format!("{}+{}@users.noreply.github.com", user.id, user.login),
                    login,
                    name: github_name,
                    emails,
                    current_email,
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn write_commit_identity(
        &mut self,
        dir: PathBuf,
        name: String,
        email: String,
        overwrite: bool,
        cx: &mut Context<Self>,
    ) {
        let Some(login) = self.signed_in_login().map(str::to_string) else {
            return;
        };
        let key = self.commit_identity_key.clone();
        // The emails Helm has set for the user's accounts: replacing one of
        // them is just following an account switch.
        let own_emails: Vec<String> = HelmPanelSettings::get_global(cx)
            .accounts
            .iter()
            .filter_map(|account| account.git_email.clone())
            .collect();
        self.commit_identity = CommitIdentity::Checking;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = {
                let (name, email) = (name.clone(), email.clone());
                cx.background_spawn(async move {
                    write_identity(&dir, &name, &email, &own_emails, overwrite)
                })
                .await
            };
            this.update(cx, |this, cx| {
                if this.commit_identity_key != key {
                    return;
                }
                this.commit_identity = match result {
                    Ok(Written::Done) => CommitIdentity::Applied { name, email },
                    Ok(Written::Conflict(current_email)) => CommitIdentity::Conflict {
                        login,
                        name,
                        email,
                        current_email,
                    },
                    Err(error) => CommitIdentity::Failed(error),
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Picks `email` for the account the workspace uses (from the git
    /// panel's choices), remembers it, and applies it.
    pub fn choose_commit_email(&mut self, email: String, cx: &mut Context<Self>) {
        let CommitIdentity::NeedsEmail { login, name, .. } = self.commit_identity.clone() else {
            return;
        };
        self.save_commit_identity(login, name, email, cx);
    }

    /// Remembers `name` and `email` for `login`, and applies them to the
    /// workspace when it uses that account.
    pub(super) fn save_commit_identity(
        &mut self,
        login: String,
        name: String,
        email: String,
        cx: &mut Context<Self>,
    ) {
        set_commit_identity(&login, &name, &email, cx);
        let uses_login = self
            .signed_in_login()
            .is_some_and(|signed_in| signed_in.eq_ignore_ascii_case(&login));
        if uses_login && let Some(dir) = self.workspace_repository_dir(cx) {
            self.write_commit_identity(dir, name, email, false, cx);
        }
    }

    /// GitHub's private noreply address for the workspace's account, once
    /// it is signed in.
    pub fn noreply_email(&self) -> Option<String> {
        let user = self.user.as_ref()?;
        Some(format!("{}+{}@users.noreply.github.com", user.id, user.login))
    }

    /// Makes the workspace's account commit with its noreply address, after
    /// GitHub refused a push for publishing a private email (GH007). The
    /// repository's own email is replaced whatever it was.
    pub fn use_noreply_commit_email(&mut self, cx: &mut Context<Self>) {
        let (Some(login), Some(email)) = (
            self.signed_in_login().map(str::to_string),
            self.noreply_email(),
        ) else {
            return;
        };
        let name = HelmPanelSettings::get_global(cx)
            .account(&login)
            .and_then(|account| account.git_name.clone())
            .or_else(|| self.user.as_ref().and_then(|user| user.name.clone()))
            .unwrap_or_else(|| login.clone());
        set_commit_identity(&login, &name, &email, cx);
        if let Some(dir) = self.workspace_repository_dir(cx) {
            self.write_commit_identity(dir, name, email, true, cx);
        }
    }

    /// Resolves a conflict: writes the account's identity over the
    /// repository's own.
    pub fn use_account_identity(&mut self, cx: &mut Context<Self>) {
        let CommitIdentity::Conflict { name, email, .. } = self.commit_identity.clone() else {
            return;
        };
        if let Some(dir) = self.workspace_repository_dir(cx) {
            self.write_commit_identity(dir, name, email, true, cx);
        }
    }

    /// Resolves a conflict: keeps the repository's own identity.
    pub fn keep_repository_identity(&mut self, cx: &mut Context<Self>) {
        let CommitIdentity::Conflict { current_email, .. } = self.commit_identity.clone() else {
            return;
        };
        let name = self
            .workspace_repository_dir(cx)
            .and_then(|dir| git_config(&dir, "--local", "user.name"))
            .unwrap_or_default();
        self.commit_identity = CommitIdentity::Applied {
            name,
            email: current_email,
        };
        cx.notify();
    }

    /// Opens the dialog for the commit name and email of the account the
    /// workspace uses.
    pub fn open_commit_identity_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(login) = self.signed_in_login().map(str::to_string) else {
            return;
        };
        let account = HelmPanelSettings::get_global(cx).account(&login).cloned();
        let (name, email) = match &self.commit_identity {
            CommitIdentity::Applied { name, email } => (name.clone(), email.clone()),
            CommitIdentity::NeedsEmail { name, emails, .. } => {
                (name.clone(), emails.first().cloned().unwrap_or_default())
            }
            CommitIdentity::Conflict { name, email, .. } => (name.clone(), email.clone()),
            _ => (
                account
                    .as_ref()
                    .and_then(|account| account.git_name.clone())
                    .unwrap_or_else(|| login.clone()),
                account
                    .as_ref()
                    .and_then(|account| account.git_email.clone())
                    .unwrap_or_default(),
            ),
        };
        self.open_workspace_modal(
            HelmModalKind::CommitIdentity { login, name, email },
            window,
            cx,
        );
    }
}
