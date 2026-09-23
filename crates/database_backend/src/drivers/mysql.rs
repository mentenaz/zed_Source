//! MySQL/MariaDB driver.
//!
//! `mysql_async::Pool` is already poolable/shareable on its own — unlike
//! the other three drivers, no `Arc<Mutex<_>>` wrapper is needed in
//! `ConnectionRegistry`.

use mysql_async::prelude::Queryable;
use mysql_async::{OptsBuilder, Pool, SslOpts, params};

use crate::connection::NetworkConnectParams;
use crate::errors::DatabaseError;
use crate::metadata::{ColumnInfo, ForeignKey, TableInfo};
use crate::query::QueryResult;

/// Opens a MySQL/MariaDB connection pool and proves it works with a trivial
/// query — matching every other driver's eager-validation behavior (a
/// `Pool` alone doesn't actually open a connection until first use).
pub async fn connect(params: NetworkConnectParams) -> Result<Pool, DatabaseError> {
    let mut opts = OptsBuilder::default()
        .ip_or_hostname(params.host)
        .tcp_port(params.port)
        // An empty dbname is a legitimate "no default database yet" connect
        // (used for server-level discovery before a database is picked) —
        // sending it as `Some("")` instead of `None` makes the server reject
        // the `USE` with an empty identifier, so only set it when non-empty.
        .db_name(if params.database.is_empty() {
            None
        } else {
            Some(params.database)
        })
        .user(Some(params.username))
        .pass(Some(params.password));

    if params.ssl {
        opts = opts.ssl_opts(Some(SslOpts::default()));
    }

    let pool = Pool::new(opts);

    let mut conn = pool
        .get_conn()
        .await
        .map_err(|e| DatabaseError::Connection(format!("failed to connect: {e}")))?;
    conn.query_drop("SELECT 1")
        .await
        .map_err(|e| DatabaseError::Connection(format!("connection check failed: {e}")))?;
    drop(conn);

    Ok(pool)
}

/// Lists every database visible on the server. `params.database` should be
/// left empty by callers doing server-level discovery — MySQL, unlike
/// Postgres, connects fine with no default database selected.
pub async fn list_databases(params: NetworkConnectParams) -> Result<Vec<String>, DatabaseError> {
    let pool = connect(params).await?;
    let mut conn = pool
        .get_conn()
        .await
        .map_err(|e| DatabaseError::Connection(format!("failed to get connection: {e}")))?;
    let names: Vec<String> = conn
        .query("SHOW DATABASES")
        .await
        .map_err(|e| DatabaseError::Connection(format!("failed to list databases: {e}")))?;
    drop(conn);
    pool.disconnect()
        .await
        .map_err(|e| DatabaseError::Connection(format!("failed to close connection: {e}")))?;
    Ok(names)
}

/// Creates a new database on the server `params` is already pointed at.
pub async fn create_database(params: NetworkConnectParams, name: &str) -> Result<(), DatabaseError> {
    let pool = connect(params).await?;
    let mut conn = pool
        .get_conn()
        .await
        .map_err(|e| DatabaseError::Connection(format!("failed to get connection: {e}")))?;
    conn.query_drop(format!("CREATE DATABASE `{}`", name.replace('`', "``")))
        .await
        .map_err(|e| DatabaseError::Connection(format!("failed to create database: {e}")))?;
    drop(conn);
    pool.disconnect()
        .await
        .map_err(|e| DatabaseError::Connection(format!("failed to close connection: {e}")))?;
    Ok(())
}

/// Runs one ad-hoc SQL statement — the SQL workbench's entry point.
///
/// Unlike Postgres/MSSQL, `mysql_async` has no single call that uniformly
/// hands back either a result set or an affected-row count — a write
/// statement run through the row-returning path errors instead of just
/// returning zero rows. So the statement's leading keyword decides which
/// path to take, matching Forge's own `db_query` (confirmed via
/// `src-tauri/src/database.rs`).
pub async fn execute_query(pool: &Pool, sql: &str) -> Result<QueryResult, DatabaseError> {
    let start = std::time::Instant::now();
    let mut conn = pool
        .get_conn()
        .await
        .map_err(|e| DatabaseError::Connection(format!("SQL error: {e}")))?;

    let upper = sql.trim_start().to_ascii_uppercase();
    let is_read = upper.starts_with("SELECT")
        || upper.starts_with("WITH")
        || upper.starts_with("EXPLAIN")
        || upper.starts_with("SHOW")
        || upper.starts_with("DESCRIBE")
        || upper.starts_with("DESC ");

    if is_read {
        let mut result = conn
            .query_iter(sql)
            .await
            .map_err(|e| DatabaseError::Connection(format!("SQL error: {e}")))?;
        let columns: Vec<String> = result
            .columns()
            .as_deref()
            .unwrap_or_default()
            .iter()
            .map(|c| c.name_str().to_string())
            .collect();
        let raw: Vec<mysql_async::Row> = result
            .collect()
            .await
            .map_err(|e| DatabaseError::Connection(format!("SQL error: {e}")))?;
        let rows = raw
            .into_iter()
            .map(|mut row| {
                (0..row.len())
                    .map(|i| mysql_cell(row.take(i).unwrap_or(mysql_async::Value::NULL)))
                    .collect()
            })
            .collect();

        Ok(QueryResult {
            columns,
            rows,
            rows_affected: None,
            exec_ms: start.elapsed().as_millis() as u64,
        })
    } else {
        conn.query_drop(sql)
            .await
            .map_err(|e| DatabaseError::Connection(format!("SQL error: {e}")))?;
        let rows_affected = conn.affected_rows();

        Ok(QueryResult {
            columns: Vec::new(),
            rows: Vec::new(),
            rows_affected: Some(rows_affected),
            exec_ms: start.elapsed().as_millis() as u64,
        })
    }
}

fn mysql_cell(value: mysql_async::Value) -> Option<String> {
    use mysql_async::Value;
    match value {
        Value::NULL => None,
        Value::Bytes(bytes) => Some(String::from_utf8_lossy(&bytes).into_owned()),
        Value::Int(n) => Some(n.to_string()),
        Value::UInt(n) => Some(n.to_string()),
        Value::Float(f) => Some(f.to_string()),
        Value::Double(d) => Some(d.to_string()),
        Value::Date(year, month, day, hour, min, sec, _) => Some(format!(
            "{year:04}-{month:02}-{day:02} {hour:02}:{min:02}:{sec:02}"
        )),
        Value::Time(negative, days, hours, minutes, seconds, _) => Some(format!(
            "{}{:02}:{:02}:{:02}",
            if negative { "-" } else { "" },
            days as u32 * 24 + hours as u32,
            minutes,
            seconds
        )),
    }
}

/// Discovers every user table's columns and foreign keys in the connected
/// database.
pub async fn fetch_schema(pool: &Pool) -> Result<Vec<TableInfo>, DatabaseError> {
    let mut conn = pool
        .get_conn()
        .await
        .map_err(|e| DatabaseError::Connection(format!("failed to get connection: {e}")))?;

    let table_names: Vec<String> = conn
        .query(
            "SELECT TABLE_NAME FROM INFORMATION_SCHEMA.TABLES \
             WHERE TABLE_SCHEMA = DATABASE() AND TABLE_TYPE = 'BASE TABLE' \
             ORDER BY TABLE_NAME",
        )
        .await
        .map_err(|e| DatabaseError::Connection(format!("failed to list tables: {e}")))?;

    let mut tables = Vec::with_capacity(table_names.len());
    for name in table_names {
        let columns = table_columns(&mut conn, &name).await?;
        let foreign_keys = table_foreign_keys(&mut conn, &name).await?;
        tables.push(TableInfo {
            name,
            columns,
            foreign_keys,
        });
    }
    Ok(tables)
}

async fn table_columns(
    conn: &mut mysql_async::Conn,
    table: &str,
) -> Result<Vec<ColumnInfo>, DatabaseError> {
    let rows: Vec<(String, String, String, String)> = conn
        .exec(
            "SELECT COLUMN_NAME, DATA_TYPE, IS_NULLABLE, COLUMN_KEY \
             FROM INFORMATION_SCHEMA.COLUMNS \
             WHERE TABLE_SCHEMA = DATABASE() AND TABLE_NAME = :table \
             ORDER BY ORDINAL_POSITION",
            params! { "table" => table },
        )
        .await
        .map_err(|e| {
            DatabaseError::Connection(format!("failed to read columns for {table}: {e}"))
        })?;

    Ok(rows
        .into_iter()
        .map(|(name, type_name, is_nullable, column_key)| ColumnInfo {
            name,
            type_name,
            nullable: is_nullable == "YES",
            primary_key: column_key == "PRI",
        })
        .collect())
}

async fn table_foreign_keys(
    conn: &mut mysql_async::Conn,
    table: &str,
) -> Result<Vec<ForeignKey>, DatabaseError> {
    let rows: Vec<(String, String, String)> = conn
        .exec(
            "SELECT COLUMN_NAME, REFERENCED_TABLE_NAME, REFERENCED_COLUMN_NAME \
             FROM INFORMATION_SCHEMA.KEY_COLUMN_USAGE \
             WHERE TABLE_SCHEMA = DATABASE() AND TABLE_NAME = :table \
                 AND REFERENCED_TABLE_NAME IS NOT NULL",
            params! { "table" => table },
        )
        .await
        .map_err(|e| {
            DatabaseError::Connection(format!("failed to read foreign keys for {table}: {e}"))
        })?;

    Ok(rows
        .into_iter()
        .map(|(from_column, to_table, to_column)| ForeignKey {
            from_column,
            to_table,
            to_column,
        })
        .collect())
}
