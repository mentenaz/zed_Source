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
            self.org_logins.status_for::<HelmPanel>(),
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

        if self.load_state == LoadState::Loading {
            return loading_screen("Loading organization…", cx);
        }

        let Some(org) = self.org_detail.clone() else {
            return failed_screen(
                "Failed to load organization",
                &self.error_msg,
                Some(
                    Button::new("org-retry")
                        .outline()
                        .label("Retry")
                        .on_click(cx.listener(|this, _, _, cx| {
                            if let Some(org) = this.selected_org.clone() {
                                this.load_org(org, cx);
                            }
                        })),
                ),
                cx,
            );
        };

        let total_repos = org.public_repos + org.total_private_repos.unwrap_or(0);

        let stats = div().px_3().py_2().child(
            DescriptionList::vertical()
                .bordered(false)
                .columns(2)
                .item("Repositories", fmt_num(total_repos), 1)
                .item("Followers", fmt_num(org.followers), 1),
        );

        let link = |id: &'static str, text: String, url: String| {
            Link::new(id).href(url).child(text).into_any_element()
        };
        let has_about = org.location.is_some()
            || org.email.is_some()
            || org.blog.is_some()
            || org.twitter_username.is_some();
        let mut about = DescriptionList::horizontal()
            .bordered(false)
            .columns(1)
            .label_width(px(80.));
        if let Some(location) = org.location.clone() {
            about = about.item("Location", location, 1);
        }
        if let Some(email) = org.email.clone() {
            about = about.item("Email", email, 1);
        }
        if let Some(blog) = org.blog.clone() {
            about = about.item("Website", link("helm-org-blog", blog.clone(), blog), 1);
        }
        if let Some(twitter) = org.twitter_username.clone() {
            about = about.item(
                "X",
                link(
                    "helm-org-twitter",
                    format!("@{twitter}"),
                    format!("https://x.com/{twitter}"),
                ),
                1,
            );
        }

        v_flex()
            .child(stats)
            .child(Separator::horizontal())
            .when(has_about, |column| {
                column
                    .child(div().px_3().py_2().child(about))
                    .child(Separator::horizontal())
            })
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
