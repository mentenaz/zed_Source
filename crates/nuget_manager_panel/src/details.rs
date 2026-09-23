//! The right-hand details pane shown while a package is selected: a header,
//! then the details content or the markdown-rendered README.

use dotnet_backend::{InstalledPackage, NugetPackageDetails};
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

use crate::NuGetManagerPanel;

pub(super) fn details_pane(
    panel: &NuGetManagerPanel,
    view: &Entity<NuGetManagerPanel>,
    cx: &mut App,
) -> impl IntoElement {
    let theme = cx.theme();
    let details = panel.details.clone();
    let details_loading = panel.details_loading;
    let details_error = panel.details_error.clone();
    let show_readme = panel.show_readme;
    let installed = panel.installed.clone();
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
    details: &NugetPackageDetails,
    view: &Entity<NuGetManagerPanel>,
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
                        .child(format!("{} — README", details.id)),
                ),
        )
        .child(markdown(readme).flex_1().scrollable(true).selectable(true))
}

/// The static details body — a snapshot of the fetched registration catalog
/// rendered as rows, plus the version list with install/update/installed
/// actions, the latest-version vulnerabilities, and the dependency groups.
fn details_content(
    details: &NugetPackageDetails,
    installed: &[InstalledPackage],
    view: &Entity<NuGetManagerPanel>,
    cx: &App,
) -> impl IntoElement {
    let theme = cx.theme();
    let installed_any = installed
        .iter()
        .any(|p| p.id.eq_ignore_ascii_case(&details.id));

    // Registration catalogs are capped by the UI (not the backend) — most
    // packages never get within an order of magnitude of this.
    let versions: Vec<&String> = details.versions.iter().take(20).collect();

    let mut col = v_flex().gap_1p5().w_full().p_4();
    col = col
        .child(info_row(
            theme,
            "Description",
            details.description.clone().unwrap_or_else(|| "—".into()),
        ))
        .child(info_row(theme, "Version", details.version.clone()))
        .when(!details.authors.is_empty(), |col| {
            col.child(info_row(theme, "Authors", details.authors.clone()))
        })
        .when(details.project_url.is_some(), |col| {
            col.child(info_row(
                theme,
                "Project URL",
                details.project_url.clone().unwrap_or_default(),
            ))
        })
        .when(details.license_url.is_some(), |col| {
            col.child(info_row(
                theme,
                "License",
                details.license_url.clone().unwrap_or_default(),
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

    if !details.vulnerabilities.is_empty() {
        col = col.child(div().h_1()).child(
            div()
                .text_xs()
                .font_semibold()
                .text_color(theme.foreground)
                .child(format!(
                    "Vulnerabilities ({})",
                    details.vulnerabilities.len()
                )),
        );
        for vuln in &details.vulnerabilities {
            let severity_color = match vuln.severity.as_str() {
                "critical" | "high" => theme.danger,
                "moderate" => theme.warning,
                _ => theme.muted_foreground,
            };
            col = col.child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .w_full()
                    .child(
                        div()
                            .px_1p5()
                            .py_0p5()
                            .rounded_md()
                            .bg(severity_color.opacity(0.2))
                            .text_color(severity_color)
                            .text_xs()
                            .child(vuln.severity.clone()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(vuln.advisory_url.clone()),
                    ),
            );
        }
    }

    col = col.child(div().h_1()).child(
        div()
            .text_xs()
            .font_semibold()
            .text_color(theme.foreground)
            .child(format!("Versions ({})", versions.len())),
    );

    for version in versions {
        let id = details.id.clone();
        let version = version.clone();
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
                .child(version_action(
                    id,
                    version,
                    installed_any,
                    installed,
                    theme,
                    view,
                )),
        );
    }

    if !details.dependencies.is_empty() {
        col = col.child(div().h_1()).child(
            div()
                .text_xs()
                .font_semibold()
                .text_color(theme.foreground)
                .child("Dependencies"),
        );
        for group in &details.dependencies {
            col = col.child(
                div()
                    .text_xs()
                    .font_semibold()
                    .text_color(theme.muted_foreground)
                    .child(group.target_framework.clone()),
            );
            for dep in &group.dependencies {
                col = col.child(
                    div()
                        .text_xs()
                        .text_color(theme.foreground)
                        .child(format!("{}  {}", dep.id, dep.range)),
                );
            }
        }
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

/// Per-version action: a muted "Installed" chip when that exact version is a
/// reference, an "Update" button when some other version is, else "Install".
fn version_action(
    name: String,
    version: String,
    installed_any: bool,
    installed: &[InstalledPackage],
    theme: &gpui_component::Theme,
    view: &Entity<NuGetManagerPanel>,
) -> impl IntoElement {
    let exact = installed
        .iter()
        .any(|p| p.id.eq_ignore_ascii_case(&name) && p.version == version);
    if exact {
        div()
            .px_1p5()
            .py_0p5()
            .rounded_md()
            .bg(theme.muted_foreground.opacity(0.15))
            .text_color(theme.muted_foreground)
            .text_xs()
            .child("Installed")
            .into_any_element()
    } else if installed_any {
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
                        panel.install_pkg(&name, &version, window, cx);
                    });
                }
            })
            .into_any_element()
    }
}
