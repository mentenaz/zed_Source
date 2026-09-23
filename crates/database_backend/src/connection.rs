use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use rusqlite::Connection as SqliteConnection;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex as AsyncMutex;

use crate::drivers::{DbType, mssql, mysql, postgres, sqlite};
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

/// Builds the credential-store lookup key for a network connection, per the
/// spec's documented format. Connection-level, not per-database — a
/// connection's password is the same regardless of which database within it
/// is selected, so per-database credential scoping is left for a later
/// phase (if it ever becomes relevant) rather than added speculatively here.
pub fn credential_url(db_type: DbType, host: &str, port: u16) -> String {
    format!("database://{}/{host}:{port}", db_type.as_str())
}

/// Normalizes `params.database` for a server-level operation (discovery or
/// database creation) that needs *some* admin/maintenance database to
/// connect through, not a specific already-chosen one.
///
/// Postgres has no "no database" connect mode — an unset dbname makes the
/// server default to one named after the connecting user, which almost
/// never exists, so it's defaulted to `"postgres"` (every install's
/// maintenance DB) here. MySQL and MSSQL both connect fine with the field
/// left empty (MySQL: no default DB selected; MSSQL: falls back to the
/// login's configured default DB), so they're passed through unchanged.
fn admin_params(db_type: DbType, mut params: NetworkConnectParams) -> NetworkConnectParams {
    if db_type == DbType::Postgres && params.database.trim().is_empty() {
        params.database = "postgres".to_string();
    }
    params
}

/// Lists every database visible on the server `params` points at, dispatched
/// by `db_type`. `params.database` need not be set — see [`admin_params`].
pub async fn list_databases(
    db_type: DbType,
    params: NetworkConnectParams,
) -> Result<Vec<String>, DatabaseError> {
    let params = admin_params(db_type, params);
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
    let params = admin_params(db_type, params);
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
/// Mirrors Forge's own `DbConn` enum shape (confirmed via the earlier survey
/// of its Rust backend).
enum DbConn {
    Sqlite(Arc<Mutex<SqliteConnection>>),
    Postgres(Arc<AsyncMutex<tokio_postgres::Client>>),
    MySql(mysql_async::Pool),
    MsSql(Arc<AsyncMutex<mssql::MsSqlClient>>),
}

/// Live-connection registry, keyed by [`ConnectionId`].
#[derive(Default)]
pub struct ConnectionRegistry {
    connections: HashMap<ConnectionId, DbConn>,
}

impl ConnectionRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    fn reject_if_already_connected(&self, id: ConnectionId) -> Result<(), DatabaseError> {
        if self.connections.contains_key(&id) {
            return Err(DatabaseError::Connection(format!(
                "{id:?} is already connected"
            )));
        }
        Ok(())
    }

    /// Opens `path` as a SQLite connection and registers it under `id`.
    ///
    /// Errors if `id` already has a live connection — callers must
    /// `disconnect` first, so a stale handle is never silently replaced.
    pub fn connect_sqlite(&mut self, id: ConnectionId, path: &str) -> Result<(), DatabaseError> {
        self.reject_if_already_connected(id)?;
        let conn = sqlite::connect(path)?;
        self.connections
            .insert(id, DbConn::Sqlite(Arc::new(Mutex::new(conn))));
        Ok(())
    }

    /// Opens a PostgreSQL connection and registers it under `id`. Async —
    /// callers run this on a real tokio runtime (`gpui_tokio::Tokio::spawn_result`),
    /// never on GPUI's own executor.
    pub async fn connect_postgres(
        &mut self,
        id: ConnectionId,
        params: NetworkConnectParams,
    ) -> Result<(), DatabaseError> {
        self.reject_if_already_connected(id)?;
        let client = postgres::connect(params).await?;
        self.connections
            .insert(id, DbConn::Postgres(Arc::new(AsyncMutex::new(client))));
        Ok(())
    }

    /// Opens a MySQL/MariaDB connection pool and registers it under `id`.
    pub async fn connect_mysql(
        &mut self,
        id: ConnectionId,
        params: NetworkConnectParams,
    ) -> Result<(), DatabaseError> {
        self.reject_if_already_connected(id)?;
        let pool = mysql::connect(params).await?;
        self.connections.insert(id, DbConn::MySql(pool));
        Ok(())
    }

    /// Opens an MSSQL connection and registers it under `id`.
    pub async fn connect_mssql(
        &mut self,
        id: ConnectionId,
        params: NetworkConnectParams,
    ) -> Result<(), DatabaseError> {
        self.reject_if_already_connected(id)?;
        let client = mssql::connect(params).await?;
        self.connections
            .insert(id, DbConn::MsSql(Arc::new(AsyncMutex::new(client))));
        Ok(())
    }

    /// Closes and removes the connection registered under `id`. Dropping the
    /// `DbConn` closes whichever driver's handle it holds.
    pub fn disconnect(&mut self, id: ConnectionId) -> Result<(), DatabaseError> {
        self.connections
            .remove(&id)
            .map(|_| ())
            .ok_or(DatabaseError::NotFound(id))
    }

    pub fn is_connected(&self, id: ConnectionId) -> bool {
        self.connections.contains_key(&id)
    }

    /// Returns a cloned handle to the live SQLite connection registered
    /// under `id`, if any, so callers (e.g. a schema fetch) can run queries
    /// off the main thread without holding a live borrow of the registry.
    pub fn sqlite_handle(&self, id: ConnectionId) -> Option<Arc<Mutex<SqliteConnection>>> {
        match self.connections.get(&id) {
            Some(DbConn::Sqlite(conn)) => Some(conn.clone()),
            _ => None,
        }
    }

    /// Fetches `id`'s schema, dispatching to whichever driver it's actually
    /// connected with — callers don't need their own per-type match.
    pub async fn fetch_schema(
        &self,
        id: ConnectionId,
    ) -> Result<Vec<crate::metadata::TableInfo>, DatabaseError> {
        match self.connections.get(&id) {
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
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_sqlite_file() -> tempfile::TempPath {
        let file = tempfile::NamedTempFile::new().unwrap();
        rusqlite::Connection::open(file.path())
            .unwrap()
            .execute_batch("CREATE TABLE t (id INTEGER PRIMARY KEY);")
            .unwrap();
        file.into_temp_path()
    }

    #[test]
    fn connects_to_a_valid_sqlite_file() {
        let path = temp_sqlite_file();
        let mut registry = ConnectionRegistry::new();
        let id = ConnectionId(1);

        registry
            .connect_sqlite(id, path.to_str().unwrap())
            .unwrap();

        assert!(registry.is_connected(id));
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

        let result =
            registry.connect_sqlite(ConnectionId(1), not_sqlite.path().to_str().unwrap());

        assert!(result.is_err());
    }

    #[test]
    fn rejects_connecting_a_duplicate_id() {
        let path = temp_sqlite_file();
        let mut registry = ConnectionRegistry::new();
        let id = ConnectionId(1);
        registry
            .connect_sqlite(id, path.to_str().unwrap())
            .unwrap();

        let result = registry.connect_sqlite(id, path.to_str().unwrap());

        assert!(result.is_err());
    }

    #[test]
    fn disconnect_removes_a_live_connection() {
        let path = temp_sqlite_file();
        let mut registry = ConnectionRegistry::new();
        let id = ConnectionId(1);
        registry
            .connect_sqlite(id, path.to_str().unwrap())
            .unwrap();

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
    fn credential_url_is_connection_level_not_per_database() {
        assert_eq!(
            credential_url(DbType::Postgres, "db.example.com", 5432),
            "database://postgres/db.example.com:5432"
        );
        assert_eq!(
            credential_url(DbType::MsSql, "localhost", 1433),
            "database://mssql/localhost:1433"
        );
    }
}
