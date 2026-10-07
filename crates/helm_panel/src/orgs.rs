//! Helm's organisation screens: the list and an organisation's detail.

use super::*;

impl HelmPanel {
    /// User picked an org from the list: sets `selected_org` (which also
    /// switches the identity header to the org's own avatar/name, see
    /// `render_identity_header`), navigates to `OrgDetail`, and kicks off
    /// the detail fetch.
    pub(super) fn select_org(&mut self, org: String, cx: &mut Context<Self>) {
        self.selected_org = Some(org.clone());
        self.org_detail = None;
        self.navigate_to(HelmScreen::OrgDetail, cx);
        self.load_org(org, cx);
    }

    pub(super) fn load_org(&mut self, org: String, cx: &mut Context<Self>) {
        self.load_with(
            cx,
            |gh_state| async move { gh_get_org_detail(org, &gh_state).await },
            |this, detail| this.org_detail = Some(detail),
        );
    }

    /// The org list — mirrors the old `OrgListScreen`.
    pub(super) fn render_org_list(&self, cx: &mut Context<Self>) -> impl IntoElement {
        self.list_screen(
            self.org_logins.status(),
            &self.org_logins_list,
            None,
            // The organisations arrive with sign-in, so this list has no
            // load of its own to wait for or to fail.
            ListLabels {
                loading: "Loading organizations…",
                error: "Failed to load organizations",
                empty: "No organizations",
            },
            |_, _| {},
            cx,
        )
    }

    /// The selected org's detail — mirrors the old `OrgDetailScreen`.
    /// "Repositories" shows the org's real repo count and routes into
    /// `RepoList` via [`Self::open_repo_list`].
    pub(super) fn render_org_detail(&self, cx: &mut Context<Self>) -> impl IntoElement {
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
                                .child("Loading organization…"),
                        ),
                )
                .into_any_element();
        }

        let Some(org) = self.org_detail.clone() else {
            return v_flex()
                .gap_3()
                .p_4()
                .child(
                    div()
                        .text_sm()
                        .text_color(muted_foreground)
                        .child("Failed to load"),
                )
                .child(
                    Button::new("org-retry")
                        .outline()
                        .label("Retry")
                        .on_click(cx.listener(|this, _, _, cx| {
                            if let Some(org) = this.selected_org.clone() {
                                this.load_org(org, cx);
                            }
                        })),
                )
                .into_any_element();
        };

        let total_repos = org.public_repos + org.total_private_repos.unwrap_or(0);

        let stats_row = h_flex()
            .items_center()
            .gap_4()
            .px_3()
            .py_2()
            .text_sm()
            .text_color(muted_foreground)
            .child(format!("Repos {}", fmt_num(total_repos)))
            .child(format!("Followers {}", fmt_num(org.followers)));

        let info_row = |icon: IconName, value: String| {
            h_flex()
                .items_center()
                .gap_2()
                .px_3()
                .py_1()
                .text_sm()
                .child(Icon::new(icon).xsmall().text_color(muted_foreground))
                .child(div().flex_1().min_w_0().text_color(foreground).child(value))
        };

        let mut info_rows = v_flex();
        if let Some(location) = org.location.clone() {
            info_rows = info_rows.child(info_row(IconName::Globe, location));
        }
        if let Some(email) = org.email.clone() {
            info_rows = info_rows.child(info_row(IconName::Inbox, email));
        }
        if let Some(blog) = org.blog.clone() {
            let url = blog.clone();
            info_rows = info_rows.child(
                info_row(IconName::ExternalLink, blog)
                    .id("helm-org-blog")
                    .cursor_pointer()
                    .on_click(move |_, _, cx| cx.open_url(&url)),
            );
        }
        if let Some(twitter) = org.twitter_username.clone() {
            info_rows = info_rows.child(
                info_row(IconName::ExternalLink, format!("@{twitter}"))
                    .id("helm-org-twitter")
                    .cursor_pointer()
                    .on_click(move |_, _, cx| {
                        cx.open_url(&format!("https://x.com/{twitter}"));
                    }),
            );
        }

        v_flex()
            .child(stats_row)
            .child(div().h_px().w_full().bg(border))
            .child(info_rows)
            .child(div().h_px().w_full().bg(border))
            .child(
                ListItem::new("helm-org-repositories")
                    .child(div().text_color(foreground).child("Repositories"))
                    .suffix(move |_, _| {
                        h_flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .text_color(muted_foreground)
                                    .child(total_repos.to_string()),
                            )
                            .child(
                                Icon::new(IconName::ChevronRight)
                                    .xsmall()
                                    .text_color(muted_foreground),
                            )
                    })
                    .on_click(cx.listener(|this, _, _, cx| this.open_repo_list(cx))),
            )
            .into_any_element()
    }
}

/// One row of the Organizations screen.
pub(super) fn org_row(ix: usize, org: &String, cx: &App) -> ListItem {
    let foreground = cx.theme().foreground;
    let muted_foreground = cx.theme().muted_foreground;
    ListItem::new(("helm-org", ix))
        .child(div().text_color(foreground).child(org.clone()))
        .suffix(move |_, _| {
            Icon::new(IconName::ChevronRight)
                .xsmall()
                .text_color(muted_foreground)
        })
}
