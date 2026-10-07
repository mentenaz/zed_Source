//! Helm's list screens, on `gpui_component`'s `List`.
//!
//! Every list screen is the same shape: an optional header, then a spinner,
//! an error with Retry, an "empty" line, or the rows. That shape is here
//! once ([`HelmPanel::list_screen`]). A screen supplies its header, its
//! labels, and how to draw one row.
//!
//! The rows are drawn by `gpui_component::list::List`, which gives every
//! screen the same keyboard handling (`up`, `down`, `enter`), draws only the
//! rows in view, and scrolls by itself. It asks one thing in return: every
//! row of a list must be the same height.
//!
//! The rows themselves stay where they were, in the screen's [`Section`] on
//! the panel. [`RowsDelegate`] reads them from there when the list is drawn,
//! so there is one copy of the data and nothing to keep in step.

use std::rc::Rc;

use gpui_component::{
    IndexPath,
    list::{List, ListDelegate, ListEvent, ListState},
};

use super::*;

/// Draws row `ix` of a list. Selection highlighting and clicks are the
/// list's business, so a row sets neither.
type RowFn<T> = Rc<dyn Fn(usize, &T, &App) -> ListItem>;

/// What happens when a row is clicked, or `enter` is pressed on it.
type ConfirmFn = fn(&mut HelmPanel, usize, &mut Window, &mut Context<HelmPanel>);

pub(super) struct RowsDelegate<T: 'static> {
    panel: WeakEntity<HelmPanel>,
    section: fn(&HelmPanel) -> &Section<T>,
    row: RowFn<T>,
    selected: Option<IndexPath>,
}

impl<T: 'static> ListDelegate for RowsDelegate<T> {
    type Item = ListItem;

    fn items_count(&self, _section: usize, cx: &App) -> usize {
        self.panel
            .upgrade()
            .map_or(0, |panel| (self.section)(panel.read(cx)).items.len())
    }

    fn render_item(
        &mut self,
        ix: IndexPath,
        _window: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) -> Option<ListItem> {
        let panel = self.panel.upgrade()?;
        let item = (self.section)(panel.read(cx)).items.get(ix.row)?;
        Some((self.row)(ix.row, item, cx))
    }

    fn set_selected_index(
        &mut self,
        ix: Option<IndexPath>,
        _window: &mut Window,
        _cx: &mut Context<ListState<Self>>,
    ) {
        self.selected = ix;
    }
}

/// One screen's list widget. Created once with the panel and kept for its
/// lifetime; it shows whatever its [`Section`] holds at the time.
pub(super) struct ListView<T: 'static> {
    state: Entity<ListState<RowsDelegate<T>>>,
    _confirm: Subscription,
}

impl<T: 'static> ListView<T> {
    pub(super) fn new(
        section: fn(&HelmPanel) -> &Section<T>,
        row: impl Fn(usize, &T, &App) -> ListItem + 'static,
        on_confirm: ConfirmFn,
        window: &mut Window,
        cx: &mut Context<HelmPanel>,
    ) -> Self {
        let delegate = RowsDelegate {
            panel: cx.weak_entity(),
            section,
            row: Rc::new(row),
            selected: None,
        };
        let state = cx.new(|cx| ListState::new(delegate, window, cx));
        let confirm = cx.subscribe_in(
            &state,
            window,
            move |this, _list, event: &ListEvent, window, cx| {
                if let ListEvent::Confirm(ix) = event {
                    on_confirm(this, ix.row, window, cx);
                }
            },
        );
        ListView {
            state,
            _confirm: confirm,
        }
    }
}

/// The three lines a list screen shows in place of its rows.
pub(super) struct ListLabels {
    pub(super) loading: &'static str,
    pub(super) error: &'static str,
    pub(super) empty: &'static str,
}

impl HelmPanel {
    /// A whole list screen: `header` (always shown, whatever the list is
    /// doing), then the section's spinner, error, empty line or rows.
    pub(super) fn list_screen<T: 'static>(
        &self,
        section: &Section<T>,
        list: &ListView<T>,
        header: Option<gpui::AnyElement>,
        labels: ListLabels,
        retry: impl Fn(&mut Self, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let muted_foreground = cx.theme().muted_foreground;
        let border = cx.theme().border;
        let centered = || v_flex().flex_1().items_center().justify_center().p_4();
        let line = |text: &str| {
            div()
                .text_sm()
                .text_color(muted_foreground)
                .child(text.to_string())
        };

        let body = if section.state == LoadState::Loading {
            centered()
                .child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(Spinner::new().small())
                        .child(line(labels.loading)),
                )
                .into_any_element()
        } else if section.state == LoadState::Error {
            v_flex()
                .gap_3()
                .p_4()
                .child(line(labels.error))
                // The reason GitHub gave, so a failure is not just "failed".
                .when(!section.error.is_empty(), |column| {
                    column.child(
                        div()
                            .text_xs()
                            .text_color(muted_foreground)
                            .child(section.error.clone()),
                    )
                })
                .child(
                    Button::new("helm-list-retry")
                        .outline()
                        .label("Retry")
                        .on_click(cx.listener(move |this, _, _, cx| retry(this, cx))),
                )
                .into_any_element()
        } else if section.items.is_empty() {
            centered().child(line(labels.empty)).into_any_element()
        } else {
            let state = list.state.clone();
            div()
                // The list draws only the rows in view, so it needs a height
                // to fill: `flex_1` takes what the header leaves, `min_h_0`
                // lets it be shorter than its content.
                .flex_1()
                .min_h_0()
                .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                    state.update(cx, |list, cx| list.focus(window, cx));
                })
                .child(List::new(&list.state))
                .into_any_element()
        };

        v_flex()
            .size_full()
            .when_some(header, |screen, header| {
                screen
                    .child(header)
                    .child(div().h_px().w_full().bg(border))
            })
            .child(body)
            .into_any_element()
    }
}
