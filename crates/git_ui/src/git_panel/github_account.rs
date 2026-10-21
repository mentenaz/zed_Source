//! The GitHub account a workspace uses, shown in the git panel (section 11,
//! item 1b of `helm_backend/octocrab-helm-coverage.md`).
//!
//! For a repository on github.com, the footer shows the account the Helm
//! panel uses for this workspace, with a menu to switch, and the email
//! commits are authored with (item 2), with its one-time choice and a
//! question when the repository already names someone else. Over HTTPS it
//! also warns when Helm has found that the account cannot push, with a
//! button to switch to an account that can.
//!
//! Push, pull and fetch log in as that account (Helm hands git its token,
//! section 11 item 3), so when Helm knows the account cannot do one of them,
//! the remote button is shown disabled and the action is refused with the
//! same explanation, instead of running git into an error that does not say
//! the account is the problem.

use super::*;
use helm_panel::{CommitIdentity, HelmPanel, HelmPanelSettings};
use settings::Settings as _;

/// What a remote command needs from the account.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum GitHubNeed {
    /// Fetch and pull: seeing the repository.
    Read,
    /// Push, publish and republish: writing to it.
    Push,
}

/// The toast refusing a remote command.
struct GitHubRefusal;

impl GitPanel {
    /// Why the workspace's GitHub account cannot do what `need` asks, when
    /// Helm knows it cannot, with the account that can (login, name). `None`
    /// when it can, or when nothing is known: not a github.com HTTPS
    /// remote, not signed in to Helm, or not checked yet.
    fn github_refusal(
        &self,
        need: GitHubNeed,
        cx: &App,
    ) -> Option<(String, Option<(String, String)>)> {
        let helm = self.helm_panel()?;
        let helm = helm.read(cx);
        let access = helm.repo_access()?;
        let login = helm.signed_in_login()?;
        if !access.remote.https || !access.login.eq_ignore_ascii_case(login) {
            return None;
        }
        let blocked = match need {
            GitHubNeed::Read => !access.can_read,
            GitHubNeed::Push => access.can_push == Some(false),
        };
        if !blocked {
            return None;
        }
        let settings = HelmPanelSettings::get_global(cx);
        let name_of = |login: &str| {
            settings
                .account(login)
                .map_or_else(|| login.to_string(), |account| account.display_name().to_string())
        };
        let name = name_of(login);
        let repository = access.remote.full_name();
        let reason = match need {
            GitHubNeed::Read => format!("{name} can't see {repository}."),
            GitHubNeed::Push => format!("{name} can't push to {repository}."),
        };
        let suggested = access
            .suggested_login
            .as_deref()
            .map(|suggested| (suggested.to_string(), name_of(suggested)));
        Some((reason, suggested))
    }

    /// Refuses a remote command the workspace's account cannot do, with a
    /// toast saying why and offering the account that can. Returns whether
    /// it refused.
    pub(super) fn refuse_github_remote_command(
        &mut self,
        need: GitHubNeed,
        cx: &mut Context<Self>,
    ) -> bool {
        // Right after an account switch (or at startup) the account's token
        // is not registered yet; git would log in with its own login.
        let loading = self
            .helm_panel()
            .is_some_and(|helm| helm.read(cx).is_loading_github_account());
        if loading {
            let toast = workspace::Toast::new(
                NotificationId::unique::<GitHubRefusal>(),
                "Helm is still loading the GitHub account for this workspace. Try again in a \
                 moment.",
            );
            let workspace = self.workspace.clone();
            cx.defer(move |cx| {
                workspace
                    .update(cx, |workspace, cx| workspace.show_toast(toast, cx))
                    .ok();
            });
            return true;
        }
        let Some((reason, suggested)) = self.github_refusal(need, cx) else {
            return false;
        };
        let mut toast = workspace::Toast::new(NotificationId::unique::<GitHubRefusal>(), reason);
        if let (Some((login, name)), Some(helm)) = (suggested, self.helm_panel.clone()) {
            toast = toast.on_click(format!("Switch to {name}"), move |_, cx| {
                helm.update(cx, |helm, cx| helm.switch_account(login.clone(), cx))
                    .ok();
            });
        }
        // A command can arrive while the workspace is being updated (an
        // action), so the toast is shown once that update has finished.
        let workspace = self.workspace.clone();
        cx.defer(move |cx| {
            workspace
                .update(cx, |workspace, cx| workspace.show_toast(toast, cx))
                .ok();
        });
        true
    }

    /// The remote button drawn disabled, when the command it would run is
    /// one the workspace's account cannot do. `None` otherwise, and the
    /// usual button is drawn.
    pub(super) fn render_refused_remote_button(
        &self,
        branch: &Branch,
        cx: &App,
    ) -> Option<AnyElement> {
        let (label, need) = match branch.upstream.as_ref() {
            Some(Upstream {
                tracking: UpstreamTracking::Tracked(UpstreamTrackingStatus { ahead, behind }),
                ..
            }) => match (*ahead, *behind) {
                (0, 0) => ("Fetch", GitHubNeed::Read),
                (_, 0) => ("Push", GitHubNeed::Push),
                _ => ("Pull", GitHubNeed::Read),
            },
            Some(Upstream {
                tracking: UpstreamTracking::Gone,
                ..
            }) => ("Republish", GitHubNeed::Push),
            None => ("Publish", GitHubNeed::Push),
        };
        let (reason, suggested) = self.github_refusal(need, cx)?;
        let tooltip = match suggested {
            Some((_, name)) => format!("{reason} Switch to {name} to {}.", label.to_lowercase()),
            None => format!("{reason} None of your signed-in accounts can."),
        };
        Some(
            div()
                .id("remote-button-refused")
                .tooltip(Tooltip::text(tooltip))
                .child(
                    Button::new("remote-button-refused-button", label)
                        .label_size(LabelSize::Small)
                        .disabled(true),
                )
                .into_any_element(),
        )
    }

    /// Re-renders the git panel when the Helm panel changes (its account,
    /// or what the account may do). Started once the Helm panel exists.
    pub(super) fn observe_helm_panel(&mut self, cx: &mut Context<Self>) {
        if self._helm_observation.is_some() {
            return;
        }
        let Some(workspace) = self.workspace.upgrade() else {
            return;
        };
        let Some(helm) = workspace.read(cx).panel::<HelmPanel>(cx) else {
            return;
        };
        self.helm_panel = Some(helm.downgrade());
        self._helm_observation = Some(cx.observe(&helm, |_, _, cx| cx.notify()));
    }

    /// The workspace's Helm panel, once [`Self::observe_helm_panel`] has
    /// found it. Read through this rather than the workspace: push, pull and
    /// fetch run while the workspace is being updated, where reading it
    /// would panic.
    fn helm_panel(&self) -> Option<Entity<HelmPanel>> {
        self.helm_panel.as_ref()?.upgrade()
    }

    /// The account row, the commit-email lines, and the warning when the
    /// account cannot push. Nothing until Helm has checked a github.com
    /// repository. The push warning is for HTTPS remotes only: over SSH the
    /// SSH key, not the account, decides access.
    pub(super) fn render_github_account(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let helm = self.helm_panel()?;
        let (access, login, identity) = {
            let helm = helm.read(cx);
            (
                helm.repo_access()?.clone(),
                helm.signed_in_login()?.to_string(),
                helm.commit_identity().clone(),
            )
        };

        let settings = HelmPanelSettings::get_global(cx);
        let name_of = |login: &str| {
            settings
                .account(login)
                .map_or_else(|| login.to_string(), |account| account.display_name().to_string())
        };
        let name = name_of(&login);
        let suggested = access
            .suggested_login
            .as_deref()
            .map(|suggested| (suggested.to_string(), name_of(suggested)));
        let accounts: Vec<(String, String)> = settings
            .accounts
            .iter()
            .map(|account| (account.login.clone(), account.display_name().to_string()))
            .collect();
        let repository = access.remote.full_name();

        let menu_helm = helm.downgrade();
        let menu_login = login.clone();
        let edit_helm = helm.downgrade();
        let account_row = h_flex()
            .id("github-account-row")
            .px_2()
            .py_0p5()
            .gap_1()
            .border_t_1()
            .border_color(cx.theme().colors().border_variant)
            .child(
                Icon::new(IconName::Github)
                    .size(IconSize::XSmall)
                    .color(Color::Muted),
            )
            .child(
                Label::new("GitHub account")
                    .size(LabelSize::Small)
                    .color(Color::Muted),
            )
            .child(
                PopoverMenu::new("github-account-menu")
                    .trigger(
                        Button::new("github-account-button", name.clone())
                            .label_size(LabelSize::Small)
                            .end_icon(
                                Icon::new(IconName::ChevronDown)
                                    .size(IconSize::XSmall)
                                    .color(Color::Muted),
                            ),
                    )
                    .menu(move |window, cx| {
                        let helm = menu_helm.clone();
                        let current = menu_login.clone();
                        let accounts = accounts.clone();
                        Some(ContextMenu::build(window, cx, move |menu, _, _| {
                            accounts.iter().fold(menu, |menu, (login, label)| {
                                let helm = helm.clone();
                                let login = login.clone();
                                let is_current = login.eq_ignore_ascii_case(&current);
                                menu.toggleable_entry(
                                    label.clone(),
                                    is_current,
                                    IconPosition::Start,
                                    None,
                                    move |_, cx| {
                                        if is_current {
                                            return;
                                        }
                                        helm.update(cx, |helm, cx| {
                                            helm.switch_account(login.clone(), cx)
                                        })
                                        .ok();
                                    },
                                )
                            })
                        }))
                    }),
            )
            .when_some(
                match &identity {
                    CommitIdentity::Applied { email, .. } => Some(email.clone()),
                    _ => None,
                },
                |row, email| {
                    row.child(
                        Label::new(email)
                            .size(LabelSize::Small)
                            .color(Color::Muted)
                            .truncate(),
                    )
                    .child(
                        IconButton::new("github-commit-email-edit", IconName::Pencil)
                            .icon_size(IconSize::XSmall)
                            .icon_color(Color::Muted)
                            .tooltip(Tooltip::text("Change the commit name and email"))
                            .on_click(move |_, window, cx| {
                                edit_helm
                                    .update(cx, |helm, cx| {
                                        helm.open_commit_identity_dialog(window, cx)
                                    })
                                    .ok();
                            }),
                    )
                },
            )
            .tooltip(Tooltip::text(
                "The GitHub account this workspace uses in Helm, and the email commits are \
                 authored with. Push, pull and fetch from the git panel log in as this account.",
            ));

        let mut column = v_flex().child(account_row);
        if let Some(line) = self.render_commit_identity_line(&identity, &helm, cx) {
            column = column.child(line);
        }
        if access.remote.https && access.can_push == Some(false) {
            let message = if access.can_read {
                format!("{name} can't push to {repository}.")
            } else {
                format!("{name} can't see {repository}.")
            };
            let mut warning = h_flex()
                .px_2()
                .py_1()
                .gap_1()
                .flex_wrap()
                .border_t_1()
                .border_color(cx.theme().status().warning_border)
                .bg(cx.theme().status().warning_background.opacity(0.5))
                .child(
                    Icon::new(IconName::Warning)
                        .size(IconSize::XSmall)
                        .color(Color::Warning),
                )
                .child(Label::new(message).size(LabelSize::Small));
            warning = match suggested {
                Some((suggested_login, suggested_name)) => {
                    let helm = helm.downgrade();
                    warning.child(
                        Button::new("github-account-switch", format!("Switch to {suggested_name}"))
                            .style(ButtonStyle::Filled)
                            .label_size(LabelSize::Small)
                            .on_click(move |_, _, cx| {
                                helm.update(cx, |helm, cx| {
                                    helm.switch_account(suggested_login.clone(), cx)
                                })
                                .ok();
                            }),
                    )
                }
                None => warning.child(
                    Label::new("None of your signed-in accounts can push to it.")
                        .size(LabelSize::Small)
                        .color(Color::Muted),
                ),
            };
            column = column.child(warning);
        }
        Some(column.into_any_element())
    }

    /// The one-time email choice, the question about a repository that
    /// already names someone else, or why the identity could not be set.
    fn render_commit_identity_line(
        &self,
        identity: &CommitIdentity,
        helm: &Entity<HelmPanel>,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let line = || {
            h_flex()
                .px_2()
                .py_1()
                .gap_1()
                .flex_wrap()
                .border_t_1()
                .border_color(cx.theme().colors().border_variant)
        };
        let helm = helm.downgrade();
        match identity {
            CommitIdentity::NeedsEmail {
                login,
                emails,
                noreply,
                current_email,
                ..
            } => {
                let mut row = line()
                    .child(
                        Label::new(format!("Commits as {login}: which email?"))
                            .size(LabelSize::Small),
                    )
                    .when_some(current_email.clone(), |row, current| {
                        row.child(
                            Label::new(format!("Until you choose, commits use {current}."))
                                .size(LabelSize::Small)
                                .color(Color::Muted),
                        )
                    });
                for (index, email) in emails.iter().take(2).enumerate() {
                    let helm = helm.clone();
                    let email = email.clone();
                    row = row.child(
                        Button::new(("github-commit-email", index), email.clone())
                            .style(ButtonStyle::Filled)
                            .label_size(LabelSize::Small)
                            .on_click(move |_, _, cx| {
                                helm.update(cx, |helm, cx| {
                                    helm.choose_commit_email(email.clone(), cx)
                                })
                                .ok();
                            }),
                    );
                }
                let noreply_helm = helm.clone();
                let noreply = noreply.clone();
                row = row
                    .child(
                        Button::new("github-commit-email-noreply", "Private (noreply)")
                            .label_size(LabelSize::Small)
                            .tooltip(Tooltip::text(noreply.clone()))
                            .on_click(move |_, _, cx| {
                                noreply_helm
                                    .update(cx, |helm, cx| {
                                        helm.choose_commit_email(noreply.clone(), cx)
                                    })
                                    .ok();
                            }),
                    )
                    .child(
                        Button::new(
                            "github-commit-email-other",
                            if emails.is_empty() {
                                "Type your email…"
                            } else {
                                "Other…"
                            },
                        )
                        .label_size(LabelSize::Small)
                        .on_click(move |_, window, cx| {
                            helm.update(cx, |helm, cx| {
                                helm.open_commit_identity_dialog(window, cx)
                            })
                            .ok();
                        }),
                    );
                Some(row.into_any_element())
            }
            CommitIdentity::Conflict {
                login,
                email,
                current_email,
                ..
            } => {
                let keep_helm = helm.clone();
                Some(
                    line()
                        .child(
                            Label::new(format!(
                                "This repository commits as {current_email}. Use {email} for \
                                 {login}?"
                            ))
                            .size(LabelSize::Small),
                        )
                        .child(
                            Button::new("github-commit-email-use", format!("Use {email}"))
                                .style(ButtonStyle::Filled)
                                .label_size(LabelSize::Small)
                                .on_click(move |_, _, cx| {
                                    helm.update(cx, |helm, cx| helm.use_account_identity(cx))
                                        .ok();
                                }),
                        )
                        .child(
                            Button::new("github-commit-email-keep", "Keep")
                                .label_size(LabelSize::Small)
                                .on_click(move |_, _, cx| {
                                    keep_helm
                                        .update(cx, |helm, cx| helm.keep_repository_identity(cx))
                                        .ok();
                                }),
                        )
                        .into_any_element(),
                )
            }
            CommitIdentity::Failed(error) => Some(
                line()
                    .child(
                        Icon::new(IconName::Warning)
                            .size(IconSize::XSmall)
                            .color(Color::Warning),
                    )
                    .child(
                        Label::new(format!("Could not set the commit email: {error}"))
                            .size(LabelSize::Small)
                            .color(Color::Muted),
                    )
                    .into_any_element(),
            ),
            CommitIdentity::Unknown
            | CommitIdentity::Checking
            | CommitIdentity::Applied { .. } => None,
        }
    }
}
