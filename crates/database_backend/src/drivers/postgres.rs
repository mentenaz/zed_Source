//! PostgreSQL driver.
//!
//! Uses `tokio_postgres` over a rustls TLS connector (never native-tls/
//! OpenSSL — matches the rustls stack Zed's own `reqwest` dependency already
//! commits to workspace-wide, avoiding a second TLS implementation and the
//! extra native build dependencies that come with it).

use tokio_postgres::{Client, Config, NoTls};
use tokio_postgres_rustls::MakeRustlsConnect;

use crate::connection::NetworkConnectParams;
use crate::errors::DatabaseError;
use crate::metadata::{ColumnInfo, ForeignKey, TableInfo};
use crate::query::QueryResult;

/// Opens a PostgreSQL connection and proves it works with a trivial query.
///
/// `tokio_postgres::connect` returns a `(Client, Connection)` pair — the
/// `Connection` is a background I/O driver that must be polled for the
/// `Client` to make progress at all, so it's spawned onto the same tokio
/// runtime immediately. If that background task ever exits (connection
/// dropped/errored), every future `Client` call simply starts failing —
/// there's no separate "is it still alive" flag to maintain.
pub async fn connect(params: NetworkConnectParams) -> Result<Client, DatabaseError> {
    let mut config = Config::new();
    config
        .host(&params.host)
        .port(params.port)
        .dbname(&params.database)
        .user(&params.username)
        .password(&params.password);

    let client = if params.ssl {
        let tls_config = rustls::ClientConfig::builder()
            .with_root_certificates(load_root_certs())
            .with_no_client_auth();
        let connector = MakeRustlsConnect::new(tls_config);
        let (client, connection) = config
            .connect(connector)
            .await
            .map_err(|e| DatabaseError::Connection(format!("failed to connect: {e}")))?;
        tokio::spawn(async move {
            if let Err(e) = connection.await {
                log::warn!("postgres connection closed: {e}");
            }
        });
        client
    } else {
        let (client, connection) = config
            .connect(NoTls)
            .await
            .map_err(|e| DatabaseError::Connection(format!("failed to connect: {e}")))?;
        tokio::spawn(async move {
            if let Err(e) = connection.await {
                log::warn!("postgres connection closed: {e}");
            }
        });
        client
    };

    client
        .simple_query("SELECT 1")
        .await
        .map_err(|e| DatabaseError::Connection(format!("connection check failed: {e}")))?;

    Ok(client)
}

/// Lists every non-template database visible on the server. `params.database`
/// is used verbatim if set; callers doing server-level discovery (no specific
/// database picked yet) should default it to `"postgres"` themselves — an
/// empty/unset dbname makes the server fall back to a database named after
/// the connecting user instead, which almost never exists.
pub async fn list_databases(params: NetworkConnectParams) -> Result<Vec<String>, DatabaseError> {
    let client = connect(params).await?;
    let rows = client
        .query(
            "SELECT datname FROM pg_database WHERE datistemplate = false ORDER BY datname",
            &[],
        )
        .await
        .map_err(|e| DatabaseError::Connection(format!("failed to list databases: {e}")))?;
    Ok(rows.into_iter().map(|row| row.get(0)).collect())
}

/// Creates a new database on the server `params` is already pointed at (e.g.
/// its admin/maintenance database).
pub async fn create_database(params: NetworkConnectParams, name: &str) -> Result<(), DatabaseError> {
    let client = connect(params).await?;
    let sql = format!("CREATE DATABASE {}", quote_ident(name));
    client
        .batch_execute(&sql)
        .await
        .map_err(|e| DatabaseError::Connection(format!("failed to create database: {e}")))
}

/// Quotes an identifier for use as a bare `CREATE DATABASE` target, stripping
/// anything that isn't alphanumeric/underscore rather than attempting to
/// escape arbitrary input.
fn quote_ident(name: &str) -> String {
    let sanitized: String = name.chars().filter(|c| c.is_alphanumeric() || *c == '_').collect();
    format!("\"{sanitized}\"")
}

/// Runs one ad-hoc SQL statement — the SQL workbench's entry point.
///
/// Uses the simple query protocol (`simple_query`) rather than
/// `prepare`+`query`: it handles a row-returning statement and a plain
/// write/DDL statement uniformly (a `CommandComplete` message carries the
/// affected-row count either way), so there's no need to sniff the SQL
/// keyword upfront the way the MySQL driver has to. Simple-query results
/// come back already stringified (Postgres's text wire format), so no
/// per-type cell conversion is needed here either.
pub async fn execute_query(client: &Client, sql: &str) -> Result<QueryResult, DatabaseError> {
    use tokio_postgres::SimpleQueryMessage;

    let start = std::time::Instant::now();
    let messages = client
        .simple_query(sql)
        .await
        .map_err(|e| DatabaseError::Connection(format!("SQL error: {e}")))?;

    let mut columns: Vec<String> = Vec::new();
    let mut rows: Vec<Vec<Option<String>>> = Vec::new();
    let mut command_complete_count: Option<u64> = None;

    for message in messages {
        match message {
            SimpleQueryMessage::Row(row) => {
                if columns.is_empty() {
                    columns = row.columns().iter().map(|c| c.name().to_string()).collect();
                }
                rows.push((0..row.columns().len()).map(|i| row.get(i).map(str::to_string)).collect());
            }
            SimpleQueryMessage::CommandComplete(n) => command_complete_count = Some(n),
            _ => {}
        }
    }

    // Only surface "N rows affected" for a statement that returned no result
    // set at all — a SELECT also gets a CommandComplete tag, but `rows`
    // (however empty) is the right thing for the UI to render for those.
    let rows_affected = if columns.is_empty() {
        command_complete_count
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

fn load_root_certs() -> rustls::RootCertStore {
    let mut store = rustls::RootCertStore::empty();
    for cert in rustls_native_certs::load_native_certs().certs {
        let _ = store.add(cert);
    }
    store
}

/// Discovers every user table's columns and foreign keys in the connected
/// database's `public` schema.
pub async fn fetch_schema(client: &Client) -> Result<Vec<TableInfo>, DatabaseError> {
    let table_rows = client
        .query(
            "SELECT table_name FROM information_schema.tables \
             WHERE table_schema = 'public' AND table_type = 'BASE TABLE' \
             ORDER BY table_name",
            &[],
        )
        .await
        .map_err(|e| DatabaseError::Connection(format!("failed to list tables: {e}")))?;

    let mut tables = Vec::with_capacity(table_rows.len());
    for row in table_rows {
        let name: String = row.get(0);
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

async fn table_columns(client: &Client, table: &str) -> Result<Vec<ColumnInfo>, DatabaseError> {
    let rows = client
        .query(
            "SELECT c.column_name, c.data_type, c.is_nullable = 'YES', \
             EXISTS ( \
                 SELECT 1 FROM information_schema.key_column_usage kcu \
                 JOIN information_schema.table_constraints tc \
                     ON tc.constraint_name = kcu.constraint_name \
                     AND tc.constraint_type = 'PRIMARY KEY' \
                 WHERE kcu.table_name = c.table_name AND kcu.column_name = c.column_name \
             ) AS is_primary_key \
             FROM information_schema.columns c \
             WHERE c.table_schema = 'public' AND c.table_name = $1 \
             ORDER BY c.ordinal_position",
            &[&table],
        )
        .await
        .map_err(|e| DatabaseError::Connection(format!("failed to read columns for {table}: {e}")))?;

    Ok(rows
        .into_iter()
        .map(|row| ColumnInfo {
            name: row.get(0),
            type_name: row.get(1),
            nullable: row.get(2),
            primary_key: row.get(3),
        })
        .collect())
}

async fn table_foreign_keys(client: &Client, table: &str) -> Result<Vec<ForeignKey>, DatabaseError> {
    let rows = client
        .query(
            "SELECT kcu.column_name, ccu.table_name, ccu.column_name \
             FROM information_schema.table_constraints tc \
             JOIN information_schema.key_column_usage kcu \
                 ON tc.constraint_name = kcu.constraint_name \
             JOIN information_schema.constraint_column_usage ccu \
                 ON tc.constraint_name = ccu.constraint_name \
             WHERE tc.constraint_type = 'FOREIGN KEY' \
                 AND tc.table_schema = 'public' AND tc.table_name = $1",
            &[&table],
        )
        .await
        .map_err(|e| {
            DatabaseError::Connection(format!("failed to read foreign keys for {table}: {e}"))
        })?;

    Ok(rows
        .into_iter()
        .map(|row| ForeignKey {
            from_column: row.get(0),
            to_table: row.get(1),
            to_column: row.get(2),
        })
        .collect())
}
