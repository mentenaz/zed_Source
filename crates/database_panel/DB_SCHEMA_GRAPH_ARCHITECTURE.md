# Database Schema Graph Architecture & Implementation Specification

This specification provides a complete blueprint for building a **Database Schema Graph Visualizer** in GPUI using [`crates/gpui_flow`](file:///E:/zed_Source/crates/gpui_flow) within `database_panel`.

---

## 1. High-Level Architecture & Pipeline

```
  ┌─────────────────────────────────────────────────────────┐
  │              1. Schema Extraction Layer                │
  │   - Live Introspection (PostgreSQL, SQLite, MySQL, MSSQL) │
  │   - SQL DDL Parser (.sql migration / dump files)        │
  └────────────────────────────┬────────────────────────────┘
                               │
                               ▼
  ┌─────────────────────────────────────────────────────────┐
  │               2. Normalized Schema IR                   │
  │   - Tables, Columns, Data Types, PKs, FK Constraints    │
  └────────────────────────────┬────────────────────────────┘
                               │
                               ▼
  ┌─────────────────────────────────────────────────────────┐
  │         3. Sugiyama / Layered DAG Layout Engine         │
  │   - Cycle Removal -> Topological Layering              │
  │   - Crossing Reduction -> FlowPoint(x, y) Assignment    │
  └────────────────────────────┬────────────────────────────┘
                               │
                               ▼
  ┌─────────────────────────────────────────────────────────┐
  │             4. gpui_flow State Generation               │
  │   - FlowNode (with column-level HandleDefs)             │
  │   - FlowEdge (SmoothStep, "1 : N" labels, colors)       │
  └────────────────────────────┬────────────────────────────┘
                               │
                               ▼
  ┌─────────────────────────────────────────────────────────┐
  │                5. GPUI Render Layer                     │
  │   - Custom Table Card Renderer (Header + Column Rows)   │
  │   - Relationship Highlighting & Search / Filter Toolbar │
  │   - Minimap & Zoom / Pan Controls Overlay               │
  └─────────────────────────────────────────────────────────┘
```

---

## 2. Normalized Schema Intermediate Representation (Schema IR)

To support multiple SQL dialects seamlessly, decouple database drivers/parsers from the graph visualizer using a common Intermediate Representation:

```rust
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TableId(pub String);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ColumnId {
    pub table: TableId,
    pub column: String,
}

#[derive(Debug, Clone)]
pub struct ColumnDef {
    pub name: String,
    pub data_type: String,
    pub is_primary_key: bool,
    pub is_foreign_key: bool,
    pub is_nullable: bool,
    pub default_value: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ForeignKeyConstraint {
    pub name: String,
    pub source_table: TableId,
    pub source_columns: Vec<String>,
    pub target_table: TableId,
    pub target_columns: Vec<String>,
    pub cardinality: Cardinality,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cardinality {
    OneToOne,
    OneToMany,
    ManyToMany,
}

#[derive(Debug, Clone)]
pub struct TableDef {
    pub id: TableId,
    pub name: String,
    pub schema_name: Option<String>,
    pub columns: Vec<ColumnDef>,
    pub foreign_keys: Vec<ForeignKeyConstraint>,
}

#[derive(Debug, Clone, Default)]
pub struct SchemaIR {
    pub tables: HashMap<TableId, TableDef>,
}
```

---

## 3. Multi-Dialect Introspection Layer

Provide a unified trait for fetching database metadata across SQLite, PostgreSQL, MySQL, and MSSQL:

```rust
pub trait SchemaProvider {
    fn dialect_name(&self) -> &'static str;
    fn fetch_schema(&self) -> Result<SchemaIR, String>;
}

// PostgreSQL Introspection Query (Information Schema)
// Queries information_schema.tables, information_schema.columns,
// information_schema.table_constraints, and key_column_usage.

// SQLite Introspection
// Queries `PRAGMA table_list`, `PRAGMA table_info(tbl)`, and `PRAGMA foreign_key_list(tbl)`.

// MySQL Introspection
// Queries INFORMATION_SCHEMA.KEY_COLUMN_USAGE and REFERENTIAL_CONSTRAINTS.

// MSSQL Introspection
// Queries sys.foreign_keys, sys.foreign_key_columns, sys.tables, sys.columns.
```

---

## 4. Automatic Sugiyama / Layered DAG Layout Engine

Database schemas naturally form Directed Acyclic Graphs (DAGs) where foreign key arrows point from child tables to parent primary key tables. The **Sugiyama Layout Engine** automatically calculates optimal `(x, y)` flow coordinates.

### 4 Stages of the Layout Pipeline

1. **Cycle Breaking**:
   Detect circular foreign key dependencies (e.g. `users` -> `teams` -> `users`) using Depth-First Search (DFS) back-edge detection. Temporarily reverse back-edges during layout calculation to enforce acyclic hierarchy.

2. **Layer Assignment (Rank Assignment)**:
   Place root tables (tables with zero outgoing foreign keys, e.g. lookup tables or core entities) at Rank 0. Dependent child tables are placed at Rank `N + 1`.

   ```
   [Rank 0: Organizations]       [Rank 0: Roles]
              ▲                         ▲
              │                         │
   [Rank 1: Users (FK org_id, role_id)]
              ▲
              │
   [Rank 2: Orders (FK user_id)]
   ```

3. **Crossing Reduction (Barycenter / Median Heuristic)**:
   Reorder nodes within each horizontal rank to minimize crossing lines between column handles.

4. **Coordinate Assignment**:
   Calculate exact pixel flow positions `FlowPoint(x, y)` taking into account table card widths (e.g. 260px) and vertical height calculated from column counts:

```rust
pub struct LayoutEngine {
    pub node_width: f32,       // 260.0 px
    pub column_row_height: f32,// 26.0 px
    pub header_height: f32,    // 42.0 px
    pub layer_spacing_x: f32,  // 120.0 px
    pub node_spacing_y: f32,   // 60.0 px
}

impl LayoutEngine {
    pub fn compute_layout(&self, schema: &SchemaIR) -> HashMap<TableId, FlowPoint> {
        let mut positions = HashMap::new();
        let ranks = self.assign_ranks(schema);

        let mut current_x = 40.0;
        for rank in ranks {
            let mut current_y = 40.0;
            for table_id in rank {
                if let Some(table) = schema.tables.get(&table_id) {
                    positions.insert(table_id.clone(), FlowPoint::new(current_x, current_y));
                    
                    let table_height = self.header_height + (table.columns.len() as f32 * self.column_row_height);
                    current_y += table_height + self.node_spacing_y;
                }
            }
            current_x += self.node_width + self.layer_spacing_x;
        }

        positions
    }

    fn assign_ranks(&self, schema: &SchemaIR) -> Vec<Vec<TableId>> {
        // Topological sort / longest path layering
        // ...
        vec![]
    }
}
```

---

## 5. Column-Level Handle Anchoring in `gpui_flow`

To anchor relationship edges directly to the specific PK and FK column rows inside tables:

### A. Constructing Column-Level `HandleDef`s
Assign a unique handle ID for every column in the table:

```rust
use gpui_flow::*;

pub fn build_table_node(table: &TableDef, pos: FlowPoint) -> FlowNode {
    let mut handles = Vec::new();

    for col in &table.columns {
        let handle_id = format!("{}.{}", table.name, col.name);

        if col.is_primary_key || col.is_foreign_key {
            // Target handle on Left for incoming relationships
            handles.push(
                HandleDef::target(HandlePosition::Left)
                    .id(format!("target_{}", handle_id))
            );

            // Source handle on Right for outgoing foreign keys
            handles.push(
                HandleDef::source(HandlePosition::Right)
                    .id(format!("source_{}", handle_id))
            );
        }
    }

    let card_height = 42.0 + (table.columns.len() as f32 * 26.0);

    FlowNode::new(table.id.0.as_str(), pos.x, pos.y)
        .label(table.name.as_str())
        .node_type("table")
        .size(260.0, card_height)
        .handles(handles)
}
```

### B. Building Foreign Key `FlowEdge`s
Link the source FK column handle to the target PK column handle:

```rust
pub fn build_relationship_edge(fk: &ForeignKeyConstraint) -> FlowEdge {
    let edge_id = format!("{}_to_{}", fk.source_table.0, fk.target_table.0);
    
    let src_col = fk.source_columns.first().cloned().unwrap_or_default();
    let tgt_col = fk.target_columns.first().cloned().unwrap_or_default();

    let source_handle_id = format!("source_{}.{}", fk.source_table.0, src_col);
    let target_handle_id = format!("target_{}.{}", fk.target_table.0, tgt_col);

    let label = match fk.cardinality {
        Cardinality::OneToOne => "1 : 1",
        Cardinality::OneToMany => "1 : N",
        Cardinality::ManyToMany => "N : M",
    };

    FlowEdge::new(edge_id, fk.source_table.0.as_str(), fk.target_table.0.as_str())
        .source_handle(source_handle_id)
        .target_handle(target_handle_id)
        .edge_type(EdgeType::SmoothStep {
            border_radius: 8.0,
            offset: 24.0,
        })
        .color(0x89b4fa) // Soft Blue accent
        .stroke_width(2.0)
        .label(label)
}
```

---

## 6. Table Renderer Implementation (GPUI Card)

```rust
use gpui::*;

const CARD_BG: u32 = 0x181825;
const HEADER_BG: u32 = 0x1e1e2e;
const BORDER_COLOR: u32 = 0x313244;
const TEXT_MAIN: u32 = 0xcdd6f4;
const TEXT_MUTED: u32 = 0xa6adc8;
const COLOR_PK: u32 = 0xf9e2af; // Catppuccin Gold
const COLOR_FK: u32 = 0x89b4fa; // Catppuccin Blue
const COLOR_TYPE: u32 = 0xa6e3a1;// Catppuccin Green

pub fn render_table_card(node: &FlowNode, _window: &mut Window, _cx: &mut App) -> AnyElement {
    div()
        .w(px(260.0))
        .flex()
        .flex_col()
        .bg(gpui::rgb(CARD_BG))
        .border_1()
        .border_color(gpui::rgb(BORDER_COLOR))
        .rounded_md()
        .shadow_md()
        .overflow_hidden()
        // Header
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .px_3()
                .py_2()
                .bg(gpui::rgb(HEADER_BG))
                .border_b_1()
                .border_color(gpui::rgb(BORDER_COLOR))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::BOLD)
                                .text_color(gpui::rgb(TEXT_MAIN))
                                .child(node.label.to_string()),
                        ),
                )
                .child(
                    div()
                        .px_1p5()
                        .py_0p5()
                        .rounded_xs()
                        .bg(gpui::rgba(0x313244ff))
                        .text_xs()
                        .text_color(gpui::rgb(TEXT_MUTED))
                        .child("TABLE"),
                ),
        )
        .into_any_element()
}
```

---

## 7. Read-Only Schema Explorer UI Features

### A. Search & Filter Bar
A top toolbar allows users to search tables and columns by name:
- Matching table nodes stay highlighted.
- Non-matching table nodes are dimmed (`opacity = 0.3`).

### B. Interactive Relationship Highlighting
When a user clicks or hovers over a table node:
- Connected edges stay bright (`stroke_width = 3.0`, `color = 0xf5e0dc`).
- Unconnected tables & edges fade into the background.

### C. Controls & Bird's-Eye Minimap Overlay
- [`Controls::new(state)`](file:///E:/zed_Source/crates/gpui_flow/src/controls.rs#L10) positioned bottom-left for quick Zoom In, Zoom Out, and Fit View.
- [`Minimap::new(state)`](file:///E:/zed_Source/crates/gpui_flow/src/minimap.rs#L11) positioned bottom-right displaying graph overview box and viewport indicator.

---

## 8. File Structure Recommendation

```
crates/database_panel/
├── Cargo.toml
├── DB_SCHEMA_GRAPH_ARCHITECTURE.md   # <--- Architecture & Design Spec
├── src/
│   ├── database_panel.rs              # Main GPUI View & Panel registration
│   ├── ir.rs                          # Schema Intermediate Representation structs
│   ├── layout.rs                      # Sugiyama / Layered DAG layout engine
│   ├── graph_view.rs                  # Schema graph view component built on gpui_flow
│   └── providers/                     # Multi-dialect database drivers/parsers
│       ├── mod.rs
│       ├── postgres.rs
│       ├── sqlite.rs
│       ├── mysql.rs
│       ├── mssql.rs
│       └── ddl_parser.rs              # SQL migration file parser
```
