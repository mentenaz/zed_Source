//! Database panel UI.
//!
//! See `crates/gpui_component/DATABASE_PANEL_SPEC.md` for the full
//! architecture and phased delivery plan. This is Delivery Phase 3: on top
//! of Phase 2's SQLite-only connect/disconnect + schema tree, the panel now
//! also speaks PostgreSQL, MySQL/MariaDB, and MSSQL. Passwords are never
//! persisted on `ConnectionConfig` — they're read/written exclusively
//! through `zed_credentials_provider`, keyed by `credential_url(db_type,
//! host, port)`. Reconnect is manual only (no automatic dead-connection
//! detection/retry yet — flagged as a later follow-up, not in scope here).

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
    Render, SharedString, Styled as _, Task, WeakEntity, Window, actions, div,
    prelude::FluentBuilder as _, px,
};
use gpui_component::{
    ActiveTheme as _, Icon, IconName as GIconName,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{Input, InputState},
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
                let items: Vec<TreeItem> = tables.iter().map(table_tree_item).collect();
                let tree_state = cx.new(|cx| TreeState::new(cx).items(items));
                tree_states.insert(id, tree_state);
                schemas.insert(id, SchemaState::Loaded(tables.into()));
            }

            let new_connection_password =
                cx.new(|cx| InputState::new(window, cx).masked(true));
            let new_database_name = cx.new(|cx| InputState::new(window, cx));

            Self {
                focus_handle: cx.focus_handle(),
                connections,
                registry: ConnectionRegistry::new(),
                registry_busy: false,
                statuses: HashMap::default(),
                schemas,
                tree_states,
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
                        .child(Input::new(&password_state).mask_toggle())
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

    fn render_connection_row(&self, connection: &ConnectionConfig, cx: &mut Context<Self>) -> impl IntoElement {
        let id = connection.id;
        let (indicator, color) = self.status_indicator(id);
        let is_connected = self.registry.is_connected(id);
        let error_message = match self.statuses.get(&id) {
            Some(ConnectionStatus::Error(message)) => Some(message.clone()),
            _ => None,
        };

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
                                        .outline()
                                        .label("Refresh")
                                        .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                            this.fetch_schema(id, cx);
                                        })),
                                )
                            })
                            .child(if is_connected {
                                Button::new(("disconnect", id.0))
                                    .outline()
                                    .label("Disconnect")
                                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                        this.disconnect(id, cx);
                                    }))
                            } else {
                                Button::new(("connect", id.0))
                                    .primary()
                                    .label("Connect")
                                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                        this.connect(id, cx);
                                    }))
                            })
                            .child(
                                Button::new(("delete", id.0))
                                    .danger()
                                    .icon(Icon::new(GIconName::Delete))
                                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                        this.delete_connection(id, cx);
                                    })),
                            ),
                    ),
            )
            .when_some(error_message, |this, message| {
                this.child(
                    div().px_2().pb_1().child(
                        Label::new(message)
                            .size(LabelSize::Small)
                            .color(Color::Error),
                    ),
                )
            })
            .when(!is_connected && !connection.databases.is_empty(), |this| {
                this.child(self.render_database_picker(connection, cx))
            })
            .children(self.render_schema_section(id, is_connected, cx))
    }

    /// Renders the discovered-database picker for a not-yet-activated
    /// network connection: a clickable list of every database `discover_databases`
    /// found, plus "+ Create database" / "Register existing database"
    /// actions — mirrors the original Forge panel's post-connect database
    /// browser and `DATABASE_PANEL_SPEC.md`'s Navigation Phase 3 layout.
    fn render_database_picker(
        &self,
        connection: &ConnectionConfig,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let id = connection.id;
        let is_creating = self.creating_database_for == Some(id);

        v_flex()
            .w_full()
            .px_4()
            .py_1()
            .gap_1()
            .child(
                Label::new("No databases connected")
                    .size(LabelSize::Small)
                    .color(Color::Muted),
            )
            .children(connection.databases.iter().map(|database| {
                let name = database.name.clone();
                ListItem::new(("database-picker-item", database.id.0))
                    .child(Label::new(database.name.clone()).size(LabelSize::Small))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.select_database(id, name.clone(), cx);
                    }))
            }))
            .child(if is_creating {
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(Input::new(&self.new_database_name).w_48())
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
                        .outline()
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
        px(320.)
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
