//! Panel header row and the system-metrics header line.

use gpui::{App, ClickEvent, Hsla, Window, div, prelude::*, px};
use gpui_component::{
    Icon, IconName, Sizable as _,
    button::{Button, ButtonVariants as _},
};

/// The icon + title heading block, plus a trailing "Dashboard" button, with
/// a separator underneath. `show_dashboard_button` is false when the panel is
/// mirrored inside the Dashboard's own "System" section, where the button
/// would be redundant (the Dashboard is already open).
pub fn cockpit_header(
    icon: IconName,
    title: &'static str,
    foreground: Hsla,
    border: Hsla,
    show_dashboard_button: bool,
    on_open_dashboard: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .flex_shrink_0()
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .px_3()
                .py_2()
                .child(
                    div()
                        .flex()
                        .gap_2()
                        .items_center()
                        .child(Icon::new(icon).text_color(foreground))
                        .child(
                            div()
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .text_sm()
                                .text_color(foreground)
                                .child(title),
                        ),
                )
                .when(show_dashboard_button, |row| {
                    row.child(
                        Button::new("cockpit-open-dashboard")
                            .ghost()
                            .xsmall()
                            .icon(IconName::LayoutDashboard)
                            .label("Dashboard")
                            .on_click(on_open_dashboard),
                    )
                }),
        )
        .child(div().h(px(1.0)).w_full().bg(border))
}

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
