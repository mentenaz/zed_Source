//! Helm's profile screens: the signed-in user's profile and menu, the
//! identity header shown above it, other users' profiles, and editing your own.

use super::*;

impl HelmPanel {
    /// User clicked a username shown elsewhere in the panel (e.g. a
    /// collaborator row) — opens their public profile.
    pub(super) fn open_user_profile(&mut self, username: String, cx: &mut Context<Self>) {
        self.navigate_to(HelmScreen::UserProfile, cx);
        self.load_user_profile(username, cx);
    }

    pub(super) fn load_user_profile(&mut self, username: String, cx: &mut Context<Self>) {
        self.viewed_user = None;
        self.load_with(
            cx,
            |gh_state| async move { gh_get_user(username, &gh_state).await },
            |this, user| this.viewed_user = Some(user),
        );
    }

    /// Opens the "Edit profile" modal, prefilled from `self.user`.
    pub(super) fn open_edit_profile_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(user) = self.user.clone() else {
            return;
        };
        self.open_workspace_modal(HelmModalKind::EditProfile(user), window, cx);
    }

    /// Updates the signed-in user's profile. GitHub's `/user` PATCH endpoint
    /// needs the `user` OAuth scope, which `gh_login` requests up front
    /// (`repo,read:org,user`, see cli.rs). A login made outside Helm may not
    /// have it; in that case `run_action` sends this through the auth gate
    /// for `user`, where the device code is shown, and re-sends it.
    pub(super) fn handle_update_profile(
        &mut self,
        name: String,
        bio: String,
        company: String,
        location: String,
        blog: String,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let changes = json!({
            "name": name,
            "bio": bio,
            "company": company,
            "location": location,
            "blog": blog,
        });
        self.run_action(HelmAction::UpdateProfile { changes }, true, cx);
    }

    /// "View Profile" — stats row plus nav rows into Edit
    /// Profile/Repositories/Organizations/Invitations/Account Security.
    /// Organizations and Repositories show real counts; Invitations shows
    /// its count only when there are pending invites to surface
    /// (`repo_invitation_count`, loaded by [`Self::load_repo_invitations`]).
    /// OrgList/RepoList land on real screens; the remaining rows are
    /// informational for now.
    pub(super) fn render_profile(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let muted_foreground = cx.theme().muted_foreground;
        let foreground = cx.theme().foreground;
        let border = cx.theme().border;

        let Some(user) = self.user.clone() else {
            return v_flex()
                .flex_1()
                .items_center()
                .justify_center()
                .p_4()
                .child(
                    div()
                        .text_sm()
                        .text_color(muted_foreground)
                        .child("No profile data."),
                )
                .into_any_element();
        };

        let total_repos = user.public_repos + user.total_private_repos.unwrap_or(0);

        let stats_row = h_flex()
            .items_center()
            .gap_4()
            .px_3()
            .py_2()
            .text_sm()
            .text_color(muted_foreground)
            .child(format!("Followers {}", fmt_num(user.followers)))
            .child(format!("Following {}", fmt_num(user.following)))
            .child(format!("Repos {}", fmt_num(total_repos)));

        let rows: Vec<NavRow> = [
            NavRow {
                id: "edit-profile",
                label: "Edit Profile",
                hint: None,
            },
            NavRow {
                id: "repos",
                label: "Repositories",
                hint: Some(total_repos.to_string()),
            },
        ]
        .into_iter()
        .chain(if !self.org_logins.is_empty() {
            Some(NavRow {
                id: "orgs",
                label: "Organizations",
                hint: Some(self.org_logins.len().to_string()),
            })
        } else {
            None
        })
        .chain(if self.repo_invitation_count > 0 {
            Some(NavRow {
                id: "invitations",
                label: "Invitations",
                hint: Some(self.repo_invitation_count.to_string()),
            })
        } else {
            None
        })
        .chain([NavRow {
            id: "account-security",
            label: "Account Security",
            hint: None,
        }])
        .collect();

        let len = rows.len();
        let cursor = self.profile_menu_cursor;
        let row_ids: Vec<&'static str> = rows.iter().map(|row| row.id).collect();
        v_flex()
            .child(stats_row)
            .child(div().h_px().w_full().bg(border))
            .child(
                v_flex()
                    .id("helm-profile-menu")
                    .track_focus(&self.profile_menu_focus)
                    .on_mouse_down(MouseButton::Left, cx.listener(|this, _, window, cx| {
                        window.focus(&this.profile_menu_focus, cx);
                    }))
                    .key_context("HelmRowList")
                    .on_action(cx.listener(move |this, _: &SelectNextRow, _, cx| {
                        this.profile_menu_cursor = step_selected(this.profile_menu_cursor, len, true);
                        cx.notify();
                    }))
                    .on_action(cx.listener(move |this, _: &SelectPrevRow, _, cx| {
                        this.profile_menu_cursor = step_selected(this.profile_menu_cursor, len, false);
                        cx.notify();
                    }))
                    .on_action(cx.listener(move |this, _: &OpenSelectedRow, window, cx| {
                        let Some(&id) = this.profile_menu_cursor.and_then(|ix| row_ids.get(ix)) else {
                            return;
                        };
                        match id {
                            "orgs" => this.navigate_to(HelmScreen::OrgList, cx),
                            "repos" => this.open_repo_list(cx),
                            "invitations" => this.navigate_to(HelmScreen::Invitations, cx),
                            "edit-profile" => this.open_edit_profile_dialog(window, cx),
                            "account-security" => {
                                cx.open_url("https://github.com/settings/security")
                            }
                            _ => {}
                        }
                    }))
                    .py_1()
                    .children(rows.into_iter().enumerate().map(|(ix, row)| {
                        let hint = row.hint;
                        let id = row.id;
                        ListItem::new(format!("helm-profile-{}", row.id))
                            .selected(cursor == Some(ix))
                            .child(div().text_color(foreground).child(row.label))
                            .suffix(move |_, _| {
                                h_flex()
                                    .items_center()
                                    .gap_2()
                                    .children(
                                        hint.clone()
                                            .map(|hint| div().text_color(muted_foreground).child(hint)),
                                    )
                                    .child(
                                        Icon::new(IconName::ChevronRight)
                                            .xsmall()
                                            .text_color(muted_foreground),
                                    )
                            })
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.profile_menu_cursor = Some(ix);
                                match id {
                                    "orgs" => this.navigate_to(HelmScreen::OrgList, cx),
                                    "repos" => this.open_repo_list(cx),
                                    "invitations" => this.navigate_to(HelmScreen::Invitations, cx),
                                    "edit-profile" => this.open_edit_profile_dialog(window, cx),
                                    // No per-account security API is wired up
                                    // (Dependabot/secret-scanning alerts,
                                    // elsewhere in this panel, are
                                    // repo-scoped, not account-scoped) —
                                    // opens GitHub's own settings page
                                    // instead of a dead click.
                                    "account-security" => {
                                        cx.open_url("https://github.com/settings/security")
                                    }
                                    _ => {}
                                }
                            }))
                    })),
            )
            .into_any_element()
    }

    /// The identity strip shown between the panel header and the current
    /// screen on every authenticated screen (mirrors the old app's
    /// `render_identity_header`, with the org-identity swap since `OrgDetail`
    /// landed). `None` before `self.user` has loaded, which in practice only
    /// happens on Gate/Auth (excluded by `render`'s `show_nav` condition
    /// anyway).
    pub(super) fn render_identity_header(&self, cx: &mut Context<Self>) -> Option<impl IntoElement> {
        let muted_foreground = cx.theme().muted_foreground;
        let border = cx.theme().border;

        // `OrgDetail` swaps in the org's own avatar/name instead of the
        // user's — matches the old app's `PanelHeader`/`OrgHeader` split.
        // `selected_org` being set but `org_detail` still loading just
        // hides the header for a moment, rather than flashing stale user
        // identity.
        let (avatar_url, name, login, description) = if self.selected_org.is_some() {
            let org = self.org_detail.as_ref()?;
            (
                org.avatar_url.clone(),
                org.name.clone().unwrap_or_else(|| org.login.clone()),
                org.login.clone(),
                org.description.clone(),
            )
        } else {
            let user = self.user.as_ref()?;
            (
                user.avatar_url.clone(),
                user.name.clone().unwrap_or_else(|| user.login.clone()),
                user.login.clone(),
                user.bio.clone(),
            )
        };

        Some(
            v_flex()
                .flex_shrink_0()
                .child(
                    h_flex()
                        .gap_3()
                        .items_center()
                        .px_3()
                        .py_2()
                        .child(
                            Avatar::new()
                                .src(avatar_url)
                                .name(name.clone())
                                .with_size(px(40.)),
                        )
                        .child(
                            v_flex()
                                .gap_0()
                                .min_w_0()
                                .child(div().font_semibold().child(name))
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(muted_foreground)
                                        .child(format!("@{login}")),
                                )
                                .when_some(
                                    description.filter(|bio| !bio.trim().is_empty()),
                                    |el, bio| {
                                        el.child(
                                            div()
                                                .text_xs()
                                                .text_color(muted_foreground)
                                                .child(bio),
                                        )
                                    },
                                ),
                        ),
                )
                .child(div().h_px().w_full().bg(border)),
        )
    }

    /// Main menu — mirrors the old `MenuScreen`. Every row is wired to a real
    /// destination (Profile/OrgList/RepoList/Create Repository/Logout);
    /// Organizations only shows once real org data confirms the account
    /// actually belongs to any.
    pub(super) fn render_menu(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let items: Vec<MenuItem> = [
            MenuItem {
                id: "profile",
                label: "View Profile",
                danger: false,
            },
            MenuItem {
                id: "orgs",
                label: "Organizations",
                danger: false,
            },
            MenuItem {
                id: "repos",
                label: "Repositories",
                danger: false,
            },
            MenuItem {
                id: "create",
                label: "Create Repository",
                danger: false,
            },
            MenuItem {
                id: "logout",
                label: "Logout",
                danger: true,
            },
        ]
        .into_iter()
        .filter(|item| item.id != "orgs" || !self.org_logins.is_empty())
        .collect();

        let foreground = cx.theme().foreground;
        let danger_color = cx.theme().danger;
        let muted_foreground = cx.theme().muted_foreground;

        v_flex()
            .child(v_flex().py_1().children(items.into_iter().map(|item| {
                let id = item.id;
                let label_color = if item.danger {
                    danger_color
                } else {
                    foreground
                };

                ListItem::new(id)
                    .child(div().text_color(label_color).child(item.label))
                    .suffix(move |_, _| {
                        Icon::new(IconName::ChevronRight)
                            .xsmall()
                            .text_color(muted_foreground)
                    })
                    .on_click(cx.listener(move |this, _, window, cx| match id {
                        "logout" => this.handle_logout(cx),
                        "profile" => this.navigate_to(HelmScreen::Profile, cx),
                        "orgs" => this.navigate_to(HelmScreen::OrgList, cx),
                        "repos" => this.open_repo_list(cx),
                        "create" => this.open_create_repo_dialog(window, cx),
                        _ => {}
                    }))
            })))
            .into_any_element()
    }

    /// A public profile view for someone other than the signed-in user,
    /// reached via [`Self::open_user_profile`] (e.g. clicking a
    /// collaborator). Read-only — editing is only available on `Profile`,
    /// the signed-in user's own screen.
    pub(super) fn render_user_profile(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let muted_foreground = cx.theme().muted_foreground;
        let foreground = cx.theme().foreground;
        let border = cx.theme().border;

        if self.load_state == LoadState::Loading {
            return v_flex()
                .flex_1()
                .items_center()
                .justify_center()
                .p_4()
                .child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(Spinner::new().small())
                        .child(
                            div()
                                .text_sm()
                                .text_color(muted_foreground)
                                .child("Loading profile…"),
                        ),
                )
                .into_any_element();
        }

        let Some(user) = self.viewed_user.clone() else {
            return v_flex()
                .gap_3()
                .p_4()
                .child(
                    div()
                        .text_sm()
                        .text_color(muted_foreground)
                        .child("Failed to load profile"),
                )
                .into_any_element();
        };

        let url = user.html_url.clone();

        v_flex()
            .child(
                h_flex()
                    .items_center()
                    .gap_3()
                    .px_3()
                    .py_2()
                    .child(
                        Avatar::new()
                            .src(user.avatar_url.clone())
                            .name(user.login.clone())
                            .with_size(px(40.)),
                    )
                    .child(
                        v_flex()
                            .gap_0()
                            .min_w_0()
                            .child(
                                div()
                                    .font_semibold()
                                    .text_color(foreground)
                                    .child(user.name.clone().unwrap_or_else(|| user.login.clone())),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(muted_foreground)
                                    .child(format!("@{}", user.login)),
                            ),
                    )
                    .child(
                        Button::new("helm-user-profile-open-browser")
                            .ghost()
                            .xsmall()
                            .icon(IconName::ExternalLink)
                            .tooltip("Open in Browser")
                            .on_click(move |_, _, cx| cx.open_url(&url)),
                    ),
            )
            .child(div().h_px().w_full().bg(border))
            .when_some(user.bio.clone(), |col, bio| {
                col.child(div().p_3().text_sm().text_color(foreground).child(bio))
            })
            .child(
                h_flex()
                    .items_center()
                    .gap_4()
                    .px_3()
                    .py_2()
                    .text_sm()
                    .text_color(muted_foreground)
                    .child(format!("Repos {}", fmt_num(user.public_repos)))
                    .child(format!("Followers {}", fmt_num(user.followers)))
                    .child(format!("Following {}", fmt_num(user.following))),
            )
            .when_some(user.company.clone(), |col, company| {
                col.child(
                    div()
                        .px_3()
                        .py_1()
                        .text_xs()
                        .text_color(muted_foreground)
                        .child(format!("Company: {company}")),
                )
            })
            .when_some(user.location.clone(), |col, location| {
                col.child(
                    div()
                        .px_3()
                        .py_1()
                        .text_xs()
                        .text_color(muted_foreground)
                        .child(format!("Location: {location}")),
                )
            })
            .into_any_element()
    }
}
