//! Signing in to GitHub through Helm's own OAuth App (device flow) or a
//! pasted token, authorizing a missing scope, signing out, and the auth
//! screen. Tokens are kept in the system credential store (see
//! `accounts.rs`); no `gh` install is involved.

use super::*;
use super::accounts::{delete_token, read_token, remember_account, write_token};

/// Where a user asks an organisation to approve Helm's OAuth App, or
/// revokes Helm's access.
fn app_settings_url() -> String {
    format!("https://github.com/settings/connections/applications/{HELM_OAUTH_CLIENT_ID}")
}

/// Asks GitHub who the panel's token belongs to and whether it carries what
/// Helm needs, then loads the organisations for the menu.
async fn check_session(gh_state: &GhState) -> AuthOutcome {
    let user = match gh_get_current_user(gh_state).await {
        Ok(user) => user,
        Err(GhError::Unauthorized { .. }) => return AuthOutcome::Revoked,
        Err(error) => return AuthOutcome::Failed(error.to_string()),
    };
    // `None` for a token that does not report scopes (a fine-grained
    // personal access token): a missing permission then shows up per
    // request instead.
    let scopes = gh_state.token_scopes();
    if let Some(scopes) = &scopes
        && !token_has_scope(scopes, "repo")
    {
        return AuthOutcome::MissingRepoScope {
            account: user.login,
            scopes: scopes.clone(),
        };
    }
    let orgs = match gh_get_org_logins(gh_state).await {
        Ok(orgs) => orgs,
        Err(error) => return AuthOutcome::Failed(error.to_string()),
    };
    AuthOutcome::Ready {
        account: user.login.clone(),
        scopes: scopes.unwrap_or_default(),
        orgs,
        user,
    }
}

impl HelmPanel {
    /// The login this panel uses: the one it signed in as, else the default
    /// account from settings.
    pub(super) fn bound_login(&self, cx: &App) -> Option<String> {
        self.active_login.clone().or_else(|| {
            HelmPanelSettings::get_global(cx)
                .default_account()
                .map(|account| account.login.clone())
        })
    }

    /// How to show `login`: its label from settings, else the login.
    fn account_name(login: &str, cx: &App) -> String {
        HelmPanelSettings::get_global(cx)
            .account(login)
            .map_or_else(|| login.to_string(), |account| account.display_name().to_string())
    }

    /// Loads the bound account's stored token and checks it with GitHub.
    /// Lands on the menu when it works, else on the sign-in screen.
    pub(super) fn start_session(&mut self, cx: &mut Context<Self>) {
        self.screen = HelmScreen::Auth;
        self.load_state = LoadState::Loading;
        self.error_msg.clear();
        cx.notify();

        let Some(login) = self.bound_login(cx) else {
            self.auth_initialized = true;
            self.load_state = LoadState::Idle;
            self.check_gh_cli(false, cx);
            cx.notify();
            return;
        };
        self.active_login = Some(login.clone());

        let gh_state = self.gh_state.clone();
        let started = gh_state.clone();
        cx.spawn(async move |this, cx| {
            let outcome = match read_token(&login, cx).await {
                Err(error) => AuthOutcome::Failed(format!(
                    "Could not read the GitHub token for {login}: {error:#}"
                )),
                Ok(None) => {
                    gh_state.set_token(None).await;
                    AuthOutcome::NotLoggedIn
                }
                Ok(Some(token)) => {
                    let outcome = on_tokio(async move {
                        gh_state.set_token(Some(token)).await;
                        check_session(&gh_state).await
                    })
                    .await;
                    if matches!(outcome, AuthOutcome::Revoked)
                        && let Err(error) = delete_token(&login, cx).await
                    {
                        log::error!("helm_panel: could not delete the revoked token: {error:#}");
                    }
                    outcome
                }
            };
            this.update(cx, |this, cx| {
                // Switched to another account while this one was checked.
                if this.is_current_state(&started) {
                    this.apply_auth_outcome(login, outcome, cx);
                }
            })
            .ok();
        })
        .detach();
    }

    fn apply_auth_outcome(&mut self, login: String, outcome: AuthOutcome, cx: &mut Context<Self>) {
        self.auth_initialized = true;
        self.load_state = LoadState::Idle;
        match outcome {
            AuthOutcome::Ready {
                account,
                scopes,
                orgs,
                user,
            } => {
                self.active_login = Some(account.clone());
                self.account = account;
                self.scopes = scopes;
                self.org_logins.items = orgs;
                self.user = Some(user);
                self.error_msg.clear();
                self.screen = HelmScreen::Menu;
                self.load_repo_invitations(cx);
                self.check_repo_access(false, cx);
                // An action that was bounced here for a missing scope: put
                // the user back where they were and re-send it.
                // `may_reauthorize: false` so a second rejection reports the
                // failure instead of looping the gate.
                if let Some(pending) = self.pending_action.take() {
                    self.screen = pending.resume_screen;
                    self.run_action(pending.action, false, cx);
                }
            }
            AuthOutcome::MissingRepoScope { account, scopes } => {
                self.account = account;
                self.scopes = scopes;
                self.scope_to_authorize = "repo";
                self.error_msg = missing_scope_message("repo");
            }
            AuthOutcome::NotLoggedIn => self.check_gh_cli(false, cx),
            AuthOutcome::Revoked => {
                self.check_gh_cli(false, cx);
                self.error_msg = format!(
                    "GitHub no longer accepts the saved sign-in for {}. Sign in again.",
                    Self::account_name(&login, cx)
                );
                HelmAccounts::emit(HelmAccountEvent::SignedOut { login }, cx);
            }
            AuthOutcome::Failed(error) => {
                self.load_state = LoadState::Error;
                self.error_msg = error;
            }
        }
        cx.notify();
    }

    /// Reacts to an account signing in or out in any window.
    pub(super) fn handle_account_event(&mut self, event: &HelmAccountEvent, cx: &mut Context<Self>) {
        let ours = self
            .active_login
            .as_deref()
            .is_some_and(|login| login.eq_ignore_ascii_case(event.login()));
        match event {
            // A panel with no account yet takes the first one signed in to.
            HelmAccountEvent::SignedIn { .. } if ours || self.active_login.is_none() => {
                if self.active_login.is_none() {
                    self.active_login = Some(event.login().to_string());
                }
                self.start_session(cx);
            }
            HelmAccountEvent::SignedOut { .. } if ours => self.reset_session(cx),
            // The account is gone: fall back to the default account, or the
            // sign-in screen.
            HelmAccountEvent::Removed { login } if ours => {
                // The settings file may not have been reloaded yet, so the
                // removed account can still be listed (even as default).
                let settings = HelmPanelSettings::get_global(cx);
                let remaining: Vec<&HelmAccount> = settings
                    .accounts
                    .iter()
                    .filter(|account| !account.login.eq_ignore_ascii_case(login))
                    .collect();
                let fallback = settings
                    .default_account()
                    .filter(|account| !account.login.eq_ignore_ascii_case(login))
                    .or(match remaining.as_slice() {
                        [only] => Some(*only),
                        _ => None,
                    })
                    .map(|account| account.login.clone());

                self.drop_current_token(cx);
                self.save_account_binding(None, cx);
                let has_fallback = fallback.is_some();
                self.use_account(fallback, cx);
                if has_fallback {
                    self.start_session(cx);
                } else {
                    self.auth_initialized = true;
                    self.load_state = LoadState::Idle;
                    cx.notify();
                }
            }
            _ => {}
        }
    }

    /// Forgets everything about the signed-in account and shows the
    /// sign-in screen for it. The account stays this panel's.
    fn reset_session(&mut self, cx: &mut Context<Self>) {
        self.drop_current_token(cx);
        // Kept: a notice about why the account was signed out.
        let notice = std::mem::take(&mut self.error_msg);
        let login = self.active_login.clone();
        self.use_account(login, cx);
        self.error_msg = notice;
        self.auth_initialized = true;
        self.load_state = LoadState::Idle;
        cx.notify();
    }

    /// Starts a device-flow sign-in: shows a one-time code, then asks
    /// GitHub every few seconds whether the user has entered it.
    pub(super) fn start_sign_in(&mut self, purpose: SignInFor, cx: &mut Context<Self>) {
        let mut scopes: Vec<String> = HELM_OAUTH_SCOPES.iter().map(|s| s.to_string()).collect();
        if let SignInFor::Scope { scope, .. } = &purpose {
            // Ask for everything the token has as well, so adding a scope
            // does not drop one.
            for scope in self.scopes.iter().map(String::as_str).chain([*scope]) {
                if !scopes.iter().any(|known| known == scope) {
                    scopes.push(scope.to_string());
                }
            }
        }

        self.sign_in = SignIn::Starting;
        self.sign_in_for = purpose;
        self.code_copied = false;
        self.error_msg.clear();
        cx.notify();

        let http = self.gh_state.http.clone();
        self.sign_in_task = Some(cx.spawn(async move |this, cx| {
            let requested = {
                let http = http.clone();
                on_tokio(async move {
                    let scopes: Vec<&str> = scopes.iter().map(String::as_str).collect();
                    request_device_code(http.as_ref(), &scopes).await
                })
                .await
            };
            let mut flow = match requested {
                Ok(flow) => flow,
                Err(error) => {
                    this.update(cx, |this, cx| this.sign_in_failed(error.to_string(), cx))
                        .ok();
                    return;
                }
            };
            let shown = this.update(cx, |this, cx| {
                this.sign_in = SignIn::Waiting(flow.clone());
                cx.notify();
            });
            if shown.is_err() {
                return;
            }

            let expires_at = Instant::now() + Duration::from_secs(flow.expires_in);
            let token = loop {
                cx.background_executor()
                    .timer(Duration::from_secs(flow.interval))
                    .await;
                if Instant::now() >= expires_at {
                    break Err("The sign-in code expired. Start the sign-in again.".to_string());
                }
                let polled = {
                    let http = http.clone();
                    let flow = flow.clone();
                    on_tokio(async move { poll_for_token(http.as_ref(), &flow).await }).await
                };
                match polled {
                    Ok(PollOutcome::Token(token)) => break Ok(token),
                    Ok(PollOutcome::Pending) => {}
                    Ok(PollOutcome::SlowDown { interval }) => flow.interval = interval,
                    // A dropped connection is worth another try before the
                    // code expires.
                    Err(GhError::Network(error)) => {
                        log::warn!("helm_panel: polling for the GitHub sign-in failed: {error}");
                    }
                    Err(error) => break Err(error.to_string()),
                }
            };
            this.update(cx, |this, cx| match token {
                Ok(token) => this.identify_token(token, HelmAccountSource::OauthApp, cx),
                Err(error) => this.sign_in_failed(error, cx),
            })
            .ok();
        }));
    }

    /// Signs in with a personal access token pasted into the auth screen.
    pub(super) fn submit_pasted_token(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let token = self.token_input.read(cx).value().trim().to_string();
        if token.is_empty() {
            return;
        }
        self.token_input
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.identify_token(token, HelmAccountSource::Token, cx);
    }

    /// Asks GitHub whose `token` is, then stores it for that account, unless
    /// the sign-in was for a different account.
    pub(super) fn identify_token(
        &mut self,
        token: String,
        source: HelmAccountSource,
        cx: &mut Context<Self>,
    ) {
        self.sign_in = SignIn::Finishing;
        self.error_msg.clear();
        cx.notify();

        let http = self.gh_state.http.clone();
        self.sign_in_task = Some(cx.spawn(async move |this, cx| {
            let identity = on_tokio(async move {
                // A state of its own, so the panel's token and remembered
                // answers are not touched until the new token is accepted.
                let probe = GhState::new(http);
                probe.set_token(Some(token.clone())).await;
                gh_get_current_user(&probe)
                    .await
                    .map(|user| (user.login, token))
            })
            .await;
            this.update(cx, |this, cx| match identity {
                Ok((login, token)) => {
                    if let Some(expected) = this.sign_in_for.expected_login()
                        && !expected.eq_ignore_ascii_case(&login)
                    {
                        this.sign_in = SignIn::WrongAccount {
                            expected: expected.to_string(),
                            actual: login,
                            token,
                            source,
                        };
                        cx.notify();
                        return;
                    }
                    this.store_account(login, token, source, cx);
                }
                Err(GhError::Unauthorized { .. }) => {
                    this.sign_in_failed("GitHub did not accept that token.".into(), cx)
                }
                Err(error) => this.sign_in_failed(error.to_string(), cx),
            })
            .ok();
        }));
    }

    /// Stores `token` as `login`'s, lists the account in settings and makes
    /// it this panel's. Every panel bound to `login` then reloads.
    fn store_account(
        &mut self,
        login: String,
        token: String,
        source: HelmAccountSource,
        cx: &mut Context<Self>,
    ) {
        self.sign_in = SignIn::Finishing;
        cx.notify();

        self.sign_in_task = Some(cx.spawn(async move |this, cx| {
            let stored = write_token(&login, &token, cx).await;
            this.update(cx, |this, cx| match stored {
                Ok(()) => {
                    remember_account(&login, source, cx);
                    if source == HelmAccountSource::GhCli {
                        this.notify(
                            format!(
                                "Imported {login} from the GitHub CLI. Signing out of gh does not sign Helm out; use Sign out in Helm's account menu."
                            ),
                            cx,
                        );
                    }
                    // Signed in to an account this panel was not using (a
                    // new account, or "Use a different account"): switch to
                    // it with fresh state, so nothing of the previous
                    // account, a parked action included, carries over.
                    if !this
                        .active_login
                        .as_deref()
                        .is_some_and(|active| active.eq_ignore_ascii_case(&login))
                    {
                        this.use_account(Some(login.clone()), cx);
                        this.save_account_binding(Some(&login), cx);
                    }
                    this.adding_account = false;
                    this.sign_in = SignIn::Idle;
                    HelmAccounts::emit(HelmAccountEvent::SignedIn { login }, cx);
                }
                Err(error) => {
                    this.sign_in_failed(format!("Could not save the GitHub sign-in: {error:#}"), cx)
                }
            })
            .ok();
        }));
    }

    pub(super) fn sign_in_failed(&mut self, error: String, cx: &mut Context<Self>) {
        self.sign_in = SignIn::Failed(error);
        cx.notify();
    }

    /// Stops a sign-in in progress. Dropping its task stops the polling.
    pub(super) fn cancel_sign_in(&mut self, cx: &mut Context<Self>) {
        self.sign_in_task = None;
        self.sign_in = SignIn::Idle;
        // "Add account" was started from a working session: go back to it.
        if std::mem::take(&mut self.adding_account) && self.user.is_some() {
            self.set_screen(HelmScreen::Menu, cx);
        }
        cx.notify();
    }

    pub(super) fn prepare_workspace_scope_authorization(&mut self, cx: &mut Context<Self>) {
        self.screen = HelmScreen::Auth;
        self.auth_initialized = true;
        self.cancel_sign_in(cx);
        self.pending_action = None;
        self.scope_to_authorize = "repo";
        self.load_state = LoadState::Idle;
        self.error_msg = missing_scope_message("repo");
        cx.notify();
    }

    /// Re-runs the sign-in for the current account, asking for
    /// `self.scope_to_authorize` on top of the scopes it already has.
    pub(super) fn handle_authorize_scope(&mut self, cx: &mut Context<Self>) {
        let Some(login) = self.bound_login(cx) else {
            self.start_sign_in(SignInFor::AnyAccount, cx);
            return;
        };
        let scope = self.scope_to_authorize;
        self.start_sign_in(SignInFor::Scope { login, scope }, cx);
    }

    /// Copies the device code and flips the icon to a checkmark for 2s.
    pub(super) fn handle_copy_code(&mut self, cx: &mut Context<Self>) {
        let SignIn::Waiting(flow) = &self.sign_in else {
            return;
        };
        cx.write_to_clipboard(ClipboardItem::new_string(flow.user_code.clone()));
        self.code_copied = true;
        cx.notify();

        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_secs(2)).await;
            this.update(cx, |this, cx| {
                this.code_copied = false;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Signs the panel's account out: deletes its stored token. The account
    /// stays listed in settings, as needing sign-in, and every window using
    /// it goes back to the sign-in screen.
    pub(super) fn handle_logout(&mut self, cx: &mut Context<Self>) {
        let Some(login) = self.bound_login(cx) else {
            self.reset_session(cx);
            return;
        };
        self.load_state = LoadState::Loading;
        cx.notify();

        cx.spawn(async move |this, cx| {
            let deleted = delete_token(&login, cx).await;
            this.update(cx, |this, cx| match deleted {
                Ok(()) => HelmAccounts::emit(HelmAccountEvent::SignedOut { login }, cx),
                Err(error) => {
                    this.load_state = LoadState::Error;
                    this.notify(format!("Could not sign out: {error:#}"), cx);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// The sign-in screen, in whichever state the sign-in is.
    pub(super) fn render_auth(&self, cx: &mut Context<Self>) -> impl IntoElement {
        // The stored sign-in hasn't been checked yet: never flash the
        // sign-in button.
        if !self.auth_initialized && self.sign_in.is_idle() {
            return loading_screen("Checking GitHub sign-in…", cx).into_any_element();
        }
        if self.sign_in.is_idle() && self.load_state == LoadState::Loading {
            return loading_screen("Checking GitHub sign-in…", cx).into_any_element();
        }

        match &self.sign_in {
            SignIn::Idle => {}
            SignIn::Starting | SignIn::Finishing => {
                let label = if matches!(self.sign_in, SignIn::Starting) {
                    "Asking GitHub for a sign-in code…"
                } else {
                    "Signing in…"
                };
                return self.render_sign_in_progress(None, label, cx);
            }
            SignIn::Waiting(flow) => {
                return self.render_sign_in_progress(
                    Some(flow),
                    "Open the link, enter the code, then return here.",
                    cx,
                );
            }
            SignIn::Failed(error) => {
                return v_flex()
                    .gap_3()
                    .p_4()
                    .child(Alert::error("helm-login-error", error.clone()))
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                Button::new("auth-retry")
                                    .primary()
                                    .label("Try again")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.start_sign_in(this.sign_in_for.clone(), cx)
                                    })),
                            )
                            .child(
                                Button::new("auth-back")
                                    .outline()
                                    .label("Back")
                                    .on_click(
                                        cx.listener(|this, _, _, cx| this.cancel_sign_in(cx)),
                                    ),
                            ),
                    )
                    .into_any_element();
            }
            SignIn::WrongAccount {
                expected, actual, ..
            } => {
                return self.render_wrong_account(expected, actual, cx);
            }
        }

        // Missing scope: `repo` from the startup check, or whichever scope
        // a rejected action needs.
        if self.error_msg == missing_scope_message(self.scope_to_authorize) {
            // An account imported from `gh` gets its scope there.
            if let Some(login) = self.bound_login(cx)
                && HelmPanelSettings::get_global(cx)
                    .account(&login)
                    .is_some_and(|account| account.source == HelmAccountSource::GhCli)
            {
                return self.render_gh_cli_scope_prompt(&login, self.scope_to_authorize, cx);
            }
            let muted_foreground = cx.theme().muted_foreground;
            let scope = self.scope_to_authorize;
            return v_flex()
                .gap_3()
                .p_4()
                .child(Alert::warning(
                    "helm-scope-alert",
                    format!("Missing '{scope}' scope"),
                ))
                .child(div().text_sm().text_color(muted_foreground).child(
                    if self.pending_action.is_some() {
                        format!(
                            "GitHub rejected your last change because this sign-in lacks \
                             the {scope} scope. Authorize it and Helm will send the \
                             change again."
                        )
                    } else {
                        format!("Helm needs the {scope} scope for this GitHub operation.")
                    },
                ))
                .child(
                    Button::new("auth-authorize-scope")
                        .primary()
                        .label(format!("Authorize {scope} scope"))
                        .on_click(cx.listener(|this, _, _, cx| this.handle_authorize_scope(cx))),
                )
                .into_any_element();
        }

        self.render_sign_in_start(cx)
    }

    /// The device code and link while GitHub waits for the user, or a
    /// spinner before the code arrives and after the token does.
    fn render_sign_in_progress(
        &self,
        flow: Option<&DeviceFlow>,
        hint: &'static str,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let muted_foreground = cx.theme().muted_foreground;
        let foreground = cx.theme().foreground;
        let title = match self.sign_in_for.expected_login() {
            Some(login) => format!("Sign in to GitHub as {}", Self::account_name(login, cx)),
            None => "Sign in to GitHub".to_string(),
        };

        let mut col = v_flex()
            .gap_3()
            .p_4()
            .child(div().text_color(foreground).child(title))
            .child(Separator::horizontal());

        col = match flow {
            Some(flow) => {
                let copied = self.code_copied;
                let url = flow.verification_uri.clone();
                col.child(
                    v_flex()
                        .gap_1()
                        .child(
                            div()
                                .text_xs()
                                .text_color(muted_foreground)
                                .child("Your one-time code"),
                        )
                        .child(
                            h_flex()
                                .items_center()
                                .gap_2()
                                .child(
                                    div()
                                        .font_family("Cascadia Mono")
                                        .text_lg()
                                        .text_color(foreground)
                                        .child(flow.user_code.clone()),
                                )
                                .child(
                                    Button::new("auth-copy-code")
                                        .ghost()
                                        .xsmall()
                                        .icon(if copied {
                                            IconName::Check
                                        } else {
                                            IconName::Copy
                                        })
                                        .tooltip(if copied { "Copied" } else { "Copy code" })
                                        .on_click(
                                            cx.listener(|this, _, _, cx| this.handle_copy_code(cx)),
                                        ),
                                ),
                        ),
                )
                .child(
                    Button::new("auth-open-device-url")
                        .outline()
                        .icon(IconName::Github)
                        .label("Open")
                        .child(Icon::new(IconName::ExternalLink).xsmall())
                        .tooltip(url.clone())
                        .on_click(move |_, _, cx| cx.open_url(&url)),
                )
            }
            None => col.child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(Spinner::new().small())
                    .child(div().text_sm().text_color(muted_foreground).child(hint)),
            ),
        };

        if flow.is_some() {
            col = col.child(div().text_sm().text_color(muted_foreground).child(hint));
        }
        if !matches!(self.sign_in, SignIn::Finishing) {
            col = col.child(
                Button::new("auth-cancel")
                    .ghost()
                    .label("Cancel")
                    .on_click(cx.listener(|this, _, _, cx| this.cancel_sign_in(cx))),
            );
        }
        col.into_any_element()
    }

    /// The browser signed in a different account than the one asked for.
    fn render_wrong_account(
        &self,
        expected: &str,
        actual: &str,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let muted_foreground = cx.theme().muted_foreground;
        let expected = Self::account_name(expected, cx);
        v_flex()
            .gap_3()
            .p_4()
            .child(Alert::warning(
                "helm-wrong-account",
                format!("You signed in as {actual}, not {expected}."),
            ))
            .child(div().text_sm().text_color(muted_foreground).child(format!(
                "GitHub signs in whichever account your browser is using. To sign in as \
                 {expected}, use a private browser window, or sign out of {actual} on \
                 github.com first."
            )))
            .child(
                h_flex()
                    .gap_2()
                    .flex_wrap()
                    .child(
                        Button::new("auth-wrong-try-again")
                            .primary()
                            .label("Try again")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.start_sign_in(this.sign_in_for.clone(), cx)
                            })),
                    )
                    .child(
                        Button::new("auth-wrong-add")
                            .outline()
                            .label(format!("Add {actual} as a new account"))
                            .on_click(cx.listener(|this, _, _, cx| {
                                let SignIn::WrongAccount {
                                    actual,
                                    token,
                                    source,
                                    ..
                                } = std::mem::replace(&mut this.sign_in, SignIn::Idle)
                                else {
                                    return;
                                };
                                this.store_account(actual, token, source, cx);
                            })),
                    )
                    .child(
                        Button::new("auth-wrong-cancel")
                            .ghost()
                            .label("Cancel")
                            .on_click(cx.listener(|this, _, _, cx| this.cancel_sign_in(cx))),
                    ),
            )
            .into_any_element()
    }

    /// Nothing in progress: the button that starts a sign-in, and the
    /// pasted-token option.
    fn render_sign_in_start(&self, cx: &mut Context<Self>) -> gpui::AnyElement {
        let muted_foreground = cx.theme().muted_foreground;
        let foreground = cx.theme().foreground;
        // "Add account" from the menu: any account, nobody to sign in again.
        let known_login = if self.adding_account {
            None
        } else {
            self.active_login.clone()
        };

        let mut col = v_flex()
            .gap_3()
            .p_4()
            .child(div().text_color(foreground).child(if self.adding_account {
                "Add a GitHub account"
            } else {
                "GitHub"
            }))
            .child(Separator::horizontal());

        if !self.error_msg.is_empty() {
            col = col.child(Alert::warning("helm-auth-notice", self.error_msg.clone()));
        }

        col = match known_login {
            Some(login) => {
                let name = Self::account_name(&login, cx);
                col.child(div().text_sm().text_color(muted_foreground).child(format!(
                    "{name} needs to sign in again on this machine."
                )))
                .child(
                    Button::new("helm-login")
                        .primary()
                        .icon(IconName::Github)
                        .label(format!("Sign in as {name}"))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.start_sign_in(SignInFor::Account(login.clone()), cx)
                        })),
                )
                .child(
                    Button::new("helm-login-other")
                        .ghost()
                        .small()
                        .label("Use a different account")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.start_sign_in(SignInFor::AnyAccount, cx)
                        })),
                )
            }
            None => col
                .child(div().text_sm().text_color(muted_foreground).child(
                    "Connect your GitHub account to manage repositories and organizations.",
                ))
                .child(
                    Button::new("helm-login")
                        .primary()
                        .icon(IconName::Github)
                        .label("Login with GitHub")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.start_sign_in(SignInFor::AnyAccount, cx)
                        })),
                ),
        };

        let others: Vec<HelmAccount> = HelmPanelSettings::get_global(cx)
            .accounts
            .iter()
            .filter(|account| {
                !self
                    .active_login
                    .as_deref()
                    .is_some_and(|login| login.eq_ignore_ascii_case(&account.login))
            })
            .cloned()
            .collect();
        if !others.is_empty() && !self.adding_account {
            col = col.child(
                div()
                    .text_xs()
                    .text_color(muted_foreground)
                    .child("Or use another account:"),
            );
            for (ix, account) in others.into_iter().enumerate() {
                let label = format!("Switch to {}", account.display_name());
                col = col.child(
                    Button::new(("helm-switch-account", ix))
                        .outline()
                        .small()
                        .label(label)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.switch_account(account.login.clone(), cx)
                        })),
                );
            }
        }

        col = col.child(Separator::horizontal());
        col = col.children(self.render_gh_cli_option(cx));
        if self.adding_account {
            col = col.child(
                Button::new("helm-add-account-cancel")
                    .ghost()
                    .small()
                    .label("Back to the current account")
                    .on_click(cx.listener(|this, _, _, cx| this.cancel_sign_in(cx))),
            );
        }
        col = if self.token_input_open {
            col.child(
                v_flex()
                    .gap_2()
                    .child(div().text_sm().text_color(muted_foreground).child(
                        "Paste a personal access token. A classic token needs the repo, \
                         read:org and user scopes.",
                    ))
                    .child(Input::new(&self.token_input).mask_toggle())
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                Button::new("helm-token-submit")
                                    .outline()
                                    .label("Sign in with token")
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.submit_pasted_token(window, cx)
                                    })),
                            )
                            .child(
                                Button::new("helm-token-close")
                                    .ghost()
                                    .label("Cancel")
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.token_input_open = false;
                                        cx.notify();
                                    })),
                            ),
                    ),
            )
        } else {
            col.child(
                Button::new("helm-token-open")
                    .ghost()
                    .small()
                    .label("Use a token instead")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.token_input_open = true;
                        cx.notify();
                    })),
            )
        };

        col.child(
            div()
                .text_xs()
                .text_color(muted_foreground)
                .child(
                    "Organizations that restrict third-party apps must approve Helm before \
                     their repositories show up. You can request approval from Helm's page \
                     in your GitHub settings.",
                ),
        )
        .child(
            Link::new("helm-app-settings")
                .href(app_settings_url())
                .child("Helm in your GitHub settings"),
        )
        .into_any_element()
    }
}
