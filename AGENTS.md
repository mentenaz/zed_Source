.rules

# Cockpit panel ports: `gpui_component` is the requirement, not an option

Panels ported from the Forge repo (`E:\Forge_GPUI`) into this tree
(`cockpit_panel`, `script_runner_panel`, `processes_panel`, `python_panel`,
`node_panel`, `npm_manager_panel`, `dotnet_panel`, `nuget_manager_panel`) are
expected to be built on
**`gpui_component`'s actual widget and layout system** —
`gpui_component::setting::{Settings, SettingPage, SettingGroup, SettingItem}`,
`h_resizable`/`resizable_panel`, `gpui_component::text::markdown`,
`Input`/`InputState`, etc. A port that renders flat, hand-rolled `div` lists
instead of these widgets is a **failed port**, even if it compiles and works.

The first version of `npm_manager_panel` shipped exactly that way (a
hand-rolled three-tab list, no `setting::`, no resizable split, no registry
search). The approved remedy lives in
`crates/gpui_component/NPM_MANAGER_PANEL_UI_RESTORE_PLAN.md` — read it before
touching the npm manager, and follow `crates/gpui_component/PORTING.md` for the
general porting conventions (crate layout, `AppState`-stripping, theme
bridging). Before declaring any such port done, confirm the UI is actually
composed from `gpui_component` widgets, not just `div`s that resemble them.

For the record, `node_panel` (the first standalone port, and the template
`dotnet_panel` clones) is **not** on the `setting::` stack either — it renders
`gpui_component` `Collapsible` sections with `div` rows, and its quick actions
are deliberately disabled chips because nothing wired
`script_runner_panel::run_external` to them at port time. The `.NET` panels
shipped the corrected pattern: `dotnet_panel`'s quick actions run for real via
`run_external` (with a silent `background_run` fallback), and
`nuget_manager_panel` follows the restored npm-manager pattern
(`setting::{Settings, SettingPage, …}` + `h_resizable`). The rule above
governs any future rework of `node_panel` and all new ports.

# Database Panel & Schema Graph (`database_panel`)

The architecture and design specification for the Database Schema Graph Visualizer lives in
[`crates/database_panel/DB_SCHEMA_GRAPH_ARCHITECTURE.md`](file:///E:/zed_Source/crates/database_panel/DB_SCHEMA_GRAPH_ARCHITECTURE.md).

Key architectural guidelines for `database_panel`:
- Built on **`gpui_component`** widgets and **`gpui_flow`** graph engine.
- Supports multi-dialect DB introspection (SQLite, PostgreSQL, MySQL, MSSQL) via a normalized `SchemaIR`.
- Employs a 4-stage **Sugiyama Layered DAG** layout engine for automatic graph positioning based on foreign-key dependency hierarchy.
- Uses **column-level handle anchoring** (`HandleDef::source` / `HandleDef::target` with column handle IDs) on table card elements.
- Provides a read-only schema explorer with search, relationship highlighting, minimap, and zoom/pan controls.