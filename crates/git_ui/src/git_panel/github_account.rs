//! The GitHub account a workspace uses, shown in the git panel (section 11,
//! item 1b of `helm_backend/octocrab-helm-coverage.md`).
//!
//! For a repository with a github.com HTTPS remote, the footer shows the
//! account the Helm panel uses for this workspace, with a menu to switch,
//! and a warning when Helm has found that the account cannot push, with a
//! button to switch to an account that can. Nothing is disabled yet: until
//! the git panel pushes with the Helm account, pushes still go through
//! git's own login.

use super::*;
use helm_panel::{HelmPanel, HelmPanelSettings};
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

    /// The account row, and the warning under it when the account cannot
    /// push. Nothing until Helm has checked a github.com HTTPS repository:
    /// over SSH the SSH key, not the account, decides access.
    pub(super) fn render_github_account(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let workspace = self.workspace.upgrade()?;
        let helm = workspace.read(cx).panel::<HelmPanel>(cx)?;
        let (access, login) = {
            let helm = helm.read(cx);
            (helm.repo_access()?.clone(), helm.signed_in_login()?.to_string())
        };
        if !access.remote.https {
            return None;
        }

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
            .tooltip(Tooltip::text(
                "The GitHub account this workspace uses in Helm. Pushes from the git panel \
                 still use git's own login for now.",
            ));

        let mut column = v_flex().child(account_row);
        if access.can_push == Some(false) {
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
}
