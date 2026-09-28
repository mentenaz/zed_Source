//! The system-metrics header line (the panel's icon/title header itself is
//! `gpui_component::panel_header::PanelHeader` — see `cockpit_panel.rs`).

use gpui::{Hsla, div, prelude::*, px};

/// The system-metrics line under the panel header: core count + uptime. A
/// pinned row with a separator underneath, sitting between the panel header
/// and the scrollable body.
pub fn system_header(
    cores: usize,
    uptime: u64,
    accent: Hsla,
    muted: Hsla,
    border: Hsla,
) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .flex_shrink_0()
        .child(
            div()
                .flex()
                .flex_col()
                .gap_1()
                .px_3()
                .py_2()
                .child(
                    div()
                        .font_family("Cascadia Mono")
                        .text_color(accent)
                        .child(format!("{cores} Logical Cores")),
                )
                .child(
                    div()
                        .font_family("Cascadia Mono")
                        .text_color(muted)
                        .child(format!("up {}", fmt_uptime(uptime))),
                ),
        )
        .child(div().h(px(1.0)).w_full().bg(border))
}

/// Format seconds as `Xd HH:MM:SS`, omitting the day segment when zero.
fn fmt_uptime(total_seconds: u64) -> String {
    let days = total_seconds / 86_400;
    let hours = (total_seconds % 86_400) / 3_600;
    let minutes = (total_seconds % 3_600) / 60;
    let seconds = total_seconds % 60;
    if days > 0 {
        format!("{days}d {hours:02}:{minutes:02}:{seconds:02}")
    } else {
        format!("{hours:02}:{minutes:02}:{seconds:02}")
    }
}
