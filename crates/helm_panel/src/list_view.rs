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
//! The rows themselves stay where they were, on the panel. [`RowsDelegate`]
//! reads them from there when the list is drawn, so there is one copy of the
//! data and nothing to keep in step.

use std::rc::Rc;

use gpui_component::{
    IndexPath,
    list::{List, ListDelegate, ListEvent, ListState},
};

use super::*;

/// How many rows a section of the list has.
type CountFn = Rc<dyn Fn(&HelmPanel, usize) -> usize>;

/// Draws one row. Selection highlighting and clicks are the list's
/// business, so a row sets neither. `None` skips the row, for one that
/// went away between counting and drawing.
type RowFn = Rc<dyn Fn(&HelmPanel, IndexPath, &App) -> Option<ListItem>>;

pub(super) struct RowsDelegate {
    panel: WeakEntity<HelmPanel>,
    /// One title per section. Empty for a list that is a single run of rows
    /// with no heading.
    titles: Vec<&'static str>,
    count: CountFn,
    row: RowFn,
    selected: Option<IndexPath>,
}

impl ListDelegate for RowsDelegate {
    type Item = ListItem;

    fn sections_count(&self, _cx: &App) -> usize {
        self.titles.len().max(1)
    }

    fn items_count(&self, section: usize, cx: &App) -> usize {
        self.panel
            .upgrade()
            .map_or(0, |panel| (self.count)(panel.read(cx), section))
    }

    fn render_item(
        &mut self,
        ix: IndexPath,
        _window: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) -> Option<ListItem> {
        let panel = self.panel.upgrade()?;
        (self.row)(panel.read(cx), ix, cx)
    }

    /// A section's heading. The list leaves out a section with no rows,
    /// heading included, so a screen that needs to say "none" says it in
    /// its own header.
    fn render_section_header(
        &mut self,
        section: usize,
        _window: &mut Window,
        cx: &mut Context<ListState<Self>>,
    ) -> Option<impl IntoElement> {
        let title = *self.titles.get(section)?;
        Some(
            div()
                .px_3()
                .pt_2()
                .pb_1()
                .text_xs()
                .font_semibold()
                .text_color(cx.theme().muted_foreground)
                .child(title),
        )
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
/// lifetime; it shows whatever the panel holds at the time.
pub(super) struct ListView {
    state: Entity<ListState<RowsDelegate>>,
    _confirm: Subscription,
}

impl ListView {
    /// A list over one [`Section`]. `on_confirm` runs when a row is clicked
    /// or `enter` is pressed on it.
    pub(super) fn new<T: 'static>(
        section: fn(&HelmPanel) -> &Section<T>,
        row: impl Fn(usize, &T, &App) -> ListItem + 'static,
        on_confirm: fn(&mut HelmPanel, usize, &mut Window, &mut Context<HelmPanel>),
        window: &mut Window,
        cx: &mut Context<HelmPanel>,
    ) -> Self {
        Self::sectioned(
            Vec::new(),
            move |panel, _| section(panel).items.len(),
            move |panel, ix, cx| Some(row(ix.row, section(panel).items.get(ix.row)?, cx)),
            move |panel, ix, window, cx| on_confirm(panel, ix.row, window, cx),
            window,
            cx,
        )
    }

    /// A list of several headed sections, or one whose rows are not simply
    /// a [`Section`]'s items (a filtered view, say). `count` and `row` read
    /// what they need from the panel.
    pub(super) fn sectioned(
        titles: Vec<&'static str>,
        count: impl Fn(&HelmPanel, usize) -> usize + 'static,
        row: impl Fn(&HelmPanel, IndexPath, &App) -> Option<ListItem> + 'static,
        on_confirm: impl Fn(&mut HelmPanel, IndexPath, &mut Window, &mut Context<HelmPanel>) + 'static,
        window: &mut Window,
        cx: &mut Context<HelmPanel>,
    ) -> Self {
        let delegate = RowsDelegate {
            panel: cx.weak_entity(),
            titles,
            count: Rc::new(count),
            row: Rc::new(row),
            selected: None,
        };
        let state = cx.new(|cx| ListState::new(delegate, window, cx));
        let confirm = cx.subscribe_in(
            &state,
            window,
            move |this, _list, event: &ListEvent, window, cx| {
                if let ListEvent::Confirm(ix) = event {
                    on_confirm(this, *ix, window, cx);
                }
            },
        );
        ListView {
            state,
            _confirm: confirm,
        }
    }
}

/// What a list screen needs to know about its data to choose between the
/// spinner, the error, the empty line and the rows.
pub(super) struct ListStatus {
    pub(super) state: LoadState,
    pub(super) error: String,
    pub(super) is_empty: bool,
}

impl<T> Section<T> {
    pub(super) fn status(&self) -> ListStatus {
        ListStatus {
            state: self.state,
            error: self.error.clone(),
            is_empty: self.items.is_empty(),
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
    /// doing), then the spinner, error, empty line or rows.
    pub(super) fn list_screen(
        &self,
        status: ListStatus,
        list: &ListView,
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

        let body = if status.state == LoadState::Loading {
            centered()
                .child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(Spinner::new().small())
                        .child(line(labels.loading)),
                )
                .into_any_element()
        } else if status.state == LoadState::Error {
            v_flex()
                .gap_3()
                .p_4()
                .child(line(labels.error))
                // The reason GitHub gave, so a failure is not just "failed".
                .when(!status.error.is_empty(), |column| {
                    column.child(
                        div()
                            .text_xs()
                            .text_color(muted_foreground)
                            .child(status.error.clone()),
                    )
                })
                .child(
                    Button::new("helm-list-retry")
                        .outline()
                        .label("Retry")
                        .on_click(cx.listener(move |this, _, _, cx| retry(this, cx))),
                )
                .into_any_element()
        } else if status.is_empty {
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
