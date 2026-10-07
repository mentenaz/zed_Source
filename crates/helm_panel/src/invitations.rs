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
                    this.invitations.items = invitations;
                }
                if let Ok(org_invitations) = org_result {
                    this.org_invitations = org_invitations;
                }
                this.repo_invitation_count = this.invitations.items.len() + this.org_invitations.len();
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
        self.list_screen(
            ListStatus {
                // Invitations are refreshed in the background whenever this
                // screen is shown, so there is no spinner or error to draw.
                state: LoadState::Idle,
                error: String::new(),
                is_empty: self.invitations.items.is_empty() && self.org_invitations.is_empty(),
            },
            &self.invitations_list,
            None,
            ListLabels {
                loading: "Loading invitations…",
                error: "Failed to load invitations",
                empty: "No pending invitations",
            },
            |this, cx| this.load_repo_invitations(cx),
            cx,
        )
    }
}

/// Accept and Decline, as every invitation row ends with them. Accepting or
/// declining is only ever done with these buttons: a click on the row, or
/// `enter`, does nothing, so an invitation cannot be accepted by accident.
fn invitation_buttons(
    key: impl std::fmt::Display,
    accept: impl Fn(&mut HelmPanel, &mut Window, &mut Context<HelmPanel>) + Clone + 'static,
    decline: impl Fn(&mut HelmPanel, &mut Window, &mut Context<HelmPanel>) + Clone + 'static,
    panel: WeakEntity<HelmPanel>,
) -> gpui::Div {
    let accept_panel = panel.clone();
    let decline_panel = panel;
    h_flex()
        .items_center()
        .gap_2()
        .child(
            Button::new(format!("helm-invitation-accept-{key}"))
                .primary()
                .xsmall()
                .label("Accept")
                .on_click(move |_, window, cx| {
                    accept_panel
                        .update(cx, |this, cx| accept(this, window, cx))
                        .ok();
                }),
        )
        .child(
            Button::new(format!("helm-invitation-decline-{key}"))
                .ghost()
                .xsmall()
                .icon(IconName::Delete)
                .tooltip("Decline invitation")
                .on_click(move |_, window, cx| {
                    decline_panel
                        .update(cx, |this, cx| decline(this, window, cx))
                        .ok();
                }),
        )
}

/// One organisation invitation: the organisation and the role offered.
pub(super) fn org_invitation_row(
    ix: usize,
    invitation: &OrgInvitation,
    panel: &WeakEntity<HelmPanel>,
    cx: &App,
) -> ListItem {
    let foreground = cx.theme().foreground;
    let muted_foreground = cx.theme().muted_foreground;
    let org = invitation.organization.login.clone();
    let role = invitation.role.clone();
    let panel = panel.clone();
    ListItem::new(("helm-org-invitation", ix))
        .child(
            v_flex()
                .gap_0p5()
                .min_w_0()
                .child(
                    div()
                        .truncate()
                        .text_sm()
                        .font_semibold()
                        .text_color(foreground)
                        .child(org.clone()),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(muted_foreground)
                        .child(format!("role: {role}")),
                ),
        )
        .suffix(move |_, _| {
            let accept_org = org.clone();
            let decline_org = org.clone();
            invitation_buttons(
                format!("org-{org}"),
                move |this, window, cx| {
                    this.handle_accept_org_invitation(accept_org.clone(), window, cx)
                },
                move |this, window, cx| {
                    this.handle_decline_org_invitation(decline_org.clone(), window, cx)
                },
                panel.clone(),
            )
        })
}

/// One repository invitation: the repository, whether it is private, the
/// permission offered and who sent it.
pub(super) fn repo_invitation_row(
    ix: usize,
    invitation: &RepoInvitation,
    panel: &WeakEntity<HelmPanel>,
    cx: &App,
) -> ListItem {
    let foreground = cx.theme().foreground;
    let muted_foreground = cx.theme().muted_foreground;
    let id = invitation.id;
    let visibility = if invitation.repository.private {
        "private"
    } else {
        "public"
    };
    let panel = panel.clone();
    ListItem::new(("helm-repo-invitation", ix))
        .child(
            v_flex()
                .gap_0p5()
                .min_w_0()
                .child(
                    div()
                        .truncate()
                        .text_sm()
                        .font_semibold()
                        .text_color(foreground)
                        .child(invitation.repository.full_name.clone()),
                )
                .child(div().truncate().text_xs().text_color(muted_foreground).child(format!(
                    "{visibility} · {} · invited by @{}",
                    invitation.permissions, invitation.inviter.login
                ))),
        )
        .suffix(move |_, _| {
            invitation_buttons(
                format!("repo-{id}"),
                move |this, window, cx| this.handle_accept_invitation(id, window, cx),
                move |this, window, cx| this.handle_decline_invitation(id, window, cx),
                panel.clone(),
            )
        })
}
