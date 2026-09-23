//! Database panel backend.
//!
//! See `crates/gpui_component/DATABASE_PANEL_SPEC.md` for the full
//! architecture and phased delivery plan. This crate must never depend on
//! `gpui_component` — UI concerns live in `database_panel`.
//!
//! Credentials are read and written exclusively through
//! `zed_credentials_provider::global`/`credentials_provider::CredentialsProvider`;
//! this crate holds no credential storage of its own. Confirmed reachable
//! from this crate by the `credentials_provider`/`zed_credentials_provider`
//! dependencies below compiling against the rest of the workspace.

pub mod connection;
pub mod drivers;
pub mod errors;
pub mod metadata;
pub mod models;
pub mod query;

pub use connection::{
    ConnectionId, ConnectionRegistry, ConnectionStatus, DatabaseId, NetworkConnectParams,
    create_database, credential_url, list_databases,
};
pub use drivers::DbType;
pub use errors::DatabaseError;
pub use metadata::{ColumnInfo, ForeignKey, TableInfo};
pub use models::{ConnectionConfig, SavedDatabase};
pub use query::QueryResult;
