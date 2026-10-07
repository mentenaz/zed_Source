//! Helm's branches screen for a repository.

use super::*;

impl HelmPanel {
    /// Loads the branch list for `self.selected_repo` — mirrors `load_repos`'s
    /// shape.
    pub(super) fn load_branches(&mut self, cx: &mut Context<Self>) {
        self.load_branches_page(1, cx);
    }

    pub(super) fn load_branches_page(&mut self, page: u32, cx: &mut Context<Self>) {
        if self.selected_repo.is_none() {
            return;
        }
        self.branches.items.clear();
        self.load_repo_page(cx, |this| &mut this.branches, page, |repo| {
            requests::branches(&repo.owner.login, &repo.name)
        });
    }

    /// The branches list — read-only, mirrors `render_repo_list`'s
    /// loading/error/empty states.
    pub(super) fn render_branches(&self, cx: &mut Context<Self>) -> impl IntoElement {
        self.list_screen(
            self.branches
                .paged_status(|this, page, cx| this.load_branches_page(page, cx)),
            &self.branches_list,
            None,
            ListLabels {
                loading: "Loading branches…",
                error: "Failed to load branches",
                empty: "No branches found",
            },
            |this, cx| this.load_branches_page(this.branches.page, cx),
            cx,
        )
    }
}

/// One row of the Branches screen: the branch's name, and whether it is the
/// repository's default branch or protected.
pub(super) fn branch_row(ix: usize, branch: &Branch, default_branch: &str, cx: &App) -> ListItem {
    let muted_foreground = cx.theme().muted_foreground;
    let foreground = cx.theme().foreground;
    let is_default = branch.name == default_branch;
    let protected = branch.protected;
    ListItem::new(("helm-branch", ix))
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
}
