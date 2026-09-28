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
use crate::metadata::{ColumnInfo, ForeignKey, IndexInfo, Schema, TableInfo, ViewInfo};
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

/// Discovers every user table's columns/foreign keys/indexes, and every
/// view's columns and defining SQL, in the connected database's `public`
/// schema.
pub async fn fetch_schema(client: &Client) -> Result<Schema, DatabaseError> {
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
        let indexes = table_indexes(client, &name).await?;
        tables.push(TableInfo {
            name,
            columns,
            foreign_keys,
            indexes,
        });
    }

    let view_rows = client
        .query(
            "SELECT table_name, view_definition FROM information_schema.views \
             WHERE table_schema = 'public' ORDER BY table_name",
            &[],
        )
        .await
        .map_err(|e| DatabaseError::Connection(format!("failed to list views: {e}")))?;

    let mut views = Vec::with_capacity(view_rows.len());
    for row in view_rows {
        let name: String = row.get(0);
        let definition: Option<String> = row.get(1);
        let columns = table_columns(client, &name).await?;
        views.push(ViewInfo { name, columns, definition: definition.unwrap_or_default() });
    }

    Ok(Schema { tables, views })
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
    // Uses pg_catalog (pg_constraint / pg_attribute / pg_class) rather than
    // information_schema — this is the same query that works in the original
    // Forge Tauri implementation and is reliable on Supabase's pooler where
    // information_schema.constraint_column_usage requires going through
    // referential_constraints to bridge FK→PK constraint names correctly.
    // pg_catalog has no such indirection: conkey/confkey are direct column
    // ordinal arrays on pg_constraint itself.
    //
    // Note: ANY(con.conkey)/ANY(con.confkey) expands multi-column FK arrays
    // correctly without needing unnest+zip — for composite FKs each source
    // column matches all target columns, but single-column FKs (the
    // overwhelming majority) work precisely.
    let rows = client
        .query(
            "SELECT a.attname  AS from_column, \
                    c.relname  AS to_table, \
                    af.attname AS to_column \
             FROM   pg_constraint con \
             JOIN   pg_attribute  a  ON  a.attrelid  = con.conrelid \
                                     AND a.attnum    = ANY(con.conkey) \
             JOIN   pg_attribute  af ON  af.attrelid = con.confrelid \
                                     AND af.attnum   = ANY(con.confkey) \
             JOIN   pg_class      cl ON  cl.oid      = con.conrelid \
             JOIN   pg_class      c  ON  c.oid       = con.confrelid \
             JOIN   pg_namespace  n  ON  n.oid       = cl.relnamespace \
             WHERE  con.contype = 'f' \
               AND  cl.relname  = $1 \
               AND  n.nspname   = 'public'",
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

/// Every index on `table` (including the implicit one backing a `PRIMARY
/// KEY`/`UNIQUE` constraint — Postgres always represents those as real
/// indexes, so this reports what's actually there rather than filtering
/// them out). `unnest(...) WITH ORDINALITY` walks `pg_index.indkey` (the
/// index's column numbers, in key order) so `array_agg` can aggregate
/// column names back in that same order instead of alphabetically.
async fn table_indexes(client: &Client, table: &str) -> Result<Vec<IndexInfo>, DatabaseError> {
    let rows = client
        .query(
            "SELECT i.relname, ix.indisunique, array_agg(a.attname ORDER BY ord.n) \
             FROM pg_class t \
             JOIN pg_namespace ns ON ns.oid = t.relnamespace AND ns.nspname = 'public' \
             JOIN pg_index ix ON t.oid = ix.indrelid \
             JOIN pg_class i ON i.oid = ix.indexrelid \
             JOIN unnest(ix.indkey) WITH ORDINALITY AS ord(attnum, n) ON true \
             JOIN pg_attribute a ON a.attrelid = t.oid AND a.attnum = ord.attnum \
             WHERE t.relname = $1 AND t.relkind = 'r' \
             GROUP BY i.relname, ix.indisunique \
             ORDER BY i.relname",
            &[&table],
        )
        .await
        .map_err(|e| DatabaseError::Connection(format!("failed to read indexes for {table}: {e}")))?;

    Ok(rows
        .into_iter()
        .map(|row| IndexInfo {
            name: row.get(0),
            unique: row.get(1),
            columns: row.get(2),
        })
        .collect())
}
