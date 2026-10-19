//! "Use GitHub CLI": signing in by copying an account's token from the
//! GitHub CLI once. Offered next to Helm's own sign-in only when `gh` is
//! installed. An imported account is then stored and used like any other;
//! only adding a missing scope differs, since that has to happen in `gh`
//! (Helm's own sign-in would swap in a token of Helm's app, without the
//! organisation approvals `gh`'s app may have).

use super::*;

impl HelmPanel {
    /// Finds out once whether `gh` is installed, so the sign-in screen
    /// knows whether to offer it. `force` checks again (when the picker
    /// opens, to list the accounts as they are now).
    pub(super) fn check_gh_cli(&mut self, force: bool, cx: &mut Context<Self>) {
        let checked = !matches!(self.gh_cli, GhCliState::Unknown | GhCliState::Failed(_));
        if (checked && !force) || matches!(self.gh_cli, GhCliState::Checking) {
            return;
        }
        self.gh_cli = GhCliState::Checking;
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = on_tokio(gh_cli_accounts()).await;
            this.update(cx, |this, cx| {
                this.gh_cli = match result {
                    Ok(None) => GhCliState::NotInstalled,
                    Ok(Some(accounts)) => GhCliState::Listed(accounts),
                    Err(error) => GhCliState::Failed(error),
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub(super) fn open_gh_cli_picker(&mut self, cx: &mut Context<Self>) {
        self.gh_cli_picker_open = true;
        self.check_gh_cli(true, cx);
    }

    /// Copies `login`'s token from `gh` and signs in with it. The token must
    /// turn out to belong to `login`, and to whichever account this sign-in
    /// was for.
    pub(super) fn import_from_gh_cli(&mut self, login: String, cx: &mut Context<Self>) {
        self.gh_cli_picker_open = false;
        if self.sign_in_for == SignInFor::AnyAccount {
            self.sign_in_for = SignInFor::Account(login.clone());
        }
        self.sign_in = SignIn::Finishing;
        self.error_msg.clear();
        cx.notify();

        self.sign_in_task = Some(cx.spawn(async move |this, cx| {
            let token = on_tokio(async move { gh_cli_token(&login).await }).await;
            this.update(cx, |this, cx| match token {
                Ok(token) => this.identify_token(token, HelmAccountSource::GhCli, cx),
                Err(error) => this.sign_in_failed(error, cx),
            })
            .ok();
        }));
    }

    /// The "Use GitHub CLI" button, or the list of `gh`'s accounts once it
    /// is open. Nothing when `gh` is not installed.
    pub(super) fn render_gh_cli_option(&self, cx: &mut Context<Self>) -> Option<gpui::AnyElement> {
        let muted_foreground = cx.theme().muted_foreground;
        if !self.gh_cli_picker_open {
            return match &self.gh_cli {
                GhCliState::Listed(_) | GhCliState::Failed(_) => Some(
                    Button::new("helm-gh-cli-open")
                        .ghost()
                        .small()
                        .label("Use GitHub CLI")
                        .tooltip("Copy an account's token from the GitHub CLI (gh)")
                        .on_click(cx.listener(|this, _, _, cx| this.open_gh_cli_picker(cx)))
                        .into_any_element(),
                ),
                GhCliState::Unknown | GhCliState::Checking | GhCliState::NotInstalled => None,
            };
        }

        let close = Button::new("helm-gh-cli-close")
            .ghost()
            .small()
            .label("Cancel")
            .on_click(cx.listener(|this, _, _, cx| {
                this.gh_cli_picker_open = false;
                cx.notify();
            }));
        let mut col = v_flex().gap_2().child(div().text_sm().text_color(muted_foreground).child(
            "Copy an account's token from the GitHub CLI. Helm keeps its own copy; signing out \
             of gh later does not sign Helm out.",
        ));

        match &self.gh_cli {
            GhCliState::Unknown | GhCliState::Checking => {
                col = col.child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(Spinner::new().small())
                        .child(
                            div()
                                .text_sm()
                                .text_color(muted_foreground)
                                .child("Asking the GitHub CLI for its accounts…"),
                        ),
                );
            }
            GhCliState::NotInstalled => {
                col = col.child(Alert::warning(
                    "helm-gh-cli-missing",
                    "The GitHub CLI (gh) is not installed.",
                ));
            }
            GhCliState::Failed(error) => {
                col = col.child(Alert::error("helm-gh-cli-error", error.clone()));
            }
            GhCliState::Listed(accounts) if accounts.is_empty() => {
                col = col.child(div().text_sm().text_color(muted_foreground).child(
                    "The GitHub CLI is not signed in to any github.com account. Run \
                     `gh auth login` first.",
                ));
            }
            GhCliState::Listed(accounts) => {
                let settings = HelmPanelSettings::get_global(cx);
                for (ix, account) in accounts.iter().enumerate() {
                    let in_helm = settings.account(&account.login).is_some();
                    let has_repo = account.scopes.iter().any(|scope| scope == "repo");
                    let mut notes = Vec::new();
                    if account.active {
                        notes.push("active in gh".to_string());
                    }
                    if in_helm {
                        notes.push("already in Helm; importing replaces its token".to_string());
                    }
                    if !account.usable {
                        notes.push("gh cannot use this sign-in; sign in again in gh".to_string());
                    } else if !has_repo {
                        notes.push("no repo scope; private repositories will not work".to_string());
                    }
                    let scopes = if account.scopes.is_empty() {
                        "scopes unknown".to_string()
                    } else {
                        account.scopes.join(", ")
                    };
                    let login = account.login.clone();
                    col = col.child(
                        v_flex()
                            .gap_0p5()
                            .child(
                                Button::new(("helm-gh-cli-import", ix))
                                    .outline()
                                    .small()
                                    .label(format!("Import {}", account.login))
                                    .disabled(!account.usable)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.import_from_gh_cli(login.clone(), cx)
                                    })),
                            )
                            .child(div().text_xs().text_color(muted_foreground).child(scopes))
                            .children(notes.into_iter().map(|note| {
                                div().text_xs().text_color(muted_foreground).child(note)
                            })),
                    );
                }
            }
        }
        Some(col.child(close).into_any_element())
    }

    /// The missing-scope prompt for an account imported from `gh`: the scope
    /// has to be added in `gh`, then the token imported again.
    pub(super) fn render_gh_cli_scope_prompt(
        &self,
        login: &str,
        scope: &'static str,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let muted_foreground = cx.theme().muted_foreground;
        let commands = format!("gh auth switch --user {login}\ngh auth refresh --scopes {scope}");
        let import_login = login.to_string();
        v_flex()
            .gap_3()
            .p_4()
            .child(Alert::warning(
                "helm-gh-scope-alert",
                format!("Missing '{scope}' scope"),
            ))
            .child(div().text_sm().text_color(muted_foreground).child(format!(
                "{login} was imported from the GitHub CLI, so the scope is added there. Run \
                 these in a terminal, then import the token again:"
            )))
            .child(
                div()
                    .font_family("Cascadia Mono")
                    .text_xs()
                    .text_color(cx.theme().foreground)
                    .child(commands.clone()),
            )
            .child(
                h_flex()
                    .gap_2()
                    .flex_wrap()
                    .child(
                        Button::new("helm-gh-scope-reimport")
                            .primary()
                            .label("Import again")
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.import_from_gh_cli(import_login.clone(), cx)
                            })),
                    )
                    .child(
                        Button::new("helm-gh-scope-copy")
                            .ghost()
                            .label("Copy commands")
                            .on_click(move |_, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(commands.clone()))
                            }),
                    )
                    .child(
                        Button::new("helm-gh-scope-helm")
                            .ghost()
                            .label("Use Helm's sign-in instead")
                            .on_click(cx.listener(|this, _, _, cx| this.handle_authorize_scope(cx))),
                    ),
            )
            .into_any_element()
    }
}
