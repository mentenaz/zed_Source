use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use rusqlite::Connection as SqliteConnection;
use serde::{Deserialize, Serialize};

use crate::drivers::sqlite;
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

/// Live-connection registry, keyed by [`ConnectionId`].
///
/// SQLite connections are synchronous (`rusqlite`) and not `Sync`, so each
/// one is wrapped in a `Mutex`, matching Forge's own `Arc<Mutex<Connection>>`
/// shape for its SQLite driver.
#[derive(Default)]
pub struct ConnectionRegistry {
    sqlite: HashMap<ConnectionId, Arc<Mutex<SqliteConnection>>>,
}

impl ConnectionRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Opens `path` as a SQLite connection and registers it under `id`.
    ///
    /// Errors if `id` already has a live connection — callers must
    /// `disconnect` first, so a stale handle is never silently replaced.
    pub fn connect_sqlite(&mut self, id: ConnectionId, path: &str) -> Result<(), DatabaseError> {
        if self.sqlite.contains_key(&id) {
            return Err(DatabaseError::Connection(format!(
                "{id:?} is already connected"
            )));
        }

        let conn = sqlite::connect(path)?;
        self.sqlite.insert(id, Arc::new(Mutex::new(conn)));
        Ok(())
    }

    /// Closes and removes the connection registered under `id`.
    pub fn disconnect(&mut self, id: ConnectionId) -> Result<(), DatabaseError> {
        self.sqlite
            .remove(&id)
            .map(|_| ())
            .ok_or(DatabaseError::NotFound(id))
    }

    pub fn is_connected(&self, id: ConnectionId) -> bool {
        self.sqlite.contains_key(&id)
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
}
