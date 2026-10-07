//! The one way Helm's screens fetch what they show.
//!
//! Every screen used to carry its own copy of the same steps: mark the panel
//! as loading, clear the last error, run the request on the tokio runtime,
//! then store either the result or the error and repaint. Those steps are
//! here once. A screen's loader now says only what to fetch and where to put
//! it.

use std::future::Future;

use super::*;

impl HelmPanel {
    /// Runs `fetch` and hands what it returns to `store`.
    ///
    /// While the request is in flight the panel is `Loading`. It ends `Idle`
    /// with the result stored, or `Error` with the message in `error_msg`.
    pub(super) fn load_with<T, E, Fut>(
        &mut self,
        cx: &mut Context<Self>,
        fetch: impl FnOnce(Arc<GhState>) -> Fut + Send + 'static,
        store: impl FnOnce(&mut Self, T) + 'static,
    ) where
        T: Send + 'static,
        E: std::fmt::Display + Send + 'static,
        Fut: Future<Output = Result<T, E>> + Send + 'static,
    {
        self.load_state = LoadState::Loading;
        self.error_msg.clear();
        cx.notify();
        let gh_state = self.gh_state.clone();
        cx.spawn(async move |this, cx| {
            let result = on_tokio(fetch(gh_state)).await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(value) => {
                        store(this, value);
                        this.load_state = LoadState::Idle;
                    }
                    Err(e) => {
                        this.load_state = LoadState::Error;
                        this.error_msg = e.to_string();
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// [`Self::load_with`] for something that belongs to the repository the
    /// user drilled into. Does nothing when no repository is selected.
    pub(super) fn load_for_repo<T, E, Fut>(
        &mut self,
        cx: &mut Context<Self>,
        fetch: impl FnOnce(Repo, Arc<GhState>) -> Fut + Send + 'static,
        store: impl FnOnce(&mut Self, T) + 'static,
    ) where
        T: Send + 'static,
        E: std::fmt::Display + Send + 'static,
        Fut: Future<Output = Result<T, E>> + Send + 'static,
    {
        let Some(repo) = self.selected_repo.clone() else {
            return;
        };
        self.load_with(cx, move |gh_state| fetch(repo, gh_state), store);
    }
}
