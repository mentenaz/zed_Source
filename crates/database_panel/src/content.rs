//! The content pane: a `TabBar` with Overview / Tables / Views /
//! Relationships, plus the per-tab body. The Tables tab shows a table detail
//! (columns + foreign keys) when a table is selected in the explorer tree,
//! otherwise the flat tables list. Views and Relationships are read-only
//! until backend introspection lands — Views renders the spec's empty state,
//! and Relationships derives FK edges from `TableInfo::foreign_keys`.

use database_backend::{ConnectionConfig, TableInfo};
use gpui::{
    AnyElement, ClickEvent, Context, FontWeight, IntoElement, InteractiveElement as _,
    ParentElement as _, StatefulInteractiveElement as _, Styled as _, div,
    prelude::FluentBuilder as _,
};
use gpui_component::{
    ActiveTheme as _, Icon, IconName as GIconName, Sizable as _, Size,
    button::{Button, ButtonVariants as _},
    group_box::{GroupBox, GroupBoxVariants as _},
    h_flex,
    tab::{Tab, TabBar},
    table::{Table, TableBody, TableCell, TableHeader, TableHead, TableRow},
    tag::Tag,
    v_flex,
};
use ui::{Color, Label, LabelCommon as _, LabelSize};

use crate::{ContentTab, DatabasePanel};

impl DatabasePanel {
    /// The center pane: content `TabBar` on top, scrollable tab body below.
    pub(crate) fn render_content_pane(
        &self,
        connection: &ConnectionConfig,
        tables: &[TableInfo],
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let id = connection.id;

        v_flex()
            .id(("database-content", id.0))
            .size_full()
            .min_w_0()
            .child(
                TabBar::new(("database-content-tabs", id.0))
                    .children(ContentTab::ALL.iter().map(|tab| Tab::new().label(tab.title())))
                    .selected_index(self.content_tab.index())
                    .on_click(cx.listener(move |this, ix: &usize, _, cx| {
                        this.content_tab = ContentTab::from_index(*ix);
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .id(("database-content-scroll", id.0))
                    .w_full()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p_3()
                    .child(self.render_content_tab_body(connection, tables, cx)),
            )
    }

    fn render_content_tab_body(
        &self,
        connection: &ConnectionConfig,
        tables: &[TableInfo],
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match self.content_tab {
            ContentTab::Overview => self.render_overview(connection, tables).into_any_element(),
            ContentTab::Tables => self.render_tables_tab(connection, tables, cx),
            ContentTab::Views => self.render_views_empty_state(cx).into_any_element(),
            ContentTab::Relationships => {
                self.render_relationships(connection, tables).into_any_element()
            }
        }
    }

    /// Four summary cards (`GroupBox`), matching the spec's Overview tab.
    fn render_overview(
        &self,
        connection: &ConnectionConfig,
        tables: &[TableInfo],
    ) -> impl IntoElement {
        let total_columns = tables
            .iter()
            .map(|table| table.columns.len())
            .sum::<usize>();
        let primary_keys = tables
            .iter()
            .flat_map(|table| &table.columns)
            .filter(|column| column.primary_key)
            .count();
        let foreign_keys = tables
            .iter()
            .map(|table| table.foreign_keys.len())
            .sum::<usize>();

        v_flex()
            .w_full()
            .gap_2()
            .child(
                Label::new(connection.title.clone())
                    .size(LabelSize::Default)
                    .weight(FontWeight::BOLD),
            )
            .child(
                h_flex()
                    .w_full()
                    .gap_2()
                    .child(
                        GroupBox::new()
                            .outline()
                            .title(Label::new("Tables"))
                            .child(Label::new(tables.len().to_string()).size(LabelSize::Large)),
                    )
                    .child(
                        GroupBox::new()
                            .outline()
                            .title(Label::new("Columns"))
                            .child(Label::new(total_columns.to_string()).size(LabelSize::Large)),
                    )
                    .child(
                        GroupBox::new()
                            .outline()
                            .title(Label::new("Primary Keys"))
                            .child(Label::new(primary_keys.to_string()).size(LabelSize::Large)),
                    )
                    .child(
                        GroupBox::new()
                            .outline()
                            .title(Label::new("Foreign Keys"))
                            .child(Label::new(foreign_keys.to_string()).size(LabelSize::Large)),
                    ),
            )
            .child(
                Label::new("Select a table in the schema explorer to inspect it.")
                    .size(LabelSize::Small)
                    .color(Color::Muted),
            )
    }

    /// Tables tab: table detail when something is selected in the tree,
    /// otherwise the flat tables list.
    fn render_tables_tab(
        &self,
        connection: &ConnectionConfig,
        tables: &[TableInfo],
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match self.selected_table(connection, tables, cx) {
            Some((selected_column, table)) => self
                .render_table_detail(connection, table, selected_column, cx)
                .into_any_element(),
            None => self.render_tables_list(tables).into_any_element(),
        }
    }

    /// The flat tables list: name, type, size hint (columns), and whether
    /// this table owns any foreign keys.
    fn render_tables_list(&self, tables: &[TableInfo]) -> impl IntoElement {
        v_flex()
            .w_full()
            .gap_2()
            .child(
                Label::new("Tables")
                    .size(LabelSize::Default)
                    .weight(FontWeight::BOLD),
            )
            .child(
                Table::new()
                    .child(
                        TableHeader::new().child(
                            TableRow::new()
                                .child(TableHead::new().child(Label::new("Name")))
                                .child(TableHead::new().child(Label::new("Columns")))
                                .child(TableHead::new().child(Label::new("Foreign Keys"))),
                        ),
                    )
                    .child(
                        TableBody::new().children(tables.iter().map(|table| {
                            TableRow::new()
                                .child(
                                    TableCell::new().child(
                                        Label::new(table.name.clone())
                                            .size(LabelSize::Small)
                                            .weight(FontWeight::MEDIUM),
                                    ),
                                )
                                .child(
                                    TableCell::new().child(
                                        Label::new(table.columns.len().to_string())
                                            .size(LabelSize::Small)
                                            .color(Color::Muted),
                                    ),
                                )
                                .child(
                                    TableCell::new().child(
                                        Label::new(table.foreign_keys.len().to_string())
                                            .size(LabelSize::Small)
                                            .color(Color::Muted),
                                    ),
                                )
                        })),
                    ),
            )
    }

    /// A single table's detail view: name + stats header, a back-to-list
    /// action, a columns table (highlighting the currently selected column,
    /// if any), and a foreign-keys table. Ported from the original Forge
    /// panel's table detail, rebuilt on `gpui_component` widgets.
    fn render_table_detail(
        &self,
        connection: &ConnectionConfig,
        table: &TableInfo,
        selected_column: Option<String>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let id = connection.id;
        let primary_key_count = table
            .columns
            .iter()
            .filter(|column| column.primary_key)
            .count();
        let has_foreign_keys = !table.foreign_keys.is_empty();
        let selected_column = selected_column.as_deref();

        v_flex()
            .w_full()
            .gap_2()
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .gap_2()
                    .child(
                        Button::new(("back-to-list", id.0))
                            .ghost()
                            .label("← Tables")
                            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                this.clear_tree_selection(id, cx);
                            })),
                    )
                    .child(
                        Label::new(table.name.clone())
                            .size(LabelSize::Large)
                            .weight(FontWeight::BOLD),
                    )
                    .child(
                        Tag::secondary()
                            .outline()
                            .with_size(Size::Small)
                            .child(format!("{} columns", table.columns.len())),
                    )
                    .child(
                        Tag::secondary()
                            .outline()
                            .with_size(Size::Small)
                            .child(format!("{primary_key_count} PK")),
                    )
                    .child(
                        Tag::secondary()
                            .outline()
                            .with_size(Size::Small)
                            .child(format!("{} FK", table.foreign_keys.len())),
                    ),
            )
            .child(
                GroupBox::new()
                    .outline()
                    .title(Label::new("Columns"))
                    .child(
                        Table::new()
                            .child(
                                TableHeader::new().child(
                                    TableRow::new()
                                        .child(TableHead::new().child(Label::new("Name")))
                                        .child(TableHead::new().child(Label::new("Type")))
                                        .child(TableHead::new().child(Label::new("Null")))
                                        .child(TableHead::new().child(Label::new("PK"))),
                                ),
                            )
                            .child(
                                TableBody::new().children(table.columns.iter().map(|column| {
                                    let is_selected =
                                        selected_column == Some(column.name.as_str());
                                    TableRow::new()
                                        .child(
                                            TableCell::new()
                                                .child(
                                                    Label::new(column.name.clone())
                                                        .size(LabelSize::Small)
                                                        .weight(if is_selected {
                                                            FontWeight::BOLD
                                                        } else {
                                                            FontWeight::MEDIUM
                                                        })
                                                        .color(if is_selected {
                                                            Color::Accent
                                                        } else {
                                                            Color::Default
                                                        }),
                                                )
.when(is_selected, |this| {
                                    this.child(
                                        Tag::info()
                                            .with_size(Size::Small)
                                            .child("selected"),
                                    )
                                }),
                                        )
                                        .child(
                                            TableCell::new().child(
                                                Tag::new().outline().with_size(Size::Small)
                                                    .child(column.type_name.clone()),
                                            ),
                                        )
                                        .child(
                                            TableCell::new().child(
                                                Label::new(if column.nullable { "yes" } else { "no" })
                                                    .size(LabelSize::Small)
                                                    .color(Color::Muted),
                                            ),
                                        )
                                        .child(
                                            TableCell::new().child(
                                                Label::new(if column.primary_key { "yes" } else { "" })
                                                    .size(LabelSize::Small)
                                                    .color(if column.primary_key {
                                                        Color::Accent
                                                    } else {
                                                        Color::Muted
                                                    }),
                                            ),
                                        )
                                })),
                            ),
                    ),
            )
            .when(has_foreign_keys, |this| {
                this.child(
                    GroupBox::new()
                        .outline()
                        .title(Label::new("Foreign Keys"))
                        .child(
                            Table::new()
                                .child(
                                    TableHeader::new().child(
                                        TableRow::new()
                                            .child(TableHead::new().child(Label::new("Column")))
                                            .child(
                                                TableHead::new()
                                                    .child(Label::new("References")),
                                            ),
                                    ),
                                )
                                .child(
                                    TableBody::new().children(table.foreign_keys.iter().map(
                                        |foreign_key| {
                                            TableRow::new()
                                                .child(
                                                    TableCell::new().child(
                                                        Label::new(foreign_key.from_column.clone())
                                                            .size(LabelSize::Small),
                                                    ),
                                                )
                                                .child(
                                                    TableCell::new().child(
                                                        Tag::secondary().outline().with_size(Size::Small)
                                                            .child(format!(
                                                                "{}.{}",
                                                                foreign_key.to_table,
                                                                foreign_key.to_column
                                                            )),
                                                    ),
                                                )
                                        },
                                    )),
                                ),
                        ),
                )
            })
            .when(!has_foreign_keys, |this| {
                this.child(
                    Label::new("This table defines no foreign keys.")
                        .size(LabelSize::Small)
                        .color(Color::Muted),
                )
            })
    }

    /// Empty state for Views — no backend introspection yet, per the spec.
    fn render_views_empty_state(&self, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .w_full()
            .items_center()
            .justify_center()
            .gap_2()
            .py_10()
            .child(
                Icon::new(GIconName::Inbox)
                    .with_size(Size::Large)
                    .text_color(cx.theme().muted_foreground),
            )
            .child(
                Label::new("No views")
                    .size(LabelSize::Default)
                    .weight(FontWeight::MEDIUM),
            )
            .child(
                Label::new("View introspection is not available yet.")
                    .size(LabelSize::Small)
                    .color(Color::Muted),
            )
    }

    /// Relationship edges derived from `TableInfo::foreign_keys`, rendered as
    /// a table — the read-only substitute for the future schema-graph layer.
    fn render_relationships(
        &self,
        _connection: &ConnectionConfig,
        tables: &[TableInfo],
    ) -> impl IntoElement {
        let edges = tables
            .iter()
            .flat_map(|table| {
                table
                    .foreign_keys
                    .iter()
                    .map(move |foreign_key| (table, foreign_key))
            })
            .collect::<Vec<_>>();

        v_flex()
            .w_full()
            .gap_2()
            .child(
                Label::new("Relationships")
                    .size(LabelSize::Default)
                    .weight(FontWeight::BOLD),
            )
            .child(
                Label::new(format!("{} foreign-key edges", edges.len()))
                    .size(LabelSize::XSmall)
                    .color(Color::Muted),
            )
            .child(if edges.is_empty() {
                v_flex()
                    .w_full()
                    .items_center()
                    .gap_2()
                    .py_10()
                    .child(
                        Label::new("No relationships found")
                            .size(LabelSize::Small)
                            .color(Color::Muted),
                    )
                    .into_any_element()
            } else {
                Table::new()
                    .child(
                        TableHeader::new().child(
                            TableRow::new()
                                .child(TableHead::new().child(Label::new("From Table")))
                                .child(TableHead::new().child(Label::new("Column")))
                                .child(TableHead::new().child(Label::new("References"))),
                        ),
                    )
                    .child(
                        TableBody::new().children(edges.iter().map(|(table, foreign_key)| {
                            TableRow::new()
                                .child(
                                    TableCell::new().child(
                                        Label::new(table.name.clone()).size(LabelSize::Small),
                                    ),
                                )
                                .child(
                                    TableCell::new().child(
                                        Label::new(foreign_key.from_column.clone())
                                            .size(LabelSize::Small),
                                    ),
                                )
                                .child(
                                    TableCell::new().child(
                                        Tag::secondary().outline().with_size(Size::Small).child(format!(
                                            "{}.{}",
                                            foreign_key.to_table, foreign_key.to_column
                                        )),
                                    ),
                                )
                        })),
                    )
                    .into_any_element()
            })
    }
}