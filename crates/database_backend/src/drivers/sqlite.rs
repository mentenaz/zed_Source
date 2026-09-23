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
use crate::metadata::{ColumnInfo, ForeignKey, TableInfo};

/// Wraps an identifier for interpolation into a PRAGMA statement.
///
/// SQLite's PRAGMA statements don't accept bound parameters (`?`) for object
/// names, only literal SQL — so table names must be inlined. They come from
/// `sqlite_master` itself, not external input, so doubling embedded `"` is
/// enough identifier-escaping for this trust boundary (the same one Forge's
/// own backend uses for its PRAGMA calls).
fn quote_identifier(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

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

/// Discovers every user table's columns and foreign keys.
///
/// Internal `sqlite_` tables (e.g. `sqlite_sequence`) are excluded — they're
/// SQLite bookkeeping, not part of the user's schema.
pub fn fetch_schema(conn: &Connection) -> Result<Vec<TableInfo>, DatabaseError> {
    let table_names = list_table_names(conn)?;

    table_names
        .into_iter()
        .map(|name| {
            let columns = table_columns(conn, &name)?;
            let foreign_keys = table_foreign_keys(conn, &name)?;
            Ok(TableInfo {
                name,
                columns,
                foreign_keys,
            })
        })
        .collect()
}

fn list_table_names(conn: &Connection) -> Result<Vec<String>, DatabaseError> {
    let mut stmt = conn
        .prepare(
            "SELECT name FROM sqlite_master \
             WHERE type = 'table' AND name NOT LIKE 'sqlite\\_%' ESCAPE '\\' \
             ORDER BY name",
        )
        .map_err(|err| DatabaseError::Connection(format!("failed to list tables: {err}")))?;

    stmt.query_map([], |row| row.get::<_, String>(0))
        .and_then(Iterator::collect)
        .map_err(|err| DatabaseError::Connection(format!("failed to list tables: {err}")))
}

fn table_columns(conn: &Connection, table: &str) -> Result<Vec<ColumnInfo>, DatabaseError> {
    let sql = format!("PRAGMA table_info({})", quote_identifier(table));
    let mut stmt = stmt_for(conn, &sql, table, "column info")?;

    stmt.query_map([], |row| {
        let notnull: i64 = row.get("notnull")?;
        let pk: i64 = row.get("pk")?;
        Ok(ColumnInfo {
            name: row.get("name")?,
            type_name: row.get("type")?,
            nullable: notnull == 0,
            primary_key: pk > 0,
        })
    })
    .and_then(Iterator::collect)
    .map_err(|err| {
        DatabaseError::Connection(format!("failed to read columns for {table}: {err}"))
    })
}

fn table_foreign_keys(conn: &Connection, table: &str) -> Result<Vec<ForeignKey>, DatabaseError> {
    let sql = format!("PRAGMA foreign_key_list({})", quote_identifier(table));
    let mut stmt = stmt_for(conn, &sql, table, "foreign key list")?;

    stmt.query_map([], |row| {
        Ok(ForeignKey {
            from_column: row.get("from")?,
            to_table: row.get("table")?,
            to_column: row.get("to")?,
        })
    })
    .and_then(Iterator::collect)
    .map_err(|err| {
        DatabaseError::Connection(format!("failed to read foreign keys for {table}: {err}"))
    })
}

fn stmt_for<'a>(
    conn: &'a Connection,
    sql: &str,
    table: &str,
    what: &str,
) -> Result<rusqlite::Statement<'a>, DatabaseError> {
    conn.prepare(sql)
        .map_err(|err| DatabaseError::Connection(format!("failed to read {what} for {table}: {err}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn schema_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE authors (
                 id INTEGER PRIMARY KEY,
                 name TEXT NOT NULL
             );
             CREATE TABLE posts (
                 id INTEGER PRIMARY KEY,
                 title TEXT NOT NULL,
                 author_id INTEGER,
                 published_at TEXT,
                 FOREIGN KEY (author_id) REFERENCES authors(id)
             );",
        )
        .unwrap();
        conn
    }

    #[test]
    fn fetches_tables_in_name_order_excluding_internal_tables() {
        let conn = schema_conn();

        let tables = fetch_schema(&conn).unwrap();

        let names: Vec<&str> = tables.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, vec!["authors", "posts"]);
    }

    #[test]
    fn reads_column_shape_including_primary_key_and_nullability() {
        let conn = schema_conn();

        let tables = fetch_schema(&conn).unwrap();
        let authors = tables.iter().find(|t| t.name == "authors").unwrap();

        let id = authors.columns.iter().find(|c| c.name == "id").unwrap();
        assert!(id.primary_key);

        let name = authors.columns.iter().find(|c| c.name == "name").unwrap();
        assert!(!name.primary_key);
        assert!(!name.nullable);

        let posts = tables.iter().find(|t| t.name == "posts").unwrap();
        let author_id = posts
            .columns
            .iter()
            .find(|c| c.name == "author_id")
            .unwrap();
        assert!(author_id.nullable);
    }

    #[test]
    fn reads_foreign_keys() {
        let conn = schema_conn();

        let tables = fetch_schema(&conn).unwrap();
        let posts = tables.iter().find(|t| t.name == "posts").unwrap();

        assert_eq!(posts.foreign_keys.len(), 1);
        let fk = &posts.foreign_keys[0];
        assert_eq!(fk.from_column, "author_id");
        assert_eq!(fk.to_table, "authors");
        assert_eq!(fk.to_column, "id");
    }

    #[test]
    fn empty_database_has_no_tables() {
        let conn = Connection::open_in_memory().unwrap();

        let tables = fetch_schema(&conn).unwrap();

        assert!(tables.is_empty());
    }
}
