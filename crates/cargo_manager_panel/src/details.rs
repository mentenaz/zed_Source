//! The right-hand details pane shown while a dependency is selected: how
//! the crate declares it, and the versions crates.io has, each marked with
//! its minimum Rust version.
//!
//! The version list is the picker from the design note (decision 4): yanked
//! versions are never offered, and "compatible versions only" hides the ones
//! that declare a minimum Rust version above the installed toolchain while
//! keeping the ones that declare nothing. It costs no request: the index
//! entries were already fetched for the Updates page.

use cargo_backend::{Declaration, ListedDependency, available_versions};
use gpui::{
    AnyElement, App, Entity, InteractiveElement as _, IntoElement, ParentElement as _,
    StatefulInteractiveElement as _, Styled as _, div,
};
use gpui_component::{
    ActiveTheme as _, Disableable as _, IconName, Sizable as _, Size, StyledExt as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    switch::Switch,
    tag::Tag,
    v_flex,
};

use crate::CargoManagerPanel;
use crate::pages::rust_tag;

/// How many versions the pane lists. Popular crates have hundreds; the
/// newest are the ones anyone picks from.
const LISTED_VERSIONS: usize = 40;

pub(super) fn details_pane(
    panel: &CargoManagerPanel,
    view: &Entity<CargoManagerPanel>,
    cx: &App,
) -> impl IntoElement {
    let theme = cx.theme();
    let package = panel.selected.clone().unwrap_or_default();

    let header = h_flex()
        .w_full()
        .items_center()
        .justify_between()
        .gap_2()
        .px_3()
        .py_1p5()
        .border_b_1()
        .border_color(theme.border)
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_xs()
                .font_semibold()
                .text_color(theme.foreground)
                .child(package.clone()),
        )
        .child(
            Button::new("cargo-close-details")
                .icon(IconName::Close)
                .ghost()
                .xsmall()
                .on_click({
                    let view = view.clone();
                    move |_, _, cx| view.update(cx, |panel, cx| panel.close_details(cx))
                }),
        );

    let body: AnyElement = match panel.listed(&package) {
        Some(row) => details_content(panel, row, view, cx).into_any_element(),
        None => div()
            .p_4()
            .text_xs()
            .text_color(theme.muted_foreground)
            .child("This crate no longer depends on it.")
            .into_any_element(),
    };

    v_flex()
        .size_full()
        .bg(theme.background)
        .child(header)
        .child(body)
}

fn info_row(cx: &App, label: &str, value: String) -> impl IntoElement {
    let theme = cx.theme();
    h_flex()
        .items_start()
        .gap_2()
        .w_full()
        .child(
            div()
                .w_24()
                .flex_none()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(label.to_string()),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_xs()
                .font_family("Cascadia Mono")
                .text_color(theme.foreground)
                .child(value),
        )
}

fn details_content(
    panel: &CargoManagerPanel,
    row: &ListedDependency,
    view: &Entity<CargoManagerPanel>,
    cx: &App,
) -> impl IntoElement {
    let theme = cx.theme();
    let declared = panel.declaration_of(row);
    // A version can be chosen here only when the requirement is this
    // crate's own to change: see `CargoManagerPanel::change_version`.
    let changeable = declared == Some(Declaration::Literal) && row.target.is_none();

    let mut table = row.kind.label().to_string();
    if let Some(target) = &row.target {
        table.push_str(&format!(", {target}"));
    }
    if row.optional {
        table.push_str(", optional");
    }

    let link = |id: &'static str, label: &'static str, url: String| {
        Button::new(id)
            .ghost()
            .xsmall()
            .label(label)
            .on_click(move |_, _, cx| cx.open_url(&url))
    };

    let mut column = v_flex()
        .id("cargo-details-scroll")
        .flex_1()
        .min_h_0()
        .overflow_y_scroll()
        .gap_1p5()
        .w_full()
        .p_4()
        .child(info_row(cx, "Requires", row.requirement.clone()))
        .child(info_row(
            cx,
            "Locked",
            // Unknown stays unknown: never a guessed version.
            row.locked_version.clone().unwrap_or_else(|| "unknown".into()),
        ))
        .child(info_row(
            cx,
            "Declared",
            match declared {
                Some(Declaration::Inherited) => "in the root Cargo.toml (workspace = true)",
                Some(Declaration::Literal) => "in this crate's Cargo.toml",
                None => "unknown",
            }
            .to_string(),
        ))
        .child(info_row(cx, "Table", table));
    if let Some(rename) = &row.rename {
        column = column.child(info_row(cx, "Imported as", rename.clone()));
    }
    column = column.child(
        h_flex()
            .gap_1()
            .child(link(
                "cargo-open-crates-io",
                "crates.io",
                format!("https://crates.io/crates/{}", row.name),
            ))
            .child(link(
                "cargo-open-docs-rs",
                "docs.rs",
                match &row.locked_version {
                    Some(version) => format!("https://docs.rs/{}/{version}", row.name),
                    None => format!("https://docs.rs/{}", row.name),
                },
            )),
    );

    let Some(cached) = panel.index.get(&row.name) else {
        return column.child(
            div()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child("Versions have not been loaded from crates.io yet."),
        );
    };

    let choices = available_versions(
        &cached.versions,
        panel.rustc_version.as_deref(),
        panel.compatible_only,
        false,
    );
    column = column
        .child(div().h_1())
        .child(
            h_flex()
                .gap_2()
                .items_center()
                .justify_between()
                .w_full()
                .child(
                    div()
                        .text_xs()
                        .font_semibold()
                        .text_color(theme.foreground)
                        .child(format!("Versions ({})", choices.len())),
                )
                .child(
                    Switch::new("cargo-compatible-only")
                        .checked(panel.compatible_only)
                        .label("Compatible only")
                        .with_size(Size::Small)
                        .tooltip("Hide versions that need a newer Rust than the one installed")
                        .on_click({
                            let view = view.clone();
                            move |checked, _, cx| {
                                let checked = *checked;
                                view.update(cx, |panel, cx| {
                                    panel.compatible_only = checked;
                                    cx.notify();
                                });
                            }
                        }),
                ),
        );
    if !changeable {
        column = column.child(
            div()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(match (declared, row.target.is_some()) {
                    (Some(Declaration::Inherited), _) => {
                        "The version is set in [workspace.dependencies] in the root Cargo.toml, \
                         for every crate that inherits it. No Cargo command edits that table, \
                         so it can't be changed from here."
                    }
                    (Some(Declaration::Literal), _) => {
                        "This dependency is declared for one target only. Change its version in \
                         Cargo.toml by hand."
                    }
                    (None, _) => {
                        "How this crate declares the dependency could not be read, so its \
                         version can't be changed from here."
                    }
                }),
        );
    }

    let busy = panel.is_busy();
    let shown = choices.len().min(LISTED_VERSIONS);
    for (ix, choice) in choices.iter().take(LISTED_VERSIONS).enumerate() {
        let version = choice.version.to_string();
        let locked = row.locked_version.as_deref() == Some(version.as_str());
        let mut line = h_flex().gap_2().items_center().w_full().child(
            div()
                .text_xs()
                .font_family("Cascadia Mono")
                .text_color(theme.foreground)
                .child(version.clone()),
        );
        if let Some(tag) = rust_tag(&choice.rust) {
            line = line.child(tag);
        }
        line = line.child(div().flex_1());
        if locked {
            line = line.child(Tag::secondary().xsmall().child("locked"));
        } else if changeable {
            line = line.child(
                Button::new(("cargo-use-version", ix))
                    .label("Use")
                    .with_size(Size::Small)
                    .disabled(busy)
                    .tooltip("cargo add: rewrites the requirement in this crate's Cargo.toml")
                    .on_click({
                        let view = view.clone();
                        let package = row.name.clone();
                        move |_, window, cx| {
                            view.update(cx, |panel, cx| {
                                panel.change_version(&package, &version, window, cx);
                            });
                        }
                    }),
            );
        }
        column = column.child(line);
    }
    if choices.len() > shown {
        column = column.child(
            div()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(format!("{} older versions not shown.", choices.len() - shown)),
        );
    }
    column
}
