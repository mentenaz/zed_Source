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
    /// Which page `items` is, counting from 1. A list that is not paged is
    /// always on page 1 of 1.
    pub(super) page: u32,
    /// The last page there is.
    pub(super) last_page: u32,
    /// The rows are a remembered answer, shown while GitHub is asked
    /// whether they are still right.
    pub(super) refreshing: bool,
    /// Counts page loads. An answer that arrives after a newer load began,
    /// or after the screen was left, belongs to an older number and is
    /// dropped (see [`Self::is_current`]).
    load: u64,
}

impl<T> Default for Section<T> {
    fn default() -> Self {
        Section {
            items: Vec::new(),
            state: LoadState::Idle,
            error: String::new(),
            page: 1,
            last_page: 1,
            refreshing: false,
            load: 0,
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
        self.page = 1;
        self.last_page = 1;
        self.refreshing = false;
        self.load += 1;
    }

    /// Marks a load as started. [`HelmPanel::load_section`] calls this; a
    /// loader that fills several sections from one request calls it itself.
    pub(super) fn begin(&mut self) {
        self.state = LoadState::Loading;
        self.error.clear();
        self.refreshing = false;
    }

    /// Stores a finished load. A failure keeps the rows already shown: the
    /// screen says the load failed, and the old rows are not passed off as
    /// the answer because the state is `Error`.
    pub(super) fn finish(&mut self, result: Result<Vec<T>, String>) {
        match result {
            Ok(items) => {
                self.items = items;
                self.state = LoadState::Idle;
                self.page = 1;
                self.last_page = 1;
            }
            Err(error) => {
                self.state = LoadState::Error;
                self.error = error;
            }
        }
    }

    /// Marks a load of page `page` as started. The page is recorded now, so
    /// that the pager shows where the user asked to go while it loads, and
    /// a retry after a failure asks for the same page.
    ///
    /// Returns the load's number, to hand to [`Self::is_current`] when the
    /// answer arrives.
    pub(super) fn begin_page(&mut self, page: u32) -> u64 {
        self.begin();
        self.page = page.max(1);
        self.last_page = self.last_page.max(self.page);
        self.load += 1;
        self.load
    }

    /// Starts a load of a page whose last answer is remembered: shows that
    /// answer at once, with no spinner, while GitHub is asked again.
    /// Returns the load's number, as [`Self::begin_page`] does.
    pub(super) fn begin_page_with(&mut self, remembered: Page<T>) -> u64 {
        self.items = remembered.items;
        self.page = remembered.page;
        self.last_page = remembered.last_page;
        self.state = LoadState::Idle;
        self.error.clear();
        self.refreshing = true;
        self.load += 1;
        self.load
    }

    /// Whether `load` is still the latest page load, so that its answer is
    /// the one to show.
    pub(super) fn is_current(&self, load: u64) -> bool {
        self.load == load
    }

    /// Stores a finished load of one page.
    pub(super) fn finish_page(&mut self, result: Result<Page<T>, String>) {
        self.refreshing = false;
        match result {
            Ok(page) => {
                self.items = page.items;
                self.page = page.page;
                self.last_page = page.last_page;
                self.state = LoadState::Idle;
            }
            Err(error) => {
                self.state = LoadState::Error;
                self.error = error;
            }
        }
    }
}

/// How many rows a paged list shows at a time.
pub(super) const PAGE_SIZE: u32 = 10;

/// Which rows of a list of `len` are on page `page`, for a list the panel
/// holds in full and pages itself. Returns the page actually shown (a page
/// past the end becomes the last one), the last page, and the rows.
pub(super) fn page_slice(len: usize, page: u32, per_page: u32) -> (u32, u32, std::ops::Range<usize>) {
    let per_page = per_page.max(1) as usize;
    let last_page = len.div_ceil(per_page).max(1) as u32;
    let page = page.clamp(1, last_page);
    let start = (page as usize - 1) * per_page;
    (page, last_page, start..(start + per_page).min(len))
}

impl HelmPanel {
    /// Loads one section's rows.
    ///
    /// `section` picks the section out of the panel. It is called once
    /// before the request and once after, so it is a plain function, not
    /// a closure that captures anything.
    pub(super) fn load_section_with<T, E, Fut>(
        &mut self,
        cx: &mut Context<Self>,
        section: fn(&mut Self) -> &mut Section<T>,
        fetch: impl FnOnce(Arc<GhState>) -> Fut + Send + 'static,
    ) where
        T: Send + 'static,
        E: std::fmt::Display + Send + 'static,
        Fut: Future<Output = Result<Vec<T>, E>> + Send + 'static,
    {
        section(self).begin();
        cx.notify();
        let gh_state = self.gh_state.clone();
        cx.spawn(async move |this, cx| {
            let result = on_tokio(fetch(gh_state)).await;
            this.update(cx, |this, cx| {
                section(this).finish(result.map_err(|error| error.to_string()));
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// [`Self::load_section_with`] for rows that belong to the repository
    /// the user drilled into. Does nothing when no repository is selected.
    pub(super) fn load_section<T, E, Fut>(
        &mut self,
        cx: &mut Context<Self>,
        section: fn(&mut Self) -> &mut Section<T>,
        fetch: impl FnOnce(Repo, Arc<GhState>) -> Fut + Send + 'static,
    ) where
        T: Send + 'static,
        E: std::fmt::Display + Send + 'static,
        Fut: Future<Output = Result<Vec<T>, E>> + Send + 'static,
    {
        let Some(repo) = self.selected_repo.clone() else {
            return;
        };
        self.load_section_with(cx, section, move |gh_state| fetch(repo, gh_state));
    }
}

impl HelmPanel {
    /// Loads page `page` of one section's rows for the selected repository.
    /// Does nothing when no repository is selected.
    ///
    /// `peek` looks for the page among the answers already remembered. When
    /// it is there the rows are shown at once and `fetch` only confirms or
    /// replaces them; GitHub answers "not modified" for free when nothing
    /// changed.
    pub(super) fn load_section_page<T, E, Fut>(
        &mut self,
        cx: &mut Context<Self>,
        section: fn(&mut Self) -> &mut Section<T>,
        page: u32,
        peek: impl FnOnce(&Repo, &GhState) -> Option<Page<T>>,
        fetch: impl FnOnce(Repo, Arc<GhState>) -> Fut + Send + 'static,
    ) where
        T: Send + 'static,
        E: std::fmt::Display + Send + 'static,
        Fut: Future<Output = Result<Page<T>, E>> + Send + 'static,
    {
        let Some(repo) = self.selected_repo.clone() else {
            return;
        };
        let gh_state = self.gh_state.clone();
        let load = match peek(&repo, &gh_state) {
            Some(remembered) => section(self).begin_page_with(remembered),
            None => section(self).begin_page(page),
        };
        cx.notify();
        cx.spawn(async move |this, cx| {
            let result = on_tokio(fetch(repo, gh_state)).await;
            this.update(cx, |this, cx| {
                let section = section(this);
                // Another page was asked for, or the screen was left, while
                // this one was on its way.
                if section.is_current(load) {
                    section.finish_page(result.map_err(|error| error.to_string()));
                }
                // Even a dropped answer changed how many requests are left.
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// [`Self::load_section_page`] for the common case: one request, whose
    /// answer is the list itself. `request` builds it for the repository.
    pub(super) fn load_repo_page<T>(
        &mut self,
        cx: &mut Context<Self>,
        section: fn(&mut Self) -> &mut Section<T>,
        page: u32,
        request: impl Fn(&Repo) -> requests::ApiRequest + Send + 'static,
    ) where
        T: serde::de::DeserializeOwned + Send + 'static,
    {
        let Some(repo) = self.selected_repo.clone() else {
            return;
        };
        let remembered = request(&repo);
        self.load_section_page(
            cx,
            section,
            page,
            move |_, gh_state| peek_page::<T>(gh_state, remembered, page, PAGE_SIZE),
            move |repo, gh_state| async move {
                fetch_page::<T>(&gh_state, request(&repo), page, PAGE_SIZE).await
            },
        );
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
    fn a_page_load_records_where_it_is() {
        let mut section = Section::<u32>::default();
        assert_eq!((section.page, section.last_page), (1, 1));

        section.begin_page(1);
        section.finish_page(Ok(Page {
            items: vec![1, 2],
            page: 1,
            last_page: 7,
        }));
        assert_eq!((section.page, section.last_page), (1, 7));

        // Going to page 3 shows page 3 at once, while it loads.
        section.begin_page(3);
        assert!(section.state == LoadState::Loading);
        assert_eq!((section.page, section.last_page), (3, 7));
        section.finish_page(Ok(Page {
            items: vec![5, 6],
            page: 3,
            last_page: 7,
        }));
        assert_eq!(section.items, vec![5, 6]);

        // A failed page keeps its number, so Retry asks for the same one.
        section.begin_page(4);
        section.finish_page(Err("boom".to_string()));
        assert!(section.state == LoadState::Error);
        assert_eq!(section.page, 4);

        // Leaving the screen goes back to the start.
        section.clear();
        assert_eq!((section.page, section.last_page), (1, 1));
    }

    #[test]
    fn a_remembered_page_is_shown_while_it_is_checked() {
        let mut section = Section::<u32>::default();
        let load = section.begin_page_with(Page {
            items: vec![1, 2],
            page: 2,
            last_page: 5,
        });
        // The rows are there with no spinner, marked as being checked.
        assert!(section.state == LoadState::Idle);
        assert!(section.refreshing);
        assert_eq!(section.items, vec![1, 2]);
        assert_eq!((section.page, section.last_page), (2, 5));
        assert!(section.status().refreshing);

        assert!(section.is_current(load));
        section.finish_page(Ok(Page {
            items: vec![1, 2, 3],
            page: 2,
            last_page: 6,
        }));
        assert!(!section.refreshing);
        assert_eq!(section.items, vec![1, 2, 3]);
        assert_eq!(section.last_page, 6);

        // A check that fails says so; it does not leave old rows standing
        // as if they had been confirmed.
        section.begin_page_with(Page {
            items: vec![9],
            page: 1,
            last_page: 1,
        });
        section.finish_page(Err("offline".to_string()));
        assert!(section.state == LoadState::Error);
        assert!(!section.refreshing);
    }

    #[test]
    fn only_the_latest_page_load_counts() {
        let mut section = Section::<u32>::default();
        // Next is clicked twice before the first answer arrives.
        let first = section.begin_page(2);
        let second = section.begin_page(3);
        assert!(!section.is_current(first));
        assert!(section.is_current(second));

        // Leaving the screen drops whatever is still on its way.
        section.clear();
        assert!(!section.is_current(second));
        assert!(!section.refreshing);

        // A remembered page is a load like any other.
        let third = section.begin_page_with(Page {
            items: vec![1],
            page: 1,
            last_page: 1,
        });
        let fourth = section.begin_page(2);
        assert!(!section.is_current(third));
        assert!(section.is_current(fourth));
    }

    #[test]
    fn a_list_held_in_full_is_cut_into_pages() {
        // 25 rows, ten a page: three pages, the last one short.
        assert_eq!(page_slice(25, 1, 10), (1, 3, 0..10));
        assert_eq!(page_slice(25, 2, 10), (2, 3, 10..20));
        assert_eq!(page_slice(25, 3, 10), (3, 3, 20..25));
        // A page past the end, as after a search narrows the list, becomes
        // the last one.
        assert_eq!(page_slice(25, 9, 10), (3, 3, 20..25));
        assert_eq!(page_slice(25, 0, 10), (1, 3, 0..10));
        // Exactly full pages, one row, and nothing at all.
        assert_eq!(page_slice(20, 2, 10), (2, 2, 10..20));
        assert_eq!(page_slice(1, 1, 10), (1, 1, 0..1));
        assert_eq!(page_slice(0, 1, 10), (1, 1, 0..0));
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
