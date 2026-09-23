//! Database panel UI.
//!
//! See `crates/gpui_component/DATABASE_PANEL_SPEC.md` for the full
//! architecture and phased delivery plan. This is Delivery Phase 2: on top
//! of Phase 1's SQLite connect/disconnect, connections now auto-discover
//! their tables/columns/foreign keys on connect, rendered as a real schema
//! tree, with the last-known schema cached across restarts. There is still
//! no nested connection→database hierarchy (a SQLite file is always exactly
//! one database — that nesting becomes real once Phase 3's network drivers
//! land) and no foreign-key graph rendering yet (Phase 5) — foreign keys are
//! fetched and stored now so that phase won't need another backend
//! round-trip.

use std::collections::HashMap;
use std::rc::Rc;

use anyhow::Result;
use database_backend::{
    ConnectionConfig, ConnectionId, ConnectionRegistry, ConnectionStatus, TableInfo,
};
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
    list::ListItem,
    setting::{SettingField, SettingGroup, SettingItem, SettingPage, Settings},
    tree::{TreeItem, TreeState, tree},
    v_flex,
};
use ui::{Color, Label, LabelCommon as _, LabelSize};
use workspace::{
    SERIALIZATION_THROTTLE_TIME, Workspace,
    dock::{DockPosition, Panel, PanelEvent},
};

const DATABASE_CONNECTIONS_KVP_KEY: &str = "database-panel-connections";
const DATABASE_SCHEMA_CACHE_KVP_KEY: &str = "database-panel-schema-cache";

/// The state of a connection's schema discovery.
#[derive(Clone)]
enum SchemaState {
    Loading,
    Loaded(Rc<[TableInfo]>),
    Error(String),
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
struct SerializedSchemaCache {
    schemas: HashMap<ConnectionId, Vec<TableInfo>>,
}

/// Builds the tree items for one table: the table itself, with a child item
/// per column. Column labels bake in type/PK/nullability, since `TreeItem`
/// carries only an id and a display label — there's no separate typed slot
/// for a renderer to inspect, so `render_item` differentiates tables from
/// columns purely by `TreeEntry::depth()`.
fn table_tree_item(table: &TableInfo) -> TreeItem {
    let children = table.columns.iter().map(|column| {
        let pk_suffix = if column.primary_key { " · PK" } else { "" };
        let null_suffix = if column.nullable { "" } else { " · NOT NULL" };
        TreeItem::new(
            format!("{}::{}", table.name, column.name),
            format!(
                "{}  {}{}{}",
                column.name, column.type_name, pk_suffix, null_suffix
            ),
        )
    });
    TreeItem::new(table.name.clone(), table.name.clone()).children(children)
}

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
    schemas: HashMap<ConnectionId, SchemaState>,
    tree_states: HashMap<ConnectionId, Entity<TreeState>>,
    new_connection_title: String,
    new_connection_path: String,
    next_connection_id: u64,
    _persist_task: Task<()>,
    _schema_persist_task: Task<()>,
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

            let cached_schemas = Self::load_persisted_schema_cache(cx);
            let mut schemas = HashMap::default();
            let mut tree_states = HashMap::default();
            for (id, tables) in cached_schemas {
                let items: Vec<TreeItem> = tables.iter().map(table_tree_item).collect();
                let tree_state = cx.new(|cx| TreeState::new(cx).items(items));
                tree_states.insert(id, tree_state);
                schemas.insert(id, SchemaState::Loaded(tables.into()));
            }

            Self {
                focus_handle: cx.focus_handle(),
                connections,
                registry: ConnectionRegistry::new(),
                statuses: HashMap::default(),
                schemas,
                tree_states,
                new_connection_title: String::new(),
                new_connection_path: String::new(),
                next_connection_id,
                _persist_task: Task::ready(()),
                _schema_persist_task: Task::ready(()),
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

    fn load_persisted_schema_cache(cx: &App) -> HashMap<ConnectionId, Vec<TableInfo>> {
        KeyValueStore::global(cx)
            .read_kvp(DATABASE_SCHEMA_CACHE_KVP_KEY)
            .ok()
            .flatten()
            .and_then(|json| serde_json::from_str::<SerializedSchemaCache>(&json).ok())
            .map(|cache| cache.schemas)
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

    /// Persists the `Loaded` schemas (ignoring in-flight `Loading`/`Error`
    /// states, which are useless as a restart-time cache) after a short
    /// debounce, mirroring `persist_connections`.
    fn persist_schema_cache(&mut self, cx: &mut Context<Self>) {
        let schemas: HashMap<ConnectionId, Vec<TableInfo>> = self
            .schemas
            .iter()
            .filter_map(|(id, state)| match state {
                SchemaState::Loaded(tables) => Some((*id, tables.to_vec())),
                SchemaState::Loading | SchemaState::Error(_) => None,
            })
            .collect();
        let kvp = KeyValueStore::global(cx);

        self._schema_persist_task = cx.spawn(async move |_this, cx| {
            cx.background_executor()
                .timer(SERIALIZATION_THROTTLE_TIME)
                .await;
            cx.background_spawn(async move {
                let json = serde_json::to_string(&SerializedSchemaCache { schemas })?;
                kvp.write_kvp(DATABASE_SCHEMA_CACHE_KVP_KEY.to_string(), json)
                    .await?;
                anyhow::Ok(())
            })
            .await
            .ok();
        });
    }

    /// Fetches (or re-fetches, for Refresh) `id`'s schema off the main
    /// thread, since it may run several PRAGMA queries per table.
    fn fetch_schema(&mut self, id: ConnectionId, cx: &mut Context<Self>) {
        let Some(handle) = self.registry.sqlite_handle(id) else {
            return;
        };

        self.schemas.insert(id, SchemaState::Loading);
        cx.notify();

        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move {
                    let conn = handle.lock().unwrap();
                    database_backend::drivers::sqlite::fetch_schema(&conn)
                })
                .await;

            this.update(cx, |this, cx| {
                match result {
                    Ok(tables) => {
                        this.update_tree_items(id, &tables, cx);
                        this.schemas.insert(id, SchemaState::Loaded(tables.into()));
                        this.persist_schema_cache(cx);
                    }
                    Err(err) => {
                        this.schemas.insert(id, SchemaState::Error(err.to_string()));
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Creates or updates the connection's `TreeState` with `tables`'
    /// current shape.
    fn update_tree_items(&mut self, id: ConnectionId, tables: &[TableInfo], cx: &mut Context<Self>) {
        let items: Vec<TreeItem> = tables.iter().map(table_tree_item).collect();
        match self.tree_states.get(&id) {
            Some(tree_state) => {
                tree_state.update(cx, |tree_state, cx| tree_state.set_items(items, cx));
            }
            None => {
                let tree_state = cx.new(|cx| TreeState::new(cx).items(items));
                self.tree_states.insert(id, tree_state);
            }
        }
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
                self.fetch_schema(id, cx);
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

        v_flex()
            .w_full()
            .child(
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
                            .when(is_connected, |this| {
                                this.child(
                                    Button::new(("refresh-schema", id.0))
                                        .label("Refresh")
                                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                            this.fetch_schema(id, cx);
                                        })),
                                )
                            })
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
                    ),
            )
            .children(self.render_schema_section(id, is_connected, cx))
    }

    fn render_schema_section(
        &self,
        id: ConnectionId,
        is_connected: bool,
        cx: &mut Context<Self>,
    ) -> Option<gpui::AnyElement> {
        if !is_connected {
            return None;
        }

        let element = match self.schemas.get(&id) {
            None | Some(SchemaState::Loading) => h_flex()
                .px_4()
                .py_1()
                .child(
                    Label::new("Loading schema…")
                        .size(LabelSize::Small)
                        .color(Color::Muted),
                )
                .into_any_element(),
            Some(SchemaState::Error(message)) => v_flex()
                .px_4()
                .py_1()
                .gap_1()
                .child(
                    Label::new(format!("Failed to load schema: {message}"))
                        .size(LabelSize::Small)
                        .color(Color::Error),
                )
                .child(
                    Button::new(("retry-schema", id.0))
                        .label("Retry")
                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                            this.fetch_schema(id, cx);
                        })),
                )
                .into_any_element(),
            Some(SchemaState::Loaded(tables)) if tables.is_empty() => h_flex()
                .px_4()
                .py_1()
                .child(
                    Label::new("No tables")
                        .size(LabelSize::Small)
                        .color(Color::Muted),
                )
                .into_any_element(),
            Some(SchemaState::Loaded(_)) => {
                let Some(tree_state) = self.tree_states.get(&id) else {
                    return None;
                };
                h_flex()
                    .w_full()
                    .px_2()
                    .child(tree(tree_state, |ix, entry, _selected, _window, _cx| {
                        let is_table = entry.depth() == 0;
                        let color = if is_table { Color::Default } else { Color::Muted };
                        ListItem::new(ix)
                            .pl(px(16.) * entry.depth() + px(8.))
                            .child(
                                Label::new(entry.item().label.clone())
                                    .size(LabelSize::Small)
                                    .color(color),
                            )
                    }))
                    .into_any_element()
            }
        };

        Some(element)
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
