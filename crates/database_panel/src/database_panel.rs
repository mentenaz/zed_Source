//! Database panel UI.
//!
//! See `crates/gpui_component/DATABASE_PANEL_SPEC.md` for the full
//! architecture and phased delivery plan. This is the Phase 1 explorer shell:
//! on top of the existing multi-driver connect/disconnect + database
//! discovery/create/register flow, the connection body is now a proper
//! `gpui_component` three-pane experience — a searchable schema explorer
//! (left), Overview/Tables/Views/Relationships content tabs (center) and an
//! object inspector (right) — all resizable via `h_resizable`/`resizable_panel`.
//! The SQL workbench is deferred (backend support stays intact in
//! `database_backend`); Views shows an empty state until backend introspection
//! lands. Passwords are never persisted on `ConnectionConfig` — they're
//! read/written exclusively through `zed_credentials_provider`, keyed by
//! `credential_url(db_type, host, port)`. Reconnect is manual only.

use std::collections::HashMap;
use std::rc::Rc;

use anyhow::Result;
use database_backend::{
    ConnectionConfig, ConnectionId, ConnectionRegistry, ConnectionStatus, DatabaseId, DbType,
    NetworkConnectParams, SavedDatabase, TableInfo, credential_url,
};
use db::kvp::KeyValueStore;
use gpui::{
    App, AppContext as _, AsyncWindowContext, ClickEvent, Context, Entity, EventEmitter,
    FocusHandle, Focusable, InteractiveElement as _, IntoElement, ParentElement as _, Pixels,
    Render, SharedString, Styled as _, Subscription, Task, WeakEntity, Window, actions, px,
};
use gpui_component::{
    ActiveTheme as _, Icon, IconName as GIconName, Sizable as _, Size, h_flex,
    button::{Button, ButtonVariants as _},
    input::{Input, InputEvent, InputState},
    resizable::{h_resizable, resizable_panel},
    setting::{SettingField, SettingGroup, SettingItem, SettingPage, Settings},
    spinner::Spinner,
    tab::{Tab, TabBar, TabVariant},
    tree::{TreeItem, TreeState},
    v_flex,
};
use ui::{Color, Label, LabelCommon as _, LabelSize};
use workspace::{
    SERIALIZATION_THROTTLE_TIME, Workspace,
    dock::{DockPosition, Panel, PanelEvent},
};

mod content;
mod explorer;
mod header;
mod inspector;

const DATABASE_CONNECTIONS_KVP_KEY: &str = "database-panel-connections";
const DATABASE_SCHEMA_CACHE_KVP_KEY: &str = "database-panel-schema-cache";

/// The state of a connection's schema discovery.
#[derive(Clone)]
enum SchemaState {
    Loading,
    Loaded(Rc<[TableInfo]>),
    Error(String),
}

/// The panel's center-pane content tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum ContentTab {
    #[default]
    Overview,
    Tables,
    Views,
    Relationships,
}

impl ContentTab {
    pub(crate) const ALL: [ContentTab; 4] = [
        ContentTab::Overview,
        ContentTab::Tables,
        ContentTab::Views,
        ContentTab::Relationships,
    ];

    pub(crate) fn index(self) -> usize {
        match self {
            ContentTab::Overview => 0,
            ContentTab::Tables => 1,
            ContentTab::Views => 2,
            ContentTab::Relationships => 3,
        }
    }

    pub(crate) fn from_index(ix: usize) -> Self {
        match ix {
            1 => ContentTab::Tables,
            2 => ContentTab::Views,
            3 => ContentTab::Relationships,
            _ => ContentTab::Overview,
        }
    }

    pub(crate) fn title(self) -> &'static str {
        match self {
            ContentTab::Overview => "Overview",
            ContentTab::Tables => "Tables",
            ContentTab::Views => "Views",
            ContentTab::Relationships => "Relationships",
        }
    }
}

/// What's currently selected in the schema explorer tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SchemaSelection {
    Table { name: String },
    Column { table: String, column: String },
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
struct SerializedSchemaCache {
    schemas: HashMap<ConnectionId, Vec<TableInfo>>,
}

/// Builds the tree items for one table: the table itself, with a child item
/// per column. Column labels bake in type/PK/nullability, since `TreeItem`
/// carries only an id and a display label — there's no separate typed slot
/// for a renderer to inspect, so `render_item` differentiates tables from
/// columns purely by `TreeEntry::depth()`. When `needle` is non-empty, only
/// matching columns are included as children.
fn table_tree_item(table: &TableInfo, needle: &str) -> TreeItem {
    let children = table
        .columns
        .iter()
        .filter(|column| needle.is_empty() || column.name.to_lowercase().contains(needle))
        .map(|column| {
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
    TreeItem::new(
        table.name.clone(),
        format!("{}  ({})", table.name, table.columns.len()),
    )
    .expanded(true)
    .children(children)
}

/// Whether `table` matches the explorer's filter `needle`: its own name, or
/// any of its columns' names (case-insensitive). An empty needle matches
/// everything.
fn table_matches(table: &TableInfo, needle: &str) -> bool {
    needle.is_empty()
        || table.name.to_lowercase().contains(needle)
        || table
            .columns
            .iter()
            .any(|column| column.name.to_lowercase().contains(needle))
}

/// The default port to pre-fill when a network type is first selected in the
/// "Add Connection" form. Meaningless for SQLite (never read for it).
fn default_port(db_type: DbType) -> u16 {
    match db_type {
        DbType::Sqlite => 0,
        DbType::Postgres => 5432,
        DbType::MySql => 3306,
        DbType::MsSql => 1433,
    }
}

fn db_type_options() -> Vec<(SharedString, SharedString)> {
    vec![
        ("sqlite".into(), "SQLite".into()),
        ("postgres".into(), "PostgreSQL".into()),
        ("mysql".into(), "MySQL / MariaDB".into()),
        ("mssql".into(), "MSSQL".into()),
    ]
}

fn db_type_from_key(key: &str) -> DbType {
    match key {
        "postgres" => DbType::Postgres,
        "mysql" => DbType::MySql,
        "mssql" => DbType::MsSql,
        _ => DbType::Sqlite,
    }
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
    /// Which connection's tab is showing in the content area. `None` shows
    /// the Add Connection form (the tab strip's trailing "+" tab). Defaults
    /// to the first connection on load/whenever the panel would otherwise
    /// render blank with connections present — see `render`.
    active_connection: Option<ConnectionId>,
    registry: ConnectionRegistry,
    /// Set while `self.registry` has been temporarily moved into an
    /// in-flight async connect/schema-fetch task (see `connect_network`'s
    /// doc comment) — every other registry-touching entry point no-ops
    /// while this is set, so a second click can't run against the
    /// placeholder `Default` registry left behind by `mem::take` and race
    /// the first task's eventual write-back.
    registry_busy: bool,
    statuses: HashMap<ConnectionId, ConnectionStatus>,
    schemas: HashMap<ConnectionId, SchemaState>,
    tree_states: HashMap<ConnectionId, Entity<TreeState>>,
    /// Shared filter for the schema explorer. Only one connection tab is
    /// visible at a time, so a single `InputState` is enough for now.
    schema_filter: Entity<InputState>,
    /// The active center-pane tab. Shared across connections (per-connection
    /// tab state is a natural follow-up, not done here).
    content_tab: ContentTab,
    new_connection_title: String,
    new_connection_db_type: DbType,
    new_connection_path: String,
    new_connection_host: String,
    new_connection_port: String,
    new_connection_username: String,
    new_connection_ssl: bool,
    new_connection_password: Entity<InputState>,
    next_connection_id: u64,
    next_database_id: u64,
    /// The connection currently showing the "Create database" / "Register
    /// existing database" text input under its discovered-database picker —
    /// only one at a time, mirroring the spec's "only one connection
    /// expanded" navigation model.
    creating_database_for: Option<ConnectionId>,
    new_database_name: Entity<InputState>,
    _filter_subscription: Subscription,
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
        window: &mut Window,
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
                let items: Vec<TreeItem> = tables.iter().map(|table| table_tree_item(table, "")) .collect();
                let tree_state = cx.new(|cx| TreeState::new(cx).items(items));
                tree_states.insert(id, tree_state);
                schemas.insert(id, SchemaState::Loaded(tables.into()));
            }

            let new_connection_password =
                cx.new(|cx| InputState::new(window, cx).masked(true));
            let new_database_name = cx.new(|cx| InputState::new(window, cx));
            let schema_filter = cx.new(|cx| {
                InputState::new(window, cx).placeholder("Filter tables or columns…")
            });

            let _filter_subscription =
                cx.subscribe_in(&schema_filter, window, Self::on_schema_filter_changed);

            let active_connection = connections.first().map(|connection| connection.id);

            Self {
                focus_handle: cx.focus_handle(),
                connections,
                active_connection,
                registry: ConnectionRegistry::new(),
                registry_busy: false,
                statuses: HashMap::default(),
                schemas,
                tree_states,
                schema_filter,
                content_tab: ContentTab::default(),
                new_connection_title: String::new(),
                new_connection_db_type: DbType::Sqlite,
                new_connection_path: String::new(),
                new_connection_host: String::new(),
                new_connection_port: String::new(),
                new_connection_username: String::new(),
                new_connection_ssl: false,
                new_connection_password,
                next_connection_id,
                next_database_id: 1,
                creating_database_for: None,
                new_database_name,
                _filter_subscription,
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

    /// Fetches (or re-fetches, for Refresh) `id`'s schema, dispatching to
    /// whichever driver it's connected with via
    /// `ConnectionRegistry::fetch_schema`. Runs on the real tokio runtime
    /// (`gpui_tokio::Tokio::spawn_result`) since the network drivers need
    /// one; `self.registry` is moved into the task for the duration (see
    /// `registry_busy`'s doc comment) since `fetch_schema` needs to hold it
    /// across the query's `.await`s.
    fn fetch_schema(&mut self, id: ConnectionId, cx: &mut Context<Self>) {
        if self.registry_busy || !self.registry.is_connected(id) {
            return;
        }

        self.schemas.insert(id, SchemaState::Loading);
        self.registry_busy = true;
        cx.notify();

        let registry = std::mem::take(&mut self.registry);

        cx.spawn(async move |this, cx| {
            let result = gpui_tokio::Tokio::spawn_result(cx, async move {
                let tables = registry.fetch_schema(id).await;
                anyhow::Ok((registry, tables))
            })
            .await;

            this.update(cx, |this, cx| {
                this.registry_busy = false;
                match result {
                    Ok((registry, Ok(tables))) => {
                        this.registry = registry;
                        this.update_tree_items(id, &tables, cx);
                        this.schemas.insert(id, SchemaState::Loaded(tables.into()));
                        this.persist_schema_cache(cx);
                    }
                    Ok((registry, Err(err))) => {
                        this.registry = registry;
                        this.schemas.insert(id, SchemaState::Error(err.to_string()));
                    }
                    Err(err) => {
                        // The tokio task itself failed to join (panicked or
                        // was cancelled) — the registry it owned is lost
                        // along with every connection that was live in it.
                        this.registry = ConnectionRegistry::new();
                        this.statuses.clear();
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
    /// current shape, applying the active schema filter.
    fn update_tree_items(&mut self, id: ConnectionId, tables: &[TableInfo], cx: &mut Context<Self>) {
        let needle = self.schema_filter.read(cx).value().to_lowercase();
        let items: Vec<TreeItem> = tables
            .iter()
            .filter(|table| table_matches(table, &needle))
            .map(|table| table_tree_item(table, &needle))
            .collect();
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

    /// The schema explorer filter input changed — re-filter the active
    /// connection's tree, preserving a previously selected table when it
    /// still matches.
    fn on_schema_filter_changed(
        &mut self,
        _: &Entity<InputState>,
        event: &InputEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !matches!(event, InputEvent::Change) {
            return;
        }
        let Some(id) = self.active_connection else {
            return;
        };
        let Some(SchemaState::Loaded(tables)) = self.schemas.get(&id) else {
            return;
        };
        let tables = tables.clone();
        let previous = self.selected_schema_item(id, cx);
        self.update_tree_items(id, &tables, cx);

        let Some(SchemaSelection::Table { name }) = previous else {
            return;
        };
        let needle = self.schema_filter.read(cx).value().to_lowercase();
        let Some(table) = tables.iter().find(|table| table.name == name) else {
            return;
        };
        if !table_matches(table, &needle) {
            return;
        }
        if let Some(tree_state) = self.tree_states.get(&id) {
            let item = table_tree_item(table, &needle);
            tree_state.update(cx, |tree_state, cx| {
                tree_state.set_selected_item(Some(&item), cx);
            });
        }
    }

    /// What (if anything) is selected in `id`'s schema tree, decoded from the
    /// item's id (`table_name` vs `table::column`).
    fn selected_schema_item(&self, id: ConnectionId, cx: &App) -> Option<SchemaSelection> {
        let tree_state = self.tree_states.get(&id)?;
        let item = tree_state.read(cx).selected_item()?;
        let id_str = item.id.as_str();
        if let Some((table, column)) = id_str.split_once("::") {
            Some(SchemaSelection::Column {
                table: table.to_string(),
                column: column.to_string(),
            })
        } else {
            Some(SchemaSelection::Table { name: id_str.to_string() })
        }
    }

    /// The table (and optional selected column) behind `connection`'s current
    /// tree selection, if any.
    fn selected_table<'a>(
        &self,
        connection: &ConnectionConfig,
        tables: &'a [TableInfo],
        cx: &App,
    ) -> Option<(Option<String>, &'a TableInfo)> {
        let selection = self.selected_schema_item(connection.id, cx)?;
        match selection {
            SchemaSelection::Table { name } => tables
                .iter()
                .find(|table| table.name == name)
                .map(|table| (None, table)),
            SchemaSelection::Column { table, column } => tables
                .iter()
                .find(|candidate| candidate.name == table)
                .map(|table| (Some(column), table)),
        }
    }

    /// Clears the explorer tree's selection (used by the content pane's
    /// "Back to list" action).
    fn clear_tree_selection(&self, id: ConnectionId, cx: &mut Context<Self>) {
        if let Some(tree_state) = self.tree_states.get(&id) {
            tree_state.update(cx, |tree_state, cx| {
                tree_state.set_selected_index(None, cx);
            });
        }
    }

    /// Builds the new connection from the "Add Connection" form fields,
    /// saves it, clears the form, and kicks off a connect attempt. For
    /// network types with a password typed in, the password is written to
    /// the credential store before connecting (`save_password_and_connect`);
    /// with the field left empty, `connect` falls straight through to its
    /// "no saved password" error, same as reconnecting a saved connection
    /// with none stored.
    fn add_connection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.registry_busy {
            return;
        }

        let title = self.new_connection_title.trim().to_string();
        if title.is_empty() {
            return;
        }
        let db_type = self.new_connection_db_type;
        let id = ConnectionId(self.next_connection_id);

        let config = match db_type {
            DbType::Sqlite => {
                let path = self.new_connection_path.trim().to_string();
                if path.is_empty() {
                    return;
                }
                ConnectionConfig {
                    id,
                    title,
                    db_type,
                    host: None,
                    port: None,
                    username: None,
                    sqlite_path: Some(path),
                    database: None,
                    ssl: false,
                    databases: Vec::new(),
                }
            }
            DbType::Postgres | DbType::MySql | DbType::MsSql => {
                let host = self.new_connection_host.trim().to_string();
                let username = self.new_connection_username.trim().to_string();
                if host.is_empty() || username.is_empty() {
                    return;
                }
                let port = self
                    .new_connection_port
                    .trim()
                    .parse()
                    .unwrap_or_else(|_| default_port(db_type));
                ConnectionConfig {
                    id,
                    title,
                    db_type,
                    host: Some(host),
                    port: Some(port),
                    username: Some(username),
                    sqlite_path: None,
                    // No database picked yet — `connect` discovers what's on
                    // the server and lets the user pick from that list
                    // instead of requiring the exact name upfront.
                    database: None,
                    ssl: self.new_connection_ssl,
                    databases: Vec::new(),
                }
            }
        };

        self.next_connection_id += 1;
        self.connections.push(config.clone());
        self.active_connection = Some(id);
        self.persist_connections(cx);

        let password = self.new_connection_password.read(cx).value().to_string();
        self.new_connection_title.clear();
        self.new_connection_path.clear();
        self.new_connection_host.clear();
        self.new_connection_port.clear();
        self.new_connection_username.clear();
        self.new_connection_ssl = false;
        self.new_connection_password.update(cx, |state, cx| {
            state.set_value("", window, cx);
        });
        cx.notify();

        match db_type {
            DbType::Sqlite => self.connect(id, cx),
            DbType::Postgres | DbType::MySql | DbType::MsSql if !password.is_empty() => {
                self.save_password_and_connect(config, password, cx);
            }
            DbType::Postgres | DbType::MySql | DbType::MsSql => self.connect(id, cx),
        }
    }

    /// Writes `password` to the credential store keyed by this connection's
    /// `credential_url`, then connects — matching the plan's "write
    /// immediately, connect reads from the store" flow so reconnecting later
    /// (no local form state to read from) works identically to this first
    /// connect.
    fn save_password_and_connect(
        &mut self,
        connection: ConnectionConfig,
        password: String,
        cx: &mut Context<Self>,
    ) {
        let id = connection.id;
        let (Some(host), Some(port), Some(username)) =
            (connection.host, connection.port, connection.username)
        else {
            return;
        };
        let key = credential_url(connection.db_type, &host, port);

        cx.spawn(async move |this, cx| {
            let credentials_provider = cx.update(|cx| zed_credentials_provider::global(cx));
            let result = credentials_provider
                .write_credentials(&key, &username, password.as_bytes(), cx)
                .await;

            this.update(cx, |this, cx| match result {
                Ok(()) => this.connect(id, cx),
                Err(err) => {
                    this.statuses.insert(
                        id,
                        ConnectionStatus::Error(format!("Failed to save credentials: {err}")),
                    );
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    fn connect(&mut self, id: ConnectionId, cx: &mut Context<Self>) {
        if self.registry_busy {
            return;
        }
        let Some(connection) = self.connections.iter().find(|c| c.id == id).cloned() else {
            return;
        };

        match connection.db_type {
            DbType::Sqlite => {
                let Some(path) = connection.sqlite_path else {
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
            DbType::Postgres | DbType::MySql | DbType::MsSql => {
                if connection.database.is_some() {
                    // A specific database was already picked (either just
                    // now via `select_database`, or this is a persisted
                    // connection from a prior session) — dial it directly.
                    self.connect_network(connection, cx);
                } else {
                    // First-time connect: discover what's on the server and
                    // let the user pick, instead of requiring them to
                    // already know the exact database name.
                    self.discover_databases(id, cx);
                }
            }
        }
    }

    /// Connects a network connection (Postgres/MySQL/MSSQL): looks up its
    /// password from the credential store, then dials out on the real tokio
    /// runtime via `gpui_tokio::Tokio::spawn_result`.
    ///
    /// `ConnectionRegistry::connect_postgres`/`connect_mysql`/`connect_mssql`
    /// are `&mut self` async methods (the connect call itself needs to hold
    /// the registry mutably across its `.await`s to insert the new handle
    /// atomically with the connect succeeding). GPUI entities can't be
    /// mutably borrowed across an await, so `self.registry` is moved out via
    /// `mem::take` for the task's duration and moved back on completion —
    /// see `registry_busy`, which blocks every other registry-touching entry
    /// point for as long as that's in flight.
    fn connect_network(&mut self, connection: ConnectionConfig, cx: &mut Context<Self>) {
        let id = connection.id;
        let db_type = connection.db_type;
        let (Some(host), Some(port), Some(username)) =
            (connection.host, connection.port, connection.username)
        else {
            return;
        };
        let database = connection.database.filter(|d| !d.trim().is_empty()).unwrap_or_else(|| {
            // An unset Postgres `dbname` isn't "no default database" — the
            // server falls back to a database named after the connecting
            // user, which almost never exists (see e.g. `FATAL: database
            // "dev_user" does not exist`). "postgres" (the standard
            // maintenance DB every install has) is the useful default here.
            // MySQL/MSSQL don't have this footgun: an empty MySQL dbname is
            // a normal "no default DB yet" connection, and MSSQL falls back
            // to the login's configured default DB (usually "master").
            if db_type == DbType::Postgres {
                "postgres".to_string()
            } else {
                String::new()
            }
        });
        let ssl = connection.ssl;
        let key = credential_url(db_type, &host, port);

        log::info!(
            "database_panel: connecting {db_type:?} connection {id:?} \
             (host={host}, port={port}, database={database:?}, username={username:?}, ssl={ssl})"
        );

        self.statuses.insert(id, ConnectionStatus::Connecting);
        self.registry_busy = true;
        cx.notify();

        let mut registry = std::mem::take(&mut self.registry);

        cx.spawn(async move |this, cx| {
            let credentials_provider = cx.update(|cx| zed_credentials_provider::global(cx));
            let password = match credentials_provider.read_credentials(&key, cx).await {
                Ok(Some((_, bytes))) => String::from_utf8(bytes).unwrap_or_default(),
                Ok(None) => {
                    log::error!("database_panel: connect {id:?} failed: no saved password");
                    this.update(cx, |this, cx| {
                        this.registry = registry;
                        this.registry_busy = false;
                        this.statuses.insert(
                            id,
                            ConnectionStatus::Error(
                                "No saved password for this connection".into(),
                            ),
                        );
                        cx.notify();
                    })
                    .ok();
                    return;
                }
                Err(err) => {
                    log::error!(
                        "database_panel: connect {id:?} failed reading saved credentials: {err}"
                    );
                    this.update(cx, |this, cx| {
                        this.registry = registry;
                        this.registry_busy = false;
                        this.statuses
                            .insert(id, ConnectionStatus::Error(err.to_string()));
                        cx.notify();
                    })
                    .ok();
                    return;
                }
            };

            let params = NetworkConnectParams {
                host,
                port,
                database,
                username,
                password,
                ssl,
            };

            let result = gpui_tokio::Tokio::spawn_result(cx, async move {
                let outcome = match db_type {
                    DbType::Postgres => registry.connect_postgres(id, params).await,
                    DbType::MySql => registry.connect_mysql(id, params).await,
                    DbType::MsSql => registry.connect_mssql(id, params).await,
                    DbType::Sqlite => {
                        unreachable!("sqlite connections never reach connect_network")
                    }
                };
                anyhow::Ok((registry, outcome))
            })
            .await;

            this.update(cx, |this, cx| {
                this.registry_busy = false;
                match result {
                    Ok((registry, Ok(()))) => {
                        log::info!("database_panel: connect {id:?} succeeded");
                        this.registry = registry;
                        this.statuses.insert(id, ConnectionStatus::Connected);
                        cx.notify();
                        this.fetch_schema(id, cx);
                        return;
                    }
                    Ok((registry, Err(err))) => {
                        log::error!("database_panel: connect {id:?} failed: {err}");
                        this.registry = registry;
                        this.statuses
                            .insert(id, ConnectionStatus::Error(err.to_string()));
                    }
                    Err(err) => {
                        log::error!("database_panel: connect {id:?} task panicked/cancelled: {err}");
                        this.registry = ConnectionRegistry::new();
                        this.statuses.clear();
                        this.statuses
                            .insert(id, ConnectionStatus::Error(err.to_string()));
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Discovers every database visible on a network connection's server and
    /// stores the result on `connection.databases`, so the panel can render
    /// a picker instead of requiring the user to already know the exact
    /// database name (matches Forge's original two-step "connect to server,
    /// then pick a database" flow — see `DATABASE_PANEL_SPEC.md`'s
    /// Navigation Phase 3). Does not touch `self.registry` — no live driver
    /// connection is opened for the picker itself, only for whichever
    /// database is eventually selected via `select_database`.
    fn discover_databases(&mut self, id: ConnectionId, cx: &mut Context<Self>) {
        if self.registry_busy {
            return;
        }
        let Some(connection) = self.connections.iter().find(|c| c.id == id).cloned() else {
            return;
        };
        let db_type = connection.db_type;
        let (Some(host), Some(port), Some(username)) =
            (connection.host, connection.port, connection.username)
        else {
            return;
        };
        let ssl = connection.ssl;
        let key = credential_url(db_type, &host, port);

        log::info!(
            "database_panel: discovering databases on {db_type:?} connection {id:?} \
             (host={host}, port={port}, username={username:?})"
        );

        self.statuses.insert(id, ConnectionStatus::Connecting);
        cx.notify();

        cx.spawn(async move |this, cx| {
            let credentials_provider = cx.update(|cx| zed_credentials_provider::global(cx));
            let password = match credentials_provider.read_credentials(&key, cx).await {
                Ok(Some((_, bytes))) => String::from_utf8(bytes).unwrap_or_default(),
                Ok(None) => {
                    log::error!("database_panel: discover {id:?} failed: no saved password");
                    this.update(cx, |this, cx| {
                        this.statuses.insert(
                            id,
                            ConnectionStatus::Error(
                                "No saved password for this connection".into(),
                            ),
                        );
                        cx.notify();
                    })
                    .ok();
                    return;
                }
                Err(err) => {
                    log::error!(
                        "database_panel: discover {id:?} failed reading saved credentials: {err}"
                    );
                    this.update(cx, |this, cx| {
                        this.statuses
                            .insert(id, ConnectionStatus::Error(err.to_string()));
                        cx.notify();
                    })
                    .ok();
                    return;
                }
            };

            let params = NetworkConnectParams {
                host,
                port,
                database: String::new(),
                username,
                password,
                ssl,
            };

            let result = gpui_tokio::Tokio::spawn_result(cx, async move {
                anyhow::Ok(database_backend::list_databases(db_type, params).await)
            })
            .await
            .map_err(|err| err.to_string())
            .and_then(|inner| inner.map_err(|err| err.to_string()));

            this.update(cx, |this, cx| {
                match result {
                    Ok(names) => {
                        log::info!(
                            "database_panel: discover {id:?} found {} databases",
                            names.len()
                        );
                        let mut next_id = this.next_database_id;
                        let databases: Vec<SavedDatabase> = names
                            .into_iter()
                            .map(|name| {
                                next_id += 1;
                                SavedDatabase {
                                    id: DatabaseId(next_id),
                                    name,
                                }
                            })
                            .collect();
                        this.next_database_id = next_id;
                        if let Some(connection) =
                            this.connections.iter_mut().find(|c| c.id == id)
                        {
                            connection.databases = databases;
                        }
                        this.statuses.insert(id, ConnectionStatus::Disconnected);
                        this.persist_connections(cx);
                    }
                    Err(err) => {
                        log::error!("database_panel: discover {id:?} failed: {err}");
                        this.statuses.insert(id, ConnectionStatus::Error(err));
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Picks `name` as the connection's active database, persists that
    /// choice, and dials the actual driver connection (`connect_network`)
    /// for it — mirrors the spec's "selecting a database connects to it and
    /// opens its schema view."
    fn select_database(&mut self, id: ConnectionId, name: String, cx: &mut Context<Self>) {
        if self.registry_busy {
            return;
        }
        let Some(connection) = self.connections.iter_mut().find(|c| c.id == id) else {
            return;
        };
        connection.database = Some(name);
        let connection = connection.clone();
        self.persist_connections(cx);
        self.connect_network(connection, cx);
    }

    /// Opens the "Create database" input for `id`'s picker (only one
    /// connection's at a time).
    fn start_create_database(&mut self, id: ConnectionId, window: &mut Window, cx: &mut Context<Self>) {
        self.creating_database_for = Some(id);
        self.new_database_name.update(cx, |state, cx| {
            state.set_value("", window, cx);
        });
        cx.notify();
    }

    fn cancel_create_database(&mut self, cx: &mut Context<Self>) {
        self.creating_database_for = None;
        cx.notify();
    }

    /// Runs `CREATE DATABASE` on the server, then re-runs discovery so the
    /// new database shows up in the picker.
    fn submit_create_database(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.creating_database_for else {
            return;
        };
        let name = self.new_database_name.read(cx).value().trim().to_string();
        if name.is_empty() {
            return;
        }
        let Some(connection) = self.connections.iter().find(|c| c.id == id).cloned() else {
            return;
        };
        let db_type = connection.db_type;
        let (Some(host), Some(port), Some(username)) =
            (connection.host, connection.port, connection.username)
        else {
            return;
        };
        let ssl = connection.ssl;
        let key = credential_url(db_type, &host, port);

        self.creating_database_for = None;
        self.new_database_name.update(cx, |state, cx| {
            state.set_value("", window, cx);
        });
        log::info!("database_panel: creating database {name:?} on connection {id:?}");
        cx.notify();

        cx.spawn(async move |this, cx| {
            let credentials_provider = cx.update(|cx| zed_credentials_provider::global(cx));
            let password = match credentials_provider.read_credentials(&key, cx).await {
                Ok(Some((_, bytes))) => String::from_utf8(bytes).unwrap_or_default(),
                _ => {
                    log::error!(
                        "database_panel: create database on {id:?} failed: no saved password"
                    );
                    this.update(cx, |this, cx| {
                        this.statuses.insert(
                            id,
                            ConnectionStatus::Error(
                                "No saved password for this connection".into(),
                            ),
                        );
                        cx.notify();
                    })
                    .ok();
                    return;
                }
            };

            let params = NetworkConnectParams {
                host,
                port,
                database: String::new(),
                username,
                password,
                ssl,
            };

            let result = gpui_tokio::Tokio::spawn_result(cx, async move {
                anyhow::Ok(database_backend::create_database(db_type, params, &name).await)
            })
            .await
            .map_err(|err| err.to_string())
            .and_then(|inner| inner.map_err(|err| err.to_string()));

            match result {
                Ok(()) => {
                    log::info!("database_panel: create database on {id:?} succeeded");
                    this.update(cx, |this, cx| this.discover_databases(id, cx)).ok();
                }
                Err(err) => {
                    log::error!("database_panel: create database on {id:?} failed: {err}");
                    this.update(cx, |this, cx| {
                        this.statuses.insert(id, ConnectionStatus::Error(err));
                        cx.notify();
                    })
                    .ok();
                }
            }
        })
        .detach();
    }

    /// Registers a database name that's already known to exist (e.g. one
    /// discovery couldn't see due to permissions) as the connection's active
    /// database, without attempting to create it.
    fn submit_register_database(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.creating_database_for else {
            return;
        };
        let name = self.new_database_name.read(cx).value().trim().to_string();
        if name.is_empty() {
            return;
        }
        self.creating_database_for = None;
        self.new_database_name.update(cx, |state, cx| {
            state.set_value("", window, cx);
        });
        self.select_database(id, name, cx);
    }

    fn disconnect(&mut self, id: ConnectionId, cx: &mut Context<Self>) {
        if self.registry_busy {
            return;
        }
        self.registry.disconnect(id).ok();
        self.statuses.insert(id, ConnectionStatus::Disconnected);
        cx.notify();
    }

    fn delete_connection(&mut self, id: ConnectionId, cx: &mut Context<Self>) {
        if self.registry_busy {
            return;
        }
        self.registry.disconnect(id).ok();
        self.statuses.remove(&id);
        self.connections.retain(|connection| connection.id != id);
        if self.active_connection == Some(id) {
            self.active_connection = self.connections.first().map(|connection| connection.id);
        }
        self.persist_connections(cx);
        cx.notify();
    }

    /// Handles a click on the connection tab strip: `ix < connections.len()`
    /// selects that connection; the trailing index (the "+" tab) opens the
    /// Add Connection form instead.
    fn select_connection_tab(&mut self, ix: usize, cx: &mut Context<Self>) {
        self.active_connection = self.connections.get(ix).map(|connection| connection.id);
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
        let db_type = self.new_connection_db_type;
        // The theme's default unfocused input border (`cx.theme().input`) is
        // too close to this panel's background to read as a field boundary
        // — users can't tell where to click. Force a higher-contrast border
        // explicitly rather than relying on that token.
        let input_border = cx.theme().border;

        let db_type_entity = entity.clone();
        let db_type_field = SettingField::dropdown(
            db_type_options(),
            {
                let entity = entity.clone();
                move |cx: &App| entity.read(cx).new_connection_db_type.as_str().into()
            },
            move |value: SharedString, cx: &mut App| {
                db_type_entity.update(cx, |this, cx| {
                    let db_type = db_type_from_key(&value);
                    this.new_connection_db_type = db_type;
                    if db_type != DbType::Sqlite && this.new_connection_port.trim().is_empty() {
                        this.new_connection_port = default_port(db_type).to_string();
                    }
                    cx.notify();
                });
            },
        );

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
        )
        .border_1()
        .border_color(input_border);

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
        )
        .border_1()
        .border_color(input_border);

        let host_entity = entity.clone();
        let host_field = SettingField::input(
            {
                let entity = entity.clone();
                move |cx: &App| entity.read(cx).new_connection_host.clone().into()
            },
            move |value: SharedString, cx: &mut App| {
                host_entity.update(cx, |this, cx| {
                    this.new_connection_host = value.to_string();
                    cx.notify();
                });
            },
        )
        .border_1()
        .border_color(input_border);

        let port_entity = entity.clone();
        let port_field = SettingField::input(
            {
                let entity = entity.clone();
                move |cx: &App| entity.read(cx).new_connection_port.clone().into()
            },
            move |value: SharedString, cx: &mut App| {
                port_entity.update(cx, |this, cx| {
                    this.new_connection_port = value.to_string();
                    cx.notify();
                });
            },
        )
        .border_1()
        .border_color(input_border);

        let username_entity = entity.clone();
        let username_field = SettingField::input(
            {
                let entity = entity.clone();
                move |cx: &App| entity.read(cx).new_connection_username.clone().into()
            },
            move |value: SharedString, cx: &mut App| {
                username_entity.update(cx, |this, cx| {
                    this.new_connection_username = value.to_string();
                    cx.notify();
                });
            },
        )
        .border_1()
        .border_color(input_border);

        let ssl_entity = entity.clone();
        let ssl_field = SettingField::checkbox(
            {
                let entity = entity.clone();
                move |cx: &App| entity.read(cx).new_connection_ssl
            },
            move |value: bool, cx: &mut App| {
                ssl_entity.update(cx, |this, cx| {
                    this.new_connection_ssl = value;
                    cx.notify();
                });
            },
        );

        let password_state = self.new_connection_password.clone();

        let browse_entity = entity.clone();
        let sqlite_connect_entity = entity.clone();
        let network_connect_entity = entity.clone();

        let mut group = SettingGroup::new()
            .item(SettingItem::new("Type", db_type_field))
            .item(SettingItem::new("Title", title_field));

        group = match db_type {
            DbType::Sqlite => group
                .item(SettingItem::new("File Path", path_field))
                .item(SettingItem::render(move |_options, _window, _cx| {
                    let browse_entity = browse_entity.clone();
                    let connect_entity = sqlite_connect_entity.clone();
                    h_flex()
                        .gap_2()
                        .child(
                            Button::new("browse-sqlite-path")
                                .outline()
                                .label("Browse…")
                                .on_click(move |_, window, cx| {
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
                                }),
                        )
                        .child(
                            Button::new("add-connection")
                                .label("Connect")
                                .primary()
                                .on_click(move |_, window, cx| {
                                    connect_entity.update(cx, |this, cx| {
                                        this.add_connection(window, cx);
                                    });
                                }),
                        )
                        .into_any_element()
                })),
            DbType::Postgres | DbType::MySql | DbType::MsSql => group
                .item(SettingItem::new("Host", host_field))
                .item(SettingItem::new("Port", port_field))
                .item(SettingItem::new("Username", username_field))
                .item(SettingItem::render(move |_options, _window, _cx| {
                    h_flex()
                        .w_full()
                        .justify_between()
                        .items_center()
                        .child(
                            Label::new("Password")
                                .size(LabelSize::Small)
                                .color(Color::Muted),
                        )
                        .child(
                            Input::new(&password_state)
                                .mask_toggle()
                                .border_1()
                                .border_color(input_border),
                        )
                        .into_any_element()
                }))
                .item(SettingItem::new("SSL", ssl_field))
                .item(SettingItem::render(move |_options, _window, _cx| {
                    let connect_entity = network_connect_entity.clone();
                    h_flex().justify_end().child(
                        Button::new("add-connection")
                            .label("Connect")
                            .primary()
                            .on_click(move |_, window, cx| {
                                connect_entity.update(cx, |this, cx| {
                                    this.add_connection(window, cx);
                                });
                            }),
                    )
                    .into_any_element()
                })),
        };

        Settings::new("database-panel-add-connection")
            .sidebar_width(px(0.))
            .page(SettingPage::new("Add Connection").group(group))
    }

    /// Renders the selected connection tab's body: the connection header
    /// (`DatabaseHeader`: status/actions + database tab strip) on top, and
    /// the three-pane explorer split below — or a centered state/error view
    /// while connecting, loading, failing, or when the schema is empty.
    fn render_connection_body(
        &self,
        connection: &ConnectionConfig,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let id = connection.id;
        let is_connected = self.registry.is_connected(id);

        v_flex()
            .w_full()
            .size_full()
            .child(self.render_connection_header(connection, cx))
            .child(self.render_body_split(connection, id, is_connected, cx))
    }

    /// The three-pane explorer shell: `h_resizable` split with a resizable
    /// explorer sidebar (left) and inspector (right) around a flex-grow
    /// content pane. Only rendered once a schema is actually loaded;
    /// everything else routes to `render_connection_state` /
    /// `render_connection_error`.
    fn render_body_split(
        &self,
        connection: &ConnectionConfig,
        id: ConnectionId,
        is_connected: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        if !is_connected {
            return self.render_connection_state(connection, cx).into_any_element();
        }

        let tables = match self.schemas.get(&id) {
            None | Some(SchemaState::Loading) => {
                return self.render_connection_state(connection, cx).into_any_element()
            }
            Some(SchemaState::Error(message)) => {
                return self.render_connection_error(connection, message, cx).into_any_element()
            }
            Some(SchemaState::Loaded(tables)) if tables.is_empty() => {
                return self.render_connection_state(connection, cx).into_any_element()
            }
            Some(SchemaState::Loaded(tables)) => tables.clone(),
        };

        h_resizable(("database-body", id.0))
            .child(
                resizable_panel()
                    .size(px(280.))
                    .size_range(px(200.)..px(420.))
                    .flex_none()
                    .child(self.render_explorer_pane(connection, &tables, cx)),
            )
            .child(
                resizable_panel()
                    .child(self.render_content_pane(connection, &tables, cx)),
            )
            .child(
                resizable_panel()
                    .size(px(320.))
                    .size_range(px(240.)..px(560.))
                    .flex_none()
                    .child(self.render_inspector_pane(connection, &tables, cx)),
            )
            .into_any_element()
    }

    /// The centered "not connected / loading / empty schema" state.
    fn render_connection_state(
        &self,
        connection: &ConnectionConfig,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let id = connection.id;
        let is_connected = self.registry.is_connected(id);
        let is_loading = matches!(self.schemas.get(&id), None | Some(SchemaState::Loading));

        let body: gpui::AnyElement = if is_connected && is_loading {
            h_flex()
                .items_center()
                .gap_2()
                .child(Spinner::new().with_size(Size::Medium))
                .child(
                    Label::new("Loading schema…")
                        .size(LabelSize::Default)
                        .color(Color::Muted),
                )
                .into_any_element()
        } else if is_connected {
            v_flex()
                .items_center()
                .gap_2()
                .child(
                    Icon::new(GIconName::Inbox)
                        .with_size(Size::Large)
                        .text_color(cx.theme().muted_foreground),
                )
                .child(Label::new("No tables").size(LabelSize::Large))
                .child(
                    Label::new("This database has no tables yet.")
                        .size(LabelSize::Small)
                        .color(Color::Muted),
                )
                .into_any_element()
        } else {
            let detail = match self.statuses.get(&id) {
                Some(ConnectionStatus::Error(message)) => {
                    format!("Could not connect: {message}")
                }
                _ => "Connect to this database to explore its schema.".to_string(),
            };
            v_flex()
                .items_center()
                .gap_3()
                .child(
                    Icon::new(GIconName::Inbox)
                        .with_size(Size::Large)
                        .text_color(cx.theme().muted_foreground),
                )
                .child(Label::new("Not connected").size(LabelSize::Large))
                .child(
                    Label::new(detail)
                        .size(LabelSize::Small)
                        .color(Color::Muted),
                )
                .child(
                    Button::new(("connect", id.0))
                        .primary()
                        .label("Connect")
                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                            this.connect(id, cx);
                        })),
                )
                .into_any_element()
        };

        v_flex()
            .w_full()
            .size_full()
            .items_center()
            .justify_center()
            .px_6()
            .child(body)
    }

    /// The centered "schema failed to load" state with a Retry action.
    fn render_connection_error(
        &self,
        connection: &ConnectionConfig,
        message: &str,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let id = connection.id;
        v_flex()
            .w_full()
            .size_full()
            .items_center()
            .justify_center()
            .gap_3()
            .px_6()
            .child(
                Icon::new(GIconName::TriangleAlert)
                    .with_size(Size::Large)
                    .text_color(cx.theme().danger_foreground),
            )
            .child(
                Label::new("Failed to load schema")
                    .size(LabelSize::Large)
                    .color(Color::Error),
            )
            .child(
                Label::new(message.to_string())
                    .size(LabelSize::Small)
                    .color(Color::Muted),
            )
            .child(
                Button::new(("retry-schema", id.0))
                    .outline()
                    .label("Retry")
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.fetch_schema(id, cx);
                    })),
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
        DockPosition::Bottom
    }

    fn position_is_valid(&self, position: DockPosition) -> bool {
        matches!(position, DockPosition::Bottom)
    }

    fn set_position(
        &mut self,
        _position: DockPosition,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        // Fixed to the bottom dock — see `position_is_valid`.
    }

    fn default_size(&self, _window: &Window, _cx: &App) -> Pixels {
        px(440.)
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

impl DatabasePanel {
    /// The top-of-panel connection tab strip — one `Tab` per connection plus
    /// a trailing "+" tab that opens the Add Connection form, mirroring
    /// `npm_manager_panel`'s `project_tabs` (see its doc comment) and the
    /// original Forge panel's connection pills.
    fn render_connection_tabs(&self, connections: &[ConnectionConfig], cx: &mut Context<Self>) -> impl IntoElement {
        let add_tab_ix = connections.len();
        let selected_index = self
            .active_connection
            .and_then(|id| connections.iter().position(|connection| connection.id == id))
            .unwrap_or(add_tab_ix);

        TabBar::new("database-connection-tabs")
            .with_variant(TabVariant::Underline)
            .children(connections.iter().map(|connection| {
                let (indicator, color) = self.status_indicator(connection.id);
                Tab::new()
                    .prefix(Label::new(indicator).color(color))
                    .label(connection.title.clone())
            }))
            .child(Tab::new().prefix(Icon::new(GIconName::Plus)).label("Add Connection"))
            .selected_index(selected_index)
            .on_click(cx.listener(move |this, ix: &usize, _, cx| {
                this.select_connection_tab(*ix, cx);
            }))
    }
}

impl Render for DatabasePanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let connections = self.connections.clone();
        let active_connection = self
            .active_connection
            .and_then(|id| connections.iter().find(|connection| connection.id == id).cloned());

        let body = match active_connection {
            Some(connection) => self.render_connection_body(&connection, cx).into_any_element(),
            None => self.render_add_connection_form(cx).into_any_element(),
        };

        v_flex()
            .id("database-panel")
            .track_focus(&self.focus_handle(cx))
            .size_full()
            .bg(cx.theme().sidebar)
            .child(self.render_connection_tabs(&connections, cx))
            .child(body)
    }
}