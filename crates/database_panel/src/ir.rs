//! UI-independent schema graph intermediate representation.
//!
//! This module converts the database backend's dialect-specific metadata into
//! stable graph identities and relationships. Layout and GPUI rendering should
//! consume this representation instead of depending on driver structs.

use std::collections::{HashMap, HashSet};

use database_backend::{ColumnInfo, ForeignKey, IndexInfo, Schema};

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TableId(pub String);

impl TableId {
    pub fn new(name: impl Into<String>) -> Self {
        Self(name.into())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ColumnId {
    pub table: TableId,
    pub name: String,
}

impl ColumnId {
    pub fn new(table: &TableId, name: impl Into<String>) -> Self {
        Self {
            table: table.clone(),
            name: name.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphColumn {
    pub id: ColumnId,
    pub type_name: String,
    pub nullable: bool,
    pub primary_key: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphIndex {
    pub name: String,
    pub columns: Vec<String>,
    pub unique: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphTable {
    pub id: TableId,
    pub name: String,
    pub columns: Vec<GraphColumn>,
    pub indexes: Vec<GraphIndex>,
    /// True when this table is a conventional many-to-many association table.
    /// The table remains in the graph; this is metadata for a later collapsed
    /// presentation mode.
    pub is_join_table: bool,
    pub join_targets: Vec<TableId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cardinality {
    OneToOne,
    OneToMany,
    /// Not produced yet: a foreign key is never many-to-many on its own.
    /// This is for the later collapsed presentation of join tables.
    #[allow(dead_code)]
    ManyToMany,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphRelationship {
    pub id: String,
    pub source_table: TableId,
    pub source_columns: Vec<ColumnId>,
    pub target_table: TableId,
    pub target_columns: Vec<ColumnId>,
    pub cardinality: Cardinality,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SchemaGraph {
    pub tables: HashMap<TableId, GraphTable>,
    pub relationships: Vec<GraphRelationship>,
}

impl SchemaGraph {
    pub fn from_schema(schema: &Schema) -> Self {
        let mut tables = schema
            .tables
            .iter()
            .map(|table| {
                let id = TableId::new(&table.name);
                let columns = table
                    .columns
                    .iter()
                    .map(|column| graph_column(&id, column))
                    .collect();
                let indexes = table.indexes.iter().map(graph_index).collect();

                (
                    id.clone(),
                    GraphTable {
                        id,
                        name: table.name.clone(),
                        columns,
                        indexes,
                        is_join_table: false,
                        join_targets: Vec::new(),
                    },
                )
            })
            .collect::<HashMap<_, _>>();

        let join_targets = schema
            .tables
            .iter()
            .map(|table| {
                (
                    TableId::new(&table.name),
                    Self::join_table_targets(table, &tables),
                )
            })
            .collect::<HashMap<_, _>>();
        for (id, targets) in join_targets {
            let Some(graph_table) = tables.get_mut(&id) else {
                continue;
            };
            graph_table.is_join_table = targets.is_some();
            graph_table.join_targets = targets.unwrap_or_default();
        }

        let mut relationships = Vec::new();
        for table in &schema.tables {
            let source_table = TableId::new(&table.name);
            for (index, foreign_key) in table.foreign_keys.iter().enumerate() {
                if let Some(relationship) =
                    relationship_from_foreign_key(&source_table, index, foreign_key, &tables)
                {
                    relationships.push(relationship);
                }
            }
        }

        Self {
            tables,
            relationships,
        }
    }

    fn join_table_targets(
        table: &database_backend::TableInfo,
        tables: &HashMap<TableId, GraphTable>,
    ) -> Option<Vec<TableId>> {
        if table.foreign_keys.len() < 2 {
            return None;
        }

        let mut targets = table
            .foreign_keys
            .iter()
            .map(|foreign_key| TableId::new(&foreign_key.to_table))
            .collect::<Vec<_>>();
        targets.sort();
        targets.dedup();
        let table_id = TableId::new(&table.name);
        if targets.len() < 2
            || targets
                .iter()
                .any(|target| target == &table_id || !tables.contains_key(target))
        {
            return None;
        }

        let foreign_key_columns = table
            .foreign_keys
            .iter()
            .map(|foreign_key| foreign_key.from_column.as_str())
            .collect::<HashSet<_>>();
        let covered_by_key = table
            .columns
            .iter()
            .filter(|column| column.primary_key)
            .count()
            == foreign_key_columns.len()
            && table
                .columns
                .iter()
                .filter(|column| column.primary_key)
                .all(|column| foreign_key_columns.contains(column.name.as_str()));
        let covered_by_unique_index = table.indexes.iter().any(|index| {
            index.unique
                && index.columns.len() == foreign_key_columns.len()
                && index
                    .columns
                    .iter()
                    .all(|column| foreign_key_columns.contains(column.as_str()))
        });

        (covered_by_key || covered_by_unique_index).then_some(targets)
    }
}

fn graph_column(table: &TableId, column: &ColumnInfo) -> GraphColumn {
    GraphColumn {
        id: ColumnId::new(table, &column.name),
        type_name: column.type_name.clone(),
        nullable: column.nullable,
        primary_key: column.primary_key,
    }
}

fn graph_index(index: &IndexInfo) -> GraphIndex {
    GraphIndex {
        name: index.name.clone(),
        columns: index.columns.clone(),
        unique: index.unique,
    }
}

fn relationship_from_foreign_key(
    source_table: &TableId,
    index: usize,
    foreign_key: &ForeignKey,
    tables: &HashMap<TableId, GraphTable>,
) -> Option<GraphRelationship> {
    let target_table = TableId::new(&foreign_key.to_table);
    let source = tables.get(source_table)?;
    let target = tables.get(&target_table)?;
    let source_column = source
        .columns
        .iter()
        .find(|column| column.id.name == foreign_key.from_column)?;
    let target_column = target
        .columns
        .iter()
        .find(|column| column.id.name == foreign_key.to_column)?;

    let source_columns = vec![source_column.id.clone()];
    let target_columns = vec![target_column.id.clone()];
    let cardinality = cardinality(source, &source_columns);

    Some(GraphRelationship {
        id: relationship_id(
            source_table,
            &source_columns,
            &target_table,
            &target_columns,
            index,
        ),
        source_table: source_table.clone(),
        source_columns,
        target_table,
        target_columns,
        cardinality,
    })
}

fn cardinality(source: &GraphTable, source_columns: &[ColumnId]) -> Cardinality {
    let columns = source_columns
        .iter()
        .map(|column| column.name.as_str())
        .collect::<HashSet<_>>();
    let is_unique = source.indexes.iter().any(|index| {
        index.unique
            && index.columns.len() == columns.len()
            && index
                .columns
                .iter()
                .all(|column| columns.contains(column.as_str()))
    });
    if is_unique {
        Cardinality::OneToOne
    } else {
        Cardinality::OneToMany
    }
}

fn relationship_id(
    source_table: &TableId,
    source_columns: &[ColumnId],
    target_table: &TableId,
    target_columns: &[ColumnId],
    index: usize,
) -> String {
    let source_columns = source_columns
        .iter()
        .map(|column| column.name.as_str())
        .collect::<Vec<_>>()
        .join(",");
    let target_columns = target_columns
        .iter()
        .map(|column| column.name.as_str())
        .collect::<Vec<_>>()
        .join(",");
    format!(
        "{}:{source_columns}->{}:{target_columns}#{index}",
        source_table.0, target_table.0
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use database_backend::{ColumnInfo, ForeignKey, IndexInfo, TableInfo};

    fn table(
        name: &str,
        columns: Vec<ColumnInfo>,
        foreign_keys: Vec<ForeignKey>,
        indexes: Vec<IndexInfo>,
    ) -> TableInfo {
        TableInfo {
            name: name.into(),
            columns,
            foreign_keys,
            indexes,
        }
    }

    fn column(name: &str, primary_key: bool) -> ColumnInfo {
        ColumnInfo {
            name: name.into(),
            type_name: "integer".into(),
            nullable: !primary_key,
            primary_key,
        }
    }

    #[test]
    fn converts_tables_and_relationships() {
        let schema = Schema {
            tables: vec![
                table(
                    "users",
                    vec![column("id", true)],
                    vec![],
                    vec![IndexInfo {
                        name: "users_pkey".into(),
                        columns: vec!["id".into()],
                        unique: true,
                    }],
                ),
                table(
                    "posts",
                    vec![column("id", true), column("user_id", false)],
                    vec![ForeignKey {
                        from_column: "user_id".into(),
                        to_table: "users".into(),
                        to_column: "id".into(),
                    }],
                    vec![],
                ),
            ],
            views: vec![],
        };

        let graph = SchemaGraph::from_schema(&schema);
        assert_eq!(graph.tables.len(), 2);
        assert_eq!(graph.relationships.len(), 1);
        assert_eq!(graph.relationships[0].cardinality, Cardinality::OneToMany);
        assert_eq!(graph.relationships[0].source_columns[0].name, "user_id");
        assert_eq!(graph.relationships[0].target_columns[0].name, "id");
    }

    #[test]
    fn unique_foreign_key_is_one_to_one() {
        let schema = Schema {
            tables: vec![
                table("users", vec![column("id", true)], vec![], vec![]),
                table(
                    "profiles",
                    vec![column("user_id", false)],
                    vec![ForeignKey {
                        from_column: "user_id".into(),
                        to_table: "users".into(),
                        to_column: "id".into(),
                    }],
                    vec![IndexInfo {
                        name: "profiles_user_id_key".into(),
                        columns: vec!["user_id".into()],
                        unique: true,
                    }],
                ),
            ],
            views: vec![],
        };

        let graph = SchemaGraph::from_schema(&schema);
        assert_eq!(graph.relationships[0].cardinality, Cardinality::OneToOne);
    }

    #[test]
    fn skips_relationships_with_missing_endpoints() {
        let schema = Schema {
            tables: vec![table(
                "posts",
                vec![column("user_id", false)],
                vec![ForeignKey {
                    from_column: "user_id".into(),
                    to_table: "users".into(),
                    to_column: "id".into(),
                }],
                vec![],
            )],
            views: vec![],
        };

        assert!(SchemaGraph::from_schema(&schema).relationships.is_empty());
    }

    #[test]
    fn identifies_visible_many_to_many_join_tables() {
        let schema = Schema {
            tables: vec![
                table("users", vec![column("id", true)], vec![], vec![]),
                table("roles", vec![column("id", true)], vec![], vec![]),
                table(
                    "user_roles",
                    vec![column("user_id", true), column("role_id", true)],
                    vec![
                        ForeignKey {
                            from_column: "user_id".into(),
                            to_table: "users".into(),
                            to_column: "id".into(),
                        },
                        ForeignKey {
                            from_column: "role_id".into(),
                            to_table: "roles".into(),
                            to_column: "id".into(),
                        },
                    ],
                    vec![],
                ),
            ],
            views: vec![],
        };

        let graph = SchemaGraph::from_schema(&schema);
        let join_table = &graph.tables[&TableId::new("user_roles")];
        assert!(join_table.is_join_table);
        assert_eq!(
            join_table.join_targets,
            vec![TableId::new("roles"), TableId::new("users")]
        );
        assert_eq!(graph.relationships.len(), 2);
        assert!(
            graph
                .relationships
                .iter()
                .all(|relationship| { relationship.cardinality == Cardinality::OneToMany })
        );
    }

    #[test]
    fn does_not_mark_unkeyed_multi_foreign_key_tables_as_join_tables() {
        let schema = Schema {
            tables: vec![
                table("users", vec![column("id", true)], vec![], vec![]),
                table("roles", vec![column("id", true)], vec![], vec![]),
                table(
                    "audit_events",
                    vec![
                        column("id", true),
                        column("user_id", false),
                        column("role_id", false),
                    ],
                    vec![
                        ForeignKey {
                            from_column: "user_id".into(),
                            to_table: "users".into(),
                            to_column: "id".into(),
                        },
                        ForeignKey {
                            from_column: "role_id".into(),
                            to_table: "roles".into(),
                            to_column: "id".into(),
                        },
                    ],
                    vec![],
                ),
            ],
            views: vec![],
        };

        let graph = SchemaGraph::from_schema(&schema);
        assert!(!graph.tables[&TableId::new("audit_events")].is_join_table);
    }

    #[test]
    fn does_not_mark_self_referential_tables_as_join_tables() {
        let schema = Schema {
            tables: vec![
                table("users", vec![column("id", true)], vec![], vec![]),
                table(
                    "user_links",
                    vec![column("user_id", true), column("related_user_id", true)],
                    vec![
                        ForeignKey {
                            from_column: "user_id".into(),
                            to_table: "users".into(),
                            to_column: "id".into(),
                        },
                        ForeignKey {
                            from_column: "related_user_id".into(),
                            to_table: "user_links".into(),
                            to_column: "user_id".into(),
                        },
                    ],
                    vec![],
                ),
            ],
            views: vec![],
        };

        let graph = SchemaGraph::from_schema(&schema);
        assert!(!graph.tables[&TableId::new("user_links")].is_join_table);
    }
}
