//! The GitHub account a workspace uses, shown in the git panel (section 11,
//! item 1b of `helm_backend/octocrab-helm-coverage.md`).
//!
//! For a repository on github.com, the footer shows the account the Helm
//! panel uses for this workspace, with a menu to switch, and the email
//! commits are authored with (item 2), with its one-time choice and a
//! question when the repository already names someone else. Over HTTPS it
//! also warns when Helm has found that the account cannot push, with a
//! button to switch to an account that can. Nothing is disabled yet: until
//! the git panel pushes with the Helm account, pushes still go through
//! git's own login.

use super::*;
use helm_panel::{CommitIdentity, HelmPanel, HelmPanelSettings};
use settings::Settings as _;

impl GitPanel {
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
        self._helm_observation = Some(cx.observe(&helm, |_, _, cx| cx.notify()));
    }

    /// The account row, the commit-email lines, and the warning when the
    /// account cannot push. Nothing until Helm has checked a github.com
    /// repository. The push warning is for HTTPS remotes only: over SSH the
    /// SSH key, not the account, decides access.
    pub(super) fn render_github_account(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let workspace = self.workspace.upgrade()?;
        let helm = workspace.read(cx).panel::<HelmPanel>(cx)?;
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
                 authored with. Pushes from the git panel still use git's own login for now.",
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
