//! Database panel UI.
//!
//! See `crates/gpui_component/DATABASE_PANEL_SPEC.md` for the full
//! architecture and phased delivery plan. This is Delivery Phase 1: real
//! SQLite connect/disconnect, non-secret connection metadata persisted
//! across restarts, and a `Settings`-based connection form. There is no
//! nested connection→database hierarchy yet (a SQLite file is always
//! exactly one database — that nesting becomes real once Phase 3's network
//! drivers land) and no schema tree yet (Phase 2).

use std::collections::HashMap;

use anyhow::Result;
use database_backend::{ConnectionConfig, ConnectionId, ConnectionRegistry, ConnectionStatus};
use db::kvp::KeyValueStore;
use gpui::{
    App, AppContext as _, AsyncWindowContext, ClickEvent, Context, Entity, EventEmitter,
    FocusHandle, Focusable, InteractiveElement as _, IntoElement, ParentElement as _, Pixels,
    Render, SharedString, Styled as _, Task, WeakEntity, Window, actions, prelude::FluentBuilder as _,
    px,
};
use gpui_component::{
    ActiveTheme as _, Icon, IconName as GIconName,
    button::{Button, ButtonVariants as _},
    h_flex,
    setting::{SettingField, SettingGroup, SettingItem, SettingPage, Settings},
    v_flex,
};
use ui::{Color, Label, LabelCommon as _, LabelSize};
use workspace::{
    SERIALIZATION_THROTTLE_TIME, Workspace,
    dock::{DockPosition, Panel, PanelEvent},
};

const DATABASE_CONNECTIONS_KVP_KEY: &str = "database-panel-connections";

actions!(
    database_panel,
    [
        /// Toggles focus on the Database panel.
        ToggleFocus
    ]
);

/// Registers the Database panel's actions on every workspace. Call once at
/// app startup, alongside the other panels' `init` functions.
pub fn init(cx: &mut App) {
    cx.observe_new(|workspace: &mut Workspace, _, _| {
        workspace.register_action(|workspace, _: &ToggleFocus, window, cx| {
            workspace.toggle_panel_focus::<DatabasePanel>(window, cx);
        });
    })
    .detach();
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
struct SerializedDatabasePanel {
    connections: Vec<ConnectionConfig>,
}

pub struct DatabasePanel {
    focus_handle: FocusHandle,
    connections: Vec<ConnectionConfig>,
    registry: ConnectionRegistry,
    statuses: HashMap<ConnectionId, ConnectionStatus>,
    new_connection_title: String,
    new_connection_path: String,
    next_connection_id: u64,
    _persist_task: Task<()>,
}

impl DatabasePanel {
    /// Loads the panel for a workspace, following the same
    /// `WeakEntity<Workspace>` + `AsyncWindowContext` convention as the other
    /// dock panels' `load` functions (see `initialize_panels` in
    /// `zed::zed`), so it can be added to the dock alongside them.
    pub async fn load(
        workspace: WeakEntity<Workspace>,
        mut cx: AsyncWindowContext,
    ) -> Result<Entity<Self>> {
        workspace.update_in(&mut cx, |workspace, window, cx| {
            DatabasePanel::new(workspace, window, cx)
        })
    }

    pub fn new(
        _workspace: &mut Workspace,
        _window: &mut Window,
        cx: &mut Context<Workspace>,
    ) -> Entity<Self> {
        cx.new(|cx| {
            let connections = Self::load_persisted_connections(cx);
            let next_connection_id = connections
                .iter()
                .map(|connection| connection.id.0)
                .max()
                .map_or(1, |max| max + 1);

            Self {
                focus_handle: cx.focus_handle(),
                connections,
                registry: ConnectionRegistry::new(),
                statuses: HashMap::default(),
                new_connection_title: String::new(),
                new_connection_path: String::new(),
                next_connection_id,
                _persist_task: Task::ready(()),
            }
        })
    }

    fn load_persisted_connections(cx: &App) -> Vec<ConnectionConfig> {
        KeyValueStore::global(cx)
            .read_kvp(DATABASE_CONNECTIONS_KVP_KEY)
            .ok()
            .flatten()
            .and_then(|json| serde_json::from_str::<SerializedDatabasePanel>(&json).ok())
            .map(|serialized| serialized.connections)
            .unwrap_or_default()
    }

    /// Persists `self.connections` after a short debounce, mirroring
    /// `git_panel::GitPanel::serialize`'s throttled-write pattern so rapid
    /// successive edits (e.g. typing in the connection list) don't hammer
    /// the key-value store.
    fn persist_connections(&mut self, cx: &mut Context<Self>) {
        let connections = self.connections.clone();
        let kvp = KeyValueStore::global(cx);

        self._persist_task = cx.spawn(async move |_this, cx| {
            cx.background_executor()
                .timer(SERIALIZATION_THROTTLE_TIME)
                .await;
            cx.background_spawn(async move {
                let json = serde_json::to_string(&SerializedDatabasePanel { connections })?;
                kvp.write_kvp(DATABASE_CONNECTIONS_KVP_KEY.to_string(), json)
                    .await?;
                anyhow::Ok(())
            })
            .await
            .ok();
        });
    }

    fn add_connection(&mut self, cx: &mut Context<Self>) {
        let title = self.new_connection_title.trim();
        let path = self.new_connection_path.trim();
        if title.is_empty() || path.is_empty() {
            return;
        }

        let id = ConnectionId(self.next_connection_id);
        self.next_connection_id += 1;
        self.connections.push(ConnectionConfig {
            id,
            title: title.to_string(),
            db_type: database_backend::DbType::Sqlite,
            host: None,
            port: None,
            username: None,
            sqlite_path: Some(path.to_string()),
            ssl: false,
            databases: Vec::new(),
        });
        self.new_connection_title.clear();
        self.new_connection_path.clear();
        self.persist_connections(cx);
        cx.notify();
    }

    fn connect(&mut self, id: ConnectionId, cx: &mut Context<Self>) {
        let Some(connection) = self.connections.iter().find(|c| c.id == id) else {
            return;
        };
        let Some(path) = connection.sqlite_path.clone() else {
            return;
        };

        match self.registry.connect_sqlite(id, &path) {
            Ok(()) => {
                self.statuses.insert(id, ConnectionStatus::Connected);
            }
            Err(err) => {
                self.statuses
                    .insert(id, ConnectionStatus::Error(err.to_string()));
            }
        }
        cx.notify();
    }

    fn disconnect(&mut self, id: ConnectionId, cx: &mut Context<Self>) {
        self.registry.disconnect(id).ok();
        self.statuses.insert(id, ConnectionStatus::Disconnected);
        cx.notify();
    }

    fn delete_connection(&mut self, id: ConnectionId, cx: &mut Context<Self>) {
        self.registry.disconnect(id).ok();
        self.statuses.remove(&id);
        self.connections.retain(|connection| connection.id != id);
        self.persist_connections(cx);
        cx.notify();
    }

    fn status_indicator(&self, id: ConnectionId) -> (SharedString, Color) {
        match self.statuses.get(&id) {
            None | Some(ConnectionStatus::Disconnected) => ("○".into(), Color::Muted),
            Some(ConnectionStatus::Connecting) => ("◐".into(), Color::Warning),
            Some(ConnectionStatus::Connected) => ("●".into(), Color::Success),
            Some(ConnectionStatus::Error(_)) => ("!".into(), Color::Error),
        }
    }

    fn render_add_connection_form(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let entity = cx.entity();

        let title_entity = entity.clone();
        let title_field = SettingField::input(
            {
                let entity = entity.clone();
                move |cx: &App| entity.read(cx).new_connection_title.clone().into()
            },
            move |value: SharedString, cx: &mut App| {
                title_entity.update(cx, |this, cx| {
                    this.new_connection_title = value.to_string();
                    cx.notify();
                });
            },
        );

        let path_entity = entity.clone();
        let path_field = SettingField::input(
            {
                let entity = entity.clone();
                move |cx: &App| entity.read(cx).new_connection_path.clone().into()
            },
            move |value: SharedString, cx: &mut App| {
                path_entity.update(cx, |this, cx| {
                    this.new_connection_path = value.to_string();
                    cx.notify();
                });
            },
        );

        let browse_entity = entity.clone();
        let connect_entity = entity.clone();

        Settings::new("database-panel-add-connection")
            .sidebar_width(px(0.))
            .page(SettingPage::new("Add SQLite Connection").group(
                SettingGroup::new()
                    .item(SettingItem::new("Title", title_field))
                    .item(SettingItem::new("File Path", path_field))
                    .item(SettingItem::render(move |_options, _window, _cx| {
                        let browse_entity = browse_entity.clone();
                        let connect_entity = connect_entity.clone();
                        h_flex()
                            .gap_2()
                            .child(Button::new("browse-sqlite-path").label("Browse…").on_click(
                                move |_, window, cx| {
                                    let prompt = cx.prompt_for_paths(gpui::PathPromptOptions {
                                        files: true,
                                        directories: false,
                                        multiple: false,
                                        prompt: Some("Select SQLite Database".into()),
                                    });
                                    let browse_entity = browse_entity.clone();
                                    window
                                        .spawn(cx, async move |cx| {
                                            let Some(mut paths) = prompt
                                                .await
                                                .ok()
                                                .and_then(Result::ok)
                                                .flatten()
                                            else {
                                                return;
                                            };
                                            let Some(path) = paths.pop() else {
                                                return;
                                            };
                                            browse_entity.update(cx, |this, cx| {
                                                this.new_connection_path =
                                                    path.to_string_lossy().into_owned();
                                                cx.notify();
                                            });
                                        })
                                        .detach();
                                },
                            ))
                            .child(
                                Button::new("add-sqlite-connection")
                                    .label("Connect")
                                    .primary()
                                    .on_click(move |_, _, cx| {
                                        connect_entity.update(cx, |this, cx| {
                                            this.add_connection(cx);
                                        });
                                    }),
                            )
                            .into_any_element()
                    })),
            ))
    }

    fn render_connection_row(&self, connection: &ConnectionConfig, cx: &mut Context<Self>) -> impl IntoElement {
        let id = connection.id;
        let (indicator, color) = self.status_indicator(id);
        let is_connected = self.registry.is_connected(id);

        h_flex()
            .id(("database-connection-row", id.0))
            .w_full()
            .justify_between()
            .px_2()
            .py_1()
            .child(
                h_flex()
                    .gap_2()
                    .child(Label::new(indicator).color(color))
                    .child(Label::new(connection.title.clone())),
            )
            .child(
                h_flex()
                    .gap_1()
                    .child(if is_connected {
                        Button::new(("disconnect", id.0))
                            .label("Disconnect")
                            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                this.disconnect(id, cx);
                            }))
                    } else {
                        Button::new(("connect", id.0))
                            .label("Connect")
                            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                this.connect(id, cx);
                            }))
                    })
                    .child(
                        Button::new(("delete", id.0))
                            .icon(Icon::new(GIconName::Delete))
                            .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                this.delete_connection(id, cx);
                            })),
                    ),
            )
    }
}

impl Focusable for DatabasePanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<PanelEvent> for DatabasePanel {}

impl Panel for DatabasePanel {
    fn persistent_name() -> &'static str {
        "Database Panel"
    }

    fn panel_key() -> &'static str {
        "DatabasePanel"
    }

    fn position(&self, _window: &Window, _cx: &App) -> DockPosition {
        DockPosition::Left
    }

    fn position_is_valid(&self, position: DockPosition) -> bool {
        matches!(position, DockPosition::Left)
    }

    fn set_position(
        &mut self,
        _position: DockPosition,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        // Fixed to the left dock — see `position_is_valid`.
    }

    fn default_size(&self, _window: &Window, _cx: &App) -> Pixels {
        px(300.)
    }

    fn icon(&self, _window: &Window, _cx: &App) -> Option<ui::IconName> {
        Some(ui::IconName::DatabaseZap)
    }

    fn icon_tooltip(&self, _window: &Window, _cx: &App) -> Option<&'static str> {
        Some("Databases")
    }

    fn toggle_action(&self) -> Box<dyn gpui::Action> {
        Box::new(ToggleFocus)
    }

    fn activation_priority(&self) -> u32 {
        5
    }
}

impl Render for DatabasePanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let connections = self.connections.clone();
        let connection_rows = connections
            .iter()
            .map(|connection| self.render_connection_row(connection, cx).into_any_element())
            .collect::<Vec<_>>();

        v_flex()
            .id("database-panel")
            .track_focus(&self.focus_handle(cx))
            .size_full()
            .bg(cx.theme().sidebar)
            .child(
                v_flex()
                    .p_2()
                    .gap_1()
                    .child(
                        Label::new("Connections")
                            .size(LabelSize::Small)
                            .color(Color::Muted),
                    )
                    .children(connection_rows)
                    .when(connections.is_empty(), |this| {
                        this.child(
                            Label::new("No connections yet")
                                .size(LabelSize::Small)
                                .color(Color::Muted),
                        )
                    }),
            )
            .child(self.render_add_connection_form(cx))
    }
}
