//! Small helpers shared by Helm's screens: a loading
//! line, a chip, and number, date and colour formatting.

use super::*;

/// A spinner beside a line of text, in the middle of the space a screen has.
/// What a screen shows while the one thing it is about is being fetched.
pub(super) fn loading_screen(label: &'static str, cx: &App) -> gpui::AnyElement {
    v_flex()
        .flex_1()
        .items_center()
        .justify_center()
        .p_4()
        .child(
            h_flex()
                .gap_2()
                .items_center()
                .child(Spinner::new().small())
                .child(
                    div()
                        .text_sm()
                        .text_color(cx.theme().muted_foreground)
                        .child(label),
                ),
        )
        .into_any_element()
}

/// One muted line in the middle of the space a screen has: "nothing here",
/// in whatever words fit.
pub(super) fn note_screen(text: &'static str, cx: &App) -> gpui::AnyElement {
    v_flex()
        .flex_1()
        .items_center()
        .justify_center()
        .p_4()
        .child(
            div()
                .text_sm()
                .text_color(cx.theme().muted_foreground)
                .child(text),
        )
        .into_any_element()
}

/// What a screen shows when the one thing it is about could not be fetched:
/// what failed, the reason GitHub gave, and a way to try again when there
/// is one. The same three lines a list screen shows.
pub(super) fn failed_screen(
    label: &'static str,
    reason: &str,
    retry: Option<Button>,
    cx: &App,
) -> gpui::AnyElement {
    let muted_foreground = cx.theme().muted_foreground;
    v_flex()
        .gap_3()
        .p_4()
        .child(div().text_sm().text_color(muted_foreground).child(label))
        .when(!reason.is_empty(), |column| {
            column.child(
                div()
                    .text_xs()
                    .text_color(muted_foreground)
                    .child(reason.to_string()),
            )
        })
        .children(retry)
        .into_any_element()
}

/// A small rounded label: an issue's label, a repository's topic, "default"
/// beside a branch.
pub(super) fn chip(text: impl Into<SharedString>) -> Pill {
    Pill::secondary().xsmall().rounded_full().child(text.into())
}

/// `gpui_flow` takes raw `u32` colors (it has no notion of a theme), so
/// anything bridging a GPUI `Hsla` theme color into it needs this — same
/// helper `designer_panel`/`database_panel` each define for their own
/// `FlowGraph` usage.
pub(super) fn hex(color: gpui::Hsla) -> u32 {
    u32::from(color.to_rgb()) >> 8
}

/// Mirrors the old TS `visLabel`: archived beats internal beats
/// private/public.
pub(super) fn repo_vis_label(repo: &Repo) -> &'static str {
    if repo.archived {
        "archived"
    } else if repo.visibility == "internal" {
        "internal"
    } else if repo.private {
        "private"
    } else {
        "public"
    }
}

/// Formats a count as `1.2k` above 1000, plain otherwise.
pub(super) fn fmt_num(n: u64) -> String {
    if n >= 1000 {
        format!("{:.1}k", n as f64 / 1000.0)
    } else {
        n.to_string()
    }
}

/// Trims an ISO-8601 date from the GitHub API down to its `YYYY-MM-DD` part.
pub(super) fn short_date(iso: &str) -> String {
    iso.get(..10)
        .map(|s| s.to_string())
        .unwrap_or_else(|| iso.to_string())
}
