use serde::{Deserialize, Serialize};

use crate::connection::{ConnectionId, DatabaseId};
use crate::drivers::DbType;

/// Non-secret connection metadata, safe to persist to workspace/app JSON.
///
/// Passwords and other secrets never live on this struct — they are read
/// and written exclusively through `zed_credentials_provider`, keyed by the
/// normalized identifier documented in the spec
/// (`database://<type>/<host>:<port>/<database>`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectionConfig {
    pub id: ConnectionId,
    pub title: String,
    pub db_type: DbType,
    /// Present for network drivers (Postgres/MySQL/MSSQL); absent for SQLite.
    pub host: Option<String>,
    pub port: Option<u16>,
    /// Present for network drivers, used both to connect and as part of the
    /// credential lookup key.
    pub username: Option<String>,
    /// Present for SQLite, absent for network drivers.
    pub sqlite_path: Option<String>,
    pub ssl: bool,
    pub databases: Vec<SavedDatabase>,
}

/// A database known to exist under a connection, as last discovered.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedDatabase {
    pub id: DatabaseId,
    pub name: String,
}
