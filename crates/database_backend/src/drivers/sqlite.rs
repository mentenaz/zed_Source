//! SQLite driver.
//!
//! Uses `rusqlite` rather than `crates/sqlez` (Zed's own internal SQLite
//! wrapper): `sqlez` is built around compile-time typed row-binding for
//! Zed's own known schemas (settings/kvp storage), with no dynamic
//! "enumerate an arbitrary table's columns at runtime" API. This panel
//! needs exactly that from day one for schema introspection — the same
//! reason Forge's Rust backend (`src-tauri/src/database.rs`) uses
//! `rusqlite`'s `Statement::column_names()` and generic `Value` row reads
//! rather than a typed ORM-style wrapper.

use rusqlite::{Connection, OpenFlags};

use crate::errors::DatabaseError;

/// Opens `path` and validates it is a readable SQLite database.
///
/// Deliberately does not pass `SQLITE_OPEN_CREATE` (`Connection::open`'s
/// default) — this is a "connect to my existing database" flow, and silently
/// creating an empty file at a typo'd path would be a surprising, hard-to-
/// notice failure mode rather than a clear error.
///
/// `Connection::open` alone also does not fail on a non-SQLite or corrupt
/// file — SQLite validates the file format lazily, on first real access.
/// Running a trivial query immediately turns that into an eager, reportable
/// error instead of a connection that silently misbehaves on first real use.
pub fn connect(path: &str) -> Result<Connection, DatabaseError> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|err| DatabaseError::Connection(format!("failed to open {path}: {err}")))?;

    conn.pragma_query_value(None, "schema_version", |_row| Ok(()))
        .map_err(|err| {
            DatabaseError::Connection(format!("{path} is not a valid SQLite database: {err}"))
        })?;

    Ok(conn)
}
