//! Helm's organisation screens: the list, an organisation's detail, and its
//! members.

use super::*;

impl HelmPanel {
    /// User picked an org from the list: sets `selected_org` (which also
    /// switches the identity header to the org's own avatar/name, see
    /// `render_identity_header`), navigates to `OrgDetail`, and kicks off
    /// the detail fetch.
    pub(super) fn select_org(&mut self, org: String, cx: &mut Context<Self>) {
        self.selected_org = Some(org.clone());
        self.org_detail = None;
        self.org_role = None;
        self.org_members.clear();
        self.navigate_to(HelmScreen::OrgDetail, cx);
        self.load_org(org, cx);
    }

    pub(super) fn load_org(&mut self, org: String, cx: &mut Context<Self>) {
        self.load_with(
            cx,
            |gh_state| async move {
                let detail = gh_get_org_detail(org.clone(), &gh_state).await?;
                // The page is worth showing without the role, so a failure
                // to read it is not a failure to load the organisation.
                let role = gh_get_org_role(&org, &gh_state).await.ok().flatten();
                Ok::<_, GhError>((detail, role))
            },
            |this, (detail, role)| {
                this.org_detail = Some(detail);
                this.org_role = role;
            },
        );
    }

    /// Opens the member list of the organisation in view.
    pub(super) fn open_org_members(&mut self, cx: &mut Context<Self>) {
        self.navigate_to(HelmScreen::OrgMembers, cx);
        self.load_org_members_page(1, cx);
    }

    /// Loads one page of members. The rows are not a repository's, so this
    /// does by hand what `load_repo_page` does for the repository screens.
    pub(super) fn load_org_members_page(&mut self, page: u32, cx: &mut Context<Self>) {
        let Some(org) = self.selected_org.clone() else {
            return;
        };
        let gh_state = self.gh_state.clone();
        let remembered =
            peek_page::<OrgMember>(&gh_state, requests::org_members(&org), page, PAGE_SIZE);
        let load = match remembered {
            Some(remembered) => self.org_members.begin_page_with(remembered),
            None => self.org_members.begin_page(page),
        };
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = on_tokio(async move {
                fetch_page::<OrgMember>(&gh_state, requests::org_members(&org), page, PAGE_SIZE)
                    .await
            })
            .await
            .map_err(|error| error.to_string());
            this.update(cx, |this, cx| {
                if this.org_members.is_current(load) {
                    this.org_members.finish_page(result);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The members of the organisation in view. A row opens that person's
    /// profile. Read-only: nobody is invited or removed from here.
    pub(super) fn render_org_members(&self, cx: &mut Context<Self>) -> impl IntoElement {
        self.list_screen(
            self.org_members
                .paged_status(|this, page, cx| this.load_org_members_page(page, cx)),
            &self.org_members_list,
            None,
            ListLabels {
                loading: "Loading members…",
                error: "Failed to load members",
                empty: "No members",
            },
            |this, cx| this.load_org_members_page(1, cx),
            cx,
        )
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

        // "admin" is what GitHub calls an owner.
        let role = match self.org_role.as_deref() {
            Some("admin") => "Owner".to_string(),
            Some("member") => "Member".to_string(),
            Some(other) => other.to_string(),
            None => "Not a member".to_string(),
        };
        let mut stats = DescriptionList::vertical()
            .bordered(false)
            .columns(2)
            .item("Repositories", fmt_num(total_repos), 1)
            .item("Followers", fmt_num(org.followers), 1)
            .item("Public", fmt_num(org.public_repos), 1);
        // GitHub only tells a member how many private repositories there are.
        if let Some(private_repos) = org.total_private_repos {
            stats = stats.item("Private", fmt_num(private_repos), 1);
        }
        let stats = div().px_3().py_2().child(
            stats
                .item("Your role", role, 1)
                .item("Created", short_date(&org.created_at), 1),
        );

        let link = |id: &'static str, text: String, url: String| {
            Link::new(id).href(url).child(text).into_any_element()
        };
        let has_about = org.is_verified
            || org.location.is_some()
            || org.email.is_some()
            || org.blog.is_some()
            || org.twitter_username.is_some();
        let mut about = DescriptionList::horizontal()
            .bordered(false)
            .columns(1)
            .label_width(px(80.));
        if org.is_verified {
            about = about.item("Verified", "Yes", 1);
        }
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
            .child(
                ListItem::new("helm-org-members")
                    .child(div().text_color(foreground).child("Members"))
                    .suffix(move |_, _| {
                        Icon::new(IconName::ChevronRight)
                            .xsmall()
                            .text_color(muted_foreground)
                    })
                    .on_click(cx.listener(|this, _, _, cx| this.open_org_members(cx))),
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

/// One row of an organisation's Members screen.
pub(super) fn org_member_row(ix: usize, member: &OrgMember, cx: &App) -> ListItem {
    let foreground = cx.theme().foreground;
    let muted_foreground = cx.theme().muted_foreground;
    ListItem::new(("helm-org-member", ix))
        .child(
            h_flex()
                .items_center()
                .gap_2()
                .child(
                    Avatar::new()
                        .src(member.avatar_url.clone())
                        .name(member.login.clone())
                        .with_size(px(24.)),
                )
                .child(div().text_color(foreground).child(member.login.clone())),
        )
        .suffix(move |_, _| {
            Icon::new(IconName::ChevronRight)
                .xsmall()
                .text_color(muted_foreground)
        })
}
