//! Conversion from the schema IR/layout into `gpui_flow` graph state.

use std::collections::HashSet;

use gpui::{
    AnyElement, App, AppContext as _, Context, EventEmitter, FocusHandle, Focusable, FontWeight,
    IntoElement, ParentElement as _, Render, SharedString, Styled as _, Window, div, px,
};
use gpui_component::ActiveTheme as _;
use gpui_flow::{Controls, FlowGraph, FlowState, Minimap};
use gpui_flow::{EdgeType, FlowEdge, FlowNode, HandleDef, HandlePosition, HandleType};

use super::{
    ir::{GraphRelationship, SchemaGraph, TableId},
    layout::{SchemaLayout, TableLayout},
};
use workspace::{Item, Workspace};

pub fn flow_graph(graph: &SchemaGraph, layout: &SchemaLayout) -> (Vec<FlowNode>, Vec<FlowEdge>) {
    let mut table_ids = graph.tables.keys().cloned().collect::<Vec<_>>();
    table_ids.sort();

    let nodes = table_ids
        .iter()
        .filter_map(|table_id| {
            let table = graph.tables.get(table_id)?;
            let table_layout = layout.tables.get(table_id)?;
            Some(table_node(table_id, table_layout, table, graph))
        })
        .collect::<Vec<_>>();
    let edges = graph
        .relationships
        .iter()
        .filter_map(|relationship| relationship_edge(relationship, graph, layout))
        .collect::<Vec<_>>();

    (nodes, edges)
}

pub struct SchemaGraphView {
    flow: gpui::Entity<FlowGraph>,
    controls: gpui::Entity<Controls>,
    minimap: gpui::Entity<Minimap>,
    focus_handle: FocusHandle,
}

impl SchemaGraphView {
    pub fn new(graph: &SchemaGraph, layout: &SchemaLayout, cx: &mut Context<Self>) -> Self {
        let (nodes, edges) = flow_graph(graph, layout);
        let state = cx.new(|_| FlowState::new(nodes, edges));
        let flow =
            cx.new(|cx| FlowGraph::new(state.clone(), cx).default_renderer(render_table_card));
        let controls = cx.new(|_| Controls::new(state.clone()));
        let minimap = cx.new(|_| Minimap::new(state));
        Self {
            flow,
            controls,
            minimap,
            focus_handle: cx.focus_handle(),
        }
    }
}

impl EventEmitter<()> for SchemaGraphView {}

impl Focusable for SchemaGraphView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for SchemaGraphView {
    fn render(&mut self, _window: &mut gpui::Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        self.flow.update(cx, |flow, _| {
            flow.set_theme_colors(
                color_u32(colors.background),
                color_u32(colors.border),
                color_u32(colors.background),
                color_u32(colors.border),
                color_u32(colors.accent),
            );
        });
        self.controls.update(cx, |controls, _| {
            controls.theme_colors(colors.popover, colors.border, colors.foreground);
        });
        self.minimap.update(cx, |minimap, _| {
            minimap.theme_colors(colors.popover, colors.border);
        });
        div()
            .size_full()
            .relative()
            .bg(colors.background)
            .child(self.flow.clone())
            .child(
                div()
                    .absolute()
                    .bottom(px(16.0))
                    .left(px(16.0))
                    .child(self.controls.clone()),
            )
            .child(
                div()
                    .absolute()
                    .bottom(px(16.0))
                    .right(px(16.0))
                    .child(self.minimap.clone()),
            )
    }
}

pub struct SchemaGraphTab {
    graph: gpui::Entity<SchemaGraphView>,
    key: super::SchemaKey,
    title: SharedString,
}

impl SchemaGraphTab {
    fn new(
        graph: gpui::Entity<SchemaGraphView>,
        key: super::SchemaKey,
        title: SharedString,
    ) -> Self {
        Self { graph, key, title }
    }
}

impl EventEmitter<()> for SchemaGraphTab {}

impl Focusable for SchemaGraphTab {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.graph.read(cx).focus_handle(cx)
    }
}

impl Render for SchemaGraphTab {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        self.graph.clone()
    }
}

impl Item for SchemaGraphTab {
    type Event = ();

    fn tab_content_text(&self, _detail: usize, _cx: &App) -> SharedString {
        self.title.clone()
    }

    fn tab_tooltip_text(&self, _cx: &App) -> Option<SharedString> {
        Some(self.title.clone())
    }
}

impl super::DatabasePanel {
    pub(crate) fn open_schema_graph_tab(
        &mut self,
        id: super::ConnectionId,
        window: &mut Window,
        cx: &mut Context<super::DatabasePanel>,
    ) {
        let database = self.registry.active_database(id).unwrap_or_default();
        let key = (id, database.clone());
        let Some(super::SchemaState::Loaded { tables, .. }) = self.schemas.get(&key) else {
            return;
        };
        let tables = tables.clone();
        let graph = self.schema_graph_for_key(&key, &tables, cx);
        let title = self
            .connections
            .iter()
            .find(|connection| connection.id == id)
            .map(|connection| {
                if database.is_empty() {
                    format!("Schema: {}", connection.title)
                } else {
                    format!("Schema: {} / {database}", connection.title)
                }
            })
            .unwrap_or_else(|| "Schema Graph".to_string())
            .into();
        let Some(workspace) = self.workspace.upgrade() else {
            return;
        };
        workspace.update(cx, |workspace, cx| {
            open_schema_graph(graph, key, title, workspace, window, cx);
        });
    }

    fn schema_graph_for_key(
        &mut self,
        key: &super::SchemaKey,
        tables: &[database_backend::TableInfo],
        cx: &mut Context<super::DatabasePanel>,
    ) -> gpui::Entity<SchemaGraphView> {
        if !self.schema_graphs.contains_key(key) {
            let schema = database_backend::Schema {
                tables: tables.to_vec(),
                views: Vec::new(),
            };
            let graph = SchemaGraph::from_schema(&schema);
            let layout = SchemaLayout::compute(&graph, crate::layout::LayoutConfig::default());
            let view = cx.new(|cx| SchemaGraphView::new(&graph, &layout, cx));
            self.schema_graphs.insert(key.clone(), view);
        }
        self.schema_graphs[key].clone()
    }
}

fn open_schema_graph(
    graph: gpui::Entity<SchemaGraphView>,
    key: super::SchemaKey,
    title: SharedString,
    workspace: &mut Workspace,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    let existing = workspace
        .active_pane()
        .read(cx)
        .items()
        .find_map(|item| item.downcast::<SchemaGraphTab>())
        .filter(|tab| tab.read(cx).title == title && key == tab.read(cx).key);

    if let Some(existing) = existing {
        workspace.activate_item(&existing, true, true, window, cx);
    } else {
        let tab = cx.new(|_| SchemaGraphTab::new(graph, key, title));
        workspace.add_item_to_active_pane(Box::new(tab), None, true, window, cx);
    }
}

fn render_table_card(
    node: &FlowNode,
    _window: &mut gpui::Window,
    cx: &mut gpui::App,
) -> AnyElement {
    let theme = cx.theme();
    div()
        .flex()
        .flex_col()
        .gap_1()
        .p_2()
        .rounded_md()
        .bg(theme.colors.background)
        .border_1()
        .border_color(theme.colors.border)
        .text_color(theme.colors.foreground)
        .child(
            div()
                .pb_1()
                .border_b_1()
                .border_color(theme.colors.border)
                .font_weight(FontWeight::BOLD)
                .child(node.label.clone()),
        )
        .children(node.properties.iter().map(|(name, type_name)| {
            div()
                .flex()
                .justify_between()
                .gap_2()
                .text_xs()
                .child(name.clone())
                .child(
                    div()
                        .text_color(theme.colors.muted_foreground)
                        .child(type_name.clone()),
                )
        }))
        .into_any_element()
}

fn color_u32(color: gpui::Hsla) -> u32 {
    let color = color.to_rgb();
    ((color.r * 255.0).round() as u32) << 16
        | ((color.g * 255.0).round() as u32) << 8
        | (color.b * 255.0).round() as u32
}

fn table_node(
    table_id: &TableId,
    table_layout: &TableLayout,
    table: &super::ir::GraphTable,
    graph: &SchemaGraph,
) -> FlowNode {
    let mut handles = Vec::new();
    let mut seen = HashSet::new();

    for relationship in &graph.relationships {
        if relationship.source_table == *table_id {
            for column in &relationship.source_columns {
                let id = handle_id(column);
                if seen.insert((id.clone(), HandleType::Source)) {
                    handles.push(HandleDef::source(HandlePosition::Left).id(id));
                }
            }
        }
        if relationship.target_table == *table_id {
            for column in &relationship.target_columns {
                let id = handle_id(column);
                if seen.insert((id.clone(), HandleType::Target)) {
                    handles.push(HandleDef::target(HandlePosition::Right).id(id));
                }
            }
        }
    }

    let mut node = FlowNode::new(
        table_id.0.clone(),
        table_layout.position.x,
        table_layout.position.y,
    )
    .label(table.name.clone())
    .node_type("database-table")
    .size(table_layout.width, table_layout.height)
    .handles(handles);
    node.properties = table
        .columns
        .iter()
        .map(|column| {
            (
                column.id.name.clone().into(),
                column.type_name.clone().into(),
            )
        })
        .collect();
    node.draggable = false;
    node.connectable = false;
    node.deletable = false;
    node
}

fn relationship_edge(
    relationship: &GraphRelationship,
    graph: &SchemaGraph,
    layout: &SchemaLayout,
) -> Option<FlowEdge> {
    if !layout.tables.contains_key(&relationship.source_table)
        || !layout.tables.contains_key(&relationship.target_table)
        || !graph.tables.contains_key(&relationship.source_table)
        || !graph.tables.contains_key(&relationship.target_table)
    {
        return None;
    }

    let mut edge = FlowEdge::new(
        relationship.id.clone(),
        relationship.source_table.0.clone(),
        relationship.target_table.0.clone(),
    )
    .edge_type(EdgeType::SmoothStep {
        border_radius: 8.0,
        offset: 24.0,
    })
    .label(cardinality_label(relationship));

    if let Some(column) = relationship.source_columns.first() {
        edge = edge.source_handle(handle_id(column));
    }
    if let Some(column) = relationship.target_columns.first() {
        edge = edge.target_handle(handle_id(column));
    }
    Some(edge)
}

fn handle_id(column: &super::ir::ColumnId) -> String {
    format!("{}.{}", column.table.0, column.name)
}

fn cardinality_label(relationship: &GraphRelationship) -> &'static str {
    match relationship.cardinality {
        super::ir::Cardinality::OneToOne => "1 : 1",
        super::ir::Cardinality::OneToMany => "1 : N",
        super::ir::Cardinality::ManyToMany => "N : N",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        ir::{Cardinality, ColumnId, GraphColumn, GraphRelationship, GraphTable},
        layout::{LayoutConfig, SchemaLayout},
    };
    use std::collections::HashMap;

    fn graph() -> SchemaGraph {
        let users = TableId::new("users");
        let posts = TableId::new("posts");
        SchemaGraph {
            tables: HashMap::from([
                (
                    users.clone(),
                    GraphTable {
                        id: users.clone(),
                        name: "users".into(),
                        columns: vec![GraphColumn {
                            id: ColumnId::new(&users, "id"),
                            type_name: "integer".into(),
                            nullable: false,
                            primary_key: true,
                        }],
                        indexes: Vec::new(),
                        is_join_table: false,
                        join_targets: Vec::new(),
                    },
                ),
                (
                    posts.clone(),
                    GraphTable {
                        id: posts.clone(),
                        name: "posts".into(),
                        columns: vec![
                            GraphColumn {
                                id: ColumnId::new(&posts, "id"),
                                type_name: "integer".into(),
                                nullable: false,
                                primary_key: true,
                            },
                            GraphColumn {
                                id: ColumnId::new(&posts, "user_id"),
                                type_name: "integer".into(),
                                nullable: false,
                                primary_key: false,
                            },
                        ],
                        indexes: Vec::new(),
                        is_join_table: false,
                        join_targets: Vec::new(),
                    },
                ),
            ]),
            relationships: vec![GraphRelationship {
                id: "posts: user_id -> users: id".into(),
                source_table: posts.clone(),
                source_columns: vec![ColumnId::new(&posts, "user_id")],
                target_table: users.clone(),
                target_columns: vec![ColumnId::new(&users, "id")],
                cardinality: Cardinality::OneToMany,
            }],
        }
    }

    #[test]
    fn builds_column_anchored_nodes_and_edges() {
        let graph = graph();
        let layout = SchemaLayout::compute(&graph, LayoutConfig::default());
        let (nodes, edges) = flow_graph(&graph, &layout);

        assert_eq!(nodes.len(), 2);
        assert_eq!(edges.len(), 1);
        assert_eq!(edges[0].source_handle.as_deref(), Some("posts.user_id"));
        assert_eq!(edges[0].target_handle.as_deref(), Some("users.id"));
        assert_eq!(edges[0].label.as_deref(), Some("1 : N"));
        assert!(!nodes.iter().any(|node| node.draggable));
        assert!(!nodes.iter().any(|node| node.connectable));
        assert!(nodes.iter().any(|node| {
            node.id.as_ref() == "posts"
                && node.handles.iter().any(|handle| {
                    handle.id.as_deref() == Some("posts.user_id")
                        && handle.handle_type == HandleType::Source
                        && handle.position == HandlePosition::Left
                })
        }));
    }
}
