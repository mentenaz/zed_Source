use serde::{Deserialize, Serialize};

use crate::connection::{ConnectionId, DatabaseId};
use crate::drivers::DbType;

/// Non-secret connection metadata, safe to persist to workspace/app JSON.
///
/// Passwords and other secrets never live on this struct — they are read
/// and written exclusively through `zed_credentials_provider`, keyed by the
/// connection-level identifier `connection::credential_url` builds
/// (`database://<type>/<host>:<port>` — not per-database; a connection's
/// password is the same regardless of which database within it is picked).
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
    /// The specific database/catalog to connect to within a network server
    /// (e.g. Postgres requires one at connect time); absent for SQLite,
    /// where the file itself is the single database, and absent for a
    /// network connection whose server-level default database is fine.
    pub database: Option<String>,
    pub ssl: bool,
    pub databases: Vec<SavedDatabase>,
}

/// A database known to exist under a connection, as last discovered.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedDatabase {
    pub id: DatabaseId,
    pub name: String,
}
