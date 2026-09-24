use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use rusqlite::Connection as SqliteConnection;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex as AsyncMutex;

use crate::drivers::{
    DbType, mssql, mysql, postgres, sqlite,
};
use crate::errors::DatabaseError;

/// Stable identifier for a saved connection, independent of connection order
/// or display title so saved metadata and keychain entries stay valid across
/// renames.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ConnectionId(pub u64);

/// Stable identifier for a database within a connection (a connection may
/// expose more than one database, e.g. Postgres/MySQL/MSSQL).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DatabaseId(pub u64);

/// Lifecycle state of a connection, surfaced by the panel's status
/// indicators (`[●] [◐] [ ] [!]` per the spec).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnectionStatus {
    Disconnected,
    Connecting,
    Connected,
    Error(String),
}

/// Everything a network driver (Postgres/MySQL/MSSQL) needs to connect.
/// `password` never comes from `ConnectionConfig` — it's read from
/// `zed_credentials_provider` by the caller and passed in fresh per connect
/// attempt, never stored on this struct beyond the call itself.
#[derive(Clone)]
pub struct NetworkConnectParams {
    pub host: String,
    pub port: u16,
    pub database: String,
    pub username: String,
    pub password: String,
    pub ssl: bool,
}

/// Builds the credential-store lookup key for a network connection. The key
/// is connection-level, NOT per-database: a connection's password is the
/// same regardless of which database within it is selected, so there is no
/// per-database credential scoping (a server login owns the whole server).
/// This is why the URL omits the database field entirely.
pub fn credential_url(db_type: DbType, host: &str, port: u16) -> String {
    format!("database://{}/{host}:{port}", db_type.as_str())
}

/// Normalizes `params.database` for a server-level operation (discovery or
/// database creation) that needs *some* admin/maintenance database to connect
/// through, not a specific already-chosen one. Postgres has no "no database"
/// connect mode — an unset dbname makes the server default to one named after
/// the connecting user, which almost never exists, so it's defaulted to
/// `"postgres"` (every install's maintenance DB). MySQL and MSSQL both connect
/// fine with the field left empty, so they pass through.
fn normalize_admin_database(db_type: DbType, mut params: NetworkConnectParams) -> NetworkConnectParams {
    if db_type == DbType::Postgres && params.database.trim().is_empty() {
        params.database = "postgres".to_string();
    }
    params
}

/// Lists every database visible on the server `params` points at, dispatched
/// by `db_type`. `params.database` need not be set — see
/// [`normalize_admin_database`].
pub async fn list_databases(
    db_type: DbType,
    params: NetworkConnectParams,
) -> Result<Vec<String>, DatabaseError> {
    let params = normalize_admin_database(db_type, params);
    match db_type {
        DbType::Sqlite => Err(DatabaseError::Unsupported(
            "SQLite has no server-level database list".into(),
        )),
        DbType::Postgres => postgres::list_databases(params).await,
        DbType::MySql => mysql::list_databases(params).await,
        DbType::MsSql => mssql::list_databases(params).await,
    }
}

/// Creates a new database named `name` on the server `params` points at.
pub async fn create_database(
    db_type: DbType,
    params: NetworkConnectParams,
    name: &str,
) -> Result<(), DatabaseError> {
    let params = normalize_admin_database(db_type, params);
    match db_type {
        DbType::Sqlite => Err(DatabaseError::Unsupported(
            "SQLite has no server-level database creation".into(),
        )),
        DbType::Postgres => postgres::create_database(params, name).await,
        DbType::MySql => mysql::create_database(params, name).await,
        DbType::MsSql => mssql::create_database(params, name).await,
    }
}

/// A live driver connection, holding whatever handle type that driver needs.
///
/// SQLite/Postgres/MSSQL wrap their client in a mutex (none of those client
/// types tolerate concurrent access from multiple callers); `mysql_async`'s
/// `Pool` is already internally poolable/shareable, so it needs none.
/// Mirrors Forge's own `DbConn` enum shape.
enum DbConn {
    Sqlite(Arc<Mutex<SqliteConnection>>),
    Postgres(Arc<AsyncMutex<tokio_postgres::Client>>),
    MySql(mysql_async::Pool),
    MsSql(Arc<AsyncMutex<mssql::MsSqlClient>>),
}

/// Live-session registry, keyed by `(ConnectionId, String)` — the string being
/// the database within the connection that the live session was dialed for. A
/// single connection keeps several *independent* live sessions open at once,
/// one per dialed database; switching between already-connected databases
/// never re-dials. SQLite registers under the empty database name (a SQLite
/// connection *is* a single database file).
///
/// `active` records which database within each connection the
/// connection-level entry points (`fetch_schema`, `execute_query`) route to —
/// set whenever a database tab is selected, so re-selecting a connected
/// database never re-dials.
#[derive(Default)]
pub struct ConnectionRegistry {
    sessions: HashMap<(ConnectionId, String), DbConn>,
    active: HashMap<ConnectionId, String>,
}

impl ConnectionRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    fn register_first_database_as_active(&mut self, id: ConnectionId, database: &str) {
        if !self.active.contains_key(&id) {
            self.active.insert(id, database.to_string());
        }
    }

    /// Opens `path` as a SQLite connection and registers it under `id`. A
    /// SQLite connection *is* a single database file, so it registers one
    /// live session under the empty database name.
    pub fn connect_sqlite(&mut self, id: ConnectionId, path: &str) -> Result<(), DatabaseError> {
        let database = String::new();
        self.reject_if_database_already_connected(id, &database)?;
        let conn = sqlite::connect(path)?;
        self.sessions
            .insert((id, database.clone()), DbConn::Sqlite(Arc::new(Mutex::new(conn))));
        self.register_first_database_as_active(id, &database);
        Ok(())
    }

    /// Opens a PostgreSQL connection to `params.database` and registers it
    /// under `(id, params.database)`. If this is the connection's first
    /// dialed database it also becomes the active one. Async — callers run
    /// this on a real tokio runtime, never on GPUI's own executor.
    pub async fn connect_postgres(
        &mut self,
        id: ConnectionId,
        params: NetworkConnectParams,
    ) -> Result<(), DatabaseError> {
        self.reject_if_database_already_connected(id, &params.database)?;
        let database = params.database.clone();
        let client = postgres::connect(params).await?;
        self.sessions.insert(
            (id, database.clone()),
            DbConn::Postgres(Arc::new(AsyncMutex::new(client))),
        );
        self.register_first_database_as_active(id, &database);
        Ok(())
    }

    /// Opens a MySQL connection pool for `params.database`, registering it
    /// under `(id, params.database)`. Async, same runtime guidance as
    /// Postgres.
    pub async fn connect_mysql(
        &mut self,
        id: ConnectionId,
        params: NetworkConnectParams,
    ) -> Result<(), DatabaseError> {
        self.reject_if_database_already_connected(id, &params.database)?;
        let database = params.database.clone();
        let pool = mysql::connect(params).await?;
        self.sessions
            .insert((id, database.clone()), DbConn::MySql(pool));
        self.register_first_database_as_active(id, &database);
        Ok(())
    }

    /// Opens an MSSQL connection for `params.database`, registering it under
    /// `(id, params.database)`. Async, same runtime guidance as Postgres.
    pub async fn connect_mssql(
        &mut self,
        id: ConnectionId,
        params: NetworkConnectParams,
    ) -> Result<(), DatabaseError> {
        self.reject_if_database_already_connected(id, &params.database)?;
        let database = params.database.clone();
        let client = mssql::connect(params).await?;
        self.sessions.insert(
            (id, database.clone()),
            DbConn::MsSql(Arc::new(AsyncMutex::new(client))),
        );
        self.register_first_database_as_active(id, &database);
        Ok(())
    }

    /// True when `id` has at least one live session, across any database.
    pub fn is_connected(&self, id: ConnectionId) -> bool {
        self.sessions.keys().any(|(conn_id, _)| *conn_id == id)
    }

    /// True when the specific `database` on `id` has a live session.
    pub fn is_database_connected(&self, id: ConnectionId, database: &str) -> bool {
        self.sessions.contains_key(&(id, database.to_string()))
    }

    /// The database names that have live sessions on `id` (excluding the
    /// SQLite empty-database slot), sorted. Used by the panel's `+ Add
    /// Database` picker.
    pub fn live_databases(&self, id: ConnectionId) -> Vec<String> {
        let mut names: Vec<String> = self
            .sessions
            .keys()
            .filter(|(conn_id, db)| *conn_id == id && !db.is_empty())
            .map(|(_, db)| db.clone())
            .collect();
        names.sort();
        names
    }

    /// Routes the connection-level entry points to `database` on `id`. The
    /// panel calls this when a database tab is selected, so re-selecting an
    /// already-connected database never re-dials it.
    pub fn set_active_database(&mut self, id: ConnectionId, database: &str) {
        if self.sessions.contains_key(&(id, database.to_string())) {
            self.active.insert(id, database.to_string());
        }
    }

    /// The database name `id`'s connection-level entry points currently route
    /// to.
    pub fn active_database(&self, id: ConnectionId) -> Option<String> {
        self.active.get(&id).cloned()
    }

    fn reject_if_database_already_connected(
        &self,
        id: ConnectionId,
        database: &str,
    ) -> Result<(), DatabaseError> {
        if self.sessions.contains_key(&(id, database.to_string())) {
            return Err(DatabaseError::Connection(format!(
                "database {} is already connected on this connection",
                database
            )));
        }
        Ok(())
    }

    /// Closes and removes *every* live session registered under `id` (all its
    /// databases), leaving the connection fully disconnected. Dropping each
    /// `DbConn` closes whichever driver handle it holds.
    pub fn disconnect(&mut self, id: ConnectionId) -> Result<(), DatabaseError> {
        let removed: Vec<(ConnectionId, String)> = self
            .sessions
            .keys()
            .filter(|(conn_id, _)| *conn_id == id)
            .cloned()
            .collect();
        if removed.is_empty() {
            return Err(DatabaseError::NotFound(id));
        }
        for key in removed {
            self.sessions.remove(&key);
        }
        self.active.remove(&id);
        Ok(())
    }

    /// Closes just the live session for `database` on `id`, leaving any other
    /// still-connected databases on the same connection alive. If this was
    /// the connection's active database, another still-connected database
    /// (if any) becomes active.
    pub fn disconnect_database(&mut self, id: ConnectionId, database: &str) -> Result<(), DatabaseError> {
        let key = (id, database.to_string());
        if self.sessions.remove(&key).is_none() {
            return Err(DatabaseError::NotFound(id));
        }
        if self.active.get(&id).map(String::as_str) == Some(database) {
            self.active.remove(&id);
            if let Some(next) = self.live_databases(id).into_iter().next() {
                self.active.insert(id, next);
            }
        }
        Ok(())
    }

    /// Returns a cloned handle to the live SQLite connection registered under
    /// `id`, so callers (schema fetch, query) can run off the main thread
    /// without holding a live borrow of the registry. SQLite registers under
    /// the empty database name.
    pub fn sqlite_handle(&self, id: ConnectionId) -> Option<Arc<Mutex<SqliteConnection>>> {
        match self.sessions.get(&(id, String::new())) {
            Some(DbConn::Sqlite(conn)) => Some(conn.clone()),
            _ => None,
        }
    }

    /// Fetches `id`'s schema for its active database, dispatching to whatever
    /// driver that session is actually connected with — callers don't need
    /// their own per-type match.
    pub async fn fetch_schema(
        &self,
        id: ConnectionId,
    ) -> Result<Vec<crate::metadata::TableInfo>, DatabaseError> {
        let database = self
            .active_database(id)
            .ok_or(DatabaseError::NotFound(id))?;
        self.fetch_schema_database(id, &database).await
    }

    /// Fetches the schema of one specific live database session on `id`.
    pub async fn fetch_schema_database(
        &self,
        id: ConnectionId,
        database: &str,
    ) -> Result<Vec<crate::metadata::TableInfo>, DatabaseError> {
        match self.sessions.get(&(id, database.to_string())) {
            None => Err(DatabaseError::NotFound(id)),
            Some(DbConn::Sqlite(conn)) => {
                let conn = conn.lock().unwrap();
                sqlite::fetch_schema(&conn)
            }
            Some(DbConn::Postgres(client)) => {
                let client = client.lock().await;
                postgres::fetch_schema(&client).await
            }
            Some(DbConn::MySql(pool)) => mysql::fetch_schema(pool).await,
            Some(DbConn::MsSql(client)) => {
                let mut client = client.lock().await;
                mssql::fetch_schema(&mut client).await
            }
        }
    }

    /// Runs one ad-hoc SQL statement against `id`'s active database session —
    /// the SQL workbench's entry point.
    pub async fn execute_query(
        &self,
        id: ConnectionId,
        sql: &str,
    ) -> Result<crate::query::QueryResult, DatabaseError> {
        let database = self
            .active_database(id)
            .ok_or(DatabaseError::NotFound(id))?;
        self.execute_query_database(id, &database, sql).await
    }

    /// Runs one ad-hoc SQL statement against a specific live database session
    /// on `id`.
    pub async fn execute_query_database(
        &self,
        id: ConnectionId,
        database: &str,
        sql: &str,
    ) -> Result<crate::query::QueryResult, DatabaseError> {
        match self.sessions.get(&(id, database.to_string())) {
            None => Err(DatabaseError::NotFound(id)),
            Some(DbConn::Sqlite(conn)) => {
                let conn = conn.lock().unwrap();
                sqlite::execute_query(&conn, sql)
            }
            Some(DbConn::Postgres(client)) => {
                let client = client.lock().await;
                postgres::execute_query(&client, sql).await
            }
            Some(DbConn::MySql(pool)) => mysql::execute_query(pool, sql).await,
            Some(DbConn::MsSql(client)) => {
                let mut client = client.lock().await;
                mssql::execute_query(&mut client, sql).await
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_sqlite_file() -> tempfile::TempPath {
        let file = tempfile::NamedTempFile::new().unwrap();
        sqlite::connect(file.path().to_str().unwrap()).unwrap();
        file.into_temp_path()
    }

    #[test]
    fn connects_to_a_valid_sqlite_file() {
        let path = temp_sqlite_file();
        let mut registry = ConnectionRegistry::new();
        let id = ConnectionId(1);

        registry.connect_sqlite(id, path.to_str().unwrap()).unwrap();

        assert!(registry.is_connected(id));
        assert!(registry.is_database_connected(id, ""));
    }

    #[test]
    fn rejects_an_invalid_path() {
        let dir = tempfile::tempdir().unwrap();
        let missing_path = dir.path().join("does-not-exist.db");
        let mut registry = ConnectionRegistry::new();

        let result = registry.connect_sqlite(ConnectionId(1), missing_path.to_str().unwrap());

        assert!(result.is_err());
        assert!(!registry.is_connected(ConnectionId(1)));
    }

    #[test]
    fn rejects_a_non_sqlite_file() {
        let mut not_sqlite = tempfile::NamedTempFile::new().unwrap();
        std::io::Write::write_all(&mut not_sqlite, b"not a sqlite database").unwrap();
        let mut registry = ConnectionRegistry::new();

        let result = registry.connect_sqlite(ConnectionId(1), not_sqlite.path().to_str().unwrap());

        assert!(result.is_err());
    }

    #[test]
    fn rejects_connecting_a_duplicate_id() {
        let path = temp_sqlite_file();
        let mut registry = ConnectionRegistry::new();

        let id = ConnectionId(1);
        registry.connect_sqlite(id, path.to_str().unwrap()).unwrap();

        let result = registry.connect_sqlite(id, path.to_str().unwrap());

        assert!(result.is_err());
    }

    #[test]
    fn disconnect_removes_a_live_connection() {
        let path = temp_sqlite_file();
        let mut registry = ConnectionRegistry::new();

        let id = ConnectionId(1);
        registry.connect_sqlite(id, path.to_str().unwrap()).unwrap();

        registry.disconnect(id).unwrap();

        assert!(!registry.is_connected(id));
    }

    #[test]
    fn disconnecting_a_missing_connection_errors() {
        let mut registry = ConnectionRegistry::new();

        let result = registry.disconnect(ConnectionId(1));

        assert!(result.is_err());
    }

    #[test]
    fn sqlite_handle_is_available_after_connect() {
        let path = temp_sqlite_file();
        let mut registry = ConnectionRegistry::new();

        let id = ConnectionId(1);
        registry.connect_sqlite(id, path.to_str().unwrap()).unwrap();

        let handle = registry.sqlite_handle(id);

        assert!(handle.is_some());
    }

    #[test]
    fn sqlite_handle_missing_when_not_connected() {
        let registry = ConnectionRegistry::new();

        assert!(registry.sqlite_handle(ConnectionId(1)).is_none());
    }

    #[test]
    fn live_databases_tracks_the_dialed_database() {
        let path = temp_sqlite_file();
        let mut registry = ConnectionRegistry::new();

        let id = ConnectionId(1);
        registry.connect_sqlite(id, path.to_str().unwrap()).unwrap();

        let dbs = registry.live_databases(id);

        assert!(dbs.is_empty());
        assert!(registry.active_database(id).is_some());
    }
}
