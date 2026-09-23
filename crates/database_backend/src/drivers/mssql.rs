//! MSSQL driver.
//!
//! `tiberius` speaks its own protocol over any `AsyncRead + AsyncWrite`
//! stream — it doesn't dial the socket itself. `tokio_util::compat` bridges
//! a plain `tokio::net::TcpStream` into the shape tiberius expects, matching
//! Forge's own approach (confirmed via the earlier survey) rather than
//! pulling in a second TLS-aware connector; TLS is instead handled by
//! `tiberius::Config`'s own `encryption` setting, negotiated over the same
//! plain TCP stream per the MSSQL wire protocol (TDS's TLS handshake is
//! in-band, unlike Postgres/MySQL's separate-connector model).

use tiberius::{AuthMethod, Client, Config, EncryptionLevel};
use tokio::net::TcpStream;
use tokio_util::compat::{Compat, TokioAsyncWriteCompatExt};

use crate::connection::NetworkConnectParams;
use crate::errors::DatabaseError;
use crate::metadata::{ColumnInfo, ForeignKey, TableInfo};
use crate::query::QueryResult;

/// A connected MSSQL client, over a plain TCP stream wrapped for tiberius's
/// `AsyncRead + AsyncWrite` expectations.
pub type MsSqlClient = Client<Compat<TcpStream>>;

/// Opens an MSSQL connection and proves it works with a trivial query.
pub async fn connect(params: NetworkConnectParams) -> Result<MsSqlClient, DatabaseError> {
    let mut config = Config::new();
    config.host(&params.host);
    config.port(params.port);
    if !params.database.is_empty() {
        config.database(&params.database);
    }
    config.authentication(AuthMethod::sql_server(&params.username, &params.password));
    config.encryption(if params.ssl {
        EncryptionLevel::Required
    } else {
        EncryptionLevel::NotSupported
    });
    config.trust_cert();

    let tcp_stream = TcpStream::connect(config.get_addr())
        .await
        .map_err(|e| DatabaseError::Connection(format!("failed to open TCP connection: {e}")))?;
    tcp_stream
        .set_nodelay(true)
        .map_err(|e| DatabaseError::Connection(format!("failed to configure socket: {e}")))?;

    let mut client = Client::connect(config, tcp_stream.compat_write())
        .await
        .map_err(|e| DatabaseError::Connection(format!("failed to connect: {e}")))?;

    client
        .simple_query("SELECT 1")
        .await
        .map_err(|e| DatabaseError::Connection(format!("connection check failed: {e}")))?;

    Ok(client)
}

/// Lists every database visible on the server. `params.database` should be
/// left empty by callers doing server-level discovery — an empty `DATABASE`
/// field in the TDS login packet makes the server fall back to the login's
/// configured default database (usually `master`), which every login has.
pub async fn list_databases(params: NetworkConnectParams) -> Result<Vec<String>, DatabaseError> {
    let mut client = connect(params).await?;
    let rows = client
        .simple_query("SELECT name FROM sys.databases ORDER BY name")
        .await
        .map_err(|e| DatabaseError::Connection(format!("failed to list databases: {e}")))?
        .into_first_result()
        .await
        .map_err(|e| DatabaseError::Connection(format!("failed to list databases: {e}")))?;

    Ok(rows
        .into_iter()
        .filter_map(|row| row.get::<&str, _>(0).map(str::to_string))
        .collect())
}

/// Creates a new database on the server `params` is already pointed at.
pub async fn create_database(params: NetworkConnectParams, name: &str) -> Result<(), DatabaseError> {
    let mut client = connect(params).await?;
    let sql = format!("CREATE DATABASE [{}]", name.replace(']', "]]"));
    client
        .execute(sql, &[])
        .await
        .map_err(|e| DatabaseError::Connection(format!("failed to create database: {e}")))?;
    Ok(())
}

/// Runs one ad-hoc SQL statement — the SQL workbench's entry point.
///
/// A zero-column result (nothing in `into_first_result()`'s rows, so the
/// column-name loop never ran) means this wasn't a row-returning statement
/// at all — that's when `rows_affected` is populated, from the row count
/// tiberius reports for the affected statement.
pub async fn execute_query(client: &mut MsSqlClient, sql: &str) -> Result<QueryResult, DatabaseError> {
    let start = std::time::Instant::now();
    let stream = client
        .query(sql, &[])
        .await
        .map_err(|e| DatabaseError::Connection(format!("SQL error: {e}")))?;

    let mut columns: Vec<String> = Vec::new();
    let mut rows: Vec<Vec<Option<String>>> = Vec::new();
    let mut rows_affected = None;

    if let Ok(result_rows) = stream.into_first_result().await {
        for row in &result_rows {
            if columns.is_empty() {
                columns = row.columns().iter().map(|c| c.name().to_string()).collect();
            }
            rows.push((0..columns.len()).map(|i| mssql_cell(row, i)).collect());
        }
        // Only non-row-returning statements (INSERT/UPDATE/DELETE) leave
        // `columns` empty here — a SELECT that matched nothing still ran the
        // column-name loop above via at least its header, so this check
        // only fires for the former.
        if columns.is_empty() {
            rows_affected = Some(result_rows.len() as u64);
        }
    }

    Ok(QueryResult {
        columns,
        rows,
        rows_affected,
        exec_ms: start.elapsed().as_millis() as u64,
    })
}

/// Reads column `i` as a string, trying the common wire types in turn (see
/// `fetch_schema`'s doc comment on `tiberius::Row::get()` panicking on a
/// type mismatch — `try_get` is required here for the same reason). Dates,
/// UUIDs, and `DECIMAL`/`NUMERIC` aren't covered yet (no `chrono`/`uuid`/
/// `tiberius`'s `numeric` feature wired into this crate) — those cells
/// render as empty for now rather than pulling in new dependencies for the
/// workbench's first pass.
fn mssql_cell(row: &tiberius::Row, i: usize) -> Option<String> {
    if let Ok(Some(v)) = row.try_get::<&str, usize>(i) {
        return Some(v.to_owned());
    }
    if let Ok(Some(v)) = row.try_get::<i64, usize>(i) {
        return Some(v.to_string());
    }
    if let Ok(Some(v)) = row.try_get::<i32, usize>(i) {
        return Some(v.to_string());
    }
    if let Ok(Some(v)) = row.try_get::<i16, usize>(i) {
        return Some(v.to_string());
    }
    if let Ok(Some(v)) = row.try_get::<u8, usize>(i) {
        return Some(v.to_string());
    }
    if let Ok(Some(v)) = row.try_get::<f64, usize>(i) {
        return Some(v.to_string());
    }
    if let Ok(Some(v)) = row.try_get::<f32, usize>(i) {
        return Some(v.to_string());
    }
    if let Ok(Some(v)) = row.try_get::<bool, usize>(i) {
        return Some(if v { "1" } else { "0" }.to_owned());
    }
    None
}

/// Discovers every user table's columns and foreign keys in the connected
/// database.
pub async fn fetch_schema(client: &mut MsSqlClient) -> Result<Vec<TableInfo>, DatabaseError> {
    let table_names: Vec<String> = client
        .simple_query(
            "SELECT TABLE_NAME FROM INFORMATION_SCHEMA.TABLES \
             WHERE TABLE_TYPE = 'BASE TABLE' ORDER BY TABLE_NAME",
        )
        .await
        .map_err(|e| DatabaseError::Connection(format!("failed to list tables: {e}")))?
        .into_first_result()
        .await
        .map_err(|e| DatabaseError::Connection(format!("failed to list tables: {e}")))?
        .into_iter()
        .filter_map(|row| row.get::<&str, _>(0).map(str::to_string))
        .collect();

    let mut tables = Vec::with_capacity(table_names.len());
    for name in table_names {
        let columns = table_columns(client, &name).await?;
        let foreign_keys = table_foreign_keys(client, &name).await?;
        tables.push(TableInfo {
            name,
            columns,
            foreign_keys,
        });
    }
    Ok(tables)
}

async fn table_columns(
    client: &mut MsSqlClient,
    table: &str,
) -> Result<Vec<ColumnInfo>, DatabaseError> {
    let query = format!(
        "SELECT c.COLUMN_NAME, c.DATA_TYPE, c.IS_NULLABLE, \
         CASE WHEN pk.COLUMN_NAME IS NOT NULL THEN 1 ELSE 0 END AS IS_PRIMARY_KEY \
         FROM INFORMATION_SCHEMA.COLUMNS c \
         LEFT JOIN ( \
             SELECT ku.COLUMN_NAME \
             FROM INFORMATION_SCHEMA.TABLE_CONSTRAINTS tc \
             JOIN INFORMATION_SCHEMA.KEY_COLUMN_USAGE ku \
                 ON tc.CONSTRAINT_NAME = ku.CONSTRAINT_NAME \
             WHERE tc.CONSTRAINT_TYPE = 'PRIMARY KEY' AND tc.TABLE_NAME = '{table}' \
         ) pk ON pk.COLUMN_NAME = c.COLUMN_NAME \
         WHERE c.TABLE_NAME = '{table}' \
         ORDER BY c.ORDINAL_POSITION"
    );

    let rows = client
        .simple_query(query)
        .await
        .map_err(|e| {
            DatabaseError::Connection(format!("failed to read columns for {table}: {e}"))
        })?
        .into_first_result()
        .await
        .map_err(|e| {
            DatabaseError::Connection(format!("failed to read columns for {table}: {e}"))
        })?;

    Ok(rows
        .into_iter()
        .map(|row| ColumnInfo {
            name: row.get::<&str, _>(0).unwrap_or_default().to_string(),
            type_name: row.get::<&str, _>(1).unwrap_or_default().to_string(),
            nullable: row.get::<&str, _>(2) == Some("YES"),
            primary_key: row.get::<i32, _>(3) == Some(1),
        })
        .collect())
}

async fn table_foreign_keys(
    client: &mut MsSqlClient,
    table: &str,
) -> Result<Vec<ForeignKey>, DatabaseError> {
    let query = format!(
        "SELECT fk_cols.COLUMN_NAME, pk_tab.TABLE_NAME, pk_cols.COLUMN_NAME \
         FROM sys.foreign_keys fk \
         JOIN sys.foreign_key_columns fkc ON fkc.constraint_object_id = fk.object_id \
         JOIN sys.columns fk_col ON fk_col.object_id = fkc.parent_object_id \
             AND fk_col.column_id = fkc.parent_column_id \
         JOIN INFORMATION_SCHEMA.COLUMNS fk_cols \
             ON fk_cols.TABLE_NAME = OBJECT_NAME(fkc.parent_object_id) \
             AND fk_cols.COLUMN_NAME = fk_col.name \
         JOIN sys.tables pk_tab ON pk_tab.object_id = fkc.referenced_object_id \
         JOIN sys.columns pk_col ON pk_col.object_id = fkc.referenced_object_id \
             AND pk_col.column_id = fkc.referenced_column_id \
         JOIN INFORMATION_SCHEMA.COLUMNS pk_cols \
             ON pk_cols.TABLE_NAME = pk_tab.name AND pk_cols.COLUMN_NAME = pk_col.name \
         WHERE OBJECT_NAME(fkc.parent_object_id) = '{table}'"
    );

    let rows = client
        .simple_query(query)
        .await
        .map_err(|e| {
            DatabaseError::Connection(format!("failed to read foreign keys for {table}: {e}"))
        })?
        .into_first_result()
        .await
        .map_err(|e| {
            DatabaseError::Connection(format!("failed to read foreign keys for {table}: {e}"))
        })?;

    Ok(rows
        .into_iter()
        .map(|row| ForeignKey {
            from_column: row.get::<&str, _>(0).unwrap_or_default().to_string(),
            to_table: row.get::<&str, _>(1).unwrap_or_default().to_string(),
            to_column: row.get::<&str, _>(2).unwrap_or_default().to_string(),
        })
        .collect())
}
