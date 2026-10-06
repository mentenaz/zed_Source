# database_backend

Connection lifecycle, schema introspection and query execution for SQLite,
PostgreSQL, MySQL/MariaDB and Microsoft SQL Server. No UI.

## Why it exists

The Database panel needs to talk to four different engines through one
interface. This crate hides the driver differences behind a single
`ConnectionRegistry` and a common set of result types, so the UI never
contains engine-specific code.

It is deliberately separate from the panel and **must never depend on
`gpui_component`**. UI belongs in
[`database_panel`](../database_panel/README.md). The full design is in
[`DATABASE_PANEL_SPEC.md`](../gpui_component/DATABASE_PANEL_SPEC.md).

## Using it

```rust
use database_backend::{ConnectionId, ConnectionRegistry, NetworkConnectParams};

let mut registry = ConnectionRegistry::new();

// SQLite is synchronous: the file is the database.
registry.connect_sqlite(ConnectionId(1), "C:/data/app.db")?;

// Network drivers are async and take NetworkConnectParams.
registry.connect_postgres(ConnectionId(2), params).await?;

let schema = registry.fetch_schema(ConnectionId(2)).await?;
let result = registry.execute_query(ConnectionId(2), "select 1").await?;
```

Check `src/connection.rs` for exact signatures; several methods have a
`_database` variant that targets a specific database within a server
connection.

### API

| Area | Items |
| --- | --- |
| Registry | `ConnectionRegistry` — `connect_sqlite`, `connect_postgres`, `connect_mysql`, `connect_mssql`, `disconnect`, `disconnect_database`, `is_connected`, `live_databases`, `set_active_database`, `active_database` |
| Work | `fetch_schema`, `fetch_schema_database`, `execute_query`, `execute_query_database` |
| Server-level | `list_databases`, `create_database` |
| Identity | `ConnectionId`, `DatabaseId`, `ConnectionStatus`, `DbType` |
| Config | `ConnectionConfig`, `SavedDatabase`, `NetworkConnectParams` |
| Results | `Schema`, `TableInfo`, `ColumnInfo`, `ForeignKey`, `IndexInfo`, `ViewInfo`, `QueryResult` |
| Errors | `DatabaseError` |
| Credentials | `credential_url(db_type, host, port)` |

## Credentials

Passwords are **never** stored on `ConnectionConfig`, which is safe to
persist as plain JSON. Secrets are read and written only through
`zed_credentials_provider`, keyed by `credential_url`, which produces
`database://<type>/<host>:<port>`.

The key is per connection, not per database: one password covers every
database on that server. `DbType::as_str()` supplies the `<type>` part and is
a stable identifier — do not change those strings, or saved credentials will
be orphaned.

## Layout

| File | Contents |
| --- | --- |
| `src/connection.rs` | `ConnectionRegistry` and connection-level helpers |
| `src/drivers/{sqlite,postgres,mysql,mssql}.rs` | One module per engine: `connect`, `fetch_schema`, `execute_query`, and for network engines `list_databases` / `create_database` |
| `src/metadata.rs` | Schema types |
| `src/models.rs` | Persisted connection config |
| `src/query.rs` | `QueryResult` |
| `src/errors.rs` | `DatabaseError` |

Drivers used: `rusqlite`, `tokio-postgres` (with rustls), `mysql_async`,
`tiberius`.

## Development

```sh
cargo check -p database_backend -j 8
cargo test -p database_backend -j 8
```

The tests cover the SQLite driver and the connection registry. The
PostgreSQL, MySQL and MSSQL drivers have no automated tests; they need a
running server to exercise.

To run the tests of every fork crate at once: `script/test-fork-crates.ps1`.
