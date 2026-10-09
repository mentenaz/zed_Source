//! Signing in to GitHub through the `gh` CLI: the CLI check, the device-code
//! login, authorizing a missing scope, signing out, and the gate and auth
//! screens.

use super::*;

/// Pulls a `XXXX-XXXX` device code and a `https://github.com/login/device...`
/// URL out of a line of `gh auth login` output, if present.
pub(super) fn parse_device_line(line: &str) -> (Option<String>, Option<String>) {
    let code = line
        .split(|c: char| !c.is_ascii_alphanumeric() && c != '-')
        .find(|tok| {
            let bytes = tok.as_bytes();
            bytes.len() == 9
                && bytes[4] == b'-'
                && bytes[..4].iter().all(|b| b.is_ascii_alphanumeric())
                && bytes[5..].iter().all(|b| b.is_ascii_alphanumeric())
                && tok
                    .chars()
                    .any(|c| c.is_ascii_digit() || c.is_ascii_uppercase())
        })
        .map(|s| s.to_string());

    let url = line.find("https://github.com/login/device").map(|start| {
        line[start..]
            .split(|c: char| c.is_whitespace())
            .next()
            .unwrap_or("")
            .trim_end_matches(['.', ')'])
            .to_string()
    });

    (code, url)
}

impl HelmPanel {
    pub(super) fn check_cli(&mut self, cx: &mut Context<Self>) {
        self.screen = HelmScreen::Gate;
        self.load_state = LoadState::Loading;
        self.error_msg.clear();
        cx.notify();

        cx.spawn(async move |this, cx| {
            let result = on_tokio(gh_check_cli()).await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(()) => {
                        this.load_state = LoadState::Idle;
                        this.error_msg.clear();
                        this.do_auth(cx);
                    }
                    Err(_) => {
                        this.load_state = LoadState::Idle;
                        this.error_msg = "GitHub CLI (gh) is not installed or not on PATH.".into();
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn do_auth(&mut self, cx: &mut Context<Self>) {
        self.screen = HelmScreen::Auth;
        self.load_state = LoadState::Loading;
        self.error_msg.clear();
        cx.notify();

        let gh_state = self.gh_state.clone();
        cx.spawn(async move |this, cx| {
            let outcome = on_tokio(async move {
                let info = match gh_auth_status().await {
                    Ok(info) => info,
                    Err(e) => return AuthOutcome::Failed(e.to_string()),
                };
                let Some(info) = info else {
                    return AuthOutcome::NotLoggedIn;
                };
                if !info.scopes.iter().any(|s| s == "repo") {
                    return AuthOutcome::MissingRepoScope {
                        account: info.account,
                        scopes: info.scopes,
                    };
                }
                let orgs = match gh_get_org_logins(&gh_state).await {
                    Ok(o) => o,
                    Err(e) => return AuthOutcome::Failed(e.to_string()),
                };
                let user = match gh_get_current_user(&gh_state).await {
                    Ok(u) => u,
                    Err(e) => return AuthOutcome::Failed(e.to_string()),
                };
                AuthOutcome::Ready {
                    account: info.account,
                    scopes: info.scopes,
                    orgs,
                    user,
                }
            })
            .await;

            this.update(cx, |this, cx| {
                this.auth_initialized = true;
                match outcome {
                    AuthOutcome::Ready {
                        account,
                        scopes,
                        orgs,
                        user,
                    } => {
                        this.account = account;
                        this.scopes = scopes;
                        this.org_logins.items = orgs;
                        this.user = Some(user);
                        this.load_state = LoadState::Idle;
                        this.error_msg.clear();
                        this.screen = HelmScreen::Menu;
                        this.load_repo_invitations(cx);
                        // An action that was bounced here for a missing
                        // scope: put the user back where they were and
                        // re-send it. `may_reauthorize: false` so a second
                        // rejection reports the failure instead of looping
                        // the gate.
                        if let Some(pending) = this.pending_action.take() {
                            this.screen = pending.resume_screen;
                            this.run_action(pending.action, false, cx);
                        }
                    }
                    AuthOutcome::MissingRepoScope { account, scopes } => {
                        this.account = account;
                        this.scopes = scopes;
                        this.load_state = LoadState::Idle;
                        this.scope_to_authorize = "repo";
                        this.error_msg = missing_scope_message("repo");
                    }
                    AuthOutcome::NotLoggedIn => {
                        this.load_state = LoadState::Idle;
                    }
                    AuthOutcome::Failed(e) => {
                        this.load_state = LoadState::Error;
                        this.error_msg = e;
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn handle_login(&mut self, cx: &mut Context<Self>) {
        self.login_started = true;
        self.device_code.clear();
        self.device_url.clear();
        self.code_copied = false;
        self.load_state = LoadState::Loading;
        self.error_msg.clear();
        cx.notify();

        // Subscribe before starting the login process so no early lines are
        // missed.
        self.listen_for_device_code(cx);

        let gh_state = self.gh_state.clone();
        cx.spawn(async move |this, cx| {
            let result = on_tokio(async move { gh_login(&gh_state).await }).await;
            this.update(cx, |this, cx| match result {
                Ok(()) => this.do_auth(cx),
                Err(e) => {
                    this.load_state = LoadState::Error;
                    this.error_msg = e;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Mirrors the device code and verification URL that `gh auth login` /
    /// `gh auth refresh` print onto the panel, until the command reports
    /// `Done`. Call before spawning the command.
    pub(super) fn listen_for_device_code(&mut self, cx: &mut Context<Self>) {
        let mut rx = self.gh_state.auth_tx.subscribe();
        cx.spawn(async move |this, cx| {
            loop {
                let (event, rx2) = on_tokio(async move {
                    let event = rx.recv().await;
                    (event, rx)
                })
                .await;
                rx = rx2;
                match event {
                    Ok(GhAuthEvent::Line(line)) => {
                        let (code, url) = parse_device_line(&line);
                        if code.is_none() && url.is_none() {
                            continue;
                        }
                        let alive = this
                            .update(cx, |this, cx| {
                                if let Some(c) = code {
                                    this.device_code = c;
                                }
                                if let Some(u) = url {
                                    this.device_url = u;
                                }
                                cx.notify();
                            })
                            .is_ok();
                        if !alive {
                            break;
                        }
                    }
                    Ok(GhAuthEvent::Done) => break,
                    // `Lagged` means missed events, not a dead channel —
                    // resync and keep listening; only `Closed` ends this loop.
                    Err(EventRecvError::Lagged(_)) => continue,
                    Err(EventRecvError::Closed) => break,
                }
            }
        })
        .detach();
    }

    pub(super) fn prepare_workspace_scope_authorization(&mut self, cx: &mut Context<Self>) {
        self.screen = HelmScreen::Auth;
        self.auth_initialized = true;
        self.login_started = false;
        self.pending_action = None;
        self.scope_to_authorize = "repo";
        self.load_state = LoadState::Idle;
        self.error_msg = missing_scope_message("repo");
        self.device_code.clear();
        self.device_url.clear();
        self.code_copied = false;
        cx.notify();
    }

    /// Runs `gh auth refresh -s <scope> --hostname github.com` for
    /// `self.scope_to_authorize` and shows its device code the same way a
    /// first login does — `login_started` is what switches `render_auth`
    /// over to the code/URL view, without which the refresh would sit
    /// waiting on a code the user was never shown.
    pub(super) fn handle_authorize_scope(&mut self, cx: &mut Context<Self>) {
        self.login_started = true;
        self.device_code.clear();
        self.device_url.clear();
        self.code_copied = false;
        self.load_state = LoadState::Loading;
        self.error_msg.clear();
        cx.notify();

        self.listen_for_device_code(cx);

        let scope = self.scope_to_authorize;
        let gh_state = self.gh_state.clone();
        cx.spawn(async move |this, cx| {
            let result = on_tokio(async move { gh_ensure_scope(scope, &gh_state).await }).await;
            this.update(cx, |this, cx| match result {
                Ok(()) => this.do_auth(cx),
                Err(e) => {
                    this.load_state = LoadState::Error;
                    this.error_msg = e;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Copies the device code and flips the icon to a checkmark for 2s.
    pub(super) fn handle_copy_code(&mut self, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(self.device_code.clone()));
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

    pub(super) fn handle_logout(&mut self, cx: &mut Context<Self>) {
        self.load_state = LoadState::Loading;
        cx.notify();

        let gh_state = self.gh_state.clone();
        cx.spawn(async move |this, cx| {
            let result = on_tokio(async move { gh_logout(&gh_state).await }).await;
            this.update(cx, |this, cx| match result {
                Ok(()) => {
                    this.account.clear();
                    this.scopes.clear();
                    this.org_logins.clear();
                    this.user = None;
                    this.auth_initialized = false;
                    this.login_started = false;
                    this.pending_action = None;
                    this.device_code.clear();
                    this.device_url.clear();
                    this.code_copied = false;
                    this.back_stack.clear();
                    this.forward_stack.clear();
                    this.check_cli(cx);
                }
                Err(e) => {
                    this.load_state = LoadState::Error;
                    this.error_msg = e;
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// The "gh CLI required" gate — shown until `gh --version` succeeds.
    pub(super) fn render_gate(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let muted_foreground = cx.theme().muted_foreground;

        if self.load_state == LoadState::Loading {
            return loading_screen("Checking for GitHub CLI…", cx);
        }

        v_flex()
            .gap_3()
            .p_4()
            .child(Alert::warning(
                "helm-gate-alert",
                "GitHub CLI (gh) is required",
            ))
            .child(
                div()
                    .text_sm()
                    .text_color(muted_foreground)
                    .child("Helm runs on top of the GitHub CLI."),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(muted_foreground)
                    .child("Install from: https://cli.github.com"),
            )
            .child(
                Button::new("gate-retry")
                    .outline()
                    .label("Retry")
                    .on_click(cx.listener(|this, _, _, cx| this.check_cli(cx))),
            )
            .into_any_element()
    }

    /// Login / scope-authorization screen.
    pub(super) fn render_auth(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let muted_foreground = cx.theme().muted_foreground;
        let foreground = cx.theme().foreground;

        // Initial `do_auth()` hasn't resolved yet — never flash the login
        // button.
        if !self.auth_initialized && !self.login_started {
            return loading_screen("Checking GitHub login…", cx);
        }

        // Missing scope — `repo` from the startup check, or whichever scope
        // a rejected action needs.
        if self.error_msg == missing_scope_message(self.scope_to_authorize) && !self.login_started {
            let loading = self.load_state == LoadState::Loading;
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
                            "GitHub rejected your last change because this login lacks \
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
                        .disabled(loading)
                        .on_click(cx.listener(|this, _, _, cx| this.handle_authorize_scope(cx))),
                )
                .into_any_element();
        }

        // Login in progress — device code + URL.
        if self.login_started && self.load_state == LoadState::Loading {
            let mut col = v_flex()
                .gap_3()
                .p_4()
                .child(div().text_color(foreground).child("Connect to GitHub"))
                .child(Separator::horizontal());

            col = if !self.device_code.is_empty() {
                let copied = self.code_copied;
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
                                        .child(self.device_code.clone()),
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
            } else {
                col.child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(Spinner::new().small())
                        .child(
                            div()
                                .text_sm()
                                .text_color(muted_foreground)
                                .child("Starting device flow…"),
                        ),
                )
            };

            if !self.device_url.is_empty() {
                let url = self.device_url.clone();
                col = col.child(
                    Button::new("auth-open-device-url")
                        .outline()
                        .icon(IconName::Github)
                        .label("Open")
                        .child(Icon::new(IconName::ExternalLink).xsmall())
                        .tooltip(url.clone())
                        .on_click(move |_, _, cx| {
                            cx.open_url(&url);
                        }),
                );
            }

            col = col.child(div().text_sm().text_color(muted_foreground).child(
                if self.device_url.is_empty() {
                    "Waiting for GitHub…"
                } else {
                    "Open the link, enter the code, then return here."
                },
            ));

            return col.into_any_element();
        }

        // Login failed.
        if self.login_started && self.load_state == LoadState::Error {
            return v_flex()
                .gap_3()
                .p_4()
                .child(Alert::error("helm-login-error", self.error_msg.clone()))
                .child(
                    Button::new("auth-retry")
                        .outline()
                        .label("Try again")
                        .on_click(cx.listener(|this, _, _, cx| this.handle_login(cx))),
                )
                .into_any_element();
        }

        // Not logged in — show the login button.
        v_flex()
            .gap_3()
            .p_4()
            .child(div().text_color(foreground).child("GitHub"))
            .child(Separator::horizontal())
            .child(
                div()
                    .text_sm()
                    .text_color(muted_foreground)
                    .child("Connect your GitHub account to manage repositories and organizations."),
            )
            .child(
                Button::new("helm-login")
                    .primary()
                    .icon(IconName::Github)
                    .label("Login with GitHub")
                    .on_click(cx.listener(|this, _, _, cx| this.handle_login(cx))),
            )
            .into_any_element()
    }
}
