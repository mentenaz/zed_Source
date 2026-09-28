//! Ad-hoc SQL query execution — the SQL workbench's backend half.
//!
//! Mirrors the shape Forge's own `DbQueryResult`/`db_query` used (confirmed
//! via `src-tauri/src/database.rs`): a single `QueryResult` shape covers
//! both a `SELECT`-shaped result set (`columns`/`rows` populated,
//! `rows_affected: None`) and a write statement (`columns`/`rows` empty,
//! `rows_affected: Some(n)`), so the UI doesn't need per-driver branching to
//! know which one it got.

/// The result of running one ad-hoc SQL statement.
#[derive(Debug, Clone, Default)]
pub struct QueryResult {
    pub columns: Vec<String>,
    /// `None` for a cell means SQL `NULL`, not an empty string.
    pub rows: Vec<Vec<Option<String>>>,
    /// Only set for a statement that didn't return a result set (INSERT/
    /// UPDATE/DELETE/DDL) — a `SELECT` that matched zero rows still has
    /// `columns` populated and this stays `None`, so the UI can tell "no
    /// rows" apart from "not a row-returning statement" and render the
    /// (empty) table instead of a rows-affected message.
    pub rows_affected: Option<u64>,
    pub exec_ms: u64,
}
