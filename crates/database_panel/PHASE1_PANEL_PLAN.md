# Database Panel Phase 1 Plan

## Goal

Build the first database panel shell as a proper GPUI Component-style explorer, without the schema graph feature. This phase focuses on the app shell, connection management, schema tree navigation, and object inspection.

## Scope

This phase includes:

- Connection management UI
- Resizable three-pane layout
- Schema explorer tree
- Database overview and table list views
- Inspector panel for selected schema objects
- gpui_component styling and interaction patterns

This phase excludes:

- Schema graph canvas
- Foreign-key edge rendering
- Zoom/pan graph controls
- SQL execution workbench
- Migration/diff tooling

## Design Principles

- Use gpui_component widgets and layout patterns, not hand-rolled div-only structures.
- Keep the shell modular: explorer, content, and inspector are separate stateful sections.
- Treat the graph as a future feature layer, not part of the core panel shell.
- Build a polished native desktop feel using the gpui_component conventions already used elsewhere in the repo.

## Planned Structure

### 1. Panel shell

- `DatabasePanel` root entity
- `DatabaseHeader` for connection status and actions
- `DatabaseBody` split layout using `h_resizable` / `resizable_panel`
- `ConnectionExplorer` sidebar
- `DatabaseMainContent` center pane
- `InspectorPanel` right pane

### 2. Explorer sidebar

- connection list
- active database selection
- tree of schema objects
  - tables
  - views
  - indexes
  - columns
- search/filter input
- selection highlight and expand/collapse behavior

### 3. Center pane

- tabs for:
  - Overview
  - Tables
  - Views
  - Relationships summary
- overview summary cards
- table list / table detail view
- relationship summary list

### 4. Inspector pane

- selected table metadata
- column details
- foreign-key and reference summaries
- compact, readable detail cards

## Implementation Steps

### Step 1: Define the shell structure

- add a root `DatabasePanel` layout with a header and resizable body
- establish a three-pane split
- define each pane as a separate component or stateful view

### Step 2: Build the connection/workspace header

- active connection selector
- status badge
- connect / disconnect / refresh actions
- add connection action
- database picker for server-based databases

### Step 3: Implement the explorer sidebar

- tree model for schema objects
- load and render tables and columns from schema metadata
- selection on click
- search/filter while typing
- expand/collapse nodes

### Step 4: Add content pane views

- Overview tab
- Tables tab
- Views tab (if data exists)
- Relationships summary tab
- selected table detail panel

### Step 5: Add inspector panel

- inspect selected table, column, or relationship
- show useful metadata without making the panel crowded
- keep detail sections compact and consistent

### Step 6: Polish to GPUI Component standards

- use `gpui_component` controls and spacing patterns
- keep borders, colors, and gaps consistent with the rest of the system
- ensure UI feels coherent and native instead of custom/div-heavy
- verify the layout uses real component widgets and composition patterns

## Acceptance Criteria

The Phase 1 panel is complete when:

- a resizable three-pane shell exists and renders correctly
- schema explorer is functional and searchable
- selected objects update the inspector pane
- the panel looks like a native gpui_component interface
- no schema graph logic is mixed into this phase

## Notes

This phase is intentionally limited to the panel shell and explorer experience. It should be a solid base that later supports a graph feature as a distinct additional view without redesigning the shell.
