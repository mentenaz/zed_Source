//! Small helpers shared by Helm's screens: a labelled form field, list
//! selection stepping, and number, date and colour formatting.

use super::*;

/// A small label above an `Input` — for the modal forms whose fields (unlike
/// `CreateRepo`'s) have no distinguishing placeholder text of their own.
pub(super) fn labeled_field(
    label: &'static str,
    input: impl IntoElement,
    muted: gpui::Hsla,
) -> impl IntoElement {
    v_flex()
        .gap_1()
        .child(div().text_xs().text_color(muted).child(label))
        .child(input)
}

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

/// A small rounded label: an issue's label, a repository's topic, "default"
/// beside a branch.
pub(super) fn chip(text: impl Into<SharedString>) -> Pill {
    Pill::secondary().xsmall().rounded_full().child(text.into())
}

/// Moves `selected` one row up (`forward: false`) or down (`forward: true`)
/// within a `len`-row list, wrapping at both ends — matches
/// `gpui_component::table::TableState`'s default `loop_selection` behavior
/// — and starting from the top row on the very first press. Same helper as
/// `npm_manager_panel`/`nuget_manager_panel`/`python_manager_panel::pages`'s
/// own `step_selected`.
pub(super) fn step_selected(selected: Option<usize>, len: usize, forward: bool) -> Option<usize> {
    if len == 0 {
        return None;
    }
    Some(match selected {
        // Nothing selected yet: the first press lands on the nearest end
        // rather than stepping past it.
        None => {
            if forward {
                0
            } else {
                len - 1
            }
        }
        Some(ix) => {
            if forward {
                (ix + 1) % len
            } else {
                (ix + len - 1) % len
            }
        }
    })
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
