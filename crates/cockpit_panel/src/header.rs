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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uptime_under_a_day_omits_the_day_segment() {
        assert_eq!(fmt_uptime(0), "00:00:00");
        assert_eq!(fmt_uptime(59), "00:00:59");
        assert_eq!(fmt_uptime(60), "00:01:00");
        assert_eq!(fmt_uptime(3_661), "01:01:01");
        assert_eq!(fmt_uptime(86_399), "23:59:59");
    }

    #[test]
    fn uptime_of_a_day_or_more_shows_days() {
        assert_eq!(fmt_uptime(86_400), "1d 00:00:00");
        assert_eq!(fmt_uptime(90_061), "1d 01:01:01");
        assert_eq!(fmt_uptime(40 * 86_400 + 5), "40d 00:00:05");
    }
}
