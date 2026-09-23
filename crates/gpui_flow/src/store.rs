use std::collections::HashMap;

use gpui::{Bounds, Pixels, SharedString};

use crate::types::*;

/// Resolved handle position in screen coordinates, stored after layout.
#[derive(Debug, Clone)]
pub struct ResolvedHandle {
    pub node_id: NodeId,
    pub handle_id: Option<SharedString>,
    pub handle_type: HandleType,
    pub position: HandlePosition,
    pub bounds: Bounds<Pixels>,
}

/// Key for looking up a handle: (node_id, handle_id).
pub type HandleKey = (NodeId, Option<SharedString>);

/// Interior offset (from a container node's own top-left) where its
/// children's local coordinate space starts — leaves room for a header/
/// label strip. Used by `FlowState::absolute_position`/`fit_container`.
pub const CONTAINER_HEADER_HEIGHT: f32 = 32.0;
/// Interior left/right/bottom inset applied the same way.
pub const CONTAINER_PADDING: f32 = 12.0;
/// Fallback footprint for a container node with no explicit
/// `FlowNode::container_size` and nothing yet computed by `fit_container`.
pub const DEFAULT_CONTAINER_SIZE: (f32, f32) = (320.0, 220.0);
/// Padding used by an automatic (measure/zoom-driven) container re-fit when
/// the container was never fitted by an explicit `fit_container` call (so its
/// `FlowNode::container_padding` is `None`).
pub const DEFAULT_FIT_PADDING: f32 = CONTAINER_PADDING * 2.0;
/// Depth/ancestor walks bail out past this many hops rather than looping
/// forever if a cycle is ever accidentally introduced (e.g. by a buggy
/// `set_parent` caller) — graphs this crate targets are nowhere near this
/// deep, so hitting it always means a cycle, not a legitimate graph.
const MAX_NESTING_DEPTH: usize = 64;

/// A snapshot of the graph state for undo/redo.
#[derive(Clone)]
struct HistoryEntry {
    nodes: Vec<FlowNode>,
    edges: Vec<FlowEdge>,
}

/// The central state store for a flow graph. Wrapped in `Entity<FlowState>`.
pub struct FlowState {
    pub nodes: Vec<FlowNode>,
    pub edges: Vec<FlowEdge>,
    /// External imports the graph depends on. Not part of undo/redo history —
    /// nothing in the Canvas edits this list interactively yet, it only
    /// round-trips through load/save.
    pub imports: Vec<FlowImport>,
    node_lookup: HashMap<NodeId, usize>,
    pub viewport: Viewport,
    pub handle_bounds: HashMap<HandleKey, ResolvedHandle>,
    // Interaction state
    pub drag_state: Option<DragState>,
    pub pan_drag: Option<PanDragState>,
    pub connecting: Option<ConnectionDraft>,
    pub selection_box: Option<SelectionBox>,
    // Undo/redo
    undo_stack: Vec<HistoryEntry>,
    redo_stack: Vec<HistoryEntry>,
    // Config
    pub min_zoom: f32,
    pub max_zoom: f32,
    pub snap_to_grid: bool,
    pub snap_grid: (f32, f32),
    pub connection_radius: f32,
}

impl FlowState {
    pub fn new(nodes: Vec<FlowNode>, edges: Vec<FlowEdge>) -> Self {
        let mut node_lookup = HashMap::new();
        for (i, node) in nodes.iter().enumerate() {
            node_lookup.insert(node.id.clone(), i);
        }

        Self {
            nodes,
            edges,
            imports: Vec::new(),
            node_lookup,
            viewport: Viewport::default(),
            handle_bounds: HashMap::new(),
            drag_state: None,
            pan_drag: None,
            connecting: None,
            selection_box: None,
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            min_zoom: 0.8,
            max_zoom: 2.5,
            snap_to_grid: false,
            snap_grid: (20.0, 20.0),
            connection_radius: 20.0,
        }
    }

    /// Replace the import list (e.g. right after `FlowState::new` when
    /// loading a file that had `gbk` imports).
    pub fn with_imports(mut self, imports: Vec<FlowImport>) -> Self {
        self.imports = imports;
        self
    }

    /// Rebuild the node lookup index.
    pub fn rebuild_lookup(&mut self) {
        self.node_lookup.clear();
        for (i, node) in self.nodes.iter().enumerate() {
            self.node_lookup.insert(node.id.clone(), i);
        }
    }

    /// Push the current state onto the undo stack. Call before destructive operations.
    pub fn push_undo(&mut self) {
        self.undo_stack.push(HistoryEntry {
            nodes: self.nodes.clone(),
            edges: self.edges.clone(),
        });
        // Clear redo stack on new action
        self.redo_stack.clear();
        // Limit history size
        if self.undo_stack.len() > 100 {
            self.undo_stack.remove(0);
        }
    }

    /// Undo the last operation. Returns true if undo was performed.
    pub fn undo(&mut self) -> bool {
        if let Some(entry) = self.undo_stack.pop() {
            self.redo_stack.push(HistoryEntry {
                nodes: self.nodes.clone(),
                edges: self.edges.clone(),
            });
            self.nodes = entry.nodes;
            self.edges = entry.edges;
            self.rebuild_lookup();
            true
        } else {
            false
        }
    }

    /// Redo the last undone operation. Returns true if redo was performed.
    pub fn redo(&mut self) -> bool {
        if let Some(entry) = self.redo_stack.pop() {
            self.undo_stack.push(HistoryEntry {
                nodes: self.nodes.clone(),
                edges: self.edges.clone(),
            });
            self.nodes = entry.nodes;
            self.edges = entry.edges;
            self.rebuild_lookup();
            true
        } else {
            false
        }
    }

    /// Get a node by ID.
    pub fn get_node(&self, id: &NodeId) -> Option<&FlowNode> {
        self.node_lookup.get(id).map(|&i| &self.nodes[i])
    }

    /// Get a mutable node by ID.
    pub fn get_node_mut(&mut self, id: &NodeId) -> Option<&mut FlowNode> {
        self.node_lookup.get(id).map(|&i| &mut self.nodes[i])
    }

    /// Set all nodes, rebuilding the index.
    pub fn set_nodes(&mut self, nodes: Vec<FlowNode>) {
        self.nodes = nodes;
        self.rebuild_lookup();
    }

    /// Set all edges.
    pub fn set_edges(&mut self, edges: Vec<FlowEdge>) {
        self.edges = edges;
    }

    /// Apply a batch of node changes.
    pub fn apply_node_changes(&mut self, changes: &[NodeChange]) {
        let mut needs_rebuild = false;

        for change in changes {
            match change {
                NodeChange::Position {
                    id,
                    position,
                    dragging,
                } => {
                    if let Some(node) = self.get_node_mut(id) {
                        node.position = *position;
                        node.dragging = *dragging;
                    }
                }
                NodeChange::Dimensions {
                    id,
                    width,
                    height,
                } => {
                    if let Some(node) = self.get_node_mut(id) {
                        node.measured_width = Some(*width);
                        node.measured_height = Some(*height);
                    }
                }
                NodeChange::Select { id, selected } => {
                    if let Some(node) = self.get_node_mut(id) {
                        node.selected = *selected;
                    }
                }
                NodeChange::Remove { id } => {
                    if let Some(&idx) = self.node_lookup.get(id) {
                        self.nodes.remove(idx);
                        needs_rebuild = true;
                    }
                }
            }
        }

        if needs_rebuild {
            self.rebuild_lookup();
        }
    }

    /// Apply a batch of edge changes.
    pub fn apply_edge_changes(&mut self, changes: &[EdgeChange]) {
        for change in changes {
            match change {
                EdgeChange::Select { id, selected } => {
                    if let Some(edge) = self.edges.iter_mut().find(|e| e.id == *id) {
                        edge.selected = *selected;
                    }
                }
                EdgeChange::Remove { id } => {
                    self.edges.retain(|e| e.id != *id);
                }
            }
        }
    }

    /// Set the viewport, clamping zoom to min/max. Re-fits containers when
    /// the zoom actually changes, so boxes keep enclosing their children.
    pub fn set_viewport(&mut self, viewport: Viewport) {
        let new_zoom = viewport.zoom.clamp(self.min_zoom, self.max_zoom);
        let changed = (new_zoom - self.viewport.zoom).abs() > f32::EPSILON;
        self.viewport = Viewport {
            x: viewport.x,
            y: viewport.y,
            zoom: new_zoom,
        };
        if changed {
            self.refit_all_containers();
        }
    }

    /// Register a resolved handle position (called during layout).
    pub fn set_handle_bounds(&mut self, key: HandleKey, resolved: ResolvedHandle) {
        self.handle_bounds.insert(key, resolved);
    }

    /// Get the direct children of a container node (nodes whose `parent_id`
    /// points at `parent_id`). Empty for an ordinary leaf node.
    pub fn children_of(&self, parent_id: &NodeId) -> Vec<&FlowNode> {
        self.nodes
            .iter()
            .filter(|n| n.parent_id.as_ref() == Some(parent_id))
            .collect()
    }

    /// Whether `node_id` has at least one child — i.e. should render as a
    /// container box rather than an ordinary leaf node.
    pub fn is_container(&self, node_id: &NodeId) -> bool {
        self.nodes
            .iter()
            .any(|n| n.parent_id.as_ref() == Some(node_id))
    }

    /// How many ancestors `node_id` has (0 for a root/top-level node). Used
    /// to guarantee a container always paints behind its own descendants
    /// regardless of their individual `z_index`.
    pub fn depth_of(&self, node_id: &NodeId) -> usize {
        let mut depth = 0;
        let mut current = self.get_node(node_id).and_then(|n| n.parent_id.clone());
        while let Some(pid) = current {
            depth += 1;
            if depth >= MAX_NESTING_DEPTH {
                break;
            }
            current = self.get_node(&pid).and_then(|n| n.parent_id.clone());
        }
        depth
    }

    /// Whether `node_id` is `ancestor_id` itself or nested (at any depth)
    /// inside it — used to reject a reparent that would create a cycle.
    pub fn is_descendant(&self, ancestor_id: &NodeId, node_id: &NodeId) -> bool {
        let mut current = Some(node_id.clone());
        let mut hops = 0;
        while let Some(id) = current {
            if &id == ancestor_id {
                return true;
            }
            hops += 1;
            if hops >= MAX_NESTING_DEPTH {
                break;
            }
            current = self.get_node(&id).and_then(|n| n.parent_id.clone());
        }
        false
    }

    /// The deepest descendant of `ancestor_id` whose on-screen bounds — the
    /// same `flow_to_screen(absolute_position)` + `screen_footprint` box
    /// `graph.rs::render_node` lays out — contain `point` (canvas-local
    /// screen pixels).
    ///
    /// GPUI dispatches a mouse-down to *every* Normal-behavior hitbox that
    /// contains the cursor (only `BlockMouse` occluders stop the hit-test),
    /// and container boxes render behind their children as siblings. A click
    /// on a node nested inside a loop/if/try body therefore reaches both the
    /// child and the enclosing container, and whichever handler runs last
    /// wins the selection — which is the container's, since it registers
    /// first. Containers call this to rebind the effective click target to
    /// the descendant actually under the cursor instead; `None` means the
    /// click landed on the container's own surface (header/empty interior)
    /// and should select the container itself.
    pub fn descendant_at_screen(&self, ancestor_id: &NodeId, point: (f32, f32)) -> Option<NodeId> {
        let mut deepest: Option<(usize, NodeId)> = None;
        for node in &self.nodes {
            if &node.id == ancestor_id || !self.is_descendant(ancestor_id, &node.id) {
                continue;
            }
            let Some(abs_pos) = self.absolute_position(&node.id) else {
                continue;
            };
            let (sx, sy) = self.viewport.flow_to_screen(abs_pos);
            let (fw, fh) = self.screen_footprint(node);
            if point.0 >= sx && point.0 <= sx + fw && point.1 >= sy && point.1 <= sy + fh {
                let depth = self.depth_of(&node.id);
                if deepest.as_ref().map_or(true, |(d, _)| depth > *d) {
                    deepest = Some((depth, node.id.clone()));
                }
            }
        }
        deepest.map(|(_, id)| id)
    }

    /// Returns the deepest node whose rendered screen bounds contain `point`.
    pub fn node_at_screen(&self, point: (f32, f32)) -> Option<NodeId> {
        self.nodes
            .iter()
            .filter_map(|node| {
                let abs_pos = self.absolute_position(&node.id)?;
                let (sx, sy) = self.viewport.flow_to_screen(abs_pos);
                let (width, height) = self.screen_footprint(node);
                (point.0 >= sx
                    && point.0 <= sx + width
                    && point.1 >= sy
                    && point.1 <= sy + height)
                    .then_some((self.depth_of(&node.id), node.z_index, node.id.clone()))
            })
            .max_by_key(|(depth, z_index, _)| (*depth, *z_index))
            .map(|(_, _, id)| id)
    }

    /// A node's footprint (width, height) for layout purposes — its
    /// declared/auto-fit `container_size` if it has children, otherwise its
    /// measured (or estimated) leaf-node wrapper size.
    ///
    /// These two cases are in *different unit spaces*, which is exactly why
    /// `screen_footprint` (not this) is almost always what a caller
    /// computing a screen-space offset actually wants: a leaf's
    /// `measured_width`/`height` comes from `graph.rs`'s measurement
    /// canvas timing the node's *actual rendered pixel size* — already
    /// screen-space, since leaf content is never itself zoom-scaled (only
    /// repositioned) — while `container_size` is a flow-space quantity
    /// computed by `fit_container` from children's flow-space local
    /// positions, and needs `viewport.zoom` applied before it means
    /// anything in screen pixels.
    pub fn node_footprint(&self, node: &FlowNode) -> (f32, f32) {
        if self.is_container(&node.id) {
            node.container_size.unwrap_or(DEFAULT_CONTAINER_SIZE)
        } else {
            (
                node.measured_width.map(|p| p.as_f32()).unwrap_or(114.0),
                node.measured_height.map(|p| p.as_f32()).unwrap_or(54.0),
            )
        }
    }

    /// `node_footprint`, converted to actual screen pixels at the current
    /// viewport zoom — the container-vs-leaf unit mismatch documented on
    /// `node_footprint` resolved here so callers computing a screen-space
    /// offset (handle centers, edge endpoints, hit-testing, selection
    /// boxes) never have to think about which case they're in. Leaf sizes
    /// pass through unscaled (already screen pixels); container sizes are
    /// multiplied by `viewport.zoom` to match how the container's own box
    /// is actually rendered (`graph.rs::render_node`).
    pub fn screen_footprint(&self, node: &FlowNode) -> (f32, f32) {
        let (w, h) = self.node_footprint(node);
        if self.is_container(&node.id) {
            (w * self.viewport.zoom, h * self.viewport.zoom)
        } else {
            (w, h)
        }
    }

    /// Resolve a node's position in top-level flow-space coordinates by
    /// walking its parent chain. For a root node this is just `position`
    /// unchanged; for a child, each ancestor level adds that ancestor's own
    /// absolute position plus the fixed interior offset
    /// (`CONTAINER_PADDING`/`CONTAINER_HEADER_HEIGHT`) its children's local
    /// space starts at. `None` if the node (or an ancestor referenced by a
    /// dangling `parent_id`) doesn't exist.
    pub fn absolute_position(&self, node_id: &NodeId) -> Option<FlowPoint> {
        self.absolute_position_at_depth(node_id, 0)
    }

    fn absolute_position_at_depth(&self, node_id: &NodeId, depth: usize) -> Option<FlowPoint> {
        if depth >= MAX_NESTING_DEPTH {
            return None;
        }
        let node = self.get_node(node_id)?;
        match &node.parent_id {
            Some(parent_id) => {
                let parent_abs = self.absolute_position_at_depth(parent_id, depth + 1)?;
                let parent = self.get_node(parent_id)?;
                // Each container reserves its own header strip height
                // (`header_height` when set, else the crate default), which
                // is where its children's local space starts.
                let header_h = parent.header_height.unwrap_or(CONTAINER_HEADER_HEIGHT);
                Some(FlowPoint::new(
                    parent_abs.x + CONTAINER_PADDING + node.position.x,
                    parent_abs.y + header_h + node.position.y,
                ))
            }
            None => Some(node.position),
        }
    }

    /// Auto-size a container's `container_size` to enclose all of its
    /// direct children (by their local positions/footprints) plus
    /// `padding`. Not automatic every frame — call it after adding,
    /// removing, or moving a child, e.g. right after `push_undo` on those
    /// operations. A no-op if the container currently has no children.
    ///
    /// A leaf child's footprint is its *rendered pixel size* — at home in
    /// screen space, because leaf content is never zoom-scaled (see the
    /// `node_footprint` doc) — while `node.position` and `container_size`
    /// are flow-space quantities that `viewport.zoom` scales on screen. To
    /// enclose a leaf's actual on-screen pixels, its flow-space extent at
    /// the current zoom is `pixels / zoom`, so that's what is summed into
    /// the box. A child container's footprint is already flow-space and
    /// passes through unchanged.
    ///
    /// Children's local space starts at the interior offset
    /// [`CONTAINER_PADDING`]/[`CONTAINER_HEADER_HEIGHT`] inside the box
    /// (see `absolute_position`), so those are folded into the extents too —
    /// otherwise the header strip would eat into `padding` on the bottom
    /// edge. `padding` is then breathing room on top of all of that.
    ///
    /// Because of that zoom coupling, a box fitted at zoom `Z` reliably
    /// encloses its children only while the viewport stays at or above `Z`
    /// (the box and the children's offsets all scale with zoom, but the
    /// fixed-pixel leaf content does not). The zoom-changing entry points
    /// (`set_viewport`, `fit_view`, `zoom_in`/`zoom_out`, the graph's
    /// wheel/pinch handlers) re-fit containers for exactly this reason;
    /// `graph.rs`'s measurement canvas also re-fits the enclosing
    /// containers whenever a node's *measured* size changes, so boxes track
    /// real content rather than whatever estimate the graph was built with.
    pub fn fit_container(&mut self, container_id: &NodeId, padding: f32) {
        let mut max_x = f32::MIN;
        let mut max_y = f32::MIN;
        let mut any = false;
        let zoom = self.viewport.zoom;
        // Children's local space starts at this container's own header strip
        // height, so the box must leave that much room above their offsets.
        let header_h = self
            .get_node(container_id)
            .and_then(|c| c.header_height)
            .unwrap_or(CONTAINER_HEADER_HEIGHT);
        for node in &self.nodes {
            if node.parent_id.as_ref() != Some(container_id) {
                continue;
            }
            let (w, h) = self.node_footprint(node);
            // Leaf footprints are screen pixels; a leaf's flow-space
            // extent at the current zoom is `pixels / zoom`.
            let (w, h) = if self.is_container(&node.id) {
                (w, h)
            } else {
                (w / zoom, h / zoom)
            };
            max_x = max_x.max(CONTAINER_PADDING + node.position.x + w);
            max_y = max_y.max(header_h + node.position.y + h);
            any = true;
        }
        if !any {
            return;
        }
        if let Some(container) = self.get_node_mut(container_id) {
            container.container_size = Some((max_x + padding, max_y + padding));
            container.container_padding = Some(padding);
        }
    }

    /// Re-fit this node's enclosing container(s) after its own footprint
    /// changed (a leaf's `measured_*` size updated, or a nested container
    /// resized). Walks the parent chain innermost-first, re-fitting each
    /// ancestor with its original `FlowNode::container_padding` (falling
    /// back to [`DEFAULT_FIT_PADDING`] for containers that were never
    /// explicitly fitted). A no-op for a top-level node.
    pub fn refit_ancestors(&mut self, node_id: &NodeId) {
        let mut guard = 0;
        let mut current = self.get_node(node_id).and_then(|n| n.parent_id.clone());
        while let Some(parent_id) = current {
            if guard >= MAX_NESTING_DEPTH {
                break;
            }
            let padding = self
                .get_node(&parent_id)
                .and_then(|n| n.container_padding)
                .unwrap_or(DEFAULT_FIT_PADDING);
            self.fit_container(&parent_id, padding);
            current = self.get_node(&parent_id).and_then(|n| n.parent_id.clone());
            guard += 1;
        }
    }

    /// Re-fit every container node against its current children at the
    /// current `viewport.zoom`. Called when the zoom changes (leaf content is
    /// fixed-pixel so each leaf's flow-space extent — and therefore the box
    /// that encloses it — depends on zoom; see `fit_container`'s doc).
    pub fn refit_all_containers(&mut self) {
        let mut ids: Vec<NodeId> = self.nodes.iter().filter(|n| self.is_container(&n.id)).map(|n| n.id.clone()).collect();
        // Innermost containers first, so an outer container's fit sees its
        // nested containers' *updated* sizes (`node_footprint` on a child
        // container reads `container_size`). Shallow depth sorts last.
        ids.sort_by_key(|id| std::cmp::Reverse(self.depth_of(id)));
        for id in ids {
            let padding = self
                .get_node(&id)
                .and_then(|n| n.container_padding)
                .unwrap_or(DEFAULT_FIT_PADDING);
            self.fit_container(&id, padding);
        }
    }

    /// Reparent a node, converting its `position` so its resolved
    /// *absolute* location is unchanged — moving a node into or out of a
    /// container shouldn't make it visually jump. `new_parent_id: None`
    /// makes it a root node again. Rejected silently (no-op) if
    /// `new_parent_id` would create a cycle (is the node itself, or one of
    /// its own descendants) or if either node can't be resolved.
    pub fn set_parent(&mut self, node_id: &NodeId, new_parent_id: Option<NodeId>) {
        if let Some(ref pid) = new_parent_id {
            if pid == node_id || self.is_descendant(node_id, pid) {
                return;
            }
        }
        let Some(abs) = self.absolute_position(node_id) else {
            return;
        };
        let new_local = match &new_parent_id {
            Some(pid) => {
                let Some(parent_abs) = self.absolute_position(pid) else {
                    return;
                };
                FlowPoint::new(
                    abs.x - parent_abs.x - CONTAINER_PADDING,
                    abs.y - parent_abs.y - CONTAINER_HEADER_HEIGHT,
                )
            }
            None => abs,
        };
        if let Some(node) = self.get_node_mut(node_id) {
            node.parent_id = new_parent_id;
            node.position = new_local;
        }
    }

    /// Find the screen-space center of a handle.
    ///
    /// Resolves the node's *absolute* flow-space position (walking any
    /// parent chain — see `absolute_position`) before converting through
    /// the viewport, so a handle on a nested child lands in the right place
    /// on screen. Uses `screen_footprint` (already zoom-correct for both
    /// container and leaf nodes — see its own doc comment).
    pub fn find_handle_center(
        &self,
        node_id: &NodeId,
        _handle_id: &Option<SharedString>,
        handle_position: HandlePosition,
    ) -> Option<(f32, f32)> {
        let node = self.get_node(node_id)?;
        let abs_pos = self.absolute_position(node_id)?;
        let (sx, sy) = self.viewport.flow_to_screen(abs_pos);
        let (w, h) = self.screen_footprint(node);

        let (cx, cy) = match handle_position {
            HandlePosition::Top => (sx + w / 2.0, sy),
            HandlePosition::Bottom => (sx + w / 2.0, sy + h),
            HandlePosition::Left => (sx, sy + h / 2.0),
            HandlePosition::Right => (sx + w, sy + h / 2.0),
        };
        Some((cx, cy))
    }

    /// Fit the viewport to show all nodes with padding.
    ///
    /// Only considers top-level (`parent_id.is_none()`) nodes — a child's
    /// extent is already covered by its container's own footprint
    /// (`node_footprint`, container-size-aware), so including it too would
    /// be redundant, not more accurate.
    pub fn fit_view(&mut self, padding: f32, container_width: f32, container_height: f32) {
        if self.nodes.is_empty() {
            return;
        }

        let mut min_x = f32::MAX;
        let mut min_y = f32::MAX;
        let mut max_x = f32::MIN;
        let mut max_y = f32::MIN;

        for node in &self.nodes {
            if node.hidden || node.parent_id.is_some() {
                continue;
            }
            let (w, h) = self.node_footprint(node);
            min_x = min_x.min(node.position.x);
            min_y = min_y.min(node.position.y);
            max_x = max_x.max(node.position.x + w);
            max_y = max_y.max(node.position.y + h);
        }

        let content_width = max_x - min_x + padding * 2.0;
        let content_height = max_y - min_y + padding * 2.0;

        if content_width <= 0.0 || content_height <= 0.0 {
            return;
        }

        let zoom_x = container_width / content_width;
        let zoom_y = container_height / content_height;
        let zoom = zoom_x.min(zoom_y).clamp(self.min_zoom, self.max_zoom);

        self.viewport.zoom = zoom;
        self.viewport.x = (container_width - content_width * zoom) / 2.0 - (min_x - padding) * zoom;
        self.viewport.y = (container_height - content_height * zoom) / 2.0 - (min_y - padding) * zoom;
        // Containers are fitted for the zoom they're rendered at; the graph
        // just settled on one, so re-fit every box against that zoom.
        self.refit_all_containers();
    }

    /// Zoom in by a step, centered on the container center.
    pub fn zoom_in(&mut self, container_width: f32, container_height: f32) {
        let factor = 1.2;
        let cx = container_width / 2.0;
        let cy = container_height / 2.0;
        let old_zoom = self.viewport.zoom;
        let new_zoom = (old_zoom * factor).clamp(self.min_zoom, self.max_zoom);
        self.viewport.x = cx - (cx - self.viewport.x) * (new_zoom / old_zoom);
        self.viewport.y = cy - (cy - self.viewport.y) * (new_zoom / old_zoom);
        self.viewport.zoom = new_zoom;
        self.refit_all_containers();
    }

    /// Zoom out by a step, centered on the container center.
    pub fn zoom_out(&mut self, container_width: f32, container_height: f32) {
        let factor = 1.0 / 1.2;
        let cx = container_width / 2.0;
        let cy = container_height / 2.0;
        let old_zoom = self.viewport.zoom;
        let new_zoom = (old_zoom * factor).clamp(self.min_zoom, self.max_zoom);
        self.viewport.x = cx - (cx - self.viewport.x) * (new_zoom / old_zoom);
        self.viewport.y = cy - (cy - self.viewport.y) * (new_zoom / old_zoom);
        self.viewport.zoom = new_zoom;
        self.refit_all_containers();
    }

    /// Center the viewport on a specific flow coordinate.
    pub fn set_center(&mut self, flow_x: f32, flow_y: f32, container_width: f32, container_height: f32) {
        self.viewport.x = container_width / 2.0 - flow_x * self.viewport.zoom;
        self.viewport.y = container_height / 2.0 - flow_y * self.viewport.zoom;
    }

    /// Select all nodes and edges.
    pub fn select_all(&mut self) {
        for node in &mut self.nodes {
            if node.selectable {
                node.selected = true;
            }
        }
        for edge in &mut self.edges {
            if edge.selectable {
                edge.selected = true;
            }
        }
    }

    /// Get nodes that have edges pointing TO the given node.
    pub fn get_incomers(&self, node_id: &NodeId) -> Vec<&FlowNode> {
        self.edges
            .iter()
            .filter(|e| e.target == *node_id)
            .filter_map(|e| self.get_node(&e.source))
            .collect()
    }

    /// Get nodes that the given node has edges pointing TO.
    pub fn get_outgoers(&self, node_id: &NodeId) -> Vec<&FlowNode> {
        self.edges
            .iter()
            .filter(|e| e.source == *node_id)
            .filter_map(|e| self.get_node(&e.target))
            .collect()
    }

    /// Get all edges connected to a node (as source or target).
    pub fn get_connected_edges(&self, node_id: &NodeId) -> Vec<&FlowEdge> {
        self.edges
            .iter()
            .filter(|e| e.source == *node_id || e.target == *node_id)
            .collect()
    }

    /// Find the nearest valid handle to snap to during connection dragging.
    pub fn find_snap_target(
        &self,
        draft: &ConnectionDraft,
        mouse_x: f32,
        mouse_y: f32,
    ) -> Option<SnapTarget> {
        let radius = self.connection_radius;
        let mut best: Option<(f32, SnapTarget)> = None;

        for node in &self.nodes {
            if node.hidden {
                continue;
            }
            // Can't connect to the same node
            if node.id == draft.from_node {
                continue;
            }

            for handle in &node.handles {
                // Source→Target or Target→Source only
                if handle.handle_type == draft.from_type {
                    continue;
                }
                // Check per-handle connectable flag
                if !handle.is_connectable {
                    continue;
                }

                if let Some((hx, hy)) = self.find_handle_center(&node.id, &handle.id, handle.position) {
                    let dx = mouse_x - hx;
                    let dy = mouse_y - hy;
                    let dist = (dx * dx + dy * dy).sqrt();

                    if dist <= radius {
                        if best.is_none() || dist < best.as_ref().unwrap().0 {
                            best = Some((
                                dist,
                                SnapTarget {
                                    node_id: node.id.clone(),
                                    handle_id: handle.id.clone(),
                                    handle_type: handle.handle_type,
                                    position: handle.position,
                                    point: (hx, hy),
                                },
                            ));
                        }
                    }
                }
            }
        }

        best.map(|(_, target)| target)
    }

    /// Check if a connection is valid (default rules).
    pub fn is_valid_connection(&self, connection: &Connection) -> bool {
        // No self-connections
        if connection.source == connection.target {
            return false;
        }

        // No duplicate edges
        let duplicate = self.edges.iter().any(|e| {
            e.source == connection.source
                && e.target == connection.target
                && e.source_handle == connection.source_handle
                && e.target_handle == connection.target_handle
        });
        if duplicate {
            return false;
        }

        true
    }

    /// Add an edge from a completed connection.
    pub fn add_edge_from_connection(&mut self, connection: &Connection, edge_id: impl Into<SharedString>) {
        let mut edge = FlowEdge::new(edge_id, connection.source.clone(), connection.target.clone());
        if let Some(ref sh) = connection.source_handle {
            edge.source_handle = Some(sh.clone());
        }
        if let Some(ref th) = connection.target_handle {
            edge.target_handle = Some(th.clone());
        }
        self.edges.push(edge);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One leaf child of a container encloses the parent box iff, at the fit
    /// zoom, the box's rendered edge lands at/right of (and below) where the
    /// leaf's fixed-pixel card ends. Mirrors `absolute_position` + the box
    /// rendering math in `graph.rs`, in case that ever drifts from `fit_container`.
    fn box_encloses_node(state: &FlowState, container: &FlowNode, child: &FlowNode, zoom: f32) -> bool {
        let (cw, ch) = container.container_size.unwrap_or(DEFAULT_CONTAINER_SIZE);
        let (w, h) = state.screen_footprint(child);
        let header_h = container.header_height.unwrap_or(CONTAINER_HEADER_HEIGHT);
        let (cx, cy) = (
            CONTAINER_PADDING + child.position.x,
            header_h + child.position.y,
        );
        cw * zoom >= (cx + w / zoom) * zoom && ch * zoom >= (cy + h / zoom) * zoom
    }

    fn leaf_child(id: &str, x: f32, y: f32, w_px: f32, h_px: f32, parent: &str) -> FlowNode {
        let mut n = FlowNode::new(id, x, y);
        n.parent_id = Some(parent.into());
        n.measured_width = Some(Pixels::from(w_px));
        n.measured_height = Some(Pixels::from(h_px));
        n
    }

    #[test]
    fn fit_container_encloses_children_at_zoom() {
        // Container with two stacked leaves; zoomed out to 0.5 (fit_view's
        // typical settle point for a tall graph) the leaves' flow-space
        // extents double, so the box must too.
        for zoom in [1.0_f32, 0.5, 0.35] {
            let mut state = FlowState::new(
                vec![
                    FlowNode::new("c", 0.0, 0.0).container_size(320.0, 220.0),
                    leaf_child("a", 20.0, 20.0, 172.0, 46.0, "c"),
                    leaf_child("b", 20.0, 150.0, 172.0, 46.0, "c"),
                ],
                vec![],
            );
            state.viewport.zoom = zoom;
            state.fit_container(&"c".into(), 24.0);

            let (c, a, b) = {
                let nodes = &state.nodes;
                (
                    nodes.iter().find(|n| n.id == "c").unwrap(),
                    nodes.iter().find(|n| n.id == "a").unwrap(),
                    nodes.iter().find(|n| n.id == "b").unwrap(),
                )
            };
            assert!(
                box_encloses_node(&state, c, a, zoom),
                "zoom {zoom}: 'a' (top leaf) not enclosed — box {}x{}, leaf at {}x{}",
                c.container_size.unwrap().0,
                c.container_size.unwrap().1,
                a.position.x,
                a.position.y,
            );
            assert!(
                box_encloses_node(&state, c, b, zoom),
                "zoom {zoom}: 'b' (bottom leaf) not enclosed — box {}x{}, leaf at {}x{}",
                c.container_size.unwrap().0,
                c.container_size.unwrap().1,
                b.position.x,
                b.position.y,
            );
        }
    }

    #[test]
    fn box_grows_when_zoom_shrinks() {
        // Leaf content is fixed-pixel; as the view zooms out, a leaf's
        // flow-space extent grows, so the box (in flow units) must too.
        let mut state = FlowState::new(
            vec![
                FlowNode::new("c", 0.0, 0.0).container_size(320.0, 220.0),
                leaf_child("a", 20.0, 20.0, 172.0, 46.0, "c"),
            ],
            vec![],
        );
        state.fit_container(&"c".into(), 24.0);
        let at_1 = state.get_node(&"c".into()).unwrap().container_size.unwrap();
        state.viewport.zoom = 0.5;
        state.refit_all_containers();
        let at_half = state.get_node(&"c".into()).unwrap().container_size.unwrap();
        assert!(
            at_half.0 > at_1.0 && at_half.1 > at_1.1,
            "zooming out should grow the box in flow units: {at_1:?} -> {at_half:?}"
        );
        // ...and rendering that box back through the zoom must still enclose
        // the child's fixed-pixel card.
        let (c, a) = (
            state.get_node(&"c".into()).unwrap(),
            state.get_node(&"a".into()).unwrap(),
        );
        assert!(box_encloses_node(&state, c, a, 0.5));
    }

    #[test]
    fn nested_container_encloses_child_container() {
        // ifVip-style: child containers fitted first, parent fitted against
        // their *fitted* sizes (not the 320x220 default).
        let mut state = FlowState::new(
            vec![
                FlowNode::new("ifVip", 0.0, 0.0),
                FlowNode::new("branchTrue", 20.0, 20.0).parent("ifVip"),
                leaf_child("a", 20.0, 20.0, 172.0, 46.0, "branchTrue"),
                leaf_child("b", 20.0, 280.0, 172.0, 46.0, "branchTrue"),
                FlowNode::new("branchFalse", 320.0, 20.0).parent("ifVip"),
                leaf_child("c", 20.0, 20.0, 172.0, 46.0, "branchFalse"),
            ],
            vec![],
        );
        state.viewport.zoom = 0.5;
        state.fit_container(&"branchTrue".into(), 80.0);
        state.fit_container(&"branchFalse".into(), 80.0);
        state.fit_container(&"ifVip".into(), 24.0);

        let if_vip = state.get_node(&"ifVip".into()).unwrap();
        let branch_true = state.get_node(&"branchTrue".into()).unwrap();
        assert!(
            box_encloses_node(&state, if_vip, branch_true, 0.5),
            "ifVip box {}x{} does not enclose branchTrue {}x{} at its fitted size",
            if_vip.container_size.unwrap().0,
            if_vip.container_size.unwrap().1,
            branch_true.container_size.unwrap().0,
            branch_true.container_size.unwrap().1,
        );
    }

    #[test]
    fn measuring_a_leaf_refits_its_container() {
        let mut state = FlowState::new(
            vec![
                FlowNode::new("c", 0.0, 0.0).container_size(320.0, 220.0),
                leaf_child("a", 20.0, 20.0, 172.0, 46.0, "c"),
            ],
            vec![],
        );
        state.viewport.zoom = 0.5;
        state.fit_container(&"c".into(), 24.0);
        let before = state.get_node(&"c".into()).unwrap().container_size.unwrap().0;

        // Leaf renders bigger than its bootstrap estimate (e.g. a long label).
        state.get_node_mut(&"a".into()).unwrap().measured_width = Some(Pixels::from(240.0));
        state.refit_ancestors(&"a".into());

        let after = state.get_node(&"c".into()).unwrap().container_size.unwrap().0;
        assert!(
            after > before,
            "container width should grow when a child measures wider: {before} -> {after}"
        );
    }

    #[test]
    fn custom_header_height_reserves_room_for_children() {
        // A container with a tall dedicated header (e.g. its own chrome +
        // title, 56 units) must push children down past it in
        // `absolute_position`, and `fit_container` must enclose them there.
        let mut state = FlowState::new(
            vec![
                FlowNode::new("c", 0.0, 0.0).header_height(56.0),
                leaf_child("a", 20.0, 20.0, 172.0, 46.0, "c"),
            ],
            vec![],
        );
        state.viewport.zoom = 0.5;
        state.fit_container(&"c".into(), 24.0);

        {
            let (c, a) = (
                state.get_node(&"c".into()).unwrap(),
                state.get_node(&"a".into()).unwrap(),
            );
            assert!(
                box_encloses_node(&state, c, a, 0.5),
                "box {}x{} must enclose child sitting below the 56-unit header",
                c.container_size.unwrap().0,
                c.container_size.unwrap().1,
            );
        }

        let abs = state.absolute_position(&"a".into()).unwrap();
        assert_eq!(
            abs.y, 76.0,
            "child's absolute y should sit below the 56-unit header (got {})",
            abs.y
        );
    }
}
