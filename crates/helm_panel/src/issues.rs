//! Helm's issue screens: the list with its state filter and the detail
//! screen. The state filter and the comment thread are shared with the pull
//! request screens in `pulls.rs`.

use super::*;

impl HelmPanel {
    /// Loads `self.selected_repo`'s issue list for the current
    /// `issues_filter`. The Issues tab drops PR-shaped items (the `/issues`
    /// endpoint mixes issues and PRs).
    pub(super) fn load_issues(&mut self, cx: &mut Context<Self>) {
        self.load_issues_page(1, cx);
    }

    pub(super) fn load_issues_page(&mut self, page: u32, cx: &mut Context<Self>) {
        let filter = self.issues_filter.clone();
        let remembered_filter = filter.clone();
        // GitHub's issues endpoint returns pull requests too, and counts
        // them in its pages. Leaving them out means a page can show fewer
        // than ten rows while more pages follow.
        fn without_pulls(mut issues: Page<Issue>) -> Page<Issue> {
            issues.items.retain(|issue| issue.pull_request.is_none());
            issues
        }
        self.load_section_page(
            cx,
            |this| &mut this.issues,
            page,
            move |repo, gh_state| {
                let request =
                    requests::issues(&repo.owner.login, &repo.name, &remembered_filter);
                peek_page(gh_state, request, page, PAGE_SIZE).map(without_pulls)
            },
            move |repo, gh_state| async move {
                let request = requests::issues(&repo.owner.login, &repo.name, &filter);
                let issues = fetch_page(&gh_state, request, page, PAGE_SIZE).await?;
                Ok::<_, GhError>(without_pulls(issues))
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
                        this.error_msg = e.to_string();
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The Open / Closed / All switch of the Issues and Pulls screens.
    /// `current` is the filter in force.
    pub(super) fn state_filter(
        &self,
        id: &'static str,
        current: &str,
        screen: HelmScreen,
        cx: &mut Context<Self>,
    ) -> ButtonGroup {
        const FILTERS: [(&str, &str); 3] =
            [("Open", "open"), ("Closed", "closed"), ("All", "all")];
        ButtonGroup::new(id)
            .outline()
            .compact()
            .small()
            .children(FILTERS.iter().enumerate().map(|(ix, (label, filter))| {
                Button::new(("helm-state-filter", ix))
                    .label(*label)
                    .selected(current == *filter)
            }))
            .on_click(cx.listener(move |this, clicked: &Vec<usize>, _, cx| {
                if let Some((_, filter)) = clicked.first().and_then(|ix| FILTERS.get(*ix)) {
                    this.set_tab_filter(screen, filter, cx);
                }
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

        let filter_row = h_flex().items_center().px_3().py_2().child(self.state_filter(
            "helm-issues-filter",
            &self.issues_filter,
            HelmScreen::Issues,
            cx,
        ));

        self.list_screen(
            self.issues
                .paged_status(|this, page, cx| this.load_issues_page(page, cx)),
            &self.issues_list,
            Some(filter_row.into_any_element()),
            ListLabels {
                loading: "Loading issues…",
                error: "Failed to load issues",
                empty: "No issues found",
            },
            |this, cx| this.load_issues_page(this.issues.page, cx),
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
                                        if state_open {
                                            Pill::success()
                                        } else {
                                            Pill::danger()
                                        }
                                        .xsmall()
                                        .rounded_full()
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
                                .children(
                                    issue.labels.iter().map(|label| chip(label.name.clone())),
                                ),
                        )
                    }),
            )
            .child(Separator::horizontal())
            .child(
                div().p_3().text_sm().child(markdown(
                    issue
                        .body
                        .clone()
                        .filter(|b| !b.trim().is_empty())
                        .unwrap_or_else(|| "_No description provided._".to_string()),
                )),
            )
            .child(Separator::horizontal())
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
                .children(labels.iter().map(|label| chip(label.name.clone())))
                .child(
                    Icon::new(IconName::ChevronRight)
                        .xsmall()
                        .text_color(muted_foreground),
                )
        })
}
