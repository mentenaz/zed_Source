use std::cell::Cell;
use std::collections::HashMap;
use std::rc::Rc;

use gpui::*;
use gpui::prelude::FluentBuilder;

use crate::edges;
use crate::store::FlowState;
use crate::types::*;

type NodeRendererFn = Box<dyn Fn(&FlowNode, &mut Window, &mut App) -> AnyElement>;

/// The top-level flow graph component.
///
/// Renders nodes as positioned divs and edges via a canvas paint layer.
pub struct FlowGraph {
    state: Entity<FlowState>,
    focus_handle: FocusHandle,
    node_renderers: HashMap<SharedString, NodeRendererFn>,
    /// Dedicated renderers for *container* nodes' header strips. When a type
    /// has one registered it wins over `node_renderers` for that container;
    /// without one, a container falls back to its ordinary `node_renderer`
    /// output (so pre-existing container renderers keep acting as headers).
    container_headers: HashMap<SharedString, NodeRendererFn>,
    default_renderer: Option<NodeRendererFn>,
    /// Called when a new connection is completed.
    on_connect: Option<Box<dyn Fn(&Connection, &mut FlowState)>>,
    /// Called when a connection drag ends over empty canvas (no snap
    /// target) instead of on another handle — the "drag from a handle,
    /// drop on empty space to create a new connected node" gesture. Given
    /// the in-progress draft (so the callback knows which node/handle/
    /// direction it started from) and the drop point already converted to
    /// flow-space coordinates (`Viewport::screen_to_flow`), with mutable
    /// access to `FlowState` to push the new node + edge directly.
    on_connection_drop: Option<std::rc::Rc<dyn Fn(&ConnectionDraft, FlowPoint, &mut FlowState)>>,
    /// Custom validation for connections. Return false to reject.
    is_valid_connection: Option<Box<dyn Fn(&Connection, &FlowState) -> bool>>,
    /// Whether to show the default node wrapper chrome (bg, border, shadow, padding).
    show_node_chrome: bool,
    /// Background color for the canvas (default: 0xf8f8f8).
    bg_color: u32,
    /// Dot grid color (default: 0xd4d4d4).
    grid_color: u32,
    /// Background pattern style.
    bg_pattern: BackgroundPattern,
    /// Node wrapper background color.
    node_bg_color: u32,
    /// Node wrapper border color.
    node_border_color: u32,
    /// Selection outline, connection-handle highlight, and snap-target
    /// color (default: 0x3b82f6, this crate's original hardcoded blue —
    /// unset consumers keep today's look).
    accent_color: u32,
    /// Whether we've done the initial measurement pass.
    measured: bool,
    /// Fired on right-click for any node with `FlowNode.context_menu` set.
    /// `Rc` (not `Box`) so it can be cloned into the per-node mouse-down
    /// closure built fresh on every render. Deliberately generic (node id +
    /// window-absolute click point) — this crate has no `forge_ui`/UI-kit
    /// dependency and no opinion on what menu, if any, the consumer shows.
    on_node_context_menu:
        Option<std::rc::Rc<dyn Fn(NodeId, Point<Pixels>, &mut Window, &mut App)>>,
    /// Fired on right-click for any edge (hit-tested the same way the
    /// existing left-click edge-select path already does — see
    /// `edges::hit_test_edges`), since an edge has no element of its own to
    /// attach a per-edge handler to the way a node does (edges are painted,
    /// not laid out as divs).
    on_edge_context_menu:
        Option<std::rc::Rc<dyn Fn(EdgeId, Point<Pixels>, &mut Window, &mut App)>>,
    /// Fired on right-click anywhere in the canvas. The callback receives
    /// both the window point and the corresponding canvas flow coordinate.
    on_canvas_context_menu:
        Option<std::rc::Rc<dyn Fn(Point<Pixels>, FlowPoint, Option<NodeId>, &mut Window, &mut App)>>,
    /// This element's own on-screen origin within the window, captured each
    /// paint from the background/edges `canvas()`'s bounds. Mouse events
    /// deliver window-absolute positions, but `edges::hit_test_edges` (and
    /// everything else that positions things in "flow-canvas-local" space,
    /// like `Viewport::flow_to_screen`) compares against coordinates that
    /// don't include this element's own offset within a host window that
    /// embeds it below other chrome (a toolbar, tab bar, etc.) — subtracting
    /// this origin is what makes an edge click/right-click hit-test land on
    /// the same edge the user is actually looking at, instead of missing by
    /// exactly this element's vertical/horizontal offset. Node dragging and
    /// panning don't need this: they're delta-based (`later - earlier`), so
    /// a constant offset cancels out on its own — only *absolute* position
    /// comparisons like edge hit-testing are affected.
    canvas_origin: Rc<Cell<Point<Pixels>>>,
}

impl FlowGraph {
    pub fn new(state: Entity<FlowState>, cx: &mut Context<Self>) -> Self {
        Self {
            state,
            focus_handle: cx.focus_handle(),
            node_renderers: HashMap::new(),
            container_headers: HashMap::new(),
            default_renderer: None,
            on_connect: None,
            on_connection_drop: None,
            is_valid_connection: None,
            show_node_chrome: true,
            bg_color: 0xf8f8f8,
            grid_color: 0xd4d4d4,
            bg_pattern: BackgroundPattern::Dots,
            node_bg_color: 0xffffff,
            node_border_color: 0xe2e2e2,
            accent_color: 0x3b82f6,
            measured: false,
            on_node_context_menu: None,
            on_edge_context_menu: None,
            on_canvas_context_menu: None,
            canvas_origin: Rc::new(Cell::new(Point::default())),
        }
    }

    /// Register a right-click handler for nodes with `context_menu` set.
    pub fn on_node_context_menu(
        mut self,
        callback: impl Fn(NodeId, Point<Pixels>, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_node_context_menu = Some(std::rc::Rc::new(callback));
        self
    }

    /// Register a right-click handler for edges — fires when the click
    /// hit-tests onto an edge's path (same 5px threshold the left-click
    /// edge-select gesture uses).
    pub fn on_edge_context_menu(
        mut self,
        callback: impl Fn(EdgeId, Point<Pixels>, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_edge_context_menu = Some(std::rc::Rc::new(callback));
        self
    }

    /// Register a right-click handler for the canvas background and nodes.
    pub fn on_canvas_context_menu(
        mut self,
        callback: impl Fn(Point<Pixels>, FlowPoint, Option<NodeId>, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_canvas_context_menu = Some(std::rc::Rc::new(callback));
        self
    }

    /// Register a renderer for a specific node type.
    pub fn node_renderer(
        mut self,
        node_type: impl Into<SharedString>,
        renderer: impl Fn(&FlowNode, &mut Window, &mut App) -> AnyElement + 'static,
    ) -> Self {
        self.node_renderers
            .insert(node_type.into(), Box::new(renderer));
        self
    }

    /// Set the default renderer for nodes without a specific type renderer.
    pub fn default_renderer(
        mut self,
        renderer: impl Fn(&FlowNode, &mut Window, &mut App) -> AnyElement + 'static,
    ) -> Self {
        self.default_renderer = Some(Box::new(renderer));
        self
    }

    /// Register a renderer for a *container* node's header strip. Takes
    /// precedence over `node_renderer` for container nodes; when absent, a
    /// container falls back to its `node_renderer` output acting as the
    /// header (preserving existing layouts). The rendered element is layered
    /// as a dedicated chrome strip across the container's top, sized to the
    /// container's `header_height` (`FlowNode::header_height`, default
    /// `store::CONTAINER_HEADER_HEIGHT`), and children lay out below it.
    pub fn container_header(
        mut self,
        node_type: impl Into<SharedString>,
        renderer: impl Fn(&FlowNode, &mut Window, &mut App) -> AnyElement + 'static,
    ) -> Self {
        self.container_headers
            .insert(node_type.into(), Box::new(renderer));
        self
    }

    /// Set the on_connect callback.
    pub fn on_connect(
        mut self,
        callback: impl Fn(&Connection, &mut FlowState) + 'static,
    ) -> Self {
        self.on_connect = Some(Box::new(callback));
        self
    }

    /// Set the on_connection_drop callback — see the field doc comment.
    pub fn on_connection_drop(
        mut self,
        callback: impl Fn(&ConnectionDraft, FlowPoint, &mut FlowState) + 'static,
    ) -> Self {
        self.on_connection_drop = Some(std::rc::Rc::new(callback));
        self
    }

    /// Hide the default node wrapper chrome (bg, border, shadow, padding).
    pub fn no_node_chrome(mut self) -> Self {
        self.show_node_chrome = false;
        self
    }

    /// Set the canvas background color.
    pub fn bg_color(mut self, color: u32) -> Self {
        self.bg_color = color;
        self
    }

    /// Set the dot grid color.
    pub fn grid_color(mut self, color: u32) -> Self {
        self.grid_color = color;
        self
    }

    /// Set the node wrapper background color.
    pub fn node_bg_color(mut self, color: u32) -> Self {
        self.node_bg_color = color;
        self
    }

    /// Set the node wrapper border color.
    pub fn node_border_color(mut self, color: u32) -> Self {
        self.node_border_color = color;
        self
    }

    /// Set the selection outline / connection-handle accent color.
    pub fn accent_color(mut self, color: u32) -> Self {
        self.accent_color = color;
        self
    }

    /// Live-resync every canvas color in one call, for consumers that
    /// observe the active theme and want a running canvas to track it (the
    /// builder setters above are one-shot: they take `mut self`, so they
    /// can't be re-applied to a living `Entity<FlowGraph>`).
    pub fn set_theme_colors(
        &mut self,
        bg_color: u32,
        grid_color: u32,
        node_bg_color: u32,
        node_border_color: u32,
        accent_color: u32,
    ) {
        self.bg_color = bg_color;
        self.grid_color = grid_color;
        self.node_bg_color = node_bg_color;
        self.node_border_color = node_border_color;
        self.accent_color = accent_color;
    }

    /// Set the background pattern (Dots, Lines, Cross).
    pub fn bg_pattern(mut self, pattern: BackgroundPattern) -> Self {
        self.bg_pattern = pattern;
        self
    }

    /// Set custom connection validation.
    pub fn validate_connection(
        mut self,
        validator: impl Fn(&Connection, &FlowState) -> bool + 'static,
    ) -> Self {
        self.is_valid_connection = Some(Box::new(validator));
        self
    }

    /// Render a single node using the appropriate renderer.
    ///
    /// `abs_position` is the node's *resolved* flow-space position (walking
    /// any parent chain — see `FlowState::absolute_position`), already
    /// computed by the caller since that needs a `FlowState` borrow this
    /// method doesn't otherwise take. `footprint` is likewise
    /// `FlowState::node_footprint` — the container size for a node with
    /// children, or its measured/estimated leaf size.
    fn render_node(
        &self,
        node: &FlowNode,
        abs_position: FlowPoint,
        footprint: (f32, f32),
        is_container: bool,
        viewport: &Viewport,
        is_connecting: bool,
        snap_node_id: Option<&NodeId>,
        entity_id: EntityId,
        window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        if node.hidden {
            return div().into_any_element();
        }

        let (screen_x, screen_y) = viewport.flow_to_screen(abs_position);
        let (footprint_w, footprint_h) = footprint;
        // `footprint` is in flow-space units — a child's *screen* offset
        // from this container is computed by folding this container's flow
        // position into the child's `absolute_position` and running the
        // whole thing through one `flow_to_screen` call, so that offset
        // scales with `viewport.zoom` automatically. This box's own
        // rendered size must scale the same way, or the two only agree at
        // whatever zoom `container_size`/`fit_container` happened to be
        // computed at — above that zoom the children's offset outgrows a
        // box that isn't growing with it, and they render outside their
        // own container's border.
        let (screen_footprint_w, screen_footprint_h) =
            (footprint_w * viewport.zoom, footprint_h * viewport.zoom);

        // Choose per-node content. A leaf gets its body from `node_renderer`.
        // A container's children live *below* a dedicated header strip whose
        // content comes from a `container_header` renderer when one is
        // registered for this type, otherwise falling back to the ordinary
        // `node_renderer` output (which used to render as a fixed 32px strip).
        // Splitting the two here (instead of computing one `content`) means a
        // container's body renderer isn't invoked at all when a dedicated
        // header exists — the header callbacks and the body callbacks stay
        // independent.
        let header_height = node
            .header_height
            .unwrap_or(crate::store::CONTAINER_HEADER_HEIGHT);

        let leaf_content: Option<AnyElement> = if is_container {
            None
        } else {
            Some(match node.node_type.as_ref() {
                Some(node_type) => match self.node_renderers.get(node_type) {
                    Some(renderer) => renderer(node, window, cx),
                    None => self.render_fallback_node(node, window, cx),
                },
                None => self.render_fallback_node(node, window, cx),
            })
        };

        let container_header_content: Option<AnyElement> = if is_container {
            Some(match node.node_type.as_ref() {
                Some(node_type) => match self
                    .container_headers
                    .get(node_type)
                    .or_else(|| self.node_renderers.get(node_type))
                {
                    Some(renderer) => renderer(node, window, cx),
                    None => self.render_fallback_node(node, window, cx),
                },
                None => self.render_fallback_node(node, window, cx),
            })
        } else {
            None
        };

        let node_id = node.id.clone();
        let state = self.state.clone();
        let canvas_origin = self.canvas_origin.clone();
        let selected = node.selected;
        let dragging = node.dragging;
        let show_chrome = self.show_node_chrome;
        let node_bg = self.node_bg_color;
        let node_border = node.accent_border.unwrap_or(self.node_border_color);
        let has_accent_border = node.accent_border.is_some();
        let element_id: ElementId = ElementId::Name(node.id.clone());

        // Dedicated header strip for containers: an absolutely-positioned
        // bar along the top, sized to `header_height * zoom` so the space it
        // occupies always matches the `header_h` offset applied by
        // `FlowState::absolute_position` / `fit_container`. Content is the
        // `container_header` renderer output; chrome (tint + bottom divider)
        // only when `show_chrome` is on. Inset from the container's rounded
        // top corners so the bar's square corners stay inside them.
        let container_header_strip: Option<AnyElement> = if is_container {
            container_header_content.map(|header_content| {
                div()
                    .absolute()
                    .top(px(1.0))
                    .left(px(8.0))
                    .right(px(8.0))
                    .h(px(header_height * viewport.zoom))
                    .flex()
                    .items_center()
                    .px_2()
                    .when(show_chrome, |el: Div| {
                        el.bg(gpui::rgba(0x00000008))
                            .border_b_1()
                            .border_color(gpui::rgb(node_border))
                    })
                    .child(header_content)
                    .into_any_element()
            })
        } else {
            None
        };

        // Build handle dot elements (skip if not connecting to reduce overhead)
        let handle_elements = if !node.handles.is_empty() {
            Self::render_handles(
                &node.handles,
                &node.id,
                &state,
                is_connecting,
                snap_node_id,
                node_bg,
                node_border,
                self.accent_color,
            )
        } else {
            Vec::new()
        };

        // Per-node measurement canvas: captures actual wrapper size. Whenever
        // a measurement actually changes (first paint of this node, or any
        // later change — e.g. a mindmap node added after the graph's own
        // one-shot initial-measure pass has already run), explicitly notify
        // the `FlowGraph` entity so edges get repainted against the fresh
        // size instead of staying pinned to a stale/default one indefinitely.
        let measure_state = self.state.clone();
        let measure_node_id = node.id.clone();
        let prev_w = node.measured_width;
        let prev_h = node.measured_height;
        let measure_canvas = canvas(
            |_bounds, _window, _cx| {},
            move |bounds, _: (), _window, cx| {
                let w = bounds.size.width;
                let h = bounds.size.height;
                if prev_w != Some(w) || prev_h != Some(h) {
                    measure_state.update(cx, |state, _| {
                        if let Some(node) = state.get_node_mut(&measure_node_id) {
                            node.measured_width = Some(w);
                            node.measured_height = Some(h);
                        }
                        // A container's box is only as good as its children's
                        // real (measured) sizes — this node's just changed,
                        // so re-fit every container that encloses it. A
                        // resized container then re-measures itself and refits
                        // its own ancestors, so nested boxes settle in one
                        // pass up the chain. Uses the padding each container
                        // was originally fitted with (`container_padding`).
                        state.refit_ancestors(&measure_node_id);
                    });
                    cx.notify(entity_id);
                }
            },
        )
        .size_full()
        .absolute();

        // Assemble the wrapper's children: a leaf contributes its body, a
        // container contributes its header strip (its children render as
        // separate top-level nodes by the caller, above this box), followed
        // by the always-present measurement canvas and handle dots.
        let mut wrapper_children: Vec<AnyElement> =
            Vec::with_capacity(handle_elements.len() + 2);
        if let Some(strip) = container_header_strip {
            wrapper_children.push(strip);
        } else if let Some(body) = leaf_content {
            wrapper_children.push(body);
        }
        wrapper_children.push(measure_canvas.into_any_element());
        wrapper_children.extend(handle_elements);

        div()
            .id(element_id)
            .debug_selector({
                let node_id = node_id.clone();
                move || node_id.to_string()
            })
            .absolute()
            .left(px(screen_x))
            .top(px(screen_y))
            // A container (has children — e.g. a loop body) is forced to
            // its resolved footprint so there's room to lay children out
            // inside it, and gets a dashed border instead of the ordinary
            // solid one so it reads as "holds other nodes" rather than "is
            // one." A leaf node keeps intrinsic content sizing (no
            // width/height set here at all). The container's header strip is
            // layered as an absolutely-positioned child (see below), and
            // children sit below `header_height` in flow space — so no
            // padding-top is needed to reserve header space here.
            .when(is_container, |el| {
                el.w(px(screen_footprint_w))
                    .h(px(screen_footprint_h))
            })
            // Node box styling on the wrapper so handles align to visual edges
            .when(show_chrome, |el: Stateful<Div>| {
                el.bg(gpui::rgb(node_bg))
                    .when(has_accent_border, |el| el.border_2())
                    .when(!has_accent_border, |el| el.border_1())
                    .when(is_container, |el| el.border_dashed())
                    .border_color(gpui::rgb(node_border))
                    .rounded_lg()
                    .shadow_sm()
                    .px_4()
                    .py_2()
            })
            .cursor(if dragging {
                CursorStyle::ClosedHand
            } else {
                CursorStyle::OpenHand
            })
            .when(selected, |el: Stateful<Div>| {
                el.border_2().border_color(gpui::rgb(self.accent_color))
            })
            .on_mouse_down(MouseButton::Left, {
                let node_id = node_id.clone();
                let state = state.clone();
                let canvas_origin = canvas_origin.clone();
                move |event, _window, cx| {
                    let multi = event.modifiers.platform;
                    let mouse_pos = event.position;
                    let origin = canvas_origin.get();
                    let mouse = (
                        mouse_pos.x.as_f32() - origin.x.as_f32(),
                        mouse_pos.y.as_f32() - origin.y.as_f32(),
                    );

                    state.update(cx, |state, _| {
                        // Don't start node drag if we're connecting
                        if state.connecting.is_some() {
                            return;
                        }

                        // If this click landed on a node nested inside a
                        // container (a loop/if/try body — which this handler
                        // fires for even when the child's own handler also
                        // does, because GPUI dispatches mouse-downs to every
                        // Normal-behavior hitbox under the cursor), rebind the
                        // effective target to that descendant so the
                        // container can't overwrite the child's selection.
                        let mut effective_id = node_id.clone();
                        if state.is_container(&node_id) {
                            if let Some(descendant) = state.descendant_at_screen(&node_id, mouse) {
                                effective_id = descendant;
                            }
                        }

                        // Handle selection
                        if !multi {
                            for n in &mut state.nodes {
                                n.selected = false;
                            }
                        }
                        // Z-index elevation
                        let max_z = state.nodes.iter().map(|n| n.z_index).max().unwrap_or(0);
                        if let Some(n) = state.get_node_mut(&effective_id) {
                            n.selected = !n.selected || !multi;
                            if n.selected {
                                n.z_index = max_z + 1;
                            }
                        }

                        // Start drag — collect all selected nodes
                        let mut node_origins = Vec::new();
                        if let Some(n) = state.get_node(&effective_id) {
                            if n.selected && n.draggable {
                                for n in &state.nodes {
                                    if n.selected && n.draggable {
                                        node_origins.push((n.id.clone(), n.position));
                                    }
                                }
                            }
                        }

                        if !node_origins.is_empty() {
                            state.push_undo();
                            state.drag_state = Some(DragState {
                                origin_mouse: (mouse_pos.x.as_f32(), mouse_pos.y.as_f32()),
                                node_origins,
                            });
                            for n in &mut state.nodes {
                                if n.selected && n.draggable {
                                    n.dragging = true;
                                }
                            }
                        }
                    });
                }
            })
            .when_some(
                node.context_menu.then(|| self.on_node_context_menu.clone()).flatten(),
                |el, callback| {
                    let node_id = node_id.clone();
                    let state = state.clone();
                    el.on_mouse_down(MouseButton::Right, move |event, window, cx| {
                        let target = state.update(cx, |state, _| {
                            if state.is_container(&node_id) {
                                state
                                    .descendant_at_screen(&node_id, (
                                        event.position.x.as_f32()
                                            - canvas_origin.get().x.as_f32(),
                                        event.position.y.as_f32()
                                            - canvas_origin.get().y.as_f32(),
                                    ))
                                    .unwrap_or_else(|| node_id.clone())
                            } else {
                                node_id.clone()
                            }
                        });
                        callback(target.clone(), event.position, window, cx);
                    })
                },
            )
            .children(wrapper_children)
            .into_any_element()
    }

    /// Render handle dots for a node.
    fn render_handles(
        handles: &[HandleDef],
        node_id: &NodeId,
        state: &Entity<FlowState>,
        is_connecting: bool,
        snap_node_id: Option<&NodeId>,
        default_bg: u32,
        default_border: u32,
        accent: u32,
    ) -> Vec<AnyElement> {
        let handle_size = 10.0;
        let half = handle_size / 2.0;
        let is_snapped_node = snap_node_id == Some(node_id);

        handles
            .iter()
            .enumerate()
            .map(|(i, handle)| {
                let node_id = node_id.clone();
                let handle_id = handle.id.clone();
                let handle_type = handle.handle_type;
                let handle_position = handle.position;
                let state = state.clone();

                // Highlight: strongly if this is the snapped target, mildly if potential target
                let is_snap_target = is_snapped_node && handle_type == HandleType::Target;
                let is_potential_target = is_connecting && handle_type == HandleType::Target;

                let (bg_color, border_color, size_mult) = if is_snap_target {
                    // Actively snapped — full-strength accent pulse.
                    (gpui::rgb(accent), gpui::rgb(accent), 1.4)
                } else if is_potential_target {
                    // Valid potential target — the same accent, softened.
                    (gpui::rgba((accent << 8) | 0x80), gpui::rgb(accent), 1.0)
                } else {
                    (gpui::rgb(default_bg), gpui::rgb(default_border), 1.0)
                };
                let dot_size = handle_size * size_mult;

                let dot = div()
                    .w(px(dot_size))
                    .h(px(dot_size))
                    .rounded_full()
                    .bg(bg_color)
                    .border_1()
                    .border_color(border_color)
                    .cursor(CursorStyle::Crosshair)
                    .flex_shrink_0();

                let container = div()
                    .id(ElementId::Integer(i as u64))
                    .absolute()
                    .flex()
                    .items_center()
                    .justify_center()
                    // Handle mouse down → start connection
                    .on_mouse_down(MouseButton::Left, {
                        let state = state.clone();
                        let node_id = node_id.clone();
                        let handle_id = handle_id.clone();
                        move |event, _window, cx| {
                            let mouse_pos = event.position;
                            state.update(cx, |state, _| {
                                // Find handle center for the from_point
                                let from_point = state
                                    .find_handle_center(&node_id, &handle_id, handle_position)
                                    .unwrap_or((mouse_pos.x.as_f32(), mouse_pos.y.as_f32()));

                                state.connecting = Some(ConnectionDraft {
                                    from_node: node_id.clone(),
                                    from_handle: handle_id.clone(),
                                    from_type: handle_type,
                                    from_position: handle_position,
                                    from_point,
                                    to_point: (mouse_pos.x.as_f32(), mouse_pos.y.as_f32()),
                                    snap_target: None,
                                });
                                // Prevent node drag
                                state.drag_state = None;
                            });
                        }
                    })
                    // Handle mouse up → complete connection if valid
                    .on_mouse_up(MouseButton::Left, {
                        let state = state.clone();
                        let node_id = node_id.clone();
                        let handle_id = handle_id.clone();
                        move |_event, _window, cx| {
                            state.update(cx, |state, _| {
                                if let Some(draft) = state.connecting.take() {
                                    // Build the connection
                                    let (source, target, source_handle, target_handle) =
                                        if draft.from_type == HandleType::Source {
                                            (
                                                draft.from_node.clone(),
                                                node_id.clone(),
                                                draft.from_handle.clone(),
                                                handle_id.clone(),
                                            )
                                        } else {
                                            (
                                                node_id.clone(),
                                                draft.from_node.clone(),
                                                handle_id.clone(),
                                                draft.from_handle.clone(),
                                            )
                                        };

                                    let connection = Connection {
                                        source,
                                        target,
                                        source_handle,
                                        target_handle,
                                    };

                                    if state.is_valid_connection(&connection) {
                                        state.push_undo();
                                        let edge_id: SharedString = format!(
                                            "e{}-{}",
                                            connection.source, connection.target
                                        )
                                        .into();
                                        state.add_edge_from_connection(&connection, edge_id);
                                    }
                                }
                            });
                        }
                    });

                let container = match handle.position {
                    HandlePosition::Left => container
                        .left(px(-half))
                        .top_0()
                        .bottom_0()
                        .w(px(handle_size)),
                    HandlePosition::Right => container
                        .right(px(-half))
                        .top_0()
                        .bottom_0()
                        .w(px(handle_size)),
                    HandlePosition::Top => container
                        .top(px(-half))
                        .left_0()
                        .right_0()
                        .h(px(handle_size)),
                    HandlePosition::Bottom => container
                        .bottom(px(-half))
                        .left_0()
                        .right_0()
                        .h(px(handle_size)),
                };

                container.child(dot).into_any_element()
            })
            .collect()
    }

    /// Render a node with no per-type renderer registered: the caller's
    /// `default_renderer` when one was set, otherwise the built-in
    /// type-less label. (`default_renderer` used to be stored and never
    /// consulted here, silently routing every unmapped node — e.g. a
    /// leaf whose type isn't in `node_renderers` — to the hardcoded
    /// `render_default_node` palette.)
    fn render_fallback_node(&self, node: &FlowNode, window: &mut Window, cx: &mut App) -> AnyElement {
        match &self.default_renderer {
            Some(renderer) => renderer(node, window, cx),
            None => self.render_default_node(node, window, cx),
        }
    }

    /// Default node rendering — just the label content (styling is on the wrapper).
    fn render_default_node(
        &self,
        node: &FlowNode,
        _window: &mut Window,
        _cx: &mut App,
    ) -> AnyElement {
        div()
            .min_w(px(80.0))
            .text_sm()
            .text_color(gpui::rgb(0x1a1a1a))
            .child(if node.label.is_empty() {
                node.id.to_string()
            } else {
                node.label.to_string()
            })
            .into_any_element()
    }

    /// Paint the background dot grid.
    fn paint_grid(
        bounds: &Bounds<Pixels>,
        viewport: &Viewport,
        grid_color: u32,
        pattern: BackgroundPattern,
        window: &mut Window,
    ) {
        let color = gpui::rgb(grid_color);
        let spacing = 20.0 * viewport.zoom;

        if spacing < 5.0 {
            return;
        }

        let start_x = viewport.x % spacing;
        let start_y = viewport.y % spacing;
        let bw = bounds.size.width.as_f32();
        let bh = bounds.size.height.as_f32();
        let ox = bounds.origin.x;
        let oy = bounds.origin.y;

        match pattern {
            BackgroundPattern::Dots => {
                let dot_size = px(1.5 * viewport.zoom.min(1.0));
                let mut x = start_x;
                while x < bw {
                    let mut y = start_y;
                    while y < bh {
                        let dot_bounds = Bounds::new(
                            Point::new(ox + px(x) - dot_size / 2.0, oy + px(y) - dot_size / 2.0),
                            Size { width: dot_size, height: dot_size },
                        );
                        window.paint_quad(gpui::fill(dot_bounds, color));
                        y += spacing;
                    }
                    x += spacing;
                }
            }
            BackgroundPattern::Lines => {
                let line_w = px(0.5);
                let mut x = start_x;
                while x < bw {
                    let line = Bounds::new(
                        Point::new(ox + px(x), oy),
                        Size { width: line_w, height: bounds.size.height },
                    );
                    window.paint_quad(gpui::fill(line, color));
                    x += spacing;
                }
            }
            BackgroundPattern::Cross => {
                let line_w = px(0.5);
                // Vertical lines
                let mut x = start_x;
                while x < bw {
                    let line = Bounds::new(
                        Point::new(ox + px(x), oy),
                        Size { width: line_w, height: bounds.size.height },
                    );
                    window.paint_quad(gpui::fill(line, color));
                    x += spacing;
                }
                // Horizontal lines
                let mut y = start_y;
                while y < bh {
                    let line = Bounds::new(
                        Point::new(ox, oy + px(y)),
                        Size { width: bounds.size.width, height: line_w },
                    );
                    window.paint_quad(gpui::fill(line, color));
                    y += spacing;
                }
            }
        }
    }

    /// Paint a selection box rectangle.
    fn paint_selection_box(sel: &SelectionBox, accent: u32, window: &mut Window) {
        let x = sel.start.0.min(sel.current.0);
        let y = sel.start.1.min(sel.current.1);
        let w = (sel.start.0 - sel.current.0).abs();
        let h = (sel.start.1 - sel.current.1).abs();

        if w < 1.0 || h < 1.0 {
            return;
        }

        let bounds = Bounds::new(
            Point::new(px(x), px(y)),
            Size {
                width: px(w),
                height: px(h),
            },
        );

        // Semi-transparent accent fill
        window.paint_quad(fill(bounds, gpui::rgba((accent << 8) | 0x18)));

        // Accent border
        let border_color: Background = gpui::rgb(accent).into();
        // Top
        let top = Bounds::new(Point::new(px(x), px(y)), Size { width: px(w), height: px(1.0) });
        window.paint_quad(fill(top, border_color.clone()));
        // Bottom
        let bottom = Bounds::new(Point::new(px(x), px(y + h - 1.0)), Size { width: px(w), height: px(1.0) });
        window.paint_quad(fill(bottom, border_color.clone()));
        // Left
        let left = Bounds::new(Point::new(px(x), px(y)), Size { width: px(1.0), height: px(h) });
        window.paint_quad(fill(left, border_color.clone()));
        // Right
        let right = Bounds::new(Point::new(px(x + w - 1.0), px(y)), Size { width: px(1.0), height: px(h) });
        window.paint_quad(fill(right, border_color));
    }

    /// Paint a draft connection line from handle to mouse cursor.
    ///
    /// `draft.from_point` is captured (via `find_handle_center`) in
    /// flow-canvas-local space, the same space `handle_center_from_node`
    /// uses for edges (see the doc comment on `edges::paint_edges`) — so it
    /// needs `origin` added here at paint time. `draft.to_point` tracks the
    /// live mouse cursor and is already window-absolute; it must NOT get
    /// `origin` added a second time.
    fn paint_connection_draft(draft: &ConnectionDraft, origin: (f32, f32), accent: u32, window: &mut Window) {
        let color: Background = gpui::rgba((accent << 8) | 0x80).into();
        let (sx, sy) = (draft.from_point.0 + origin.0, draft.from_point.1 + origin.1);
        let (tx, ty) = draft.to_point;

        let mut builder = PathBuilder::stroke(px(2.0));
        builder.move_to(Point::new(px(sx), px(sy)));

        // Simple bezier toward cursor
        let dx = (tx - sx).abs() * 0.5;
        let (cx1, cy1, cx2, cy2) = match draft.from_position {
            HandlePosition::Right => (sx + dx, sy, tx - dx, ty),
            HandlePosition::Left => (sx - dx, sy, tx + dx, ty),
            HandlePosition::Bottom => (sx, sy + dx, tx, ty - dx),
            HandlePosition::Top => (sx, sy - dx, tx, ty + dx),
        };

        builder.cubic_bezier_to(
            Point::new(px(tx), px(ty)),
            Point::new(px(cx1), px(cy1)),
            Point::new(px(cx2), px(cy2)),
        );

        if let Ok(path) = builder.build() {
            window.paint_path(path, color);
        }
    }

    /// Paints a small "+" badge at the dangling end of a connection draft
    /// that isn't currently snapped to a handle — the visual affordance for
    /// "release here to create a new connected node" (see
    /// `on_connection_drop`). `pos` is window-absolute, same space
    /// `draft.to_point` is already in (see `paint_connection_draft`'s doc
    /// comment on that).
    fn paint_drop_to_create_hint(pos: (f32, f32), accent: u32, window: &mut Window) {
        let size = 22.0;
        let half = size / 2.0;
        let bg: Background = gpui::rgba((accent << 8) | 0xe6).into();
        let bounds = Bounds::new(
            Point::new(px(pos.0 - half), px(pos.1 - half)),
            Size { width: px(size), height: px(size) },
        );
        window.paint_quad(fill(bounds, bg).corner_radii(px(half)));

        let line_color: Background = gpui::white().into();
        let pad = 6.0;
        let mut h = PathBuilder::stroke(px(2.0));
        h.move_to(Point::new(px(pos.0 - half + pad), px(pos.1)));
        h.line_to(Point::new(px(pos.0 + half - pad), px(pos.1)));
        if let Ok(path) = h.build() {
            window.paint_path(path, line_color.clone());
        }
        let mut v = PathBuilder::stroke(px(2.0));
        v.move_to(Point::new(px(pos.0), px(pos.1 - half + pad)));
        v.line_to(Point::new(px(pos.0), px(pos.1 + half - pad)));
        if let Ok(path) = v.build() {
            window.paint_path(path, line_color);
        }
    }
}

impl Render for FlowGraph {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // On second render, edges will have correct node measurements
        // from the first render's measurement canvases.
        if !self.measured {
            self.measured = true;
            cx.notify(); // triggers one immediate re-render
        }

        let entity_id = cx.entity_id();
        let window_size = window.viewport_size();
        let win_w = window_size.width.as_f32();
        let win_h = window_size.height.as_f32();
        let cull_margin = 200.0;
        // Only worth painting the "drop here to create a node" hint if a
        // host actually registered `on_connection_drop` — otherwise a drop
        // on empty canvas just cancels, same as before this existed.
        let show_drop_to_create_hint = self.on_connection_drop.is_some();

        // Read state, extract what we need, then release the borrow
        let (viewport, is_panning, is_connecting, snap_node_id, connecting_draft, selection_box, visible_nodes, edge_label_elements) = {
            let state = self.state.read(cx);
            let viewport = state.viewport;
            let is_panning = state.pan_drag.is_some();
            let is_connecting = state.connecting.is_some();
            let snap_node_id = state
                .connecting
                .as_ref()
                .and_then(|d| d.snap_target.as_ref())
                .map(|t| t.node_id.clone());
            let connecting_draft = state.connecting.clone();
            let selection_box = state.selection_box;

            // Build visible node indices. Culling and sizing both use the
            // *resolved* absolute position/footprint (parent-chain-aware —
            // see `FlowState::absolute_position`/`node_footprint`), not the
            // raw (possibly parent-relative) `node.position` directly.
            let mut visible_indices: Vec<usize> = Vec::new();
            for (i, node) in state.nodes.iter().enumerate() {
                if node.hidden {
                    continue;
                }
                let abs_pos = state.absolute_position(&node.id).unwrap_or(node.position);
                let (nw, nh) = state.screen_footprint(node);
                let (sx, sy) = viewport.flow_to_screen(abs_pos);
                if sx + nw < -cull_margin || sx > win_w + cull_margin
                    || sy + nh < -cull_margin || sy > win_h + cull_margin
                {
                    continue;
                }
                visible_indices.push(i);
            }
            // Depth first, so a container always paints (and is listed as a
            // later sibling than) behind every one of its own descendants,
            // regardless of individual `z_index`; z_index only breaks ties
            // within the same depth, same as before nesting existed.
            visible_indices.sort_by_key(|&i| {
                (state.depth_of(&state.nodes[i].id), state.nodes[i].z_index)
            });

            let visible_nodes: Vec<(FlowNode, FlowPoint, (f32, f32), bool)> = visible_indices
                .iter()
                .map(|&i| {
                    let node = &state.nodes[i];
                    let abs_pos = state.absolute_position(&node.id).unwrap_or(node.position);
                    let footprint = state.node_footprint(node);
                    let is_container = state.is_container(&node.id);
                    (node.clone(), abs_pos, footprint, is_container)
                })
                .collect();

            // Edge labels
            let bg = self.bg_color;
            let mut edge_label_elements: Vec<AnyElement> = Vec::new();
            for edge in &state.edges {
                if let Some(ref label) = edge.label {
                    if let Some((lx, ly)) = edges::compute_edge_label_position(state, edge) {
                        let label_color = edge.color.unwrap_or(0xb1b1b7);
                        edge_label_elements.push(
                            div()
                                .absolute()
                                .left(px(lx))
                                .top(px(ly))
                                .px_2()
                                .py_0p5()
                                .bg(gpui::rgb(bg))
                                .rounded_sm()
                                .text_xs()
                                .text_color(gpui::rgb(label_color))
                                .child(label.to_string())
                                .into_any_element(),
                        );
                    }
                }
            }

            (viewport, is_panning, is_connecting, snap_node_id, connecting_draft, selection_box, visible_nodes, edge_label_elements)
        }; // state borrow ends here

        // Render only visible nodes
        let mut node_elements: Vec<AnyElement> = Vec::with_capacity(visible_nodes.len());
        for (node, abs_pos, footprint, is_container) in &visible_nodes {
            node_elements.push(self.render_node(
                node,
                *abs_pos,
                *footprint,
                *is_container,
                &viewport,
                is_connecting,
                snap_node_id.as_ref(),
                entity_id,
                window,
                cx,
            ));
        }

        let state_for_canvas = self.state.clone();
        let viewport_for_canvas = viewport;
        let bg_color = self.bg_color;
        let grid_color = self.grid_color;
        let bg_pattern = self.bg_pattern;
        let accent_color = self.accent_color;
        let state_for_scroll = self.state.clone();
        let state_for_mouse_down = self.state.clone();
        let state_for_mouse_move = self.state.clone();
        let state_for_mouse_up = self.state.clone();
        let state_for_canvas_context = self.state.clone();
        let on_connection_drop = self.on_connection_drop.clone();
        let state_for_key = self.state.clone();
        let state_for_pinch = self.state.clone();
        let on_canvas_context_menu = self.on_canvas_context_menu.clone();

        div()
            .id("flow-graph")
            .track_focus(&self.focus_handle)
            .size_full()
            .overflow_hidden()
            .relative()
            .bg(gpui::rgb(bg_color))
            .cursor(if is_panning {
                CursorStyle::ClosedHand
            } else if is_connecting {
                CursorStyle::Crosshair
            } else {
                CursorStyle::Arrow
            })
            // Background grid + edge painting layer
            .child({
                let canvas_origin_for_layout = self.canvas_origin.clone();
                canvas(
                    move |bounds, _window, _cx| {
                        canvas_origin_for_layout.set(bounds.origin);
                    },
                    move |bounds, _: (), window, cx| {
                        Self::paint_grid(&bounds, &viewport_for_canvas, grid_color, bg_pattern, window);
                        let origin = (bounds.origin.x.as_f32(), bounds.origin.y.as_f32());
                        let state = state_for_canvas.read(cx);
                        edges::paint_edges(state, origin, window);

                        // Paint draft connection line
                        if let Some(ref draft) = connecting_draft {
                            Self::paint_connection_draft(draft, origin, accent_color, window);
                            if show_drop_to_create_hint && draft.snap_target.is_none() {
                                Self::paint_drop_to_create_hint(draft.to_point, accent_color, window);
                            }
                        }

                        // Paint selection box
                        if let Some(ref sel) = selection_box {
                            Self::paint_selection_box(sel, accent_color, window);
                        }
                    },
                )
                .absolute()
                .size_full()
            })
            // Node layer
            .children(node_elements)
            // Edge labels
            .children(edge_label_elements)
            // Mouse down on empty space → start panning, deselect, or edge selection
            .on_mouse_down(MouseButton::Left, {
                let entity_id = entity_id;
                let canvas_origin_for_left_click = self.canvas_origin.clone();
                move |event, _window, cx| {
                    let mouse_pos = event.position;
                    state_for_mouse_down.update(cx, |state, _| {
                        // If a node drag or connection was already started, skip
                        if state.drag_state.is_some() || state.connecting.is_some() {
                            return;
                        }

                        // Try edge hit testing. `hit_test_edges` compares
                        // against "flow-canvas-local" coordinates (no
                        // window offset — see `canvas_origin`'s doc
                        // comment), so the click point needs that same
                        // offset subtracted first; `mx`/`my` below stay
                        // window-absolute for the box-selection/pan-drag
                        // code further down, which is delta-based and
                        // doesn't need this adjustment.
                        let origin = canvas_origin_for_left_click.get();
                        let edge_mx = mouse_pos.x.as_f32() - origin.x.as_f32();
                        let edge_my = mouse_pos.y.as_f32() - origin.y.as_f32();
                        let mx = mouse_pos.x.as_f32();
                        let my = mouse_pos.y.as_f32();
                        if let Some(edge_id) = edges::hit_test_edges(state, edge_mx, edge_my, 5.0) {
                            if !event.modifiers.platform {
                                for n in &mut state.nodes {
                                    n.selected = false;
                                }
                                for e in &mut state.edges {
                                    e.selected = false;
                                }
                            }
                            if let Some(edge) = state.edges.iter_mut().find(|e| e.id == edge_id) {
                                edge.selected = true;
                            }
                            return;
                        }

                        // Shift+drag → start box selection
                        if event.modifiers.shift {
                            if !event.modifiers.platform {
                                for n in &mut state.nodes {
                                    n.selected = false;
                                }
                                for e in &mut state.edges {
                                    e.selected = false;
                                }
                            }
                            state.selection_box = Some(SelectionBox {
                                start: (mx, my),
                                current: (mx, my),
                            });
                            return;
                        }

                        // Deselect all nodes and edges
                        if !event.modifiers.platform {
                            for n in &mut state.nodes {
                                n.selected = false;
                            }
                            for e in &mut state.edges {
                                e.selected = false;
                            }
                        }

                        // Start pan drag
                        state.pan_drag = Some(PanDragState {
                            start_mouse: (mx, my),
                            start_viewport: (state.viewport.x, state.viewport.y),
                        });
                    });
                    cx.notify(entity_id);
                }
            })
            .when_some(on_canvas_context_menu, |el, callback| {
                let canvas_origin = self.canvas_origin.clone();
                let viewport = viewport_for_canvas;
                el.on_mouse_down(MouseButton::Right, move |event, window, cx| {
                    let origin = canvas_origin.get();
                    let x = event.position.x.as_f32() - origin.x.as_f32();
                    let y = event.position.y.as_f32() - origin.y.as_f32();
                    let node_id = state_for_canvas_context
                        .read(cx)
                        .node_at_screen((x, y));
                    callback(event.position, viewport.screen_to_flow(x, y), node_id, window, cx);
                })
            })
            // Right-click on an edge → `on_edge_context_menu`, same
            // `hit_test_edges` geometry the left-click select path above
            // uses. No-op (falls through to the OS/default context menu
            // behavior) when the click doesn't land on an edge, or no
            // callback is registered.
            .when_some(self.on_edge_context_menu.clone(), |el, callback| {
                let state_for_edge_ctx = self.state.clone();
                let canvas_origin_for_edge_ctx = self.canvas_origin.clone();
                el.on_mouse_down(MouseButton::Right, move |event, window, cx| {
                    // See `canvas_origin`'s doc comment — the click point
                    // needs the canvas's own window offset subtracted
                    // before comparing against `hit_test_edges`'s
                    // flow-canvas-local coordinates.
                    let origin = canvas_origin_for_edge_ctx.get();
                    let mx = event.position.x.as_f32() - origin.x.as_f32();
                    let my = event.position.y.as_f32() - origin.y.as_f32();
                    let hit = edges::hit_test_edges(state_for_edge_ctx.read(cx), mx, my, 5.0);
                    if let Some(edge_id) = hit {
                        callback(edge_id, event.position, window, cx);
                    }
                })
            })
            // Global mouse move → handle dragging, panning, or connecting
            .on_mouse_move({
                let entity_id = entity_id;
                move |event, _window, cx| {
                    let mouse_pos = event.position;
                    let mx = mouse_pos.x.as_f32();
                    let my = mouse_pos.y.as_f32();

                    let mut changed = false;

                    state_for_mouse_move.update(cx, |state, _| {
                        // Box selection
                        if let Some(ref mut sel) = state.selection_box {
                            sel.current = (mx, my);
                            // Select nodes whose screen bounds intersect the box
                            let (sx, sy, ex, ey) = (
                                sel.start.0.min(sel.current.0),
                                sel.start.1.min(sel.current.1),
                                sel.start.0.max(sel.current.0),
                                sel.start.1.max(sel.current.1),
                            );
                            let viewport = state.viewport;
                            // Resolved (absolute-position, footprint) per
                            // node computed first — `absolute_position`/
                            // `node_footprint` need an immutable `&state`,
                            // which can't overlap the `&mut state.nodes`
                            // loop below that writes `selected`.
                            let screen_boxes: Vec<(NodeId, (f32, f32), (f32, f32))> = state
                                .nodes
                                .iter()
                                .filter(|n| !n.hidden)
                                .map(|n| {
                                    let abs_pos = state.absolute_position(&n.id).unwrap_or(n.position);
                                    let footprint = state.screen_footprint(n);
                                    (n.id.clone(), viewport.flow_to_screen(abs_pos), footprint)
                                })
                                .collect();
                            for node in &mut state.nodes {
                                if node.hidden {
                                    continue;
                                }
                                let Some((_, (nx, ny), (nw, nh))) =
                                    screen_boxes.iter().find(|(id, _, _)| *id == node.id)
                                else {
                                    continue;
                                };
                                // AABB intersection
                                let intersects = *nx < ex && nx + nw > sx && *ny < ey && ny + nh > sy;
                                node.selected = intersects;
                            }
                            changed = true;
                        }
                        // Connection dragging with snap-to-handle
                        else if state.connecting.is_some() {
                            // Clone draft to avoid borrow conflict with find_snap_target
                            let mut draft = state.connecting.clone().unwrap();
                            let snap = state.find_snap_target(&draft, mx, my);
                            if let Some(ref target) = snap {
                                draft.to_point = target.point;
                            } else {
                                draft.to_point = (mx, my);
                            }
                            draft.snap_target = snap;
                            state.connecting = Some(draft);
                            changed = true;
                        }
                        // Node dragging
                        else if let Some(ref drag) = state.drag_state {
                            let dx = (mx - drag.origin_mouse.0) / state.viewport.zoom;
                            let dy = (my - drag.origin_mouse.1) / state.viewport.zoom;

                            let origins = drag.node_origins.clone();
                            let snap = state.snap_to_grid;
                            let snap_grid = state.snap_grid;
                            for (node_id, origin) in &origins {
                                let mut new_x = origin.x + dx;
                                let mut new_y = origin.y + dy;

                                if snap {
                                    let (gx, gy) = snap_grid;
                                    new_x = (new_x / gx).round() * gx;
                                    new_y = (new_y / gy).round() * gy;
                                }

                                if let Some(node) = state.get_node_mut(node_id) {
                                    node.position = FlowPoint::new(new_x, new_y);
                                }
                            }
                            changed = true;
                        }
                        // Canvas panning
                        else if let Some(pan) = state.pan_drag {
                            state.viewport.x = pan.start_viewport.0 + (mx - pan.start_mouse.0);
                            state.viewport.y = pan.start_viewport.1 + (my - pan.start_mouse.1);
                            changed = true;
                        }
                    });

                    if changed {
                        cx.notify(entity_id);
                    }
                }
            })
            // Global mouse up → end dragging, panning, or cancel connection
            .on_mouse_up(MouseButton::Left, {
                let entity_id = entity_id;
                let on_connection_drop = on_connection_drop.clone();
                move |_event, _window, cx| {
                    let mut changed = false;

                    state_for_mouse_up.update(cx, |state, _| {
                        if state.selection_box.is_some() {
                            state.selection_box = None;
                            changed = true;
                        }
                        if let Some(draft) = state.connecting.take() {
                            if let Some(snap) = draft.snap_target {
                                // Complete the connection
                                let (source, target, source_handle, target_handle) =
                                    if draft.from_type == HandleType::Source {
                                        (
                                            draft.from_node.clone(),
                                            snap.node_id.clone(),
                                            draft.from_handle.clone(),
                                            snap.handle_id.clone(),
                                        )
                                    } else {
                                        (
                                            snap.node_id.clone(),
                                            draft.from_node.clone(),
                                            snap.handle_id.clone(),
                                            draft.from_handle.clone(),
                                        )
                                    };

                                let connection = Connection {
                                    source,
                                    target,
                                    source_handle,
                                    target_handle,
                                };

                                if state.is_valid_connection(&connection) {
                                    state.push_undo();
                                    let edge_id: SharedString =
                                        format!("e{}-{}", connection.source, connection.target)
                                            .into();
                                    state.add_edge_from_connection(&connection, edge_id);
                                }
                            } else if let Some(callback) = &on_connection_drop {
                                // Dropped on empty canvas — the "drag from a
                                // handle to create a new connected node"
                                // gesture (no snap target means the drop
                                // wasn't on another handle).
                                let flow_point =
                                    state.viewport.screen_to_flow(draft.to_point.0, draft.to_point.1);
                                callback(&draft, flow_point, state);
                            }
                            changed = true;
                        }
                        if state.drag_state.is_some() {
                            state.drag_state = None;
                            for n in &mut state.nodes {
                                n.dragging = false;
                            }
                            changed = true;
                        }
                        if state.pan_drag.is_some() {
                            state.pan_drag = None;
                            changed = true;
                        }
                    });

                    if changed {
                        cx.notify(entity_id);
                    }
                }
            })
            // Keyboard shortcuts (delete, undo/redo)
            .on_key_down({
                let entity_id = entity_id;
                let focus = self.focus_handle.clone();
                move |event: &KeyDownEvent, window, cx| {
                    let key: &str = event.keystroke.key.as_ref();

                    // Only handle destructive keys (delete/backspace) when the
                    // flow graph itself is focused — not when a child element
                    // (like an Input inside a node) has focus.
                    let graph_focused = focus.is_focused(window);

                    // Undo: Cmd+Z (always allowed)
                    if key == "z" && event.keystroke.modifiers.platform && !event.keystroke.modifiers.shift {
                        state_for_key.update(cx, |state, _| {
                            state.undo();
                        });
                        cx.notify(entity_id);
                        return;
                    }
                    // Redo: Cmd+Shift+Z
                    if key == "z" && event.keystroke.modifiers.platform && event.keystroke.modifiers.shift {
                        state_for_key.update(cx, |state, _| {
                            state.redo();
                        });
                        cx.notify(entity_id);
                        return;
                    }

                    // Select all: Cmd+A
                    if key == "a" && event.keystroke.modifiers.platform && graph_focused {
                        state_for_key.update(cx, |state, _| {
                            state.select_all();
                        });
                        cx.notify(entity_id);
                        return;
                    }

                    if (key == "backspace" || key == "delete") && graph_focused {
                        state_for_key.update(cx, |state, _| {
                            state.push_undo();
                            // Collect IDs of selected deletable nodes
                            let node_ids_to_remove: Vec<NodeId> = state
                                .nodes
                                .iter()
                                .filter(|n| n.selected && n.deletable)
                                .map(|n| n.id.clone())
                                .collect();

                            // Remove edges connected to deleted nodes + selected edges
                            state.edges.retain(|e| {
                                let connected_to_removed = node_ids_to_remove.contains(&e.source)
                                    || node_ids_to_remove.contains(&e.target);
                                let selected_and_deletable = e.selected && e.deletable;
                                !connected_to_removed && !selected_and_deletable
                            });

                            // Remove selected nodes
                            state.nodes.retain(|n| !node_ids_to_remove.contains(&n.id));
                            if !node_ids_to_remove.is_empty() {
                                state.rebuild_lookup();
                            }
                        });
                        cx.notify(entity_id);
                    }
                }
            })
            // Scroll to zoom / pan
            .on_scroll_wheel({
                move |event, _window, cx| {
                    let delta = event.delta.pixel_delta(px(20.0));
                    let mouse_pos = event.position;

                    if event.modifiers.platform || event.modifiers.control {
                        // Zoom towards mouse
                        let zoom_delta = -delta.y.as_f32() * 0.01;
                        state_for_scroll.update(cx, |state, _| {
                            let old_zoom = state.viewport.zoom;
                            let new_zoom =
                                (old_zoom + zoom_delta).clamp(state.min_zoom, state.max_zoom);
                            let mx = mouse_pos.x.as_f32();
                            let my = mouse_pos.y.as_f32();
                            state.viewport.x =
                                mx - (mx - state.viewport.x) * (new_zoom / old_zoom);
                            state.viewport.y =
                                my - (my - state.viewport.y) * (new_zoom / old_zoom);
                            state.viewport.zoom = new_zoom;
                            state.refit_all_containers();
                        });
                    } else {
                        // Pan
                        state_for_scroll.update(cx, |state, _| {
                            state.viewport.x += delta.x.as_f32();
                            state.viewport.y += delta.y.as_f32();
                        });
                    }

                    cx.notify(entity_id);
                }
            })
            // Pinch to zoom (macOS trackpad / Linux Wayland)
            .on_pinch({
                move |event, _window, cx| {
                    let mouse_pos = event.position;
                    let zoom_delta = event.delta;
                    state_for_pinch.update(cx, |state, _| {
                        let old_zoom = state.viewport.zoom;
                        let new_zoom =
                            (old_zoom * (1.0 + zoom_delta)).clamp(state.min_zoom, state.max_zoom);
                        let mx = mouse_pos.x.as_f32();
                        let my = mouse_pos.y.as_f32();
                        state.viewport.x =
                            mx - (mx - state.viewport.x) * (new_zoom / old_zoom);
                        state.viewport.y =
                            my - (my - state.viewport.y) * (new_zoom / old_zoom);
                        state.viewport.zoom = new_zoom;
                        state.refit_all_containers();
                    });
                    cx.notify(entity_id);
                }
            })
    }
}
