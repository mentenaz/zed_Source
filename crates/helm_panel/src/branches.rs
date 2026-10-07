//! Helm's branches screen for a repository.

use super::*;

impl HelmPanel {
    /// Loads the branch list for `self.selected_repo` — mirrors `load_repos`'s
    /// shape.
    pub(super) fn load_branches(&mut self, cx: &mut Context<Self>) {
        if self.selected_repo.is_none() {
            return;
        }
        self.branches.items.clear();
        self.load_section(
            cx,
            |this| &mut this.branches,
            |repo, gh_state| async move {
                gh_get_branches(repo.owner.login, repo.name, &gh_state).await
            },
        );
    }

    /// The branches list — read-only, mirrors `render_repo_list`'s
    /// loading/error/empty states.
    pub(super) fn render_branches(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let muted_foreground = cx.theme().muted_foreground;
        let foreground = cx.theme().foreground;

        if self.branches.state == LoadState::Loading {
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
                                .child("Loading branches…"),
                        ),
                )
                .into_any_element();
        }

        if self.branches.state == LoadState::Error {
            return v_flex()
                .gap_3()
                .p_4()
                .child(
                    div()
                        .text_sm()
                        .text_color(muted_foreground)
                        .child("Failed to load branches"),
                )
                .child(
                    Button::new("branches-retry")
                        .outline()
                        .label("Retry")
                        .on_click(cx.listener(|this, _, _, cx| this.load_branches(cx))),
                )
                .into_any_element();
        }

        if self.branches.items.is_empty() {
            return v_flex()
                .flex_1()
                .items_center()
                .justify_center()
                .p_4()
                .child(
                    div()
                        .text_sm()
                        .text_color(muted_foreground)
                        .child("No branches found"),
                )
                .into_any_element();
        }

        let default_branch = self
            .selected_repo
            .as_ref()
            .map(|r| r.default_branch.clone())
            .unwrap_or_default();

        // Read-only list — branch rows have no click action, so this wires
        // up/down + a selection highlight only, no `OpenSelectedRow` (Enter
        // falls through as a no-op, matching what clicking a row already did).
        let len = self.branches.items.len();
        let cursor = self.branches.cursor;
        v_flex()
            .id("helm-branches-list")
            .track_focus(&self.branches.focus)
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, window, cx| {
                window.focus(&this.branches.focus, cx);
            }))
            .key_context("HelmRowList")
            .on_action(cx.listener(move |this, _: &SelectNextRow, _, cx| {
                this.branches.cursor = step_selected(this.branches.cursor, len, true);
                cx.notify();
            }))
            .on_action(cx.listener(move |this, _: &SelectPrevRow, _, cx| {
                this.branches.cursor = step_selected(this.branches.cursor, len, false);
                cx.notify();
            }))
            .py_1()
            .children(self.branches.items.iter().enumerate().map(|(ix, branch)| {
                let is_default = branch.name == default_branch;
                let protected = branch.protected;
                ListItem::new(format!("helm-branch-{}", branch.name))
                    .selected(cursor == Some(ix))
                    .child(div().text_color(foreground).child(branch.name.clone()))
                    .suffix(move |_, _| {
                        h_flex()
                            .items_center()
                            .gap_2()
                            .when(is_default, |row| {
                                row.child(
                                    div()
                                        .px_2()
                                        .py_0p5()
                                        .rounded_full()
                                        .text_xs()
                                        .bg(muted_foreground.opacity(0.15))
                                        .text_color(muted_foreground)
                                        .child("default"),
                                )
                            })
                            .when(protected, |row| {
                                row.child(
                                    div()
                                        .text_xs()
                                        .text_color(muted_foreground)
                                        .child("protected"),
                                )
                            })
                    })
            }))
            .into_any_element()
    }
}
