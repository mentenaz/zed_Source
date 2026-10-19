//! Shared UI components for the Helm panel and Workspace.

use std::sync::Arc;

use helm_backend::github::{GhState, Repo};

pub mod list_view;
pub mod rows;
pub mod section;
pub mod widgets;

pub use list_view::*;
pub use rows::*;
pub use section::*;
pub use widgets::*;

/// A view that displays GitHub data for an optional repository.
pub trait HelmView: Sized + 'static {
    fn gh_state(&self) -> &Arc<GhState>;
    fn repo(&self) -> Option<&Repo>;

    /// Whether `started` is still the view's GitHub state. A view that
    /// switches account gets a new state, so an answer fetched with the old
    /// one belongs to the previous account and must be dropped.
    fn is_current_state(&self, started: &Arc<GhState>) -> bool {
        Arc::ptr_eq(self.gh_state(), started)
    }
}
