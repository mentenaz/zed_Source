//! Helm's issue screens: the list with its state filter and the detail
//! screen. The state filter and the comment thread are shared with the pull
//! request screens in `pulls.rs`.

use super::*;

impl HelmPanel {
    /// Loads `self.selected_repo`'s issue list for the current
    /// `issues_filter`. The Issues tab drops PR-shaped items (the `/issues`
    /// endpoint mixes issues and PRs).
    pub(super) fn load_issues(&mut self, cx: &mut Context<Self>) {
        let filter = self.issues_filter.clone();
        self.load_section(
            cx,
            |this| &mut this.issues,
            |repo, gh_state| async move {
                let mut issues =
                    gh_list_issues(repo.owner.login, repo.name, filter, &gh_state).await?;
                // GitHub's issues endpoint returns pull requests too.
                issues.retain(|issue| issue.pull_request.is_none());
                Ok(issues)
            },
        );
    }

    /// User clicked an issue row: shows it in-panel instead of opening
    /// GitHub in the browser (`Issues`'s list `Issue` already has the full
    /// body — the list endpoint returns it — so this needs no fetch of its
    /// own beyond the comment thread).
    pub(super) fn open_issue_detail(&mut self, issue: Issue, cx: &mut Context<Self>) {
        let number = issue.number;
        self.selected_issue = Some(issue);
        self.navigate_to(HelmScreen::IssueDetail, cx);
        self.load_detail_comments(number, cx);
    }

    /// Loads the comment thread for whichever issue/PR is now open —
    /// shared between both, since GitHub serves PR "conversation" comments
    /// from the same `/issues/{number}/comments` endpoint (see `Comment`'s
    /// doc comment).
    pub(super) fn load_detail_comments(&mut self, number: u64, cx: &mut Context<Self>) {
        let Some(repo) = self.selected_repo.clone() else {
            return;
        };
        self.detail_comments_state = LoadState::Loading;
        self.detail_comments.clear();
        cx.notify();
        let gh_state = self.gh_state.clone();
        cx.spawn(async move |this, cx| {
            let result = on_tokio(async move {
                gh_list_issue_comments(repo.owner.login, repo.name, number, &gh_state).await
            })
            .await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(comments) => {
                        this.detail_comments = comments;
                        this.detail_comments_state = LoadState::Idle;
                    }
                    Err(e) => {
                        this.detail_comments_state = LoadState::Error;
                        this.error_msg = e;
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Small Open/Closed/All toggle for the Issues and Pulls screens — the
    /// active state is a primary button, the others ghost.
    pub(super) fn state_filter_button(
        &self,
        id: &'static str,
        label: &'static str,
        filter: &'static str,
        active: bool,
        screen: HelmScreen,
        cx: &mut Context<Self>,
    ) -> Button {
        Button::new(id)
            .small()
            .when(active, |b| b.primary())
            .when(!active, |b| b.ghost())
            .label(label)
            .on_click(cx.listener(move |this, _, _, cx| {
                this.set_tab_filter(screen, filter, cx);
            }))
    }

    /// Applies a filter to whichever of Issues/Pulls owns it and reloads the
    /// list.
    pub(super) fn set_tab_filter(&mut self, screen: HelmScreen, filter: &str, cx: &mut Context<Self>) {
        match screen {
            HelmScreen::Issues => {
                self.issues_filter = filter.to_string();
                self.load_issues(cx);
            }
            HelmScreen::Pulls => {
                self.pulls_filter = filter.to_string();
                self.load_pulls(cx);
            }
            _ => {}
        }
    }

    /// The Issues tab — filterable list of `self.selected_repo`'s issues
    /// (PRs filtered out), each row opening its page in the browser.
    pub(super) fn render_issues(&self, cx: &mut Context<Self>) -> impl IntoElement {

        let filter_row = h_flex()
            .items_center()
            .gap_1()
            .px_3()
            .py_2()
            .child(self.state_filter_button(
                "helm-issues-open",
                "Open",
                "open",
                self.issues_filter == "open",
                HelmScreen::Issues,
                cx,
            ))
            .child(self.state_filter_button(
                "helm-issues-closed",
                "Closed",
                "closed",
                self.issues_filter == "closed",
                HelmScreen::Issues,
                cx,
            ))
            .child(self.state_filter_button(
                "helm-issues-all",
                "All",
                "all",
                self.issues_filter == "all",
                HelmScreen::Issues,
                cx,
            ));

        self.list_screen(
            &self.issues,
            &self.issues_list,
            Some(filter_row.into_any_element()),
            ListLabels {
                loading: "Loading issues…",
                error: "Failed to load issues",
                empty: "No issues found",
            },
            |this, cx| this.load_issues(cx),
            cx,
        )
    }

    /// The open issue's own view: title/number/state/author/labels, full
    /// body (rendered as markdown — already present on `selected_issue`,
    /// since the `/issues` list endpoint returns it, so no extra fetch was
    /// needed just for this), and the comment thread
    /// `load_detail_comments` loaded on entry. Back/forward navigation
    /// comes for free from the shared nav bar (`render`'s `show_nav`).
    pub(super) fn render_issue_detail(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let muted_foreground = cx.theme().muted_foreground;
        let foreground = cx.theme().foreground;
        let success = cx.theme().success;
        let danger = cx.theme().danger;
        let border = cx.theme().border;

        let Some(issue) = self.selected_issue.clone() else {
            return v_flex()
                .p_4()
                .child(
                    div()
                        .text_sm()
                        .text_color(muted_foreground)
                        .child("No issue selected"),
                )
                .into_any_element();
        };

        let state_open = issue.state != "closed";
        let author = issue
            .user
            .as_ref()
            .map(|u| u.login.clone())
            .unwrap_or_default();
        let url = issue.html_url.clone();

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
                                            .bg((if state_open { success } else { danger })
                                                .opacity(0.15))
                                            .text_color(if state_open { success } else { danger })
                                            .child(if state_open { "Open" } else { "Closed" }),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(muted_foreground)
                                            .child(format!("#{} opened by {author}", issue.number)),
                                    ),
                            )
                            .child(
                                Button::new("helm-issue-open-browser")
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
                            .child(issue.title.clone()),
                    )
                    .when(!issue.labels.is_empty(), |col| {
                        col.child(
                            h_flex()
                                .flex_wrap()
                                .gap_1()
                                .children(issue.labels.iter().map(|label| {
                                    div()
                                        .px_1p5()
                                        .rounded_full()
                                        .text_xs()
                                        .bg(muted_foreground.opacity(0.15))
                                        .text_color(muted_foreground)
                                        .child(label.name.clone())
                                })),
                        )
                    }),
            )
            .child(div().h_px().w_full().bg(border))
            .child(
                div().p_3().text_sm().child(markdown(
                    issue
                        .body
                        .clone()
                        .filter(|b| !b.trim().is_empty())
                        .unwrap_or_else(|| "_No description provided._".to_string()),
                )),
            )
            .child(div().h_px().w_full().bg(border))
            .child(self.render_comment_thread(cx))
            .into_any_element()
    }

    /// The comment thread for whichever issue/PR is open — shared by both
    /// detail views since they're backed by the same `detail_comments`
    /// (see that field's doc comment). Each comment's body renders as
    /// markdown too, same as the issue/PR body above it.
    pub(super) fn render_comment_thread(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let muted_foreground = cx.theme().muted_foreground;
        let foreground = cx.theme().foreground;
        let border = cx.theme().border;

        if self.detail_comments_state == LoadState::Loading {
            return v_flex()
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
                                .child("Loading comments…"),
                        ),
                )
                .into_any_element();
        }

        if self.detail_comments_state == LoadState::Error {
            return v_flex()
                .gap_3()
                .p_4()
                .child(
                    div()
                        .text_sm()
                        .text_color(muted_foreground)
                        .child("Failed to load comments"),
                )
                .into_any_element();
        }

        if self.detail_comments.is_empty() {
            return v_flex()
                .p_4()
                .child(
                    div()
                        .text_sm()
                        .text_color(muted_foreground)
                        .child("No comments yet"),
                )
                .into_any_element();
        }

        v_flex()
            .gap_3()
            .p_3()
            .children(self.detail_comments.iter().map(|comment| {
                let author = comment
                    .user
                    .as_ref()
                    .map(|u| u.login.clone())
                    .unwrap_or_default();
                v_flex()
                    .gap_1()
                    .pb_3()
                    .border_b_1()
                    .border_color(border)
                    .child(
                        div()
                            .text_xs()
                            .font_semibold()
                            .text_color(foreground)
                            .child(author),
                    )
                    .child(
                        div()
                            .text_sm()
                            .child(markdown(comment.body.clone().unwrap_or_default())),
                    )
            }))
            .into_any_element()
    }
}

/// One row of the Issues screen: title, number and author, comment count,
/// and the issue's labels.
pub(super) fn issue_row(ix: usize, issue: &Issue, cx: &App) -> ListItem {
    let muted_foreground = cx.theme().muted_foreground;
    let foreground = cx.theme().foreground;
    let number = issue.number;
    let closed = issue.state == "closed";
    let author = issue
        .user
        .as_ref()
        .map(|u| u.login.clone())
        .unwrap_or_default();
    let comments = issue.comments;
    let labels = issue.labels.clone();
    ListItem::new(("helm-issue", ix))
        .child(
            v_flex()
                .gap_0p5()
                .min_w_0()
                .child(
                    div()
                        .truncate()
                        .text_sm()
                        .font_semibold()
                        .text_color(if closed {
                            muted_foreground
                        } else {
                            foreground
                        })
                        .child(issue.title.clone()),
                )
                .child(
                    h_flex()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .text_xs()
                                .text_color(muted_foreground)
                                .child(format!("#{number} · {author}")),
                        )
                        .when(comments > 0, |row| {
                            row.child(
                                div()
                                    .text_xs()
                                    .text_color(muted_foreground)
                                    .child(format!("{comments} comments")),
                            )
                        }),
                ),
        )
        .suffix(move |_, _| {
            h_flex()
                .items_center()
                .gap_2()
                .children(labels.iter().map(|label| {
                    div()
                        .px_1p5()
                        .rounded_full()
                        .text_xs()
                        .bg(muted_foreground.opacity(0.15))
                        .text_color(muted_foreground)
                        .child(label.name.clone())
                }))
                .child(
                    Icon::new(IconName::ChevronRight)
                        .xsmall()
                        .text_color(muted_foreground),
                )
        })
}
