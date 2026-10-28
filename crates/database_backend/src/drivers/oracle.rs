//! Oracle driver.
//!
//! `oracledb` is Oracle's own pure-Rust thin driver (no Oracle Client
//! libraries needed), but unlike the other network drivers it is
//! *synchronous*: every call blocks the calling thread until the server
//! answers. The public async entry points below therefore hop onto
//! `tokio::task::spawn_blocking`, so a slow query never stalls the runtime
//! (or, via the panel's `cx.spawn`, the UI). The `*_blocking` functions hold
//! the real logic and are what the tests drive directly.
//!
//! Oracle differences the rest of the backend has to live with:
//! - `NetworkConnectParams::database` is the **service name** (a PDB such as
//!   `FREEPDB1`), not a database name — one service is one database.
//! - There is no server-level "list databases" or "create database" (that is
//!   a CDB/DBA operation). `list_databases` lists the *schemas* (users)
//!   visible on the service instead, and `create_database` is unsupported.
//! - Schema metadata is read from the `ALL_*` dictionary views, scoped to the
//!   connected user's current schema.
//! - Oracle never auto-commits, so non-query statements are committed here to
//!   match the other drivers' behaviour.

use std::sync::{Arc, Mutex};

use oracledb::{Connection, Row, ToDbValue};

use crate::connection::NetworkConnectParams;
use crate::errors::DatabaseError;
use crate::metadata::{ColumnInfo, ForeignKey, IndexInfo, Schema, TableInfo, ViewInfo};
use crate::query::QueryResult;

/// A connected Oracle session. The mutex serialises callers (one Oracle
/// session runs one statement at a time) and makes the handle cheap to clone
/// into a `spawn_blocking` closure.
pub type OracleClient = Arc<Mutex<Connection>>;

/// Opens an Oracle connection and proves it works with a trivial query.
pub async fn connect(params: NetworkConnectParams) -> Result<OracleClient, DatabaseError> {
    run_blocking(move || connect_blocking(&params)).await
}

/// Lists the schemas (users) visible on the service `params` points at,
/// excluding Oracle-maintained ones such as `SYS` and `SYSTEM`.
pub async fn list_databases(params: NetworkConnectParams) -> Result<Vec<String>, DatabaseError> {
    run_blocking(move || {
        let client = connect_blocking(&params)?;
        let conn = lock(&client)?;
        list_schemas_blocking(&conn)
    })
    .await
}

/// Discovers every table's columns/foreign keys/indexes, and every view's
/// columns and defining SQL, in the connected user's current schema.
pub async fn fetch_schema(client: &OracleClient) -> Result<Schema, DatabaseError> {
    let client = client.clone();
    run_blocking(move || {
        let conn = lock(&client)?;
        fetch_schema_blocking(&conn)
    })
    .await
}

/// Runs one ad-hoc SQL statement — the SQL workbench's entry point.
pub async fn execute_query(client: &OracleClient, sql: &str) -> Result<QueryResult, DatabaseError> {
    let client = client.clone();
    let sql = sql.to_owned();
    run_blocking(move || {
        let conn = lock(&client)?;
        execute_query_blocking(&conn, &sql)
    })
    .await
}

async fn run_blocking<T, F>(work: F) -> Result<T, DatabaseError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, DatabaseError> + Send + 'static,
{
    tokio::task::spawn_blocking(work)
        .await
        .map_err(|e| DatabaseError::Connection(format!("oracle worker failed: {e}")))?
}

fn lock(client: &OracleClient) -> Result<std::sync::MutexGuard<'_, Connection>, DatabaseError> {
    client
        .lock()
        .map_err(|_| DatabaseError::Connection("oracle connection lock was poisoned".into()))
}

fn sql_error(context: &str, e: oracledb::Error) -> DatabaseError {
    DatabaseError::Connection(format!("{context}: {e}"))
}

/// Builds the Easy Connect string `[tcps://]host:port/service`.
pub(crate) fn connect_string(params: &NetworkConnectParams) -> Result<String, DatabaseError> {
    let service = params.database.trim();
    if service.is_empty() {
        return Err(DatabaseError::Connection(
            "an Oracle connection needs a service name (e.g. FREEPDB1) in the database field"
                .into(),
        ));
    }
    let scheme = if params.ssl { "tcps://" } else { "" };
    Ok(format!("{scheme}{}:{}/{service}", params.host.trim(), params.port))
}

pub fn connect_blocking(params: &NetworkConnectParams) -> Result<OracleClient, DatabaseError> {
    let connect_string = connect_string(params)?;
    let config = oracledb::Config::default()
        .set_credentials(&params.username, &params.password)
        .set_connect_string(&connect_string)
        .map_err(|e| sql_error("invalid connect string", e))?;
    let conn = oracledb::connect(config).map_err(|e| sql_error("failed to connect", e))?;
    conn.query_row("SELECT 1 FROM dual", &[])
        .map_err(|e| sql_error("connection check failed", e))?;
    Ok(Arc::new(Mutex::new(conn)))
}

pub fn list_schemas_blocking(conn: &Connection) -> Result<Vec<String>, DatabaseError> {
    // `oracle_maintained` exists from Oracle 12.2 on.
    collect_strings(
        conn,
        "SELECT username FROM all_users WHERE oracle_maintained = 'N' ORDER BY username",
        &[],
        "failed to list schemas",
    )
}

/// The schema unqualified names resolve against — the login's own schema
/// unless the session changed it.
fn current_schema(conn: &Connection) -> Result<String, DatabaseError> {
    conn.query_row("SELECT SYS_CONTEXT('USERENV', 'CURRENT_SCHEMA') FROM dual", &[])
        .and_then(|row| row.get::<String>(0))
        .map_err(|e| sql_error("failed to read current schema", e))
}

fn collect_strings(
    conn: &Connection,
    sql: &str,
    params: &[&dyn ToDbValue],
    context: &str,
) -> Result<Vec<String>, DatabaseError> {
    let cursor = conn.query(sql, params).map_err(|e| sql_error(context, e))?;
    let mut out = Vec::new();
    for row in cursor {
        let row = row.map_err(|e| sql_error(context, e))?;
        out.push(row.get::<String>(0).map_err(|e| sql_error(context, e))?);
    }
    Ok(out)
}

pub fn fetch_schema_blocking(conn: &Connection) -> Result<Schema, DatabaseError> {
    let owner = current_schema(conn)?;

    let table_names = collect_strings(
        conn,
        "SELECT table_name FROM all_tables \
         WHERE owner = :1 AND nested = 'NO' AND secondary = 'N' \
         ORDER BY table_name",
        &[&owner],
        "failed to list tables",
    )?;

    let mut tables = Vec::with_capacity(table_names.len());
    for name in table_names {
        let columns = table_columns(conn, &owner, &name)?;
        let foreign_keys = table_foreign_keys(conn, &owner, &name)?;
        let indexes = table_indexes(conn, &owner, &name)?;
        tables.push(TableInfo { name, columns, foreign_keys, indexes });
    }

    let cursor = conn
        .query(
            "SELECT view_name, text FROM all_views WHERE owner = :1 ORDER BY view_name",
            &[&owner],
        )
        .map_err(|e| sql_error("failed to list views", e))?;
    let mut view_rows = Vec::new();
    for row in cursor {
        let row = row.map_err(|e| sql_error("failed to list views", e))?;
        let name = row.get::<String>(0).unwrap_or_default();
        // `text` is a LONG column; if the driver can't hand it over as a
        // string the view still lists, just without its definition.
        let definition = row.get::<Option<String>>(1).ok().flatten().unwrap_or_default();
        view_rows.push((name, definition));
    }

    let mut views = Vec::with_capacity(view_rows.len());
    for (name, definition) in view_rows {
        let columns = table_columns(conn, &owner, &name)?;
        views.push(ViewInfo { name, columns, definition });
    }

    Ok(Schema { tables, views })
}

fn table_columns(
    conn: &Connection,
    owner: &str,
    table: &str,
) -> Result<Vec<ColumnInfo>, DatabaseError> {
    let (owner, table) = (owner.to_owned(), table.to_owned());
    let context = format!("failed to read columns for {table}");
    let cursor = conn
        .query(
            "SELECT c.column_name, c.data_type, c.nullable, \
                    CASE WHEN EXISTS ( \
                        SELECT 1 FROM all_constraints k \
                        JOIN all_cons_columns cc \
                          ON cc.owner = k.owner AND cc.constraint_name = k.constraint_name \
                        WHERE k.constraint_type = 'P' AND k.owner = c.owner \
                          AND k.table_name = c.table_name AND cc.column_name = c.column_name \
                    ) THEN 1 ELSE 0 END \
             FROM all_tab_columns c \
             WHERE c.owner = :1 AND c.table_name = :2 \
             ORDER BY c.column_id",
            &[&owner, &table],
        )
        .map_err(|e| sql_error(&context, e))?;

    let mut columns = Vec::new();
    for row in cursor {
        let row = row.map_err(|e| sql_error(&context, e))?;
        columns.push(ColumnInfo {
            name: row.get::<String>(0).unwrap_or_default(),
            type_name: row.get::<String>(1).unwrap_or_default(),
            nullable: row.get::<String>(2).is_ok_and(|n| n == "Y"),
            primary_key: row.get::<i32>(3).is_ok_and(|n| n == 1),
        });
    }
    Ok(columns)
}

fn table_foreign_keys(
    conn: &Connection,
    owner: &str,
    table: &str,
) -> Result<Vec<ForeignKey>, DatabaseError> {
    let (owner, table) = (owner.to_owned(), table.to_owned());
    let context = format!("failed to read foreign keys for {table}");
    let cursor = conn
        .query(
            "SELECT cc.column_name, rc.table_name, rcc.column_name \
             FROM all_constraints c \
             JOIN all_cons_columns cc \
               ON cc.owner = c.owner AND cc.constraint_name = c.constraint_name \
             JOIN all_constraints rc \
               ON rc.owner = c.r_owner AND rc.constraint_name = c.r_constraint_name \
             JOIN all_cons_columns rcc \
               ON rcc.owner = rc.owner AND rcc.constraint_name = rc.constraint_name \
              AND rcc.position = cc.position \
             WHERE c.constraint_type = 'R' AND c.owner = :1 AND c.table_name = :2 \
             ORDER BY c.constraint_name, cc.position",
            &[&owner, &table],
        )
        .map_err(|e| sql_error(&context, e))?;

    let mut keys = Vec::new();
    for row in cursor {
        let row = row.map_err(|e| sql_error(&context, e))?;
        keys.push(ForeignKey {
            from_column: row.get::<String>(0).unwrap_or_default(),
            to_table: row.get::<String>(1).unwrap_or_default(),
            to_column: row.get::<String>(2).unwrap_or_default(),
        });
    }
    Ok(keys)
}

/// Every index on `table`, including the implicit one backing a primary key
/// or unique constraint. Rows come back ordered by index name then key
/// position, so grouping is "same name as the row before → append".
fn table_indexes(
    conn: &Connection,
    owner: &str,
    table: &str,
) -> Result<Vec<IndexInfo>, DatabaseError> {
    let (owner, table) = (owner.to_owned(), table.to_owned());
    let context = format!("failed to read indexes for {table}");
    let cursor = conn
        .query(
            "SELECT i.index_name, i.uniqueness, ic.column_name \
             FROM all_indexes i \
             JOIN all_ind_columns ic \
               ON ic.index_owner = i.owner AND ic.index_name = i.index_name \
             WHERE i.table_owner = :1 AND i.table_name = :2 \
             ORDER BY i.index_name, ic.column_position",
            &[&owner, &table],
        )
        .map_err(|e| sql_error(&context, e))?;

    let mut indexes: Vec<IndexInfo> = Vec::new();
    for row in cursor {
        let row = row.map_err(|e| sql_error(&context, e))?;
        let name = row.get::<String>(0).unwrap_or_default();
        let unique = row.get::<String>(1).is_ok_and(|u| u == "UNIQUE");
        let column = row.get::<String>(2).unwrap_or_default();
        match indexes.last_mut() {
            Some(index) if index.name == name => index.columns.push(column),
            _ => indexes.push(IndexInfo { name, unique, columns: vec![column] }),
        }
    }
    Ok(indexes)
}

/// Runs a script: one or more statements separated by `;` (or, for PL/SQL
/// blocks, by a `/` on its own line, as in SQL*Plus). Oracle itself only
/// accepts one statement per call, so the script is split here and the
/// statements run in order on this session. The result is the *last*
/// statement's, matching what the other drivers' multi-statement batches
/// show. It stops at the first failure and says which statement it was;
/// earlier ones stay applied (each non-query statement commits as it runs).
pub fn execute_query_blocking(conn: &Connection, script: &str) -> Result<QueryResult, DatabaseError> {
    let start = std::time::Instant::now();
    let statements = split_statements(script);
    let total = statements.len();
    let mut last = QueryResult { rows_affected: Some(0), ..QueryResult::default() };
    for (ix, statement) in statements.iter().enumerate() {
        last = run_statement(conn, statement).map_err(|message| {
            DatabaseError::Connection(if total == 1 {
                message
            } else {
                format!("statement {} of {total} failed ({}): {message}", ix + 1, preview(statement))
            })
        })?;
    }
    last.exec_ms = start.elapsed().as_millis() as u64;
    Ok(last)
}

/// A short single-line label for `statement`, for error messages.
fn preview(statement: &str) -> String {
    let flat = statement.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() > 40 {
        format!("{}…", flat.chars().take(40).collect::<String>())
    } else {
        flat
    }
}

/// Runs exactly one statement. The error is already worded for the user.
fn run_statement(conn: &Connection, sql: &str) -> Result<QueryResult, String> {
    let start = std::time::Instant::now();
    let sql = prepare_statement(sql);

    if returns_rows(&sql) {
        let cursor = conn.query(&sql, &[]).map_err(|e| format!("SQL error: {e}"))?;
        let columns: Vec<String> = cursor.columns().iter().map(|c| c.name().to_string()).collect();
        let mut rows = Vec::new();
        for row in cursor {
            let row = row.map_err(|e| format!("SQL error: {e}"))?;
            rows.push((0..columns.len()).map(|i| oracle_cell(&row, i)).collect());
        }
        return Ok(QueryResult {
            columns,
            rows,
            rows_affected: None,
            exec_ms: start.elapsed().as_millis() as u64,
        });
    }

    let result = conn.execute(&sql, &[]).map_err(|e| format!("SQL error: {e}"))?;
    let rows_affected = result.rows_affected();
    conn.commit().map_err(|e| format!("commit failed: {e}"))?;
    Ok(QueryResult {
        columns: Vec::new(),
        rows: Vec::new(),
        rows_affected: Some(rows_affected),
        exec_ms: start.elapsed().as_millis() as u64,
    })
}

/// Renders column `i` as text. A SQL `NULL` is `None`. Types without a text
/// form (LOBs, JSON, vectors) show their type name in angle brackets rather
/// than silently rendering empty.
fn oracle_cell(row: &Row, i: usize) -> Option<String> {
    macro_rules! try_as {
        ($t:ty, $to_text:expr) => {
            match row.get::<Option<$t>>(i) {
                Ok(None) => return None,
                Ok(Some(v)) => return Some($to_text(v)),
                Err(_) => {}
            }
        };
    }
    try_as!(String, |v: String| v);
    try_as!(oracledb::OracleNumber, |v: oracledb::OracleNumber| v.to_string());
    try_as!(oracledb::OracleTimestamp, |v: oracledb::OracleTimestamp| v.to_string());
    try_as!(f64, |v: f64| v.to_string());
    try_as!(f32, |v: f32| v.to_string());
    try_as!(bool, |v: bool| if v { "1" } else { "0" }.to_owned());
    try_as!(oracledb::OracleIntervalDS, |v: oracledb::OracleIntervalDS| v.to_string());
    try_as!(oracledb::OracleIntervalYM, |v: oracledb::OracleIntervalYM| v.to_string());
    try_as!(Vec<u8>, |v: Vec<u8>| v.iter().map(|b| format!("{b:02X}")).collect::<String>());
    let type_name = row.columns().get(i).map(|c| c.data_type()).unwrap_or_default();
    Some(format!("<{type_name}>"))
}

/// `sql` without its leading whitespace and `--` / `/* */` comments.
fn strip_leading_comments(sql: &str) -> &str {
    let mut rest = sql.trim_start();
    loop {
        if let Some(after) = rest.strip_prefix("--") {
            rest = after.split_once('\n').map_or("", |(_, tail)| tail).trim_start();
        } else if let Some(after) = rest.strip_prefix("/*") {
            rest = after.split_once("*/").map_or("", |(_, tail)| tail).trim_start();
        } else {
            return rest;
        }
    }
}

/// The first keyword of `sql`, lower-cased, skipping leading whitespace and
/// comments.
fn first_keyword(sql: &str) -> String {
    strip_leading_comments(sql)
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect::<String>()
        .to_ascii_lowercase()
}

/// True for statements that produce a result set.
fn returns_rows(sql: &str) -> bool {
    matches!(first_keyword(sql).as_str(), "select" | "with")
}

/// True for PL/SQL blocks and stored-program DDL, whose `;`s are part of the
/// statement. Decided from the statement's leading words only, so a table
/// that merely has a column called `type` is not mistaken for one.
fn is_plsql(sql: &str) -> bool {
    let lower = strip_leading_comments(sql).to_ascii_lowercase();
    let mut words = lower
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .filter(|word| !word.is_empty());
    match words.next() {
        Some("begin" | "declare") => true,
        Some("create") => words
            .find(|word| !matches!(*word, "or" | "replace" | "editionable" | "noneditionable"))
            .is_some_and(|word| matches!(word, "procedure" | "function" | "package" | "trigger" | "type")),
        _ => false,
    }
}

/// Splits a script into its statements.
///
/// A statement ends at a `;` outside strings, quoted identifiers and
/// comments — except inside PL/SQL (see [`is_plsql`]), where the `;`s belong
/// to the block and it instead ends at a line holding only `/`, exactly as
/// in SQL*Plus. A `/` line ends any statement. Empty statements (`;;;;`) and
/// ones that are only comments are dropped.
fn split_statements(script: &str) -> Vec<String> {
    fn flush(current: &mut String, out: &mut Vec<String>) {
        let statement = current.trim();
        if !strip_leading_comments(statement).is_empty() {
            out.push(statement.to_owned());
        }
        current.clear();
    }

    let chars: Vec<char> = script.chars().collect();
    let len = chars.len();
    let mut out = Vec::new();
    let mut current = String::new();
    let mut at_line_start = true;
    let mut i = 0;

    while i < len {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        match c {
            '\'' | '"' => {
                // A string literal or quoted identifier. Inside a literal a
                // doubled `''` is an escaped quote.
                current.push(c);
                i += 1;
                while i < len {
                    current.push(chars[i]);
                    if chars[i] == c {
                        if c == '\'' && chars.get(i + 1) == Some(&'\'') {
                            current.push('\'');
                            i += 2;
                            continue;
                        }
                        i += 1;
                        break;
                    }
                    i += 1;
                }
                at_line_start = false;
            }
            '-' if next == Some('-') => {
                while i < len && chars[i] != '\n' {
                    current.push(chars[i]);
                    i += 1;
                }
                at_line_start = false;
            }
            '/' if next == Some('*') => {
                current.push_str("/*");
                i += 2;
                while i < len && !(chars[i] == '*' && chars.get(i + 1) == Some(&'/')) {
                    current.push(chars[i]);
                    i += 1;
                }
                if i < len {
                    current.push_str("*/");
                    i += 2;
                }
                at_line_start = false;
            }
            'q' | 'Q'
                if next == Some('\'')
                    && i + 2 < len
                    && !current.chars().last().is_some_and(|p| p.is_ascii_alphanumeric() || p == '_') =>
            {
                // Alternative quoting: q'[ ... ]', q'{ ... }', q'! ... !'.
                let close = match chars[i + 2] {
                    '[' => ']',
                    '{' => '}',
                    '<' => '>',
                    '(' => ')',
                    other => other,
                };
                current.extend(&chars[i..i + 3]);
                i += 3;
                while i < len {
                    current.push(chars[i]);
                    if chars[i] == close && chars.get(i + 1) == Some(&'\'') {
                        current.push('\'');
                        i += 2;
                        break;
                    }
                    i += 1;
                }
                at_line_start = false;
            }
            '/' if at_line_start && chars[i + 1..].iter().take_while(|c| **c != '\n').all(|c| c.is_whitespace()) => {
                flush(&mut current, &mut out);
                while i < len && chars[i] != '\n' {
                    i += 1;
                }
            }
            ';' => {
                if is_plsql(&current) {
                    current.push(c);
                } else {
                    flush(&mut current, &mut out);
                }
                i += 1;
                at_line_start = false;
            }
            '\n' => {
                current.push(c);
                i += 1;
                at_line_start = true;
            }
            c if c.is_whitespace() => {
                current.push(c);
                i += 1;
            }
            _ => {
                current.push(c);
                i += 1;
                at_line_start = false;
            }
        }
    }
    flush(&mut current, &mut out);
    out
}

/// Oracle rejects a trailing `;` on a plain SQL statement (ORA-00933) though
/// every other engine tolerates one, and users type it by habit. Strip it
/// except for PL/SQL, where it is mandatory.
fn prepare_statement(sql: &str) -> String {
    let trimmed = sql.trim();
    if is_plsql(trimmed) {
        return trimmed.to_owned();
    }
    trimmed.trim_end_matches(|c: char| c == ';' || c.is_whitespace()).to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(database: &str, ssl: bool) -> NetworkConnectParams {
        NetworkConnectParams {
            host: "localhost".into(),
            port: 1521,
            database: database.into(),
            username: "dev_user".into(),
            password: "devpass123".into(),
            ssl,
        }
    }

    // ---- pure helpers: always run -------------------------------------

    #[test]
    fn connect_string_uses_the_service_name() {
        assert_eq!(connect_string(&params("dev_db", false)).unwrap(), "localhost:1521/dev_db");
    }

    #[test]
    fn connect_string_uses_tcps_when_ssl_is_on() {
        assert_eq!(
            connect_string(&params("dev_db", true)).unwrap(),
            "tcps://localhost:1521/dev_db"
        );
    }

    #[test]
    fn connect_string_requires_a_service_name() {
        assert!(connect_string(&params("   ", false)).is_err());
    }

    #[test]
    fn first_keyword_skips_whitespace_and_comments() {
        assert_eq!(first_keyword("  SELECT 1 FROM dual"), "select");
        assert_eq!(first_keyword("-- note\n  with t as (select 1 from dual) select * from t"), "with");
        assert_eq!(first_keyword("/* a */ /* b */ insert into t values (1)"), "insert");
        assert_eq!(first_keyword(""), "");
    }

    #[test]
    fn only_select_and_with_return_rows() {
        assert!(returns_rows("select 1 from dual"));
        assert!(returns_rows("WITH t AS (SELECT 1 FROM dual) SELECT * FROM t"));
        assert!(!returns_rows("insert into t values (1)"));
        assert!(!returns_rows("create table t (id number)"));
        assert!(!returns_rows("begin null; end;"));
    }

    #[test]
    fn trailing_semicolons_are_stripped_from_plain_sql() {
        assert_eq!(prepare_statement("select 1 from dual;"), "select 1 from dual");
        assert_eq!(prepare_statement("  select 1 from dual ;; \n"), "select 1 from dual");
        assert_eq!(prepare_statement("select 1 from dual"), "select 1 from dual");
    }

    #[test]
    fn plsql_keeps_its_trailing_semicolon() {
        assert_eq!(prepare_statement("begin null; end;"), "begin null; end;");
        assert_eq!(prepare_statement("DECLARE x NUMBER; BEGIN x := 1; END;"), "DECLARE x NUMBER; BEGIN x := 1; END;");
        let procedure = "create or replace procedure p is begin null; end;";
        assert_eq!(prepare_statement(procedure), procedure);
    }

    #[test]
    fn plain_ddl_is_not_treated_as_plsql() {
        assert!(!is_plsql("create table t (id number);"));
        assert_eq!(prepare_statement("create table t (id number);"), "create table t (id number)");
    }

    #[test]
    fn a_table_with_a_column_named_type_is_not_plsql() {
        assert!(!is_plsql("create table t (id number, type varchar2(5))"));
        assert!(is_plsql("create or replace editionable function f return number is begin return 1; end;"));
        assert!(is_plsql("CREATE PACKAGE BODY p AS END;"));
    }

    #[test]
    fn splits_a_script_into_statements() {
        let script = "CREATE TABLE customers (id NUMBER PRIMARY KEY, name VARCHAR2(50));\n\
                      \tINSERT INTO customers (id, name) VALUES (1, 'Ada');\n\
                      INSERT INTO customers (id, name) VALUES (2, 'Grace');\n\
                      SELECT * FROM customers;";
        let statements = split_statements(script);
        assert_eq!(statements.len(), 4);
        assert!(statements[0].starts_with("CREATE TABLE"));
        assert!(statements[1].starts_with("INSERT INTO customers (id, name) VALUES (1"));
        assert_eq!(statements[3], "SELECT * FROM customers");
    }

    #[test]
    fn empty_and_comment_only_statements_are_dropped() {
        assert!(split_statements(";;;;").is_empty());
        assert!(split_statements("  \n ; ;\n").is_empty());
        assert!(split_statements("-- nothing here").is_empty());
        assert_eq!(split_statements("select 1 from dual;; ;select 2 from dual;;;").len(), 2);
        assert_eq!(split_statements("select 1 from dual; -- trailing note").len(), 1);
        assert_eq!(split_statements("-- first\nselect 1 from dual;").len(), 1);
    }

    #[test]
    fn semicolons_inside_literals_and_comments_do_not_split() {
        assert_eq!(split_statements("select 'a;b' from dual; select 2 from dual").len(), 2);
        assert_eq!(split_statements("select 'it''s; fine' from dual").len(), 1);
        assert_eq!(split_statements("select 1 AS \"odd;name\" from dual").len(), 1);
        assert_eq!(split_statements("select 1 /* a; b */ from dual; select 2 from dual").len(), 2);
        assert_eq!(split_statements("select 1 from dual -- a; b\n; select 2 from dual").len(), 2);
        assert_eq!(split_statements("select q'[a;b]' from dual; select q'!c;d!' from dual").len(), 2);
    }

    #[test]
    fn plsql_blocks_end_at_a_slash_line() {
        let statements = split_statements("BEGIN NULL; END;\n/\nSELECT 1 FROM dual;");
        assert_eq!(statements, vec!["BEGIN NULL; END;", "SELECT 1 FROM dual"]);

        let script = "create or replace procedure p is begin null; end;\n/\ncreate table t (id number);";
        let statements = split_statements(script);
        assert_eq!(statements.len(), 2);
        assert!(statements[0].ends_with("end;"));
        assert_eq!(statements[1], "create table t (id number)");
    }

    #[test]
    fn a_lone_plsql_block_needs_no_slash() {
        assert_eq!(split_statements("begin null; end;"), vec!["begin null; end;"]);
    }

    #[test]
    fn a_division_slash_is_not_a_terminator() {
        assert_eq!(split_statements("select 4\n/ 2 from dual").len(), 1);
        assert_eq!(split_statements("select 4 / 2 from dual; select 1 from dual").len(), 2);
    }

    #[test]
    fn a_column_named_type_does_not_swallow_the_next_statement() {
        let script = "create table t (id number, type varchar2(5)); insert into t values (1, 'a');";
        assert_eq!(split_statements(script).len(), 2);
    }

    // ---- live-server tests: `cargo test -p database_backend -- --ignored` ----
    //
    // They need the Docker Oracle (see the repo's Oracle compose file). The
    // defaults match it; override with ORACLE_TEST_HOST / _PORT / _SERVICE /
    // _USER / _PASSWORD. Each test works in its own uniquely named tables and
    // drops them afterwards, so they can run in parallel and re-run safely.

    fn env_or(key: &str, default: &str) -> String {
        std::env::var(key).unwrap_or_else(|_| default.to_owned())
    }

    fn live_params() -> NetworkConnectParams {
        NetworkConnectParams {
            host: env_or("ORACLE_TEST_HOST", "localhost"),
            port: env_or("ORACLE_TEST_PORT", "1521").parse().expect("ORACLE_TEST_PORT"),
            database: env_or("ORACLE_TEST_SERVICE", "dev_db"),
            username: env_or("ORACLE_TEST_USER", "dev_user"),
            password: env_or("ORACLE_TEST_PASSWORD", "devpass123"),
            ssl: false,
        }
    }

    fn live() -> OracleClient {
        connect_blocking(&live_params()).expect("is the Oracle container running and healthy?")
    }

    fn run(client: &OracleClient, sql: &str) -> QueryResult {
        execute_query_blocking(&client.lock().unwrap(), sql)
            .unwrap_or_else(|e| panic!("`{sql}` failed: {e}"))
    }

    /// Drops `table` if present — Oracle has no `DROP TABLE IF EXISTS` before
    /// 23c, and the image is 23, but this stays portable to older servers.
    fn drop_table(client: &OracleClient, table: &str) {
        let _ = execute_query_blocking(
            &client.lock().unwrap(),
            &format!("DROP TABLE {table} CASCADE CONSTRAINTS PURGE"),
        );
    }

    #[test]
    #[ignore = "needs the Oracle container"]
    fn connects_and_selects_from_dual() {
        let client = live();
        let result = run(&client, "SELECT 1 AS one, 'x' AS letter FROM dual;");
        assert_eq!(result.columns, vec!["ONE", "LETTER"]);
        assert_eq!(result.rows, vec![vec![Some("1".to_string()), Some("x".to_string())]]);
        assert_eq!(result.rows_affected, None);
    }

    #[test]
    #[ignore = "needs the Oracle container"]
    fn rejects_a_wrong_password() {
        let mut params = live_params();
        params.password = "definitely-not-the-password".into();
        assert!(connect_blocking(&params).is_err());
    }

    #[test]
    #[ignore = "needs the Oracle container"]
    fn rejects_an_unknown_service() {
        let mut params = live_params();
        params.database = "NO_SUCH_SERVICE".into();
        assert!(connect_blocking(&params).is_err());
    }

    #[test]
    #[ignore = "needs the Oracle container"]
    fn writes_are_committed_and_report_rows_affected() {
        let client = live();
        let table = "FORGE_T_WRITES";
        drop_table(&client, table);

        run(&client, &format!("CREATE TABLE {table} (id NUMBER PRIMARY KEY, name VARCHAR2(50))"));
        let insert = run(&client, &format!("INSERT INTO {table} VALUES (1, 'a')"));
        assert_eq!(insert.rows_affected, Some(1));
        run(&client, &format!("INSERT INTO {table} VALUES (2, 'b')"));
        let update = run(&client, &format!("UPDATE {table} SET name = 'z';"));
        assert_eq!(update.rows_affected, Some(2));

        // A second session only sees the rows if they were committed.
        let other = live();
        let rows = run(&other, &format!("SELECT COUNT(*) FROM {table}"));
        assert_eq!(rows.rows, vec![vec![Some("2".to_string())]]);

        drop_table(&client, table);
    }

    #[test]
    #[ignore = "needs the Oracle container"]
    fn null_empty_and_typed_values_render_correctly() {
        let client = live();
        let result = run(
            &client,
            "SELECT CAST(NULL AS VARCHAR2(5)) AS n, \
                    12.50 AS num, \
                    TO_DATE('2026-10-28', 'YYYY-MM-DD') AS d, \
                    HEXTORAW('DEADBEEF') AS raw_val \
             FROM dual",
        );
        let row = &result.rows[0];
        assert_eq!(row[0], None, "SQL NULL must be None, not an empty string");
        assert_eq!(row[1].as_deref(), Some("12.5"));
        assert!(row[2].as_deref().unwrap().starts_with("2026-10-28"), "got {:?}", row[2]);
        assert_eq!(row[3].as_deref(), Some("DEADBEEF"));
    }

    #[test]
    #[ignore = "needs the Oracle container"]
    fn a_select_matching_nothing_still_reports_its_columns() {
        let client = live();
        let result = run(&client, "SELECT dummy FROM dual WHERE 1 = 0");
        assert_eq!(result.columns, vec!["DUMMY"]);
        assert!(result.rows.is_empty());
        assert_eq!(result.rows_affected, None);
    }

    #[test]
    #[ignore = "needs the Oracle container"]
    fn a_sql_error_is_reported_not_swallowed() {
        let client = live();
        let err = execute_query_blocking(&client.lock().unwrap(), "SELECT * FROM no_such_table_xyz")
            .unwrap_err();
        assert!(err.to_string().contains("ORA-00942"), "got: {err}");
    }

    #[test]
    #[ignore = "needs the Oracle container"]
    fn plsql_blocks_run_with_their_semicolon() {
        let client = live();
        // The block must run (a stripped `;` would raise ORA-06550). Oracle
        // reports a nominal count for a PL/SQL call, so only check that it is
        // a non-row-returning statement.
        let result = run(&client, "BEGIN NULL; END;");
        assert!(result.rows_affected.is_some());
        assert!(result.columns.is_empty() && result.rows.is_empty());
    }

    #[test]
    #[ignore = "needs the Oracle container"]
    fn lists_user_schemas_but_not_oracle_maintained_ones() {
        let client = live();
        let schemas = list_schemas_blocking(&client.lock().unwrap()).unwrap();
        assert!(schemas.iter().any(|s| s == "DEV_USER"), "got {schemas:?}");
        assert!(!schemas.iter().any(|s| s == "SYS" || s == "SYSTEM"), "got {schemas:?}");
    }

    #[test]
    #[ignore = "needs the Oracle container"]
    fn fetch_schema_reports_columns_keys_indexes_and_views() {
        let client = live();
        let (parent, child, view) = ("FORGE_T_PARENT", "FORGE_T_CHILD", "FORGE_V_CHILD");
        let _ = execute_query_blocking(&client.lock().unwrap(), &format!("DROP VIEW {view}"));
        drop_table(&client, child);
        drop_table(&client, parent);

        run(&client, &format!("CREATE TABLE {parent} (id NUMBER PRIMARY KEY, label VARCHAR2(40) NOT NULL)"));
        run(
            &client,
            &format!(
                "CREATE TABLE {child} (id NUMBER PRIMARY KEY, parent_id NUMBER, note VARCHAR2(100), \
                 CONSTRAINT forge_child_fk FOREIGN KEY (parent_id) REFERENCES {parent} (id))"
            ),
        );
        run(&client, &format!("CREATE UNIQUE INDEX forge_child_note_ix ON {child} (note, parent_id)"));
        run(&client, &format!("CREATE VIEW {view} AS SELECT id, note FROM {child}"));

        let schema = fetch_schema_blocking(&client.lock().unwrap()).unwrap();

        let parent_t = schema.tables.iter().find(|t| t.name == parent).expect("parent table listed");
        let id = parent_t.columns.iter().find(|c| c.name == "ID").unwrap();
        assert!(id.primary_key, "ID is the primary key");
        assert!(!id.nullable, "a primary key column is NOT NULL");
        let label = parent_t.columns.iter().find(|c| c.name == "LABEL").unwrap();
        assert!(!label.primary_key);
        assert!(!label.nullable, "LABEL is declared NOT NULL");
        assert_eq!(label.type_name, "VARCHAR2");

        let child_t = schema.tables.iter().find(|t| t.name == child).expect("child table listed");
        let note = child_t.columns.iter().find(|c| c.name == "NOTE").unwrap();
        assert!(note.nullable);
        assert_eq!(child_t.columns[0].name, "ID", "columns come back in table order");

        assert_eq!(child_t.foreign_keys.len(), 1);
        let fk = &child_t.foreign_keys[0];
        assert_eq!(
            (fk.from_column.as_str(), fk.to_table.as_str(), fk.to_column.as_str()),
            ("PARENT_ID", parent, "ID")
        );

        let composite = child_t
            .indexes
            .iter()
            .find(|i| i.name == "FORGE_CHILD_NOTE_IX")
            .expect("explicit index listed");
        assert!(composite.unique);
        assert_eq!(composite.columns, vec!["NOTE", "PARENT_ID"], "key order, not alphabetical");
        assert!(
            child_t.indexes.iter().any(|i| i.unique && i.columns == vec!["ID"]),
            "the implicit primary-key index is reported too"
        );

        let view_i = schema.views.iter().find(|v| v.name == view).expect("view listed");
        assert_eq!(view_i.columns.len(), 2);
        assert!(view_i.definition.to_ascii_lowercase().contains("select"), "got {:?}", view_i.definition);

        let _ = execute_query_blocking(&client.lock().unwrap(), &format!("DROP VIEW {view}"));
        drop_table(&client, child);
        drop_table(&client, parent);
    }

    #[test]
    #[ignore = "needs the Oracle container"]
    fn a_whole_script_runs_in_order_and_returns_the_last_result() {
        let client = live();
        let table = "FORGE_T_SCRIPT";
        drop_table(&client, table);

        let script = format!(
            "CREATE TABLE {table} (id NUMBER PRIMARY KEY, name VARCHAR2(50) NOT NULL, created DATE DEFAULT SYSDATE);\n\
             INSERT INTO {table} (id, name) VALUES (1, 'Ada');\n\
             INSERT INTO {table} (id, name) VALUES (2, 'Grace');;;;\n\
             SELECT id, name FROM {table} ORDER BY id;"
        );
        let result = run(&client, &script);
        assert_eq!(result.columns, vec!["ID", "NAME"]);
        assert_eq!(
            result.rows,
            vec![
                vec![Some("1".to_string()), Some("Ada".to_string())],
                vec![Some("2".to_string()), Some("Grace".to_string())],
            ]
        );

        drop_table(&client, table);
    }

    #[test]
    #[ignore = "needs the Oracle container"]
    fn a_failing_statement_names_itself_and_keeps_earlier_work() {
        let client = live();
        let table = "FORGE_T_SCRIPT_FAIL";
        drop_table(&client, table);

        let script = format!(
            "CREATE TABLE {table} (id NUMBER PRIMARY KEY);\n\
             INSERT INTO {table} VALUES (1);\n\
             INSERT INTO {table} VALUES (1);\n\
             INSERT INTO {table} VALUES (2);"
        );
        let err = execute_query_blocking(&client.lock().unwrap(), &script).unwrap_err().to_string();
        assert!(err.contains("statement 3 of 4 failed"), "got: {err}");
        assert!(err.contains("ORA-00001"), "unique constraint error expected, got: {err}");

        // Statements 1-2 stay applied; 4 never ran.
        let count = run(&client, &format!("SELECT COUNT(*) FROM {table}"));
        assert_eq!(count.rows, vec![vec![Some("1".to_string())]]);

        drop_table(&client, table);
    }

    #[test]
    #[ignore = "needs the Oracle container"]
    fn a_script_can_mix_sql_and_plsql_blocks() {
        let client = live();
        let table = "FORGE_T_SCRIPT_PLSQL";
        drop_table(&client, table);

        let script = format!(
            "CREATE TABLE {table} (id NUMBER);\n\
             BEGIN\n  INSERT INTO {table} VALUES (1);\n  INSERT INTO {table} VALUES (2);\nEND;\n/\n\
             SELECT COUNT(*) AS n FROM {table};"
        );
        let result = run(&client, &script);
        assert_eq!(result.rows, vec![vec![Some("2".to_string())]]);

        drop_table(&client, table);
    }

    #[test]
    #[ignore = "needs the Oracle container"]
    fn registry_connects_queries_fetches_and_disconnects() {
        use crate::connection::{ConnectionId, ConnectionRegistry};

        let runtime = tokio::runtime::Builder::new_current_thread().build().unwrap();
        runtime.block_on(async {
            let mut registry = ConnectionRegistry::new();
            let id = ConnectionId(7);
            let params = live_params();
            let service = params.database.clone();

            registry.connect_oracle(id, params.clone()).await.unwrap();
            assert!(registry.is_database_connected(id, &service));
            assert_eq!(registry.active_database(id).as_deref(), Some(service.as_str()));

            assert!(
                registry.connect_oracle(id, params.clone()).await.is_err(),
                "dialing the same service twice is rejected"
            );

            let result = registry.execute_query(id, "SELECT 42 FROM dual").await.unwrap();
            assert_eq!(result.rows, vec![vec![Some("42".to_string())]]);
            registry.fetch_schema(id).await.unwrap();

            let schemas = crate::connection::list_databases(crate::drivers::DbType::Oracle, params.clone())
                .await
                .unwrap();
            assert!(schemas.iter().any(|s| s == "DEV_USER"));
            assert!(
                crate::connection::create_database(crate::drivers::DbType::Oracle, params, "x")
                    .await
                    .is_err(),
                "Oracle has no create-database"
            );

            registry.disconnect(id).unwrap();
            assert!(!registry.is_connected(id));
        });
    }
}
