//! One list screen's worth of data.
//!
//! Every list in Helm (issues, releases, tags, and so on) has rows, a load
//! state, and the reason the last load failed. They used to be loose fields
//! on `HelmPanel`, with a single loading flag and error message shared by
//! the whole panel. A `Section` keeps one list's three together, so two
//! lists loading at once no longer share one spinner and one error.
//!
//! Which row is selected, and keyboard focus, are not here: the list widget
//! that draws the rows owns those (see `list_view.rs`).

use std::future::Future;

use super::*;

pub(super) struct Section<T> {
    pub(super) items: Vec<T>,
    pub(super) state: LoadState,
    /// Why the last load failed. Empty unless `state` is `Error`.
    pub(super) error: String,
}

impl<T> Default for Section<T> {
    fn default() -> Self {
        Section {
            items: Vec::new(),
            state: LoadState::Idle,
            error: String::new(),
        }
    }
}

impl<T> Section<T> {
    /// Drops the rows and any load state, for when the screen they belong
    /// to is left behind.
    pub(super) fn clear(&mut self) {
        self.items.clear();
        self.state = LoadState::Idle;
        self.error.clear();
    }

    /// Marks a load as started. [`HelmPanel::load_section`] calls this; a
    /// loader that fills several sections from one request calls it itself.
    pub(super) fn begin(&mut self) {
        self.state = LoadState::Loading;
        self.error.clear();
    }

    /// Stores a finished load. A failure keeps the rows already shown: the
    /// screen says the load failed, and the old rows are not passed off as
    /// the answer because the state is `Error`.
    pub(super) fn finish(&mut self, result: Result<Vec<T>, String>) {
        match result {
            Ok(items) => {
                self.items = items;
                self.state = LoadState::Idle;
            }
            Err(error) => {
                self.state = LoadState::Error;
                self.error = error;
            }
        }
    }
}

impl HelmPanel {
    /// Loads one section's rows.
    ///
    /// `section` picks the section out of the panel. It is called once
    /// before the request and once after, so it is a plain function, not
    /// a closure that captures anything.
    pub(super) fn load_section_with<T, Fut>(
        &mut self,
        cx: &mut Context<Self>,
        section: fn(&mut Self) -> &mut Section<T>,
        fetch: impl FnOnce(Arc<GhState>) -> Fut + Send + 'static,
    ) where
        T: Send + 'static,
        Fut: Future<Output = Result<Vec<T>, String>> + Send + 'static,
    {
        section(self).begin();
        cx.notify();
        let gh_state = self.gh_state.clone();
        cx.spawn(async move |this, cx| {
            let result = on_tokio(fetch(gh_state)).await;
            this.update(cx, |this, cx| {
                section(this).finish(result);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// [`Self::load_section_with`] for rows that belong to the repository
    /// the user drilled into. Does nothing when no repository is selected.
    pub(super) fn load_section<T, Fut>(
        &mut self,
        cx: &mut Context<Self>,
        section: fn(&mut Self) -> &mut Section<T>,
        fetch: impl FnOnce(Repo, Arc<GhState>) -> Fut + Send + 'static,
    ) where
        T: Send + 'static,
        Fut: Future<Output = Result<Vec<T>, String>> + Send + 'static,
    {
        let Some(repo) = self.selected_repo.clone() else {
            return;
        };
        self.load_section_with(cx, section, move |gh_state| fetch(repo, gh_state));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_load_goes_from_loading_to_idle_with_its_rows() {
        let mut section = Section::<u32>::default();
        assert!(section.state == LoadState::Idle);

        section.begin();
        assert!(section.state == LoadState::Loading);

        section.finish(Ok(vec![1, 2, 3]));
        assert!(section.state == LoadState::Idle);
        assert_eq!(section.items, vec![1, 2, 3]);
        assert!(section.error.is_empty());
    }

    #[test]
    fn a_failed_load_records_why_and_a_retry_clears_it() {
        let mut section = Section::<u32>::default();
        section.finish(Ok(vec![1]));

        section.begin();
        section.finish(Err("GitHub API 500".to_string()));
        assert!(section.state == LoadState::Error);
        assert_eq!(section.error, "GitHub API 500");
        // The rows from before are still there; the state says not to trust
        // them as the answer to this load.
        assert_eq!(section.items, vec![1]);

        section.begin();
        assert!(section.state == LoadState::Loading);
        assert!(section.error.is_empty());
    }

    #[test]
    fn two_sections_load_independently() {
        // The point of the type: one section's failure or spinner is not
        // another's.
        let mut issues = Section::<u32>::default();
        let mut releases = Section::<u32>::default();
        issues.begin();
        releases.begin();
        issues.finish(Err("boom".to_string()));
        assert!(issues.state == LoadState::Error);
        assert!(releases.state == LoadState::Loading);
        releases.finish(Ok(vec![7]));
        assert!(releases.state == LoadState::Idle);
        assert!(releases.error.is_empty());
        assert!(issues.state == LoadState::Error);
    }

    #[test]
    fn clearing_drops_rows_and_state() {
        let mut section = Section::<u32>::default();
        section.finish(Ok(vec![1, 2, 3]));
        section.begin();
        section.finish(Err("boom".to_string()));

        section.clear();
        assert!(section.items.is_empty());
        assert!(section.state == LoadState::Idle);
        assert!(section.error.is_empty());
    }

    #[test]
    fn a_section_reports_what_its_screen_should_show() {
        let mut section = Section::<u32>::default();
        assert!(section.status().is_empty);

        section.begin();
        assert!(section.status().state == LoadState::Loading);

        section.finish(Ok(vec![1]));
        let status = section.status();
        assert!(status.state == LoadState::Idle);
        assert!(!status.is_empty);

        section.finish(Err("GitHub API 403".to_string()));
        assert_eq!(section.status().error, "GitHub API 403");
    }
}
