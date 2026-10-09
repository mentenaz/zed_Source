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
        self.load_repo_page(
            cx,
            |this| &mut this.branches,
            page,
            |repo| requests::branches(&repo.owner.login, &repo.name),
        );
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
