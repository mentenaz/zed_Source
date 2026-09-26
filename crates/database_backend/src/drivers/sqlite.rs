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
use crate::metadata::{ColumnInfo, ForeignKey, IndexInfo, Schema, TableInfo, ViewInfo};
use crate::query::QueryResult;

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

/// Discovers every user table's columns/foreign keys/indexes, and every
/// view's columns and defining SQL.
///
/// Internal `sqlite_` tables (e.g. `sqlite_sequence`) are excluded — they're
/// SQLite bookkeeping, not part of the user's schema.
pub fn fetch_schema(conn: &Connection) -> Result<Schema, DatabaseError> {
    let tables = list_table_names(conn)?
        .into_iter()
        .map(|name| {
            let columns = table_columns(conn, &name)?;
            let foreign_keys = table_foreign_keys(conn, &name)?;
            let indexes = table_indexes(conn, &name)?;
            Ok(TableInfo {
                name,
                columns,
                foreign_keys,
                indexes,
            })
        })
        .collect::<Result<Vec<_>, DatabaseError>>()?;

    let views = list_views(conn)?
        .into_iter()
        .map(|(name, definition)| {
            let columns = table_columns(conn, &name)?;
            Ok(ViewInfo { name, columns, definition: definition.unwrap_or_default() })
        })
        .collect::<Result<Vec<_>, DatabaseError>>()?;

    Ok(Schema { tables, views })
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

/// Every view's name paired with its defining SQL (`sqlite_master.sql`, the
/// literal `CREATE VIEW ...` statement — `None` only for an internal view
/// SQLite itself created without one, which doesn't happen for user views).
fn list_views(conn: &Connection) -> Result<Vec<(String, Option<String>)>, DatabaseError> {
    let mut stmt = conn
        .prepare("SELECT name, sql FROM sqlite_master WHERE type = 'view' ORDER BY name")
        .map_err(|err| DatabaseError::Connection(format!("failed to list views: {err}")))?;

    stmt.query_map([], |row| Ok((row.get::<_, String>("name")?, row.get::<_, Option<String>>("sql")?)))
        .and_then(Iterator::collect)
        .map_err(|err| DatabaseError::Connection(format!("failed to list views: {err}")))
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

/// `PRAGMA index_list` names every index on `table` (including the implicit
/// one backing a `PRIMARY KEY`/`UNIQUE` constraint) plus whether it's unique;
/// `PRAGMA index_info` then gives each index's columns in key order.
fn table_indexes(conn: &Connection, table: &str) -> Result<Vec<IndexInfo>, DatabaseError> {
    let sql = format!("PRAGMA index_list({})", quote_identifier(table));
    let mut stmt = stmt_for(conn, &sql, table, "index list")?;
    let indexes: Vec<(String, bool)> = stmt
        .query_map([], |row| {
            let unique: i64 = row.get("unique")?;
            Ok((row.get::<_, String>("name")?, unique != 0))
        })
        .and_then(Iterator::collect)
        .map_err(|err| DatabaseError::Connection(format!("failed to read indexes for {table}: {err}")))?;

    indexes
        .into_iter()
        .map(|(name, unique)| {
            let columns = index_columns(conn, &name)?;
            Ok(IndexInfo { name, columns, unique })
        })
        .collect()
}

fn index_columns(conn: &Connection, index_name: &str) -> Result<Vec<String>, DatabaseError> {
    let sql = format!("PRAGMA index_info({})", quote_identifier(index_name));
    let mut stmt = stmt_for(conn, &sql, index_name, "index info")?;
    // An expression index's column entry has a NULL name.
    stmt.query_map([], |row| {
        Ok(row
            .get::<_, Option<String>>("name")?
            .unwrap_or_else(|| "<expression>".to_string()))
    })
    .and_then(Iterator::collect)
    .map_err(|err| {
        DatabaseError::Connection(format!("failed to read index columns for {index_name}: {err}"))
    })
}

/// Runs one ad-hoc SQL statement — the SQL workbench's entry point.
///
/// `PRAGMA` statements aside, a non-row-returning statement (INSERT/UPDATE/
/// DELETE/DDL) has zero result *columns*, not just zero rows — that's how
/// this tells "wrote N rows" apart from "a SELECT matched nothing" (which
/// still has columns, just an empty `rows` vec).
pub fn execute_query(conn: &Connection, sql: &str) -> Result<QueryResult, DatabaseError> {
    let start = std::time::Instant::now();
    let mut stmt = conn
        .prepare(sql)
        .map_err(|err| DatabaseError::Connection(format!("SQL error: {err}")))?;
    let column_count = stmt.column_count();
    let columns: Vec<String> = (0..column_count)
        .map(|i| stmt.column_name(i).unwrap_or("?").to_string())
        .collect();

    let mut rows = Vec::new();
    let mut rows_iter = stmt
        .query([])
        .map_err(|err| DatabaseError::Connection(format!("SQL error: {err}")))?;
    while let Some(row) = rows_iter
        .next()
        .map_err(|err| DatabaseError::Connection(format!("SQL error: {err}")))?
    {
        rows.push((0..column_count).map(|i| sqlite_cell(row, i)).collect());
    }
    drop(rows_iter);
    drop(stmt);

    let rows_affected = if column_count == 0 {
        Some(conn.changes())
    } else {
        None
    };

    Ok(QueryResult {
        columns,
        rows,
        rows_affected,
        exec_ms: start.elapsed().as_millis() as u64,
    })
}

fn sqlite_cell(row: &rusqlite::Row, i: usize) -> Option<String> {
    use rusqlite::types::ValueRef;
    match row.get_ref(i).ok()? {
        ValueRef::Null => None,
        ValueRef::Integer(v) => Some(v.to_string()),
        ValueRef::Real(v) => Some(v.to_string()),
        ValueRef::Text(v) => Some(String::from_utf8_lossy(v).into_owned()),
        ValueRef::Blob(_) => Some("[BLOB]".to_string()),
    }
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
             );
             CREATE UNIQUE INDEX idx_posts_title ON posts(title);
             CREATE INDEX idx_posts_author_published ON posts(author_id, published_at);",
        )
        .unwrap();
        conn
    }

    #[test]
    fn fetches_tables_in_name_order_excluding_internal_tables() {
        let conn = schema_conn();

        let tables = fetch_schema(&conn).unwrap();

        let names: Vec<&str> = tables.tables.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, vec!["authors", "posts"]);
    }

    #[test]
    fn reads_column_shape_including_primary_key_and_nullability() {
        let conn = schema_conn();

        let schema = fetch_schema(&conn).unwrap();
        let authors = schema.tables.iter().find(|t| t.name == "authors").unwrap();

        let id = authors.columns.iter().find(|c| c.name == "id").unwrap();
        assert!(id.primary_key);

        let name = authors.columns.iter().find(|c| c.name == "name").unwrap();
        assert!(!name.primary_key);
        assert!(!name.nullable);

        let posts = schema.tables.iter().find(|t| t.name == "posts").unwrap();
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

        let schema = fetch_schema(&conn).unwrap();
        let posts = schema.tables.iter().find(|t| t.name == "posts").unwrap();

        assert_eq!(posts.foreign_keys.len(), 1);
        let fk = &posts.foreign_keys[0];
        assert_eq!(fk.from_column, "author_id");
        assert_eq!(fk.to_table, "authors");
        assert_eq!(fk.to_column, "id");
    }

    #[test]
    fn reads_indexes_including_composite_column_order() {
        let conn = schema_conn();

        let schema = fetch_schema(&conn).unwrap();
        let posts = schema.tables.iter().find(|t| t.name == "posts").unwrap();

        let title_index = posts
            .indexes
            .iter()
            .find(|index| index.name == "idx_posts_title")
            .unwrap();
        assert!(title_index.unique);
        assert_eq!(title_index.columns, vec!["title"]);

        let composite_index = posts
            .indexes
            .iter()
            .find(|index| index.name == "idx_posts_author_published")
            .unwrap();
        assert!(!composite_index.unique);
        assert_eq!(composite_index.columns, vec!["author_id", "published_at"]);
    }

    #[test]
    fn empty_database_has_no_tables() {
        let conn = Connection::open_in_memory().unwrap();

        let schema = fetch_schema(&conn).unwrap();

        assert!(schema.tables.is_empty());
        assert!(schema.views.is_empty());
    }

    #[test]
    fn reads_view_columns_and_definition() {
        let conn = schema_conn();
        conn.execute_batch(
            "CREATE VIEW recent_posts AS SELECT id, title FROM posts WHERE published_at IS NOT NULL;",
        )
        .unwrap();

        let schema = fetch_schema(&conn).unwrap();

        assert_eq!(schema.views.len(), 1);
        let view = &schema.views[0];
        assert_eq!(view.name, "recent_posts");
        assert!(view.definition.contains("SELECT id, title FROM posts"));
        let names: Vec<&str> = view.columns.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, vec!["id", "title"]);
    }
}
