//! The connection header strip for `DatabasePanel`: a status/identifier row
//! (connection title, db type tag, live status, Refresh / Connect / Disconnect
//! / Delete actions), an inline error row, and the discovered-database tab
//! strip with create/register actions.

use database_backend::{ConnectionConfig, ConnectionStatus, DbType};
use gpui::{
    ClickEvent, Context, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _,
    Styled as _, prelude::FluentBuilder as _,
};
use gpui_component::{
    ActiveTheme as _, Icon, IconName as GIconName, Sizable as _, Size,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::Input,
    tab::{Tab, TabBar, TabVariant},
    tag::Tag,
    v_flex,
};
use ui::{Color, Label, LabelCommon as _, LabelSize};

use crate::DatabasePanel;

fn db_type_label(db_type: DbType) -> &'static str {
    match db_type {
        DbType::Sqlite => "SQLite",
        DbType::Postgres => "PostgreSQL",
        DbType::MySql => "MySQL / MariaDB",
        DbType::MsSql => "MSSQL",
    }
}

fn connection_status_label(status: Option<&ConnectionStatus>) -> &'static str {
    match status {
        Some(ConnectionStatus::Connected) => "Connected",
        Some(ConnectionStatus::Connecting) => "Connecting…",
        Some(ConnectionStatus::Error(_)) => "Error",
        None | Some(ConnectionStatus::Disconnected) => "Disconnected",
    }
}

impl DatabasePanel {
    /// The header of the active connection's view: title/status on the left,
    /// connection actions on the right, the last connect/schema error (if
    /// any) below, and the discovered-database tab strip once databases are
    /// known. Mirrors `npm_manager_panel`'s header-row style rather than the
    /// old hand-rolled action bar.
    pub(crate) fn render_connection_header(
        &self,
        connection: &ConnectionConfig,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let id = connection.id;
        let is_connected = self.registry.is_connected(id);
        let (indicator, indicator_color) = self.status_indicator(id);
        let status = self.statuses.get(&id);
        let (status_label, error_message) = match status {
            Some(ConnectionStatus::Error(message)) => ("Error", Some(message.clone())),
            _ => (connection_status_label(status), None),
        };

        v_flex()
            .w_full()
            .child(
                h_flex()
                    .id(("database-connection-header", id.0))
                    .w_full()
                    .justify_between()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .py_1()
                    .border_b_1()
                    .border_color(cx.theme().border)
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .child(
                                Label::new(indicator)
                                    .color(indicator_color)
                                    .weight(FontWeight::BOLD),
                            )
                            .child(
                                Label::new(connection.title.clone())
                                    .size(LabelSize::Default)
                                    .weight(FontWeight::BOLD),
                            )
                            .child(
                                Tag::secondary()
                                    .outline()
                                    .with_size(Size::Small)
                                    .child(db_type_label(connection.db_type)),
                            )
                            .child(
                                Label::new(status_label)
                                    .size(LabelSize::XSmall)
                                    .color(if status_label == "Error" {
                                        Color::Error
                                    } else {
                                        Color::Muted
                                    }),
                            ),
                    )
                    .child(
                        h_flex()
                            .gap_1()
                            .items_center()
                            .when(is_connected, |this| {
                                this.child(
                                    Button::new(("refresh-schema", id.0))
                                        .outline()
                                        .label("Refresh")
                                        .on_click(cx.listener(
                                            move |this, _: &ClickEvent, _, cx| {
                                                this.fetch_schema(id, cx);
                                            },
                                        )),
                                )
                            })
                            .child(if is_connected {
                                Button::new(("disconnect", id.0))
                                    .outline()
                                    .label("Disconnect")
                                    .on_click(cx.listener(
                                        move |this, _: &ClickEvent, _, cx| {
                                            this.disconnect(id, cx);
                                        },
                                    ))
                            } else {
                                Button::new(("connect", id.0))
                                    .primary()
                                    .label("Connect")
                                    .on_click(cx.listener(
                                        move |this, _: &ClickEvent, _, cx| {
                                            this.connect(id, cx);
                                        },
                                    ))
                            })
                            .child(
                                Button::new(("delete", id.0))
                                    .outline()
                                    .danger()
                                    .icon(Icon::new(GIconName::Delete))
                                    .on_click(cx.listener(
                                        move |this, _: &ClickEvent, _, cx| {
                                            this.delete_connection(id, cx);
                                        },
                                    )),
                            ),
                    ),
            )
            .when_some(error_message, |this, message| {
                this.child(
                    h_flex()
                        .w_full()
                        .items_center()
                        .gap_2()
                        .px_3()
                        .py_1()
                        .child(
                            Icon::new(GIconName::TriangleAlert)
                                .with_size(Size::XSmall)
                                .text_color(cx.theme().danger_foreground),
                        )
                        .child(
                            Label::new(message)
                                .size(LabelSize::Small)
                                .color(Color::Error),
                        ),
                )
            })
            .when(!connection.databases.is_empty(), |this| {
                this.child(self.render_database_tabs(connection, cx))
            })
    }

    /// The discovered-database tab strip for a network connection: one `Tab`
    /// per database `discover_databases` found (the active database, if any,
    /// highlighted), plus "+ Create database" / "Register existing database"
    /// actions — mirrors the original Forge panel's post-connect database
    /// browser and `DATABASE_PANEL_SPEC.md`'s Navigation Phase 3 layout,
    /// using `gpui_component`'s `Tab`/`TabBar` (the same pattern
    /// `npm_manager_panel`'s project tabs use) instead of a plain list.
    fn render_database_tabs(
        &self,
        connection: &ConnectionConfig,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let id = connection.id;
        let is_creating = self.creating_database_for == Some(id);
        let input_border = cx.theme().border;
        let active_ix = connection.database.as_ref().and_then(|active| {
            connection
                .databases
                .iter()
                .position(|database| &database.name == active)
        });

        let mut tab_bar = TabBar::new(("database-tabs", id.0))
            .with_variant(TabVariant::Underline)
            .children(
                connection
                    .databases
                    .iter()
                    .map(|database| Tab::new().label(database.name.clone())),
            )
            .on_click(cx.listener(move |this, ix: &usize, _, cx| {
                let name = this
                    .connections
                    .iter()
                    .find(|connection| connection.id == id)
                    .and_then(|connection| connection.databases.get(*ix))
                    .map(|database| database.name.clone());
                if let Some(name) = name {
                    this.select_database(id, name, cx);
                }
            }));
        if let Some(ix) = active_ix {
            tab_bar = tab_bar.selected_index(ix);
        }

        v_flex()
            .w_full()
            .gap_1()
            .child(tab_bar)
            .child(if is_creating {
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(
                        Input::new(&self.new_database_name)
                            .w_48()
                            .border_1()
                            .border_color(input_border),
                    )
                    .child(
                        Button::new(("create-database", id.0))
                            .outline()
                            .label("Create")
                            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                                this.submit_create_database(window, cx);
                            })),
                    )
                    .child(
                        Button::new(("register-database", id.0))
                            .outline()
                            .label("Register")
                            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                                this.submit_register_database(window, cx);
                            })),
                    )
                    .child(
                        Button::new(("cancel-database", id.0))
                            .ghost()
                            .label("Cancel")
                            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                this.cancel_create_database(cx);
                            })),
                    )
                    .into_any_element()
            } else {
                Button::new(("start-create-database", id.0))
                    .ghost()
                    .label("+ Create database")
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| {
                        this.start_create_database(id, window, cx);
                    }))
                    .into_any_element()
            })
    }
}