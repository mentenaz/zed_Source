//! The branch and tag picker's list.
//!
//! The tab loads a repository's first 100 branches and 100 tags when it
//! opens. Typing in the picker filters those at once; after a short pause it
//! also asks GitHub for every branch and tag whose name contains the text
//! ([`gh_search_refs`]), so a repository with hundreds of branches can still
//! be searched and a pasted name found. A newer keystroke drops the search
//! still on its way, which is also what spaces the requests out.

use std::sync::Arc;
use std::time::Duration;

use gpui::{App, Task, WeakEntity, Window};
use gpui_component::{
    IndexPath,
    searchable_list::{SearchableListDelegate, SearchableListItem},
    select::SelectState,
};
use helm_backend::{
    github::{GhState, gh_search_refs},
    on_tokio,
};

/// How long typing must pause before GitHub is asked.
const SEARCH_DELAY: Duration = Duration::from_millis(300);
/// How many branches, and how many tags, one search asks GitHub for.
const SEARCH_LIMIT: u32 = 50;
/// Shorter text than this is only matched against what is loaded: almost
/// every name would contain one letter.
const MIN_SEARCH_LENGTH: usize = 2;

/// The picker's label for a branch, as the tab parses it back.
pub(crate) fn branch_label(name: &str) -> String {
    format!("branch: {name}")
}

/// The picker's label for a tag.
pub(crate) fn tag_label(name: &str) -> String {
    format!("tag: {name}")
}

/// What GitHub is searched for, and the picker the answer goes back to.
#[derive(Clone)]
pub(crate) struct RefSearch {
    pub(crate) gh_state: Arc<GhState>,
    pub(crate) owner: String,
    pub(crate) name: String,
    pub(crate) picker: WeakEntity<SelectState<RefList>>,
}

/// The picker's rows: the branches and tags loaded with the tab, filtered by
/// what is typed, followed by what GitHub found beyond them.
#[derive(Clone)]
pub(crate) struct RefList {
    loaded: Vec<String>,
    matched: Vec<String>,
    search: Option<RefSearch>,
}

impl RefList {
    pub(crate) fn new(loaded: Vec<String>, search: Option<RefSearch>) -> Self {
        Self {
            matched: loaded.clone(),
            loaded,
            search,
        }
    }

    /// The same list, showing `matched`.
    fn showing(&self, matched: Vec<String>) -> Self {
        Self {
            loaded: self.loaded.clone(),
            matched,
            search: self.search.clone(),
        }
    }
}

/// `local` followed by whichever of `found` it does not already have.
fn merge(local: Vec<String>, found: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut merged = local;
    for label in found {
        if !merged.contains(&label) {
            merged.push(label);
        }
    }
    merged
}

impl SearchableListDelegate for RefList {
    type Item = String;

    fn items_count(&self, _: usize) -> usize {
        self.matched.len()
    }

    fn item(&self, ix: IndexPath) -> Option<&Self::Item> {
        self.matched.get(ix.row)
    }

    fn position<V>(&self, value: &V) -> Option<IndexPath>
    where
        Self::Item: SearchableListItem<Value = V>,
        V: PartialEq,
    {
        self.matched
            .iter()
            .position(|label| label.value() == value)
            .map(|ix| IndexPath::default().row(ix))
    }

    fn perform_search(&mut self, query: &str, window: &mut Window, cx: &mut App) -> Task<()> {
        let local: Vec<String> = self
            .loaded
            .iter()
            .filter(|label| label.matches(query))
            .cloned()
            .collect();
        self.matched = local.clone();

        let query = query.trim().to_string();
        let Some(search) = self.search.clone() else {
            return Task::ready(());
        };
        if query.chars().count() < MIN_SEARCH_LENGTH {
            return Task::ready(());
        }
        let list = self.clone();
        window.spawn(cx, async move |cx| {
            cx.background_executor().timer(SEARCH_DELAY).await;
            let found = {
                let (owner, name, gh_state) = (
                    search.owner.clone(),
                    search.name.clone(),
                    search.gh_state.clone(),
                );
                on_tokio(async move {
                    gh_search_refs(owner, name, query, SEARCH_LIMIT, &gh_state).await
                })
                .await
            };
            // Offline or refused: the loaded matches stay, as before.
            let Ok(found) = found else {
                return;
            };
            let found = found
                .branches
                .iter()
                .map(|name| branch_label(name))
                .chain(found.tags.iter().map(|name| tag_label(name)));
            let matched = merge(local, found);
            cx.update(|window, cx| {
                search
                    .picker
                    .update(cx, |picker, cx| {
                        picker.set_items(list.showing(matched), window, cx);
                    })
                    .ok();
            })
            .ok();
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_github_found_follows_the_loaded_matches_without_repeats() {
        let local = vec![branch_label("login"), tag_label("login-v1")];
        let found = vec![
            branch_label("feature/login-fix"),
            branch_label("login"),
            tag_label("login-v1"),
        ];
        assert_eq!(
            merge(local, found),
            vec![
                branch_label("login"),
                tag_label("login-v1"),
                branch_label("feature/login-fix"),
            ]
        );
    }
}
