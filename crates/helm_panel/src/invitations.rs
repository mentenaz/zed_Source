//! Helm's invitations screen: pending repository and organisation
//! invitations, with accept and decline.

use super::*;

impl HelmPanel {
    /// Loads the pending repo- and org-invitation lists for the Profile
    /// screen's Invitations row (badge count + the Invitations screen
    /// itself). Fire-and-forget: a failure on either just leaves the
    /// previous state for that list in place.
    pub(super) fn load_repo_invitations(&mut self, cx: &mut Context<Self>) {
        let gh_state = self.gh_state.clone();
        cx.spawn(async move |this, cx| {
            let (repo_result, org_result) = on_tokio(async move {
                let repo_result = gh_get_repo_invitations(&gh_state).await;
                let org_result = gh_list_org_invitations(&gh_state).await;
                (repo_result, org_result)
            })
            .await;
            this.update(cx, |this, cx| {
                if let Ok(invitations) = repo_result {
                    this.invitations = invitations;
                }
                if let Ok(org_invitations) = org_result {
                    this.org_invitations = org_invitations;
                }
                this.repo_invitation_count = this.invitations.len() + this.org_invitations.len();
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Accepts a pending repo invitation and drops it from the list + badge.
    pub(super) fn handle_accept_invitation(&mut self, id: u64, _window: &mut Window, cx: &mut Context<Self>) {
        self.run_action(HelmAction::AcceptRepoInvitation(id), true, cx);
    }

    /// Declines a pending repo invitation and drops it from the list + badge.
    pub(super) fn handle_decline_invitation(&mut self, id: u64, _window: &mut Window, cx: &mut Context<Self>) {
        self.run_action(HelmAction::DeclineRepoInvitation(id), true, cx);
    }

    /// Accepts a pending org invitation and drops it from the list + badge.
    pub(super) fn handle_accept_org_invitation(
        &mut self,
        org: String,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.run_action(HelmAction::AcceptOrgInvitation(org), true, cx);
    }

    /// Declines a pending org invitation and drops it from the list + badge.
    pub(super) fn handle_decline_org_invitation(
        &mut self,
        org: String,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.run_action(HelmAction::DeclineOrgInvitation(org), true, cx);
    }

    /// The Invitations screen — pending repo invitations with Accept/Decline
    /// actions that round-trip against the GitHub API.
    pub(super) fn render_invitations(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let muted_foreground = cx.theme().muted_foreground;
        let foreground = cx.theme().foreground;

        if self.invitations.is_empty() && self.org_invitations.is_empty() {
            return v_flex()
                .flex_1()
                .items_center()
                .justify_center()
                .p_4()
                .child(
                    div()
                        .text_sm()
                        .text_color(muted_foreground)
                        .child("No pending invitations"),
                )
                .into_any_element();
        }

        let view = cx.entity();

        // Org invitations are listed before repo invitations with no visual
        // separator between them, so `invitations_list_cursor` indexes this
        // one combined, display-order list rather than either `Vec` alone.
        enum Invite {
            Org(String),
            Repo(u64),
        }
        let combined: Vec<Invite> = self
            .org_invitations
            .iter()
            .map(|inv| Invite::Org(inv.organization.login.clone()))
            .chain(self.invitations.iter().map(|inv| Invite::Repo(inv.id)))
            .collect();
        let combined_len = combined.len();
        let cursor = self.invitations_list_cursor;

        let org_rows = self.org_invitations.iter().enumerate().map(|(ix, inv)| {
            let org_login = inv.organization.login.clone();
            let role = inv.role.clone();
            let org_accept = org_login.clone();
            let org_decline = org_login.clone();
            let view_accept = view.clone();
            let view_decline = view.clone();

            ListItem::new(format!("helm-org-invitation-{}", inv.organization.login))
                .selected(cursor == Some(ix))
                .child(
                    v_flex()
                        .gap_0p5()
                        .min_w_0()
                        .child(
                            div()
                                .text_sm()
                                .font_semibold()
                                .text_color(foreground)
                                .child(inv.organization.login.clone()),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(muted_foreground)
                                .child(format!("Organization · role: {role}")),
                        ),
                )
                .suffix({
                    move |_, _| {
                        h_flex()
                            .items_center()
                            .gap_2()
                            .child(
                                Button::new(format!("helm-org-invitation-accept-{org_login}"))
                                    .primary()
                                    .xsmall()
                                    .label("Accept")
                                    .on_click({
                                        let view = view_accept.clone();
                                        let org = org_accept.clone();
                                        move |_, window, cx| {
                                            view.update(cx, |this, cx| {
                                                this.handle_accept_org_invitation(
                                                    org.clone(),
                                                    window,
                                                    cx,
                                                );
                                            });
                                        }
                                    }),
                            )
                            .child(
                                Button::new(format!("helm-org-invitation-decline-{org_decline}"))
                                    .ghost()
                                    .xsmall()
                                    .icon(IconName::Delete)
                                    .tooltip("Decline invitation")
                                    .on_click({
                                        let view = view_decline.clone();
                                        let org = org_decline.clone();
                                        move |_, window, cx| {
                                            view.update(cx, |this, cx| {
                                                this.handle_decline_org_invitation(
                                                    org.clone(),
                                                    window,
                                                    cx,
                                                );
                                            });
                                        }
                                    }),
                            )
                    }
                })
        });

        let org_count = self.org_invitations.len();
        let combined_for_open = combined;
        let combined_for_act: Vec<(bool, String)> = self
            .org_invitations
            .iter()
            .map(|inv| (true, inv.organization.login.clone()))
            .chain(self.invitations.iter().map(|inv| (false, inv.id.to_string())))
            .collect();

        v_flex()
            .id("helm-invitations-list")
            .track_focus(&self.invitations_list_focus)
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, window, cx| {
                window.focus(&this.invitations_list_focus, cx);
            }))
            .key_context("HelmRowList")
            .on_action(cx.listener(move |this, _: &SelectNextRow, _, cx| {
                this.invitations_list_cursor =
                    step_selected(this.invitations_list_cursor, combined_len, true);
                cx.notify();
            }))
            .on_action(cx.listener(move |this, _: &SelectPrevRow, _, cx| {
                this.invitations_list_cursor =
                    step_selected(this.invitations_list_cursor, combined_len, false);
                cx.notify();
            }))
            // Enter accepts the selected row's invitation...
            .on_action(cx.listener(move |this, _: &OpenSelectedRow, window, cx| {
                let Some(invite) = this.invitations_list_cursor.and_then(|ix| combined_for_open.get(ix))
                else {
                    return;
                };
                match invite {
                    Invite::Org(login) => this.handle_accept_org_invitation(login.clone(), window, cx),
                    Invite::Repo(id) => this.handle_accept_invitation(*id, window, cx),
                }
            }))
            // ...Space declines it — the Invitations screen's rows have no
            // separate "view" action for Enter to be the safe default of, so
            // Accept/Decline (its only two actions) split Enter/Space instead.
            .on_action(cx.listener(move |this, _: &ActSelectedRow, window, cx| {
                let Some((is_org, key)) =
                    this.invitations_list_cursor.and_then(|ix| combined_for_act.get(ix))
                else {
                    return;
                };
                if *is_org {
                    this.handle_decline_org_invitation(key.clone(), window, cx);
                } else if let Ok(id) = key.parse() {
                    this.handle_decline_invitation(id, window, cx);
                }
            }))
            .py_1()
            .children(org_rows)
            .children(self.invitations.iter().enumerate().map(|(ix, inv)| {
                let full_name = inv.repository.full_name.clone();
                let private = inv.repository.private;
                let inviter = inv.inviter.login.clone();
                let permissions = inv.permissions.clone();
                let id_accept = inv.id;
                let id_decline = inv.id;
                let view_accept = view.clone();
                let view_decline = view.clone();

                ListItem::new(format!("helm-invitation-{}", inv.id))
                    .selected(cursor == Some(org_count + ix))
                    .child(
                        v_flex()
                            .gap_0p5()
                            .min_w_0()
                            .child(
                                div()
                                    .text_sm()
                                    .font_semibold()
                                    .text_color(foreground)
                                    .child(full_name),
                            )
                            .child(
                                h_flex()
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(muted_foreground)
                                            .child(if private { "private" } else { "public" }),
                                    )
                                    .child(
                                        div().text_xs().text_color(muted_foreground).child(
                                            format!("{permissions} · invited by @{inviter}"),
                                        ),
                                    ),
                            ),
                    )
                    .suffix({
                        move |_, _| {
                            h_flex()
                                .items_center()
                                .gap_2()
                                .child(
                                    Button::new(("helm-invitation-accept", id_accept))
                                        .primary()
                                        .xsmall()
                                        .label("Accept")
                                        .on_click({
                                            let view = view_accept.clone();
                                            move |_, window, cx| {
                                                view.update(cx, |this, cx| {
                                                    this.handle_accept_invitation(
                                                        id_accept, window, cx,
                                                    );
                                                });
                                            }
                                        }),
                                )
                                .child(
                                    Button::new(("helm-invitation-decline", id_decline))
                                        .ghost()
                                        .xsmall()
                                        .icon(IconName::Delete)
                                        .tooltip("Decline invitation")
                                        .on_click({
                                            let view = view_decline.clone();
                                            move |_, window, cx| {
                                                view.update(cx, |this, cx| {
                                                    this.handle_decline_invitation(
                                                        id_decline, window, cx,
                                                    );
                                                });
                                            }
                                        }),
                                )
                        }
                    })
            }))
            .into_any_element()
    }
}
