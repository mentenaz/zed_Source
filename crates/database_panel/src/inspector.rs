//! The inspector pane: metadata for whatever is selected in the explorer
//! tree — a table or a single column — rendered with `DescriptionList`
//! rows and `Collapsible` sections for the columns / foreign keys.

use database_backend::{ColumnInfo, ConnectionConfig, IndexInfo, TableInfo, ViewInfo};
use gpui::{
    AnyElement, Context, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _,
    StatefulInteractiveElement as _, Styled as _, div, prelude::FluentBuilder as _,
};
use gpui_component::{
    ActiveTheme as _, Icon, IconName as GIconName, Sizable as _, Size, badge::Badge,
    collapsible::Collapsible, description_list::DescriptionList, h_flex, tag::Tag, v_flex,
};
use ui::{Color, Label, LabelCommon as _, LabelSize};

use crate::{DatabasePanel, SchemaSelection};

impl DatabasePanel {
    /// The right-hand object inspector: static header, then a per-selection
    /// body (table metadata vs column metadata), or an empty state.
    pub(crate) fn render_inspector_pane(
        &self,
        connection: &ConnectionConfig,
        tables: &[TableInfo],
        views: &[ViewInfo],
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let id = connection.id;
        let selection = self.selected_schema_item(id, cx);

        v_flex()
            .id(("database-inspector", id.0))
            .size_full()
            .min_w_0()
            .child(
                h_flex()
                    .w_full()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .py_2()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(
                        Icon::new(GIconName::Inspector)
                            .with_size(Size::Small)
                            .text_color(cx.theme().muted_foreground),
                    )
                    .child(
                        Label::new("Inspector")
                            .size(LabelSize::Small)
                            .weight(FontWeight::BOLD),
                    ),
            )
            .child(
                div()
                    .id(("database-inspector-scroll", id.0))
                    .w_full()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .p_3()
                    .child(self.render_inspector_body(connection, tables, views, selection, cx)),
            )
    }

    fn render_inspector_body(
        &self,
        connection: &ConnectionConfig,
        tables: &[TableInfo],
        views: &[ViewInfo],
        selection: Option<SchemaSelection>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(selection) = selection else {
            return v_flex()
                .w_full()
                .items_center()
                .justify_center()
                .gap_2()
                .py_10()
                .child(
                    Icon::new(GIconName::Inspector)
                        .with_size(Size::Large)
                        .text_color(cx.theme().muted_foreground),
                )
                .child(
                    Label::new("No selection")
                        .size(LabelSize::Default)
                        .weight(FontWeight::MEDIUM),
                )
                .child(
                    Label::new("Select a table or column in the schema explorer.")
                        .size(LabelSize::Small)
                        .color(Color::Muted),
                )
                .into_any_element();
        };

        match selection {
            SchemaSelection::Table { name } => {
                let Some(table) = tables.iter().find(|table| table.name == name) else {
                    return self.render_inspector_empty(cx);
                };
                self.render_table_inspector(connection, table)
                    .into_any_element()
            }
            SchemaSelection::Column {
                table: table_name,
                column,
            } => {
                let Some(table) = tables.iter().find(|table| table.name == table_name) else {
                    return self.render_inspector_empty(cx);
                };
                let Some(column_info) = table.columns.iter().find(|info| info.name == column)
                else {
                    return self.render_inspector_empty(cx);
                };
                self.render_column_inspector(connection, table, column_info)
                    .into_any_element()
            }
            SchemaSelection::Index {
                table: table_name,
                index,
            } => {
                let Some(table) = tables.iter().find(|table| table.name == table_name) else {
                    return self.render_inspector_empty(cx);
                };
                let Some(index_info) = table.indexes.iter().find(|info| info.name == index) else {
                    return self.render_inspector_empty(cx);
                };
                self.render_index_inspector(connection, table, index_info)
                    .into_any_element()
            }
            SchemaSelection::View { name } => {
                let Some(view) = views.iter().find(|view| view.name == name) else {
                    return self.render_inspector_empty(cx);
                };
                self.render_view_inspector(connection, view, cx)
                    .into_any_element()
            }
            SchemaSelection::ViewColumn {
                view: view_name,
                column,
            } => {
                let Some(view) = views.iter().find(|view| view.name == view_name) else {
                    return self.render_inspector_empty(cx);
                };
                let Some(column_info) = view.columns.iter().find(|info| info.name == column) else {
                    return self.render_inspector_empty(cx);
                };
                self.render_view_column_inspector(connection, view, column_info)
                    .into_any_element()
            }
        }
    }

    fn render_inspector_empty(&self, _cx: &mut Context<Self>) -> AnyElement {
        v_flex()
            .w_full()
            .items_center()
            .gap_2()
            .py_10()
            .child(
                Label::new("Not found")
                    .size(LabelSize::Small)
                    .color(Color::Muted),
            )
            .into_any_element()
    }

    /// Table-level metadata: name, tags, connection details, then columns and
    /// foreign keys in `Collapsible` sections.
    fn render_table_inspector(
        &self,
        connection: &ConnectionConfig,
        table: &TableInfo,
    ) -> impl IntoElement {
        let pk_names = table
            .columns
            .iter()
            .filter(|column| column.primary_key)
            .map(|column| column.name.clone())
            .collect::<Vec<_>>()
            .join(", ");
        let pk_value = if pk_names.is_empty() {
            "—".to_string()
        } else {
            pk_names
        };

        v_flex()
            .w_full()
            .gap_3()
            .child(
                Label::new(table.name.clone())
                    .size(LabelSize::Large)
                    .weight(FontWeight::BOLD),
            )
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(
                        Tag::secondary()
                            .outline()
                            .with_size(Size::Small)
                            .child("Table"),
                    )
                    .child(
                        Tag::info()
                            .outline()
                            .with_size(Size::Small)
                            .child(format!("{} columns", table.columns.len())),
                    )
                    .child(Badge::new().count(table.foreign_keys.len())),
            )
            .child(
                DescriptionList::new()
                    .columns(2)
                    .bordered(true)
                    .item("Connection", connection.title.clone(), 1)
                    .item("Database", connection.database.as_deref().unwrap_or("—"), 1)
                    .item("Columns", table.columns.len().to_string(), 1)
                    .item("Primary Key", pk_value, 1)
                    .item("Indexes", table.indexes.len().to_string(), 1)
                    .item("Foreign Keys", table.foreign_keys.len().to_string(), 1),
            )
            .child(
                Collapsible::new()
                    .open(true)
                    .child(
                        Label::new("Columns")
                            .size(LabelSize::Small)
                            .color(Color::Muted),
                    )
                    .content(
                        v_flex().gap_1().children(
                            table
                                .columns
                                .iter()
                                .map(|column| self.render_column_row(column)),
                        ),
                    ),
            )
            .when(!table.indexes.is_empty(), |this| {
                this.child(
                    Collapsible::new()
                        .open(true)
                        .child(
                            Label::new("Indexes")
                                .size(LabelSize::Small)
                                .color(Color::Muted),
                        )
                        .content(v_flex().gap_1().children(table.indexes.iter().map(|index| {
                            h_flex()
                                .w_full()
                                .items_center()
                                .gap_2()
                                .child(Label::new(index.name.clone()).size(LabelSize::Small))
                                .child(
                                    Tag::new()
                                        .outline()
                                        .with_size(Size::Small)
                                        .child(index.columns.join(", ")),
                                )
                                .when(index.unique, |this| {
                                    this.child(Tag::info().with_size(Size::Small).child("unique"))
                                })
                        }))),
                )
            })
            .when(!table.foreign_keys.is_empty(), |this| {
                this.child(
                    Collapsible::new()
                        .open(true)
                        .child(
                            Label::new("Foreign Keys")
                                .size(LabelSize::Small)
                                .color(Color::Muted),
                        )
                        .content(v_flex().gap_1().children(table.foreign_keys.iter().map(
                            |foreign_key| {
                                h_flex()
                                    .w_full()
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        Label::new(foreign_key.from_column.clone())
                                            .size(LabelSize::Small),
                                    )
                                    .child(
                                        Label::new("→").size(LabelSize::Small).color(Color::Muted),
                                    )
                                    .child(Tag::secondary().outline().with_size(Size::Small).child(
                                        format!(
                                            "{}.{}",
                                            foreign_key.to_table, foreign_key.to_column
                                        ),
                                    ))
                            },
                        ))),
                )
            })
    }

    /// A compact single-column row used inside the inspector's Columns
    /// collapsible.
    fn render_column_row(&self, column: &ColumnInfo) -> impl IntoElement {
        h_flex()
            .w_full()
            .items_center()
            .gap_2()
            .child(
                Label::new(if column.primary_key {
                    format!("★ {}", column.name)
                } else {
                    column.name.clone()
                })
                .size(LabelSize::Small),
            )
            .child(
                Tag::new()
                    .outline()
                    .with_size(Size::Small)
                    .child(column.type_name.clone()),
            )
            .child(
                Label::new(if column.nullable {
                    "nullable"
                } else {
                    "not null"
                })
                .size(LabelSize::XSmall)
                .color(Color::Muted),
            )
    }

    /// Column-level metadata for a selected column.
    fn render_column_inspector(
        &self,
        connection: &ConnectionConfig,
        table: &TableInfo,
        column: &ColumnInfo,
    ) -> impl IntoElement {
        let referenced_table = table
            .foreign_keys
            .iter()
            .find(|fk| fk.from_column == column.name);

        v_flex()
            .w_full()
            .gap_3()
            .child(
                Label::new(format!("{}.{}", table.name, column.name))
                    .size(LabelSize::Large)
                    .weight(FontWeight::BOLD),
            )
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(
                        Tag::secondary()
                            .outline()
                            .with_size(Size::Small)
                            .child("Column"),
                    )
                    .when(column.primary_key, |this| {
                        this.child(
                            Tag::info()
                                .outline()
                                .with_size(Size::Small)
                                .child("Primary Key"),
                        )
                    })
                    .when_some(referenced_table, |this, _fk| {
                        this.child(
                            Tag::warning()
                                .outline()
                                .with_size(Size::Small)
                                .child("Foreign Key"),
                        )
                    }),
            )
            .child(
                DescriptionList::new()
                    .columns(2)
                    .bordered(true)
                    .item("Connection", connection.title.clone(), 1)
                    .item("Database", connection.database.as_deref().unwrap_or("—"), 1)
                    .item("Table", table.name.clone(), 1)
                    .item("Column", column.name.clone(), 1)
                    .item("Type", column.type_name.clone(), 1)
                    .item("Nullable", if column.nullable { "yes" } else { "no" }, 1)
                    .item(
                        "Primary Key",
                        if column.primary_key { "yes" } else { "no" },
                        1,
                    )
                    .item(
                        "References",
                        referenced_table
                            .map(|fk| format!("{}.{}", fk.to_table, fk.to_column))
                            .unwrap_or_else(|| "—".to_string()),
                        1,
                    ),
            )
    }

    /// Index-level metadata for a selected index.
    fn render_index_inspector(
        &self,
        connection: &ConnectionConfig,
        table: &TableInfo,
        index: &IndexInfo,
    ) -> impl IntoElement {
        v_flex()
            .w_full()
            .gap_3()
            .child(
                Label::new(format!("{}.{}", table.name, index.name))
                    .size(LabelSize::Large)
                    .weight(FontWeight::BOLD),
            )
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(
                        Tag::secondary()
                            .outline()
                            .with_size(Size::Small)
                            .child("Index"),
                    )
                    .when(index.unique, |this| {
                        this.child(Tag::info().outline().with_size(Size::Small).child("Unique"))
                    }),
            )
            .child(
                DescriptionList::new()
                    .columns(2)
                    .bordered(true)
                    .item("Connection", connection.title.clone(), 1)
                    .item("Database", connection.database.as_deref().unwrap_or("—"), 1)
                    .item("Table", table.name.clone(), 1)
                    .item("Index", index.name.clone(), 1)
                    .item("Columns", index.columns.join(", "), 1)
                    .item("Unique", if index.unique { "yes" } else { "no" }, 1),
            )
    }

    /// View-level metadata: name, tags, connection details, columns, and the
    /// defining SQL.
    fn render_view_inspector(
        &self,
        connection: &ConnectionConfig,
        view: &ViewInfo,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        v_flex()
            .w_full()
            .gap_3()
            .child(
                Label::new(view.name.clone())
                    .size(LabelSize::Large)
                    .weight(FontWeight::BOLD),
            )
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(
                        Tag::secondary()
                            .outline()
                            .with_size(Size::Small)
                            .child("View"),
                    )
                    .child(Badge::new().count(view.columns.len())),
            )
            .child(
                DescriptionList::new()
                    .columns(2)
                    .bordered(true)
                    .item("Connection", connection.title.clone(), 1)
                    .item("Database", connection.database.as_deref().unwrap_or("—"), 1)
                    .item("Columns", view.columns.len().to_string(), 1),
            )
            .child(
                Collapsible::new()
                    .open(true)
                    .child(
                        Label::new("Columns")
                            .size(LabelSize::Small)
                            .color(Color::Muted),
                    )
                    .content(
                        v_flex().gap_1().children(
                            view.columns
                                .iter()
                                .map(|column| self.render_column_row(column)),
                        ),
                    ),
            )
            .child(
                Collapsible::new()
                    .open(true)
                    .child(
                        Label::new("Definition")
                            .size(LabelSize::Small)
                            .color(Color::Muted),
                    )
                    .content(
                        div()
                            .w_full()
                            .p_2()
                            .rounded(gpui::px(4.))
                            .bg(cx.theme().muted)
                            .child(
                                Label::new(view.definition.clone())
                                    .size(LabelSize::Small)
                                    .color(Color::Default),
                            ),
                    ),
            )
    }

    /// Column-level metadata for a selected view column.
    fn render_view_column_inspector(
        &self,
        connection: &ConnectionConfig,
        view: &ViewInfo,
        column: &ColumnInfo,
    ) -> impl IntoElement {
        v_flex()
            .w_full()
            .gap_3()
            .child(
                Label::new(format!("{}.{}", view.name, column.name))
                    .size(LabelSize::Large)
                    .weight(FontWeight::BOLD),
            )
            .child(
                Tag::secondary()
                    .outline()
                    .with_size(Size::Small)
                    .child("View Column"),
            )
            .child(
                DescriptionList::new()
                    .columns(2)
                    .bordered(true)
                    .item("Connection", connection.title.clone(), 1)
                    .item("Database", connection.database.as_deref().unwrap_or("—"), 1)
                    .item("View", view.name.clone(), 1)
                    .item("Column", column.name.clone(), 1)
                    .item("Type", column.type_name.clone(), 1),
            )
    }
}
