use serde::{Deserialize, Serialize};

/// A table discovered in a database's schema.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TableInfo {
    pub name: String,
    pub columns: Vec<ColumnInfo>,
    pub foreign_keys: Vec<ForeignKey>,
    pub indexes: Vec<IndexInfo>,
}

/// A single column's shape, as shown by the schema tree's expanded rows.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ColumnInfo {
    pub name: String,
    pub type_name: String,
    pub nullable: bool,
    pub primary_key: bool,
}

/// A foreign-key relationship, used by the later schema graph phase.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForeignKey {
    pub from_column: String,
    pub to_table: String,
    pub to_column: String,
}

/// An index on a table, including the implicit one backing a `PRIMARY KEY`
/// or `UNIQUE` constraint (drivers don't filter those out — this reports
/// what the server actually has, same as any other database tool would).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexInfo {
    pub name: String,
    /// In key order, not alphabetical — order matters for a composite index.
    pub columns: Vec<String>,
    pub unique: bool,
}

/// A view discovered in a database's schema. `columns` come from the same
/// per-name column introspection every driver already runs for a table —
/// a view's result-set shape is queryable the same way a table's is (SQLite:
/// `PRAGMA table_info` on the view name; Postgres/MySQL/MSSQL: the same
/// `information_schema.columns`-backed table_columns query with `table` set
/// to the view's name) — so no view-specific column query was needed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ViewInfo {
    pub name: String,
    pub columns: Vec<ColumnInfo>,
    /// The view's defining `SELECT` statement, as the server reports it.
    pub definition: String,
}

/// One database's full schema: every table and every view.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Schema {
    pub tables: Vec<TableInfo>,
    pub views: Vec<ViewInfo>,
}
