use serde::{Deserialize, Serialize};

/// A table discovered in a database's schema.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TableInfo {
    pub name: String,
    pub columns: Vec<ColumnInfo>,
    pub foreign_keys: Vec<ForeignKey>,
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
