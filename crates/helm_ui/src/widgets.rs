//! Small helpers shared by Helm's screens: a loading
//! line, a chip, and number, date and colour formatting.

use gpui::{
    App, Hsla, IntoElement as _, ParentElement as _, SharedString, Styled as _, div,
    prelude::FluentBuilder as _,
};
use gpui_component::{
    ActiveTheme as _, Sizable as _, Size,
    avatar::Avatar,
    button::{Button, ButtonVariants as _},
    h_flex,
    progress::Progress,
    spinner::Spinner,
    tag::Tag as Pill,
    v_flex,
};

use helm_backend::github::{ErrorLink, RateLimit, Repo};

/// The size of an avatar in a list row (members, collaborators, the
/// account menu).
pub const AVATAR_SIZE: f32 = 24.0;
/// The size of the avatar heading a profile.
pub const PROFILE_AVATAR_SIZE: f32 = 40.0;

/// `url` asking GitHub for an image just big enough for `size` pixels on a
/// 2x screen (`s=`), instead of the full-size original. Empty stays empty.
pub fn sized_avatar_url(url: &str, size: f32) -> String {
    if url.is_empty() {
        return String::new();
    }
    let pixels = (size * 2.0).ceil() as u32;
    let separator = if url.contains('?') { '&' } else { '?' };
    format!("{url}{separator}s={pixels}")
}

/// A GitHub account's avatar as Helm draws it everywhere: `size` pixels,
/// the image fetched at that size, and the login's initials while it loads
/// or when there is none. Put the name beside it; the avatar alone does
/// not say who it is.
pub fn github_avatar(url: &str, login: &str, size: f32) -> Avatar {
    let avatar = Avatar::new()
        .name(login.to_string())
        .with_size(gpui::px(size));
    let url = sized_avatar_url(url, size);
    if url.is_empty() {
        avatar
    } else {
        avatar.src(url)
    }
}

/// How much of GitHub's hourly allowance is left, drawn like the cockpit's
/// disk rows: an "API" label, a bar that empties as requests are used, and
/// `4,985 / 5,000 · resets in 9 min`. Green, amber below a tenth, red below
/// a fiftieth. `None` before the first answer and after the reset time,
/// when the count on record is the old window's.
pub fn rate_limit_bar(
    id: &'static str,
    rate: Option<RateLimit>,
    cx: &App,
) -> Option<gpui::AnyElement> {
    let rate = rate?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());
    if now >= rate.reset_at {
        return None;
    }
    let theme = cx.theme();
    let color = if rate.is_critical() {
        theme.danger
    } else if rate.is_low() {
        theme.warning
    } else {
        theme.success
    };
    Some(
        h_flex()
            .items_center()
            .gap_2()
            .px_3()
            .py_1()
            .text_xs()
            .font_family("Cascadia Mono")
            .text_color(theme.muted_foreground)
            .child(div().flex_shrink_0().child("API"))
            .child(
                Progress::new(id)
                    .value(rate.fraction_left() * 100.0)
                    .color(color)
                    .with_size(Size::XSmall)
                    .flex_1(),
            )
            .child(div().flex_shrink_0().child(rate.readout(now)))
            .into_any_element(),
    )
}

/// A spinner beside a line of text, in the middle of the space a screen has.
/// What a screen shows while the one thing it is about is being fetched.
pub fn loading_screen(label: &'static str, cx: &App) -> gpui::AnyElement {
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
pub fn note_screen(text: &'static str, cx: &App) -> gpui::AnyElement {
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
pub fn failed_screen(
    label: &'static str,
    reason: &str,
    link: Option<ErrorLink>,
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
        .child(
            h_flex()
                .gap_2()
                // Where GitHub lets the user put it right: single sign-on,
                // or an organisation's approval of Helm.
                .children(link.map(|link| {
                    Button::new("helm-error-link")
                        .primary()
                        .label(link.label)
                        .on_click(move |_, _, cx| cx.open_url(&link.url))
                }))
                .children(retry),
        )
        .into_any_element()
}

/// A small rounded label: an issue's label, a repository's topic, "default"
/// beside a branch.
pub fn chip(text: impl Into<SharedString>) -> Pill {
    Pill::secondary().xsmall().rounded_full().child(text.into())
}

/// `gpui_flow` takes raw `u32` colors (it has no notion of a theme), so
/// anything bridging a GPUI `Hsla` theme color into it needs this — same
/// helper `designer_panel`/`database_panel` each define for their own
/// `FlowGraph` usage.
pub fn hex(color: Hsla) -> u32 {
    u32::from(color.to_rgb()) >> 8
}

/// Mirrors the old TS `visLabel`: archived beats internal beats
/// private/public.
pub fn repo_vis_label(repo: &Repo) -> &'static str {
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
pub fn fmt_num(n: u64) -> String {
    if n >= 1000 {
        format!("{:.1}k", n as f64 / 1000.0)
    } else {
        n.to_string()
    }
}

/// Trims an ISO-8601 date from the GitHub API down to its `YYYY-MM-DD` part.
pub fn short_date(iso: &str) -> String {
    iso.get(..10)
        .map(|s| s.to_string())
        .unwrap_or_else(|| iso.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn avatars_are_fetched_at_the_size_they_are_drawn() {
        assert_eq!(
            sized_avatar_url("https://avatars.githubusercontent.com/u/1?v=4", 24.0),
            "https://avatars.githubusercontent.com/u/1?v=4&s=48"
        );
        assert_eq!(
            sized_avatar_url("https://avatars.githubusercontent.com/u/1", 40.0),
            "https://avatars.githubusercontent.com/u/1?s=80"
        );
        assert_eq!(sized_avatar_url("", 24.0), "");
    }
}
