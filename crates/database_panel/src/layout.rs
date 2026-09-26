//! Deterministic, UI-independent layout for schema graph tables.

use std::collections::{HashMap, HashSet, VecDeque};

use super::ir::{GraphRelationship, SchemaGraph, TableId};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LayoutConfig {
    pub node_width: f32,
    pub column_row_height: f32,
    pub header_height: f32,
    pub layer_spacing_x: f32,
    pub node_spacing_y: f32,
    pub origin_x: f32,
    pub origin_y: f32,
}

impl Default for LayoutConfig {
    fn default() -> Self {
        Self {
            node_width: 260.0,
            column_row_height: 26.0,
            header_height: 42.0,
            layer_spacing_x: 120.0,
            node_spacing_y: 60.0,
            origin_x: 40.0,
            origin_y: 40.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TableLayout {
    pub position: Point,
    pub width: f32,
    pub height: f32,
    pub layer: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SchemaLayout {
    pub tables: HashMap<TableId, TableLayout>,
    pub ignored_cycle_relationships: Vec<String>,
}

impl SchemaLayout {
    pub fn compute(graph: &SchemaGraph, config: LayoutConfig) -> Self {
        let mut ids = graph.tables.keys().cloned().collect::<Vec<_>>();
        ids.sort();

        let relationships = graph
            .relationships
            .iter()
            .filter(|relationship| {
                graph.tables.contains_key(&relationship.source_table)
                    && graph.tables.contains_key(&relationship.target_table)
            })
            .collect::<Vec<_>>();
        let (acyclic_edges, ignored_cycle_relationships) = cycle_free_edges(&ids, &relationships);

        let mut children = ids
            .iter()
            .map(|id| (id.clone(), Vec::new()))
            .collect::<HashMap<_, Vec<_>>>();
        let mut indegree = ids
            .iter()
            .map(|id| (id.clone(), 0usize))
            .collect::<HashMap<_, _>>();
        for (parent, child) in &acyclic_edges {
            children.get_mut(parent).unwrap().push(child.clone());
            *indegree.get_mut(child).unwrap() += 1;
        }
        for children in children.values_mut() {
            children.sort();
        }

        let mut queue = ids
            .iter()
            .filter(|id| indegree[*id] == 0)
            .cloned()
            .collect::<VecDeque<_>>();
        let mut ranks = ids
            .iter()
            .map(|id| (id.clone(), 0usize))
            .collect::<HashMap<_, _>>();
        while let Some(parent) = queue.pop_front() {
            let parent_rank = ranks[&parent];
            for child in &children[&parent] {
                let rank = ranks.get_mut(child).unwrap();
                *rank = (*rank).max(parent_rank + 1);
                let degree = indegree.get_mut(child).unwrap();
                *degree -= 1;
                if *degree == 0 {
                    queue.push_back(child.clone());
                }
            }
        }

        let mut layers = Vec::<Vec<TableId>>::new();
        for id in &ids {
            let rank = ranks[id];
            if layers.len() <= rank {
                layers.resize_with(rank + 1, Vec::new);
            }
            layers[rank].push(id.clone());
        }
        for layer in &mut layers {
            layer.sort();
        }
        reduce_crossings(&mut layers, &acyclic_edges);

        let mut tables = HashMap::new();
        for (layer_index, layer) in layers.iter().enumerate() {
            let mut y = config.origin_y;
            for id in layer {
                let table = &graph.tables[id];
                let height =
                    config.header_height + table.columns.len() as f32 * config.column_row_height;
                tables.insert(
                    id.clone(),
                    TableLayout {
                        position: Point {
                            x: config.origin_x
                                + layer_index as f32 * (config.node_width + config.layer_spacing_x),
                            y,
                        },
                        width: config.node_width,
                        height,
                        layer: layer_index,
                    },
                );
                y += height + config.node_spacing_y;
            }
        }

        Self {
            tables,
            ignored_cycle_relationships,
        }
    }
}

fn cycle_free_edges(
    ids: &[TableId],
    relationships: &[&GraphRelationship],
) -> (Vec<(TableId, TableId)>, Vec<String>) {
    let mut ordered = relationships.to_vec();
    ordered.sort_by(|left, right| left.id.cmp(&right.id));

    let mut adjacency = ids
        .iter()
        .map(|id| (id.clone(), Vec::new()))
        .collect::<HashMap<_, Vec<_>>>();
    let mut accepted = Vec::new();
    let mut ignored = Vec::new();

    for relationship in ordered {
        let parent = relationship.target_table.clone();
        let child = relationship.source_table.clone();
        if parent == child || reachable(&adjacency, &child, &parent) {
            ignored.push(relationship.id.clone());
        } else {
            adjacency.get_mut(&parent).unwrap().push(child.clone());
            accepted.push((parent, child));
        }
    }
    ignored.sort();
    (accepted, ignored)
}

fn reachable(
    adjacency: &HashMap<TableId, Vec<TableId>>,
    start: &TableId,
    target: &TableId,
) -> bool {
    let mut pending = vec![start.clone()];
    let mut visited = HashSet::new();
    while let Some(current) = pending.pop() {
        if &current == target {
            return true;
        }
        if visited.insert(current.clone()) {
            if let Some(next) = adjacency.get(&current) {
                pending.extend(next.iter().cloned());
            }
        }
    }
    false
}

fn reduce_crossings(layers: &mut [Vec<TableId>], edges: &[(TableId, TableId)]) {
    let positions = |layer: &[TableId]| {
        layer
            .iter()
            .enumerate()
            .map(|(index, id)| (id.clone(), index as f32))
            .collect::<HashMap<_, _>>()
    };

    for _ in 0..2 {
        let previous_positions = layers
            .iter()
            .map(|layer| positions(layer))
            .collect::<Vec<_>>();
        for layer_index in 1..layers.len() {
            let mut barycenters = layers[layer_index]
                .iter()
                .map(|id| {
                    let neighbors = edges
                        .iter()
                        .filter(|(_, child)| child == id)
                        .filter_map(|(parent, _)| previous_positions[layer_index - 1].get(parent))
                        .copied()
                        .collect::<Vec<_>>();
                    let barycenter = if neighbors.is_empty() {
                        f32::INFINITY
                    } else {
                        neighbors.iter().sum::<f32>() / neighbors.len() as f32
                    };
                    (id.clone(), barycenter)
                })
                .collect::<Vec<_>>();
            barycenters.sort_by(|left, right| {
                left.1
                    .total_cmp(&right.1)
                    .then_with(|| left.0.cmp(&right.0))
            });
            layers[layer_index] = barycenters.into_iter().map(|(id, _)| id).collect();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ir::{ColumnId, GraphColumn, GraphTable};

    fn graph(names: &[&str], relationships: &[(&str, &str, &str)]) -> SchemaGraph {
        let tables = names
            .iter()
            .map(|name| {
                let id = TableId::new(*name);
                (
                    id.clone(),
                    GraphTable {
                        id,
                        name: name.to_string(),
                        columns: vec![GraphColumn {
                            id: ColumnId::new(&TableId::new(*name), "id"),
                            type_name: "integer".into(),
                            nullable: false,
                            primary_key: true,
                        }],
                        indexes: Vec::new(),
                        is_join_table: false,
                        join_targets: Vec::new(),
                    },
                )
            })
            .collect();
        SchemaGraph {
            tables,
            relationships: relationships
                .iter()
                .map(|(id, source, target)| GraphRelationship {
                    id: (*id).into(),
                    source_table: TableId::new(*source),
                    source_columns: Vec::new(),
                    target_table: TableId::new(*target),
                    target_columns: Vec::new(),
                    cardinality: crate::ir::Cardinality::OneToMany,
                })
                .collect(),
        }
    }

    #[test]
    fn lays_out_dependencies_left_to_right() {
        let layout = SchemaLayout::compute(
            &graph(
                &["users", "posts", "comments"],
                &[
                    ("posts-users", "posts", "users"),
                    ("comments-posts", "comments", "posts"),
                ],
            ),
            LayoutConfig::default(),
        );
        assert_eq!(layout.tables[&TableId::new("users")].layer, 0);
        assert_eq!(layout.tables[&TableId::new("posts")].layer, 1);
        assert_eq!(layout.tables[&TableId::new("comments")].layer, 2);
        assert!(
            layout.tables[&TableId::new("users")].position.x
                < layout.tables[&TableId::new("comments")].position.x
        );
    }

    #[test]
    fn breaks_cycles_without_dropping_the_relationship() {
        let layout = SchemaLayout::compute(
            &graph(&["a", "b"], &[("a-b", "a", "b"), ("b-a", "b", "a")]),
            LayoutConfig::default(),
        );
        assert_eq!(layout.tables.len(), 2);
        assert_eq!(layout.ignored_cycle_relationships, vec!["b-a"]);
    }

    #[test]
    fn positions_are_deterministic_and_size_uses_columns() {
        let mut graph = graph(&["users"], &[]);
        graph
            .tables
            .get_mut(&TableId::new("users"))
            .unwrap()
            .columns
            .push(GraphColumn {
                id: ColumnId::new(&TableId::new("users"), "name"),
                type_name: "text".into(),
                nullable: true,
                primary_key: false,
            });
        let first = SchemaLayout::compute(&graph, LayoutConfig::default());
        let second = SchemaLayout::compute(&graph, LayoutConfig::default());
        assert_eq!(first, second);
        assert_eq!(first.tables[&TableId::new("users")].height, 94.0);
    }
}
