//! Helm's pull request screens: the list, the detail screen, and creating a
//! pull request. The state filter and the comment thread are in `issues.rs`.

use super::*;

impl HelmPanel {
    /// Loads `self.selected_repo`'s pull requests for the current
    /// `pulls_filter`.
    pub(super) fn load_pulls(&mut self, cx: &mut Context<Self>) {
        self.load_pulls_page(1, cx);
    }

    pub(super) fn load_pulls_page(&mut self, page: u32, cx: &mut Context<Self>) {
        let filter = self.pulls_filter.clone();
        self.load_repo_page(cx, |this| &mut this.pulls, page, move |repo| {
            requests::pulls(&repo.owner.login, &repo.name, &filter)
        });
    }

    /// Same as [`Self::open_issue_detail`], for a PR row.
    pub(super) fn open_pr_detail(&mut self, pr: Pull, cx: &mut Context<Self>) {
        let number = pr.number;
        self.selected_pr = Some(pr);
        self.navigate_to(HelmScreen::PrDetail, cx);
        self.load_detail_comments(number, cx);
    }

    /// Opens the "Create pull request" modal, prefilling the base branch
    /// with `self.selected_repo`'s default branch.
    pub(super) fn open_create_pull_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let default_base = self
            .selected_repo
            .as_ref()
            .map(|r| r.default_branch.clone())
            .unwrap_or_default();
        self.open_workspace_modal(HelmModalKind::CreatePull(default_base), window, cx);
    }

    pub(super) fn handle_create_pull(
        &mut self,
        title: String,
        body: String,
        head: String,
        base: String,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.run_action(
            HelmAction::CreatePull {
                title,
                body,
                head,
                base,
            },
            true,
            cx,
        );
    }

    /// The Pull requests tab — filterable list showing head→base branches,
    /// each row opening its page in the browser.
    pub(super) fn render_pulls(&self, cx: &mut Context<Self>) -> impl IntoElement {

        let filter_row = h_flex()
            .items_center()
            .justify_between()
            .gap_1()
            .px_3()
            .py_2()
            .child(
                h_flex()
                    .items_center()
                    .gap_1()
                    .child(self.state_filter_button(
                        "helm-pulls-open",
                        "Open",
                        "open",
                        self.pulls_filter == "open",
                        HelmScreen::Pulls,
                        cx,
                    ))
                    .child(self.state_filter_button(
                        "helm-pulls-closed",
                        "Closed",
                        "closed",
                        self.pulls_filter == "closed",
                        HelmScreen::Pulls,
                        cx,
                    ))
                    .child(self.state_filter_button(
                        "helm-pulls-all",
                        "All",
                        "all",
                        self.pulls_filter == "all",
                        HelmScreen::Pulls,
                        cx,
                    )),
            )
            .child(
                Button::new("pulls-create")
                    .ghost()
                    .xsmall()
                    .icon(IconName::Plus)
                    .label("New pull request")
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_create_pull_dialog(window, cx)
                    })),
            );

        self.list_screen(
            self.pulls
                .paged_status(|this, page, cx| this.load_pulls_page(page, cx)),
            &self.pulls_list,
            Some(filter_row.into_any_element()),
            ListLabels {
                loading: "Loading pull requests…",
                error: "Failed to load pull requests",
                empty: "No pull requests found",
            },
            |this, cx| this.load_pulls_page(this.pulls.page, cx),
            cx,
        )
    }

    /// Same as [`Self::render_issue_detail`], for a PR — head→base branch
    /// info and a merged/closed/open status take the place of labels,
    /// everything else (body, comment thread, browser link) is identical.
    pub(super) fn render_pr_detail(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let muted_foreground = cx.theme().muted_foreground;
        let foreground = cx.theme().foreground;

        let Some(pr) = self.selected_pr.clone() else {
            return v_flex()
                .p_4()
                .child(
                    div()
                        .text_sm()
                        .text_color(muted_foreground)
                        .child("No pull request selected"),
                )
                .into_any_element();
        };

        let (status_label, status_tag) = if pr.merged {
            ("Merged", Pill::info())
        } else if pr.state == "closed" {
            ("Closed", Pill::danger())
        } else {
            ("Open", Pill::success())
        };
        let author = pr
            .user
            .as_ref()
            .map(|u| u.login.clone())
            .unwrap_or_default();
        let head_label = if pr.head.label.is_empty() {
            pr.head.r#ref.clone()
        } else {
            pr.head.label.clone()
        };
        let base_label = if pr.base.label.is_empty() {
            pr.base.r#ref.clone()
        } else {
            pr.base.label.clone()
        };
        let url = pr.html_url.clone();

        v_flex()
            .child(
                v_flex()
                    .gap_2()
                    .p_3()
                    .child(
                        h_flex()
                            .items_center()
                            .justify_between()
                            .child(
                                h_flex()
                                    .items_center()
                                    .gap_2()
                                    .child(status_tag.xsmall().rounded_full().child(status_label))
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(muted_foreground)
                                            .child(format!("#{} opened by {author}", pr.number)),
                                    ),
                            )
                            .child(
                                Button::new("helm-pr-open-browser")
                                    .ghost()
                                    .xsmall()
                                    .icon(IconName::ExternalLink)
                                    .tooltip("Open in Browser")
                                    .on_click(move |_, _, cx| cx.open_url(&url)),
                            ),
                    )
                    .child(
                        div()
                            .text_base()
                            .font_semibold()
                            .text_color(foreground)
                            .child(pr.title.clone()),
                    )
                    .child(
                        div()
                            .text_xs()
                            .font_family("Cascadia Mono")
                            .text_color(muted_foreground)
                            .child(format!("{head_label} → {base_label}")),
                    ),
            )
            .child(Separator::horizontal())
            .child(
                div().p_3().text_sm().child(markdown(
                    pr.body
                        .clone()
                        .filter(|b| !b.trim().is_empty())
                        .unwrap_or_else(|| "_No description provided._".to_string()),
                )),
            )
            .child(Separator::horizontal())
            .child(self.render_comment_thread(cx))
            .into_any_element()
    }
}

/// One row of the Pull Requests screen: title, number, and the branches it
/// merges from and into.
pub(super) fn pull_row(ix: usize, pr: &Pull, cx: &App) -> ListItem {
    let muted_foreground = cx.theme().muted_foreground;
    let foreground = cx.theme().foreground;
    let number = pr.number;
    let open = !pr.merged && pr.state != "closed";
    let head_label = if pr.head.label.is_empty() {
        pr.head.r#ref.clone()
    } else {
        pr.head.label.clone()
    };
    let base_label = if pr.base.label.is_empty() {
        pr.base.r#ref.clone()
    } else {
        pr.base.label.clone()
    };
    ListItem::new(("helm-pr", ix))
        .child(
            v_flex()
                .gap_0p5()
                .min_w_0()
                .child(
                    div()
                        .truncate()
                        .text_sm()
                        .font_semibold()
                        .text_color(if open {
                            foreground
                        } else {
                            muted_foreground
                        })
                        .child(pr.title.clone()),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(muted_foreground)
                        .child(format!("#{number}")),
                ),
        )
        .suffix(move |_, _| {
            h_flex()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .text_xs()
                        .font_family("Cascadia Mono")
                        .text_color(muted_foreground)
                        .child(format!("{head_label} → {base_label}")),
                )
                .child(
                    Icon::new(IconName::ChevronRight)
                        .xsmall()
                        .text_color(muted_foreground),
                )
        })
}
