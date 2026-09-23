//! The right-hand details pane shown while a package is selected: a header,
//! then the details content or the markdown-rendered README.

use gpui::{
    AnyElement, App, Entity, IntoElement, ParentElement as _, Styled as _, div,
    prelude::FluentBuilder as _,
};
use gpui_component::{
    ActiveTheme as _, IconName, Sizable as _, Size, StyledExt as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    spinner::Spinner,
    text::markdown,
    v_flex,
};
use npm_backend::{
    NpmInstalledPkg, NpmPackageDetails, NpmVersionEntry, peer_conflicts_with_installed,
};

use crate::{NpmManagerPanel, NpmProject};

pub(super) fn details_pane(
    panel: &NpmManagerPanel,
    project: &NpmProject,
    view: &Entity<NpmManagerPanel>,
    cx: &mut App,
) -> impl IntoElement {
    let theme = cx.theme();
    let details = panel.details.clone();
    let details_loading = panel.details_loading;
    let details_error = panel.details_error.clone();
    let show_readme = panel.show_readme;
    let installed = project.installed.clone();
    let selected = panel.selected.clone();

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
                .child(selected.clone().unwrap_or_else(|| "Package details".into())),
        )
        .child(
            Button::new("close-details")
                .icon(IconName::Close)
                .ghost()
                .xsmall()
                .on_click({
                    let view = view.clone();
                    move |_, _, cx| {
                        view.update(cx, |panel, cx| {
                            panel.selected = None;
                            panel.details = None;
                            panel.details_loading = false;
                            panel.details_error = None;
                            panel.show_readme = false;
                            cx.notify();
                        });
                    }
                }),
        );

    let body: AnyElement = if details_loading {
        h_flex()
            .gap_2()
            .items_center()
            .p_4()
            .child(Spinner::new().with_size(Size::XSmall))
            .child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child("Loading package details…"),
            )
            .into_any_element()
    } else if let Some(err) = &details_error {
        div()
            .p_4()
            .text_xs()
            .text_color(theme.danger)
            .child(err.clone())
            .into_any_element()
    } else if show_readme {
        match &details {
            Some(details) => readme_view(details, view, cx).into_any_element(),
            None => empty_pane("No README available.", cx),
        }
    } else {
        match &details {
            Some(details) => details_content(details, &installed, view, cx).into_any_element(),
            None => empty_pane("Select a package to see its details here.", cx),
        }
    };

    v_flex()
        .size_full()
        .bg(theme.background)
        .child(header)
        .child(body)
}

fn empty_pane(message: &str, cx: &App) -> AnyElement {
    div()
        .p_4()
        .text_xs()
        .text_color(cx.theme().muted_foreground)
        .child(message.to_string())
        .into_any_element()
}

fn readme_view(
    details: &NpmPackageDetails,
    view: &Entity<NpmManagerPanel>,
    cx: &App,
) -> impl IntoElement {
    let theme = cx.theme();
    let readme = details.readme.clone().unwrap_or_default();
    v_flex()
        .flex_1()
        .w_full()
        .gap_3()
        .p_4()
        .child(
            h_flex()
                .w_full()
                .items_center()
                .justify_between()
                .child(
                    Button::new("back-to-details")
                        .ghost()
                        .xsmall()
                        .label("← Details")
                        .on_click({
                            let view = view.clone();
                            move |_, _, cx| {
                                view.update(cx, |panel, cx| {
                                    panel.show_readme = false;
                                    cx.notify();
                                });
                            }
                        }),
                )
                .child(
                    div()
                        .text_xs()
                        .font_semibold()
                        .text_color(theme.foreground)
                        .child(format!("{} — README", details.name)),
                ),
        )
        .child(markdown(readme).flex_1().scrollable(true).selectable(true))
}

/// The static details body — a snapshot of the fetched packument rendered as
/// rows, plus the peer-filtered version list with install/update buttons.
fn details_content(
    details: &NpmPackageDetails,
    installed: &[NpmInstalledPkg],
    view: &Entity<NpmManagerPanel>,
    cx: &App,
) -> impl IntoElement {
    let theme = cx.theme();
    let installed_any = installed.iter().any(|p| p.name == details.name);

    let compatible: Vec<NpmVersionEntry> = details
        .versions
        .iter()
        .filter(|entry| peer_conflicts_with_installed(&entry.peer_deps, installed).is_empty())
        .cloned()
        .take(12)
        .collect();

    let downloads = details
        .weekly_downloads
        .map(thousands)
        .unwrap_or_else(|| "—".to_string());
    let license = details.license.clone().unwrap_or_else(|| "—".to_string());

    let mut col = v_flex().gap_1p5().w_full().p_4();
    col = col
        .child(info_row(
            theme,
            "Description",
            details.description.clone().unwrap_or_else(|| "—".into()),
        ))
        .child(info_row(theme, "Version", details.version.clone()))
        .child(info_row(theme, "License", license))
        .child(info_row(theme, "Weekly downloads", downloads))
        .when(details.homepage.is_some(), |col| {
            col.child(info_row(
                theme,
                "Homepage",
                details.homepage.clone().unwrap_or_default(),
            ))
        });

    if details.readme.is_some() {
        col = col.child(
            Button::new("open-readme")
                .ghost()
                .xsmall()
                .label("README")
                .on_click({
                    let view = view.clone();
                    move |_, _, cx| {
                        view.update(cx, |panel, cx| {
                            panel.show_readme = true;
                            cx.notify();
                        });
                    }
                }),
        );
    }

    col = col.child(div().h_1()).child(
        div()
            .text_xs()
            .font_semibold()
            .text_color(theme.foreground)
            .child(format!("Compatible versions ({})", compatible.len())),
    );

    for entry in &compatible {
        let name = details.name.clone();
        let version = entry.version.clone();
        col = col.child(
            h_flex()
                .gap_2()
                .items_center()
                .w_full()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_xs()
                        .text_color(theme.foreground)
                        .child(version.clone()),
                )
                .child(version_action(name, version, installed_any, view)),
        );
    }

    col
}

fn info_row(theme: &gpui_component::Theme, label: &str, value: String) -> impl IntoElement {
    h_flex()
        .items_start()
        .gap_2()
        .w_full()
        .child(
            div()
                .w_32()
                .flex_none()
                .text_xs()
                .text_color(theme.muted_foreground)
                .child(label.to_string()),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_xs()
                .text_color(theme.foreground)
                .child(value),
        )
}

fn version_action(
    name: String,
    version: String,
    installed_any: bool,
    view: &Entity<NpmManagerPanel>,
) -> impl IntoElement {
    if installed_any {
        Button::new(format!("details-update-{name}-{version}"))
            .label("Update")
            .with_size(Size::Small)
            .on_click({
                let view = view.clone();
                move |_, window, cx| {
                    view.update(cx, |panel, cx| {
                        panel.update_pkg(&name, &version, window, cx);
                    });
                }
            })
            .into_any_element()
    } else {
        Button::new(format!("details-install-{name}-{version}"))
            .label("Install")
            .with_size(Size::Small)
            .on_click({
                let view = view.clone();
                move |_, window, cx| {
                    view.update(cx, |panel, cx| {
                        panel.install_pkg(&name, &version, panel.install_as_dev, window, cx);
                    });
                }
            })
            .into_any_element()
    }
}

/// Thousands grouping, e.g. `12,345,678`.
fn thousands(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::with_capacity(s.len() + s.len() / 3);
    for (ix, ch) in s.chars().enumerate() {
        out.push(ch);
        let remaining = s.len() - ix - 1;
        if remaining > 0 && remaining % 3 == 0 {
            out.push(',');
        }
    }
    out
}
