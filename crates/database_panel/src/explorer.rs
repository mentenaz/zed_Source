//! The schema explorer pane: a shared case-insensitive filter input plus a
//! `gpui_component` `tree()` built from `TreeState`'s native selection.
//! Tables are top-level folder entries (id `table_name`, label `"Name  (N)"`),
//! each expanding to one leaf row per column (id `table_name::column_name`).

use database_backend::{ConnectionConfig, ConnectionId, TableInfo};
use gpui::{
    AnyElement, Context, FontWeight, IntoElement, InteractiveElement as _,
    ParentElement as _, StatefulInteractiveElement as _, Styled as _, div, px,
};
use gpui_component::{
    ActiveTheme as _, Icon, IconName as GIconName, Sizable as _, Size, badge::Badge, h_flex,
    input::Input, list::ListItem, tree::{TreeState, tree}, v_flex,
};
use ui::{Color, Label, LabelCommon as _, LabelSize};

use crate::{DatabasePanel, SchemaState};

impl DatabasePanel {
    /// The primary schema browser: filter + tree. One `tree_state` per
    /// connection, so expansion state and rows for a connection survive tab
    /// switches and re-renders.
    pub(crate) fn render_explorer_pane(
        &self,
        connection: &ConnectionConfig,
        tables: &[TableInfo],
        _cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let id = connection.id;
        let is_loading =
            matches!(self.schemas.get(&id), None | Some(SchemaState::Loading));
        let schema_filter = self.schema_filter.clone();
        let tree_state = self.tree_states.get(&id).cloned();

        v_flex()
            .id(("database-explorer", id.0))
            .size_full()
            .min_w_0()
            .child(
                v_flex()
                    .gap_2()
                    .px_3()
                    .py_2()
                    .child(
                        h_flex()
                            .w_full()
                            .justify_between()
                            .items_center()
                            .child(
                                Label::new("Schema")
                                    .size(LabelSize::Small)
                                    .weight(FontWeight::BOLD),
                            )
                            .child(Badge::new().count(tables.len()).with_size(Size::XSmall)),
                    )
                    .child(
                        Input::new(&schema_filter)
                            .bordered(true)
                            .cleanable(true)
                            .with_size(Size::Medium),
                    )
                    .child(
                        Label::new(if is_loading {
                            "Loading schema…"
                        } else {
                            "Schema loaded"
                        })
                        .size(LabelSize::XSmall)
                        .color(Color::Muted),
                    ),
            )
            .child(
                div()
                    .id(("database-explorer-scroll", id.0))
                    .w_full()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p_1()
                    .child(self.render_schema_tree(id, tree_state)),
            )
    }

    fn render_schema_tree(
        &self,
        _id: ConnectionId,
        tree_state: Option<gpui::Entity<TreeState>>,
    ) -> AnyElement {
        let Some(tree_state) = tree_state else {
            return v_flex()
                .w_full()
                .items_center()
                .gap_2()
                .py_6()
                .child(
                    Label::new("No schema loaded")
                        .size(LabelSize::Small)
                        .color(Color::Muted),
                )
                .into_any_element();
        };

        tree(
            &tree_state,
            |ix, entry, is_selected, _window, cx| {
                let level = entry.depth() as f32;
                let is_folder = entry.is_folder();
                let item = entry.item();

                ListItem::new(ix)
                    .selected(is_selected)
                    .px(px(16.0 * level + 8.0))
                    .child(
                        h_flex()
                            .w_full()
                            .items_center()
                            .gap_1_5()
                            .child(if is_folder {
                                Icon::new(GIconName::Folder)
                                    .with_size(Size::XSmall)
                                    .text_color(cx.theme().foreground)
                            } else {
                                Icon::new(GIconName::ChevronRight)
                                    .with_size(Size::XSmall)
                                    .text_color(cx.theme().muted_foreground)
                            })
                            .child(
                                Label::new(item.label.clone())
                                    .size(if is_folder {
                                        LabelSize::Small
                                    } else {
                                        LabelSize::XSmall
                                    })
                                    .weight(if is_folder {
                                        FontWeight::MEDIUM
                                    } else {
                                        FontWeight::NORMAL
                                    }),
                            ),
                    )
            },
        )
        .into_any_element()
    }
}