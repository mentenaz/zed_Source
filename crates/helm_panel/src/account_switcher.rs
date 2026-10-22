//! Which signed-in account a workspace's Helm panel uses: the account menu
//! in the panel header, switching account without signing out, and
//! remembering the choice per workspace.
//!
//! Switching never touches the stored tokens. It gives the panel a new
//! `GhState` for the chosen account, so the previous account's remembered
//! answers and rate limits are not reused, and an answer still on its way
//! for the previous account is dropped when it arrives (see
//! `HelmView::is_current_state`).

use super::accounts::{
    HelmAccountEvent, HelmAccounts, delete_token, forget_account, set_default_account,
};
use super::*;
use db::kvp::KeyValueStore;

/// The key-value store key prefix of a workspace's chosen account, followed
/// by the workspace's id, the same way the Git panel keys its state.
const ACCOUNT_BINDING_KEY: &str = "helm_panel_account";

fn binding_key(workspace: &Workspace) -> Option<String> {
    workspace
        .database_id()
        .map(|id| i64::from(id).to_string())
        .or(workspace.session_id())
        .map(|id| format!("{ACCOUNT_BINDING_KEY}-{id}"))
}

impl HelmPanel {
    /// Starts the panel: reads which account this workspace used last time,
    /// then checks that account's sign-in. Runs as a task because the panel
    /// is created while its workspace is being updated, so the workspace
    /// cannot be read yet.
    pub(super) fn restore_account_and_start(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let key = this
                .update(cx, |this, cx| this.account_binding_key(cx))
                .ok()
                .flatten();
            let saved = match key {
                Some(key) => {
                    let kvp = cx.update(|cx| KeyValueStore::global(cx));
                    match cx.background_spawn(async move { kvp.read_kvp(&key) }).await {
                        Ok(saved) => saved,
                        Err(error) => {
                            log::error!("helm_panel: could not read the workspace's account: {error:#}");
                            None
                        }
                    }
                }
                None => None,
            };
            this.update(cx, |this, cx| {
                // Only an account that is still listed; a removed one falls
                // back to the default account.
                if this.active_login.is_none()
                    && let Some(login) = saved
                    && let Some(account) = HelmPanelSettings::get_global(cx).account(&login)
                {
                    this.active_login = Some(account.login.clone());
                }
                this.watch_git_store(cx);
                this.start_session(cx);
            })
            .ok();
        })
        .detach();
    }

    fn account_binding_key(&self, cx: &App) -> Option<String> {
        let workspace = self.workspace.upgrade()?;
        binding_key(workspace.read(cx))
    }

    /// Remembers `login` as this workspace's account, or forgets the choice
    /// so the workspace follows the default account again.
    pub(super) fn save_account_binding(&self, login: Option<&str>, cx: &mut Context<Self>) {
        let Some(key) = self.account_binding_key(cx) else {
            return;
        };
        let kvp = KeyValueStore::global(cx);
        let login = login.map(str::to_string);
        cx.background_spawn(async move {
            let result = match login {
                Some(login) => kvp.write_kvp(key, login).await,
                None => kvp.delete_kvp(key).await,
            };
            if let Err(error) = result {
                log::error!("helm_panel: could not save the workspace's account: {error:#}");
            }
        })
        .detach();
    }

    /// Makes `login` this panel's account (`None`: the default account) with
    /// a fresh `GhState`, and forgets everything shown for the previous one.
    /// The caller starts the session. Workspace tabs opened with the previous
    /// state keep it; [`Self::drop_current_token`] stops them using a token
    /// that was signed out.
    pub(super) fn use_account(&mut self, login: Option<String>, cx: &mut Context<Self>) {
        self.gh_state = Arc::new(GhState::new(self.gh_state.http.clone()));
        self.active_login = login;
        self.adding_account = false;
        // Checked again for the new account once it is signed in. The offer
        // to switch was about the previous account: it goes, and the new
        // account gets its own if it cannot push either.
        self.repo_access = None;
        self.repo_access_key = None;
        self.repo_access_offered.clear();
        self.dismiss_access_offer(cx);
        self.commit_identity = CommitIdentity::Unknown;
        self.commit_identity_key = None;
        // The previous account's token no longer goes along with pushes;
        // pushes wait until the new one is loaded (`start_session`).
        self.unregister_git_credentials();
        self.account_loading = false;

        self.account.clear();
        self.scopes.clear();
        self.user = None;
        self.org_logins.clear();
        self.repo_invitation_count = 0;
        self.repos.clear();
        self.repos_page = 1;
        self.github_search_results.clear();
        self.github_search_query.clear();
        self.branches.clear();
        self.collaborators.clear();
        self.selected_issue = None;
        self.selected_pr = None;
        self.detail_comments.clear();
        self.invitations.clear();
        self.org_invitations.clear();
        self.pending_action = None;
        self.pending_attach = None;
        self.back_stack.clear();
        self.forward_stack.clear();
        // Leaving every other screen clears the organisation and repository
        // that were open, with their tabs.
        self.set_screen(HelmScreen::Auth, cx);
    }

    /// Clears the token from the current `GhState`, which workspace tabs
    /// opened from this panel share. For when that token was deleted.
    pub(super) fn drop_current_token(&self, cx: &mut Context<Self>) {
        let state = self.gh_state.clone();
        cx.background_spawn(async move { state.set_token(None).await })
            .detach();
    }

    /// The login this workspace's panel is signed in as, once the sign-in
    /// has been checked. For other panels (the git panel) to show.
    pub fn signed_in_login(&self) -> Option<&str> {
        (!self.account.is_empty() && self.user.is_some()).then_some(self.account.as_str())
    }

    /// Switches this workspace to `login`, which is already signed in (or
    /// shows its sign-in screen if its token is missing). Nothing is signed
    /// out.
    pub fn switch_account(&mut self, login: String, cx: &mut Context<Self>) {
        self.cancel_sign_in(cx);
        self.use_account(Some(login.clone()), cx);
        self.save_account_binding(Some(&login), cx);
        self.start_session(cx);
    }

    /// Opens the sign-in screen to add another account, with every way to
    /// sign in. Cancelling it goes back to the current account.
    pub(super) fn add_account(&mut self, cx: &mut Context<Self>) {
        self.sign_in_task = None;
        self.sign_in = SignIn::Idle;
        self.sign_in_for = SignInFor::AnyAccount;
        self.adding_account = true;
        self.gh_cli_picker_open = false;
        self.error_msg.clear();
        self.screen = HelmScreen::Auth;
        self.check_gh_cli(false, cx);
        cx.notify();
    }

    /// Deletes `login`'s token and takes it off the account list. Every
    /// workspace using it falls back to the default account.
    pub(super) fn remove_account(&mut self, login: String, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| {
            let deleted = delete_token(&login, cx).await;
            this.update(cx, |this, cx| match deleted {
                Ok(()) => {
                    forget_account(&login, cx);
                    HelmAccounts::emit(HelmAccountEvent::Removed { login }, cx);
                }
                Err(error) => this.notify(format!("Could not remove the account: {error:#}"), cx),
            })
            .ok();
        })
        .detach();
    }

    /// The account button in the panel header, with the menu to switch,
    /// add, sign out of and remove accounts. Nothing until an account is
    /// listed.
    pub(super) fn render_account_menu(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let settings = HelmPanelSettings::get_global(cx);
        if settings.accounts.is_empty() {
            return None;
        }
        let accounts = settings.accounts.clone();
        // Each account's colour, beside its name in the menu.
        let colors: Vec<gpui::Hsla> = accounts
            .iter()
            .map(|account| settings.account_color(&account.login))
            .collect();
        let default_login = settings.default_account().map(|account| account.login.clone());
        let active = self.active_login.clone();
        let label = match &active {
            Some(login) => settings
                .account(login)
                .map_or_else(|| login.clone(), |account| account.display_name().to_string()),
            None => "No account".to_string(),
        };
        let panel = cx.entity().downgrade();
        // The active account's face beside its name, the same avatar Helm
        // shows for it everywhere; initials until it has loaded.
        let avatar = active.as_deref().map(|login| {
            let url = self
                .user
                .as_ref()
                .filter(|user| user.login.eq_ignore_ascii_case(login))
                .map_or("", |user| user.avatar_url.as_str());
            // Ringed in the account's colour, the one the git panel and
            // the Workspace tab show it in too.
            let color = HelmPanelSettings::get_global(cx).account_color(login);
            github_avatar(url, login, AVATAR_SIZE)
                .border_2()
                .border_color(color)
        });

        let button = Button::new("helm-account-menu")
                .ghost()
                .xsmall()
                .label(label)
                .tooltip("Switch GitHub account")
                .dropdown_menu(move |mut menu, _, _| {
                    let is_active = |login: &str| {
                        active
                            .as_deref()
                            .is_some_and(|active| active.eq_ignore_ascii_case(login))
                    };
                    for (ix, account) in accounts.iter().enumerate() {
                        let is_default = default_login
                            .as_deref()
                            .is_some_and(|default| default.eq_ignore_ascii_case(&account.login));
                        let mut label = account.display_name().to_string();
                        if account.source == HelmAccountSource::GhCli {
                            label.push_str(" · via GitHub CLI");
                        }
                        if is_default {
                            label.push_str(" (default)");
                        }
                        let panel = panel.clone();
                        let login = account.login.clone();
                        let color = colors[ix];
                        let label = SharedString::from(label);
                        menu = menu.item(
                            PopupMenuItem::element(move |_, _| {
                                h_flex()
                                    .gap_2()
                                    .items_center()
                                    .child(account_dot(color))
                                    .child(label.clone())
                            })
                            .checked(is_active(&account.login))
                            .on_click(move |_, _, cx| {
                                panel
                                    .update(cx, |this, cx| {
                                        if !this.active_login.as_deref().is_some_and(
                                            |active| active.eq_ignore_ascii_case(&login),
                                        ) {
                                            this.switch_account(login.clone(), cx);
                                        }
                                    })
                                    .ok();
                            }),
                        );
                    }

                    menu = menu.separator();
                    if active.is_some() {
                        let identity_panel = panel.clone();
                        menu = menu.item(PopupMenuItem::new("Commit name and email…").on_click(
                            move |_, window, cx| {
                                identity_panel
                                    .update(cx, |this, cx| {
                                        this.open_commit_identity_dialog(window, cx)
                                    })
                                    .ok();
                            },
                        ));
                    }
                    let add_panel = panel.clone();
                    menu = menu.item(PopupMenuItem::new("Add account…").on_click(
                        move |_, _, cx| {
                            add_panel.update(cx, |this, cx| this.add_account(cx)).ok();
                        },
                    ));

                    if let Some(login) = active.clone() {
                        let name = accounts
                            .iter()
                            .find(|account| account.login.eq_ignore_ascii_case(&login))
                            .map_or_else(|| login.clone(), |account| {
                                account.display_name().to_string()
                            });
                        let is_default = default_login
                            .as_deref()
                            .is_some_and(|default| default.eq_ignore_ascii_case(&login));
                        if !is_default {
                            let login = login.clone();
                            menu = menu.item(
                                PopupMenuItem::new(format!("Make {name} the default")).on_click(
                                    move |_, _, cx| set_default_account(&login, cx),
                                ),
                            );
                        }
                        let sign_out_panel = panel.clone();
                        menu = menu.item(
                            PopupMenuItem::new(format!("Sign out of {name}")).on_click(
                                move |_, _, cx| {
                                    sign_out_panel
                                        .update(cx, |this, cx| this.handle_logout(cx))
                                        .ok();
                                },
                            ),
                        );
                        let revoke_panel = panel.clone();
                        menu = menu.item(
                            PopupMenuItem::new(format!("Sign out of {name} and revoke…")).on_click(
                                move |_, _, cx| {
                                    revoke_panel
                                        .update(cx, |this, cx| this.sign_out_and_revoke(cx))
                                        .ok();
                                },
                            ),
                        );
                        let remove_panel = panel.clone();
                        menu = menu.item(
                            PopupMenuItem::new(format!("Remove {name}")).on_click(
                                move |_, _, cx| {
                                    remove_panel
                                        .update(cx, |this, cx| {
                                            this.remove_account(login.clone(), cx)
                                        })
                                        .ok();
                                },
                            ),
                        );
                    }
                    menu
                });
        Some(
            h_flex()
                .gap_1()
                .items_center()
                .children(avatar)
                .child(button)
                .into_any_element(),
        )
    }
}
