use serde::{Deserialize, Serialize};

pub mod mssql;
pub mod mysql;
pub mod postgres;
pub mod sqlite;

/// The database types the panel supports, per the spec's goals.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DbType {
    Sqlite,
    Postgres,
    MySql,
    MsSql,
}
