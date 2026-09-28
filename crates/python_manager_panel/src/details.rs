//! The right-hand details pane shown while a package is selected: a header,
//! then the PyPI metadata content or the markdown-rendered README. Mirrors
//! the Forge original's `details_panel_content_static` and
//! `npm_manager_panel::details`'s pane shell.

use gpui::{AnyElement, Context, Entity, IntoElement, ParentElement as _, Styled as _, div, prelude::FluentBuilder as _};
use gpui_component::{
    ActiveTheme as _, IconName, Sizable as _, Size, StyledExt as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    scroll::ScrollableElement as _,
    spinner::Spinner,
    text::markdown,
    v_flex,
};
use python_backend::PyPiPackageInfo;

use crate::PythonManagerPanel;

pub(super) fn details_pane(
    panel: &PythonManagerPanel,
    view: &Entity<PythonManagerPanel>,
    cx: &mut Context<PythonManagerPanel>,
) -> impl IntoElement {
    let theme = cx.theme().clone();
    let details = panel.details.clone();
    let details_loading = panel.details_loading;
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
            .child(div().text_xs().text_color(theme.muted_foreground).child("Loading details\u{2026}"))
            .into_any_element()
    } else if show_readme {
        match &details {
            Some(details) => readme_view(details, view, &theme).into_any_element(),
            None => empty_pane("No README available.", &theme),
        }
    } else {
        match &details {
            Some(details) => details_content(details, &installed, view, &theme).into_any_element(),
            None => empty_pane("Select a package to see its details here.", &theme),
        }
    };

    v_flex().size_full().bg(theme.background).child(header).child(body)
}

fn empty_pane(message: &str, theme: &gpui_component::Theme) -> AnyElement {
    div()
        .p_4()
        .text_xs()
        .text_color(theme.muted_foreground)
        .child(message.to_string())
        .into_any_element()
}

fn readme_view(
    details: &PyPiPackageInfo,
    view: &Entity<PythonManagerPanel>,
    theme: &gpui_component::Theme,
) -> impl IntoElement {
    let is_markdown = details
        .readme_content_type
        .as_deref()
        .map(|t| t.contains("markdown"))
        .unwrap_or(true);
    let readme = details.readme.clone().unwrap_or_default();
    let pkg_name = details.name.clone();

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
                    Button::new("back-to-details").ghost().xsmall().label("\u{2190} Details").on_click({
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
                        .child(format!("{pkg_name} \u{2014} README")),
                ),
        )
        .child(if is_markdown {
            markdown(readme).flex_1().scrollable(true).selectable(true).into_any_element()
        } else {
            div()
                .flex_1()
                .w_full()
                .overflow_y_scrollbar()
                .font_family("Cascadia Mono")
                .text_xs()
                .text_color(theme.foreground)
                .child(readme)
                .into_any_element()
        })
}

/// The static details body — a snapshot of the fetched PyPI metadata
/// rendered as rows, plus dependencies and project links. Mirrors the Forge
/// original's `details_panel_content_static`.
fn details_content(
    d: &PyPiPackageInfo,
    installed: &[python_backend::PythonPackage],
    view: &Entity<PythonManagerPanel>,
    theme: &gpui_component::Theme,
) -> AnyElement {
    let is_installed = installed.iter().any(|p| p.name == d.name);
    let installed_version = installed.iter().find(|p| p.name == d.name).map(|p| p.version.clone());

    let mut col = v_flex().gap_3().p_4();

    col = col.child(
        v_flex()
            .gap_1()
            .child(div().text_xs().text_color(theme.muted_foreground).child("Summary"))
            .child(
                div()
                    .text_xs()
                    .text_color(theme.foreground)
                    .child(d.summary.clone().unwrap_or_else(|| "(no summary)".into())),
            ),
    );

    col = col.child(
        h_flex()
            .gap_3()
            .child(
                v_flex()
                    .gap_1()
                    .child(div().text_xs().text_color(theme.muted_foreground).child("Version"))
                    .child(
                        div()
                            .font_family("Cascadia Mono")
                            .text_xs()
                            .text_color(theme.foreground)
                            .child(d.version.clone()),
                    ),
            )
            .when_some(d.author.clone(), |row, author| {
                row.child(
                    v_flex()
                        .gap_1()
                        .child(div().text_xs().text_color(theme.muted_foreground).child("Author"))
                        .child(div().text_xs().text_color(theme.foreground).child(author)),
                )
            }),
    );

    col = col.when_some(d.home_page.clone(), |col, url| {
        col.child(div().text_xs().text_color(theme.primary).child(url))
    });
    col = col.when_some(d.license.clone(), |col, lic| {
        col.child(
            h_flex()
                .gap_1()
                .child(div().text_xs().text_color(theme.muted_foreground).child("License:"))
                .child(div().text_xs().text_color(theme.foreground).child(lic)),
        )
    });
    col = col.when_some(d.requires_python.clone(), |col, req| {
        col.child(
            h_flex()
                .gap_1()
                .child(div().text_xs().text_color(theme.muted_foreground).child("Requires Python:"))
                .child(
                    div()
                        .font_family("Cascadia Mono")
                        .text_xs()
                        .text_color(theme.foreground)
                        .child(req),
                ),
        )
    });

    if !d.python_versions.is_empty() {
        col = col.child(
            h_flex()
                .gap_1()
                .flex_wrap()
                .child(div().text_xs().text_color(theme.muted_foreground).child("Supports:"))
                .children(d.python_versions.iter().map(|v| {
                    div()
                        .px_1()
                        .rounded_sm()
                        .bg(theme.secondary)
                        .font_family("Cascadia Mono")
                        .text_xs()
                        .text_color(theme.foreground)
                        .child(v.clone())
                })),
        );
    }

    col = col.when(d.readme.is_some(), |col| {
        let view = view.clone();
        col.child(Button::new("open-readme").ghost().xsmall().label("README").on_click(move |_, _, cx| {
            view.update(cx, |p, cx| {
                p.show_readme = true;
                cx.notify();
            });
        }))
    });

    // ── Install / installed status ──────────────────────────────────
    col = col.child(
        div()
            .pt_2()
            .border_t_1()
            .border_color(theme.border)
            .child(match (is_installed, &installed_version) {
                (true, Some(v)) if v == &d.version => div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(format!("Installed (v{v})"))
                    .into_any_element(),
                (true, Some(v)) => {
                    let name = d.name.clone();
                    let version = d.version.clone();
                    let view = view.clone();
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(div().text_xs().text_color(theme.muted_foreground).child(format!("Installed: v{v}")))
                        .child(
                            Button::new("update-installed")
                                .ghost()
                                .xsmall()
                                .label(format!("Update \u{2192} {version}"))
                                .on_click(move |_, window, cx| {
                                    view.update(cx, |p, cx| {
                                        p.install_pkg(&name, Some(&version), window, cx);
                                    });
                                }),
                        )
                        .into_any_element()
                }
                _ => {
                    let name = d.name.clone();
                    let version = d.version.clone();
                    let view = view.clone();
                    Button::new("install-pkg")
                        .ghost()
                        .xsmall()
                        .label(format!("Install v{version}"))
                        .on_click(move |_, window, cx| {
                            view.update(cx, |p, cx| {
                                p.install_pkg(&name, Some(&version), window, cx);
                            });
                        })
                        .into_any_element()
                }
            }),
    );

    if !d.requires_dist.is_empty() {
        col = col.child(
            div()
                .pt_2()
                .border_t_1()
                .border_color(theme.border)
                .text_xs()
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(theme.foreground)
                .child("Dependencies"),
        );
        for dep in &d.requires_dist {
            col = col.child(
                div()
                    .font_family("Cascadia Mono")
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child(dep.clone()),
            );
        }
    }

    if !d.project_urls.is_empty() {
        col = col.child(
            div()
                .pt_2()
                .border_t_1()
                .border_color(theme.border)
                .text_xs()
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(theme.foreground)
                .child("Links"),
        );
        for (label, url) in &d.project_urls {
            col = col.child(
                h_flex()
                    .gap_1()
                    .child(div().text_xs().text_color(theme.muted_foreground).child(format!("{label}:")))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_xs()
                            .text_color(theme.primary)
                            .child(url.clone()),
                    ),
            );
        }
    }

    col.into_any_element()
}
