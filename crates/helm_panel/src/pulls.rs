//! Helm's pull request screens: the list, the detail screen, and creating a
//! pull request. The state filter and the comment thread are in `issues.rs`.

use super::*;

impl HelmPanel {
    /// Loads `self.selected_repo`'s pull requests for the current
    /// `pulls_filter`.
    pub(super) fn load_pulls(&mut self, cx: &mut Context<Self>) {
        let filter = self.pulls_filter.clone();
        self.load_section(
            cx,
            |this| &mut this.pulls,
            |repo, gh_state| async move {
                gh_list_pulls(repo.owner.login, repo.name, filter, &gh_state).await
            },
        );
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
        let muted_foreground = cx.theme().muted_foreground;
        let foreground = cx.theme().foreground;
        let success = cx.theme().success;
        let danger = cx.theme().danger;

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

        if self.pulls.state == LoadState::Loading {
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
                                .child("Loading pull requests…"),
                        ),
                )
                .into_any_element();
        }

        if self.pulls.state == LoadState::Error {
            return v_flex()
                .gap_3()
                .p_4()
                .child(
                    div()
                        .text_sm()
                        .text_color(muted_foreground)
                        .child("Failed to load pull requests"),
                )
                .child(
                    Button::new("pulls-retry")
                        .outline()
                        .label("Retry")
                        .on_click(cx.listener(|this, _, _, cx| this.load_pulls(cx))),
                )
                .into_any_element();
        }

        if self.pulls.items.is_empty() {
            return v_flex()
                .flex_1()
                .items_center()
                .justify_center()
                .p_4()
                .child(
                    div()
                        .text_sm()
                        .text_color(muted_foreground)
                        .child("No pull requests found"),
                )
                .into_any_element();
        }

        let pulls_len = self.pulls.items.len();
        let pulls_cursor = self.pulls.cursor;
        let pulls_list = v_flex()
            .id("helm-pulls-list")
            .track_focus(&self.pulls.focus)
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, window, cx| {
                window.focus(&this.pulls.focus, cx);
            }))
            .key_context("HelmRowList")
            .on_action(cx.listener(move |this, _: &SelectNextRow, _, cx| {
                this.pulls.cursor = step_selected(this.pulls.cursor, pulls_len, true);
                cx.notify();
            }))
            .on_action(cx.listener(move |this, _: &SelectPrevRow, _, cx| {
                this.pulls.cursor = step_selected(this.pulls.cursor, pulls_len, false);
                cx.notify();
            }))
            .on_action(cx.listener(move |this, _: &OpenSelectedRow, _, cx| {
                let Some(pr) = this.pulls.cursor.and_then(|ix| this.pulls.items.get(ix)).cloned()
                else {
                    return;
                };
                this.open_pr_detail(pr, cx);
            }))
            .py_1()
            .children(self.pulls.items.iter().enumerate().map(|(ix, pr)| {
                let number = pr.number;
                let title = pr.title.clone();
                let merged = pr.merged;
                let status = if merged {
                    "merged"
                } else if pr.state == "closed" {
                    "closed"
                } else {
                    "open"
                };
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
                let pr_for_click = pr.clone();
                ListItem::new(format!("helm-pr-{number}"))
                    .selected(pulls_cursor == Some(ix))
                    .child(
                        v_flex()
                            .gap_0p5()
                            .min_w_0()
                            .child(
                                div()
                                    .truncate()
                                    .text_sm()
                                    .font_semibold()
                                    .text_color(if status == "open" {
                                        foreground
                                    } else {
                                        muted_foreground
                                    })
                                    .child(title),
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
                            .child(div().text_xs().text_color(if status == "open" {
                                success
                            } else {
                                danger
                            }))
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
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.pulls.cursor = Some(ix);
                        this.open_pr_detail(pr_for_click.clone(), cx);
                    }))
            }));

        v_flex()
            .child(filter_row)
            .child(div().h_px().w_full().bg(cx.theme().border))
            .child(pulls_list)
            .into_any_element()
    }

    /// Same as [`Self::render_issue_detail`], for a PR — head→base branch
    /// info and a merged/closed/open status take the place of labels,
    /// everything else (body, comment thread, browser link) is identical.
    pub(super) fn render_pr_detail(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let muted_foreground = cx.theme().muted_foreground;
        let foreground = cx.theme().foreground;
        let success = cx.theme().success;
        let danger = cx.theme().danger;
        let border = cx.theme().border;

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

        let (status_label, status_color) = if pr.merged {
            ("Merged", success)
        } else if pr.state == "closed" {
            ("Closed", danger)
        } else {
            ("Open", success)
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
                                    .child(
                                        div()
                                            .px_1p5()
                                            .py_0p5()
                                            .rounded_full()
                                            .text_xs()
                                            .bg(status_color.opacity(0.15))
                                            .text_color(status_color)
                                            .child(status_label),
                                    )
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
            .child(div().h_px().w_full().bg(border))
            .child(
                div().p_3().text_sm().child(markdown(
                    pr.body
                        .clone()
                        .filter(|b| !b.trim().is_empty())
                        .unwrap_or_else(|| "_No description provided._".to_string()),
                )),
            )
            .child(div().h_px().w_full().bg(border))
            .child(self.render_comment_thread(cx))
            .into_any_element()
    }
}
