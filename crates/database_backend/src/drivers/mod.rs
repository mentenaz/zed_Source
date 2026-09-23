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

impl DbType {
    /// Stable identifier used in credential-store keys and dropdown values —
    /// distinct from any user-facing label so a future label wording change
    /// can't silently orphan saved credentials.
    pub fn as_str(self) -> &'static str {
        match self {
            DbType::Sqlite => "sqlite",
            DbType::Postgres => "postgres",
            DbType::MySql => "mysql",
            DbType::MsSql => "mssql",
        }
    }
}
