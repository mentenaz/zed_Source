use gpui::*;

use crate::store::FlowState;

/// A controls panel with zoom +/-, fit-view, and lock buttons.
///
/// Position this absolutely in a corner of your flow graph container.
pub struct Controls {
    state: Entity<FlowState>,
    container_size: (f32, f32),
}

impl Controls {
    pub fn new(state: Entity<FlowState>) -> Self {
        Self {
            state,
            container_size: (900.0, 600.0),
        }
    }

    /// Set the container size (needed for zoom centering and fit-view).
    pub fn container_size(mut self, width: f32, height: f32) -> Self {
        self.container_size = (width, height);
        self
    }
}

impl Render for Controls {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state_zoom_in = self.state.clone();
        let state_zoom_out = self.state.clone();
        let state_fit = self.state.clone();
        let (cw, ch) = self.container_size;
        let entity_id = cx.entity_id();

        div()
            .flex()
            .flex_col()
            .gap_1()
            .p_1()
            .bg(gpui::rgba(0x18181b_ee))
            .rounded_md()
            .border_1()
            .border_color(gpui::rgb(0x27272a))
            .child(control_button("+", {
                move |_, _, cx| {
                    state_zoom_in.update(cx, |state, _| {
                        state.zoom_in(cw, ch);
                    });
                    cx.notify(entity_id);
                }
            }))
            .child(control_button("\u{2212}", {
                // Unicode minus sign
                move |_, _, cx| {
                    state_zoom_out.update(cx, |state, _| {
                        state.zoom_out(cw, ch);
                    });
                    cx.notify(entity_id);
                }
            }))
            .child(
                div()
                    .h(px(1.0))
                    .bg(gpui::rgb(0x27272a))
                    .mx_1(),
            )
            .child(control_button("\u{2922}", {
                // Fit view icon (↢)
                move |_, _, cx| {
                    state_fit.update(cx, |state, _| {
                        state.fit_view(40.0, cw, ch);
                    });
                    cx.notify(entity_id);
                }
            }))
    }
}

fn control_button(
    label: &'static str,
    on_click: impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .id(SharedString::from(label))
        .w(px(28.0))
        .h(px(28.0))
        .flex()
        .items_center()
        .justify_center()
        .rounded_sm()
        .cursor(CursorStyle::PointingHand)
        .text_sm()
        .text_color(gpui::rgb(0xa1a1aa))
        .on_mouse_down(MouseButton::Left, on_click)
        .child(label)
}
