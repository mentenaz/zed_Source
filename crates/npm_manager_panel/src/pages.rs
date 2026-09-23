//! The five `SettingPage`s of the npm manager tab.
//!
//! Each page is a *view shell*: a cheap `SettingPage` whose body is a single
//! `SettingItem::render` that embeds a dedicated child `Entity<...>` view. The
//! heavy rows live in those child views and are built — by reading the panel
//! live through a `WeakEntity<NpmManagerPanel>` in each view's `Render::render`
//! — only for the page currently open in the `Settings` widget, which renders
//! just the active page's items.
//!
//! Building the pages therefore no longer clones the package lists up front,
//! so a re-render of the panel (e.g. the 60s auto-refresh) never traces through
//! every installed / outdated / audit / search row the way the old
//! `build_*_page` functions did.

use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, StatefulInteractiveElement as _, Styled as _, WeakEntity, Window,
    div, prelude::FluentBuilder as _,
};
use gpui_component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, Size, StyledExt as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::Input,
    setting::{SettingGroup, SettingItem, SettingPage},
    spinner::Spinner,
    v_flex, white,
};
use npm_backend::{PackageManager, UpdateKind, classify_update};

use crate::NpmManagerPanel;

/// The five page views created alongside the panel. `PageViews::new` builds
/// them from a `WeakEntity` of the panel so they can read its state live in
/// their own `Render::render` without owning any of the data.
pub(super) struct PageViews {
    pub general: Entity<GeneralPage>,
    pub search: Entity<SearchPage>,
    pub installed: Entity<InstalledPage>,
    pub updates: Entity<UpdatesPage>,
    pub vuln: Entity<VulnPage>,
}

impl PageViews {
    pub(super) fn new(panel: WeakEntity<NpmManagerPanel>, cx: &mut App) -> Self {
        PageViews {
            general: cx.new(|_| GeneralPage::new(panel.clone())),
            search: cx.new(|_| SearchPage::new(panel.clone())),
            installed: cx.new(|_| InstalledPage::new(panel.clone())),
            updates: cx.new(|_| UpdatesPage::new(panel.clone())),
            vuln: cx.new(|_| VulnPage::new(panel.clone())),
        }
    }
}

pub(super) fn build_all(project: &crate::NpmProject, pages: &PageViews) -> Vec<SettingPage> {
    vec![
        build_general_page(pages),
        build_search_page(pages),
        build_installed_page(project, pages),
        build_updates_page(project, pages),
        build_vuln_page(project, pages),
    ]
}

/// A `SettingItem` that simply embeds a page view; the shell adds nothing else
/// per frame, so re-renders stay O(1) regardless of the lists' length.
fn embed_view<T>(view: Entity<T>) -> SettingItem
where
    T: Render + 'static,
{
    SettingItem::render(move |_options, _window, _cx| view.clone().into_any_element())
}

// ── General ────────────────────────────────────────────────────────────────

pub(super) struct GeneralPage {
    panel: WeakEntity<NpmManagerPanel>,
}

impl GeneralPage {
    fn new(panel: WeakEntity<NpmManagerPanel>) -> Self {
        Self { panel }
    }
}

impl Render for GeneralPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(view) = self.panel.upgrade() else {
            return div().into_any_element();
        };
        let panel = view.read(cx);
        let project = panel.active();
        let theme = cx.theme();

        let manager = project.package_manager.cli_name().to_string();
        let dev = panel.install_as_dev;
        let busy = panel.running_action.is_some()
            || project.installed_loading
            || project.outdated_loading
            || project.audit_loading;
        let running_action = panel.running_action.clone();
        let error = panel.error.clone();
        let engine = project.engine.clone();

        v_flex()
            .gap_2()
            .child(
                h_flex()
                    .gap_1()
                    .children(["npm", "yarn", "pnpm", "bun"].map(|name| {
                        let view = view.clone();
                        Button::new(format!("npm-pm-{name}"))
                            .label(name)
                            .with_size(Size::Small)
                            .map(|button| {
                                if manager == name {
                                    button.primary()
                                } else {
                                    button.outline()
                                }
                            })
                            .on_click(move |_, _window, cx| {
                                view.update(cx, |panel, cx| {
                                    panel.active_mut().package_manager = match name {
                                        "yarn" => PackageManager::Yarn,
                                        "pnpm" => PackageManager::Pnpm,
                                        "bun" => PackageManager::Bun,
                                        _ => PackageManager::Npm,
                                    };
                                    cx.notify();
                                });
                            })
                    })),
            )
            .child(
                Button::new("npm-toggle-dev")
                    .label(if dev {
                        "Save new packages as devDependencies"
                    } else {
                        "Save new packages as dependencies"
                    })
                    .with_size(Size::Small)
                    .map(|button| {
                        if dev {
                            button.primary()
                        } else {
                            button.outline()
                        }
                    })
                    .on_click({
                        let view = view.clone();
                        move |_, _window, cx| {
                            view.update(cx, |panel, cx| {
                                panel.install_as_dev = !panel.install_as_dev;
                                cx.notify();
                            });
                        }
                    }),
            )
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(
                        Button::new("npm-reload")
                            .icon(IconName::Redo2)
                            .label("Refresh")
                            .with_size(Size::Small)
                            .on_click({
                                let view = view.clone();
                                move |_, _window, cx| {
                                    view.update(cx, |panel, cx| panel.reload_visible(cx));
                                }
                            }),
                    )
                    .when(busy, |row| {
                        row.child(Spinner::new().with_size(Size::XSmall))
                    }),
            )
            .child({
                let mut status = v_flex().gap_1().text_xs();
                if let Some(action) = &running_action {
                    status = status.child(
                        div()
                            .text_color(theme.foreground)
                            .child(format!("{action}…")),
                    );
                }
                if let Some(err) = &error {
                    status =
                        status.child(div().text_color(theme.danger).truncate().child(err.clone()));
                }
                if let Some(engine) = &engine {
                    let (color, text) = if engine.supported {
                        (
                            theme.success,
                            format!(
                                "node {} / {} — engines OK",
                                engine.node_version, engine.npm_version
                            ),
                        )
                    } else {
                        (
                            theme.danger,
                            format!(
                                "node {} / {} — engines {} require {}",
                                engine.node_version,
                                engine.npm_version,
                                engine.node_range,
                                engine.node_version
                            ),
                        )
                    };
                    status = status.child(div().text_color(color).truncate().child(text));
                }
                status
            })
            .into_any_element()
    }
}

fn build_general_page(pages: &PageViews) -> SettingPage {
    SettingPage::new("General")
        .default_open(true)
        .resettable(false)
        .icon(Icon::new(IconName::Settings))
        .description("The package manager and install behavior npm commands use.")
        .group(
            SettingGroup::new()
                .title("Package manager")
                .description("Which CLI the install / remove / update actions run.")
                .item(embed_view(pages.general.clone())),
        )
}

// ── Search ─────────────────────────────────────────────────────────────────

pub(super) struct SearchPage {
    panel: WeakEntity<NpmManagerPanel>,
}

impl SearchPage {
    fn new(panel: WeakEntity<NpmManagerPanel>) -> Self {
        Self { panel }
    }
}

impl Render for SearchPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(view) = self.panel.upgrade() else {
            return div().into_any_element();
        };
        let panel = view.read(cx);
        let theme = cx.theme();

        let search_input = panel.search_input.clone();
        let search_total = panel.search_total;
        let search_loading = panel.search_loading;
        let search_error = panel.search_error.clone();
        let results = &panel.search_results;

        v_flex()
            .gap_2()
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(Input::new(&search_input).xsmall().w_full()),
                    )
                    .child(
                        Button::new("npm-search-go")
                            .icon(IconName::Search)
                            .label("Search")
                            .with_size(Size::Small)
                            .on_click({
                                let view = view.clone();
                                move |_, _window, cx| {
                                    view.update(cx, |panel, cx| {
                                        panel.search_page = 0;
                                        panel.search_npm(cx);
                                    });
                                }
                            }),
                    ),
            )
            .child({
                let content: AnyElement = if search_loading {
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(Spinner::new().with_size(Size::XSmall))
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child("Searching npmjs.com…"),
                        )
                        .into_any_element()
                } else if let Some(err) = &search_error {
                    div()
                        .text_xs()
                        .text_color(theme.danger)
                        .child(err.clone())
                        .into_any_element()
                } else if results.is_empty() {
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child("Search the npm registry for a package to install.")
                        .into_any_element()
                } else {
                    let mut list = v_flex().gap_2().w_full();
                    list = list.child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(format!("{search_total} packages found")),
                    );
                    for result in results {
                        let name = result.name.clone();
                        let version = result.version.clone();
                        let description = result.description.clone().unwrap_or_default();
                        let compat = result.compat;

                        // Card header: the package name, then the compat
                        // badge when the engines check produced an answer.
                        let mut header = h_flex().gap_2().items_center().w_full();
                        header = header.child(
                            div()
                                .id(format!("npm-result-label-{name}"))
                                .text_sm()
                                .font_semibold()
                                .text_color(theme.foreground)
                                .child(name.clone()),
                        );
                        match compat {
                            Some(true) => {
                                header = header.child(
                                    div()
                                        .px_1p5()
                                        .py_0p5()
                                        .rounded_md()
                                        .bg(theme.success.opacity(0.2))
                                        .text_color(theme.success)
                                        .text_xs()
                                        .child("node OK"),
                                );
                            }
                            Some(false) => {
                                header = header.child(
                                    div()
                                        .px_1p5()
                                        .py_0p5()
                                        .rounded_md()
                                        .bg(theme.danger.opacity(0.2))
                                        .text_color(theme.danger)
                                        .text_xs()
                                        .child("node mismatch"),
                                );
                            }
                            None => {}
                        }

                        // "latest" line: `[name@version]`, per the card spec.
                        let latest = h_flex()
                            .gap_1()
                            .items_center()
                            .w_full()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(theme.muted_foreground)
                                    .child("latest"),
                            )
                            .child(
                                div()
                                    .px_1p5()
                                    .py_0p5()
                                    .rounded_md()
                                    .bg(theme.border.opacity(0.5))
                                    .text_color(theme.foreground)
                                    .text_xs()
                                    .child(format!("{name}@{version}")),
                            );

                        // Description row with the install action on the
                        // right edge.
                        let install_button = Button::new(format!("npm-install-{name}"))
                            .label("Install")
                            .with_size(Size::Small)
                            .on_click({
                                let view = view.clone();
                                let name = name.clone();
                                let version = version.clone();
                                move |_, window, cx| {
                                    view.update(cx, |panel, cx| {
                                        panel.install_pkg(
                                            &name,
                                            &version,
                                            panel.install_as_dev,
                                            window,
                                            cx,
                                        );
                                    });
                                }
                            });

                        let card = v_flex()
                            .gap_1()
                            .w_full()
                            .p_2()
                            .rounded_md()
                            .border_1()
                            .border_color(theme.border.opacity(0.6))
                            .child(header)
                            .child(latest)
                            .child(
                                h_flex()
                                    .gap_2()
                                    .items_center()
                                    .w_full()
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .text_xs()
                                            .text_color(theme.muted_foreground)
                                            .child(description),
                                    )
                                    .child(install_button),
                            )
                            .child(
                                h_flex().w_full().child(
                                    Button::new(format!("npm-more-info-{name}"))
                                        .ghost()
                                        .xsmall()
                                        .icon(IconName::Info)
                                        .label("More info")
                                        .on_click({
                                            let view = view.clone();
                                            let name = name.clone();
                                            move |_, _window, cx| {
                                                view.update(cx, |panel, cx| {
                                                    panel
                                                        .fetch_details_and_readme(name.clone(), cx);
                                                });
                                            }
                                        }),
                                ),
                            );
                        list = list.child(card);
                    }
                    list = list.when(results.len() < search_total, |list| {
                        list.child(
                            div().w_full().child(
                                Button::new("npm-search-more")
                                    .ghost()
                                    .label("Load more")
                                    .with_size(Size::Small)
                                    .on_click({
                                        let view = view.clone();
                                        move |_, _window, cx| {
                                            view.update(cx, |panel, cx| {
                                                panel.search_page += 1;
                                                panel.search_npm(cx);
                                            });
                                        }
                                    }),
                            ),
                        )
                    });
                    list.into_any_element()
                };
                content
            })
            .into_any_element()
    }
}

fn build_search_page(pages: &PageViews) -> SettingPage {
    SettingPage::new("Search")
        .resettable(false)
        .icon(Icon::new(IconName::Search))
        .description("Find packages on the npm registry and install them.")
        .group(
            SettingGroup::new()
                .title("npmjs.com")
                .item(embed_view(pages.search.clone())),
        )
}

// ── Installed ──────────────────────────────────────────────────────────────

pub(super) struct InstalledPage {
    panel: WeakEntity<NpmManagerPanel>,
}

impl InstalledPage {
    fn new(panel: WeakEntity<NpmManagerPanel>) -> Self {
        Self { panel }
    }
}

impl Render for InstalledPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(view) = self.panel.upgrade() else {
            return div().into_any_element();
        };
        let panel = view.read(cx);
        let theme = cx.theme();
        let project = panel.active();
        let installed = &project.installed;
        let installed_loading = project.installed_loading;

        if installed_loading {
            // Rows appear once `npm ls` returns; the sidebar badge spins meanwhile.
            return div().into_any_element();
        }

        let mut list = v_flex().gap_1p5();
        if installed.is_empty() {
            list = list.child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child("No packages installed — run `npm install` first."),
            );
        } else {
            for pkg in installed {
                let name = pkg.name.clone();
                let version = pkg.version.clone();
                let is_dev = pkg.is_dev;

                // Card header: the package name (clickable → details), then
                // the dev badge in the same slot as the search cards' compat
                // badge.
                let mut header = h_flex().gap_2().items_center().w_full();
                header = header.child(
                    div()
                        .id(format!("npm-installed-{name}"))
                        .text_sm()
                        .font_semibold()
                        .text_color(theme.foreground)
                        .child(name.clone())
                        .cursor_pointer()
                        .on_click({
                            let view = view.clone();
                            let name = name.clone();
                            move |_, _window, cx| {
                                view.update(cx, |panel, cx| {
                                    panel.fetch_details(name.clone(), cx);
                                });
                            }
                        }),
                );
                header = header.when(is_dev, |header| {
                    header.child(
                        div()
                            .px_1p5()
                            .py_0p5()
                            .rounded_md()
                            .bg(theme.warning.opacity(0.2))
                            .text_color(theme.warning)
                            .text_xs()
                            .child("dev"),
                    )
                });

                // "installed" line: `[name@version]`, per the card spec.
                let installed_line = h_flex()
                    .gap_1()
                    .items_center()
                    .w_full()
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child("installed"),
                    )
                    .child(
                        div()
                            .px_1p5()
                            .py_0p5()
                            .rounded_md()
                            .bg(theme.border.opacity(0.5))
                            .text_color(theme.foreground)
                            .text_xs()
                            .child(format!("{name}@{version}")),
                    );

                let card = v_flex()
                    .gap_1()
                    .w_full()
                    .p_2()
                    .rounded_md()
                    .border_1()
                    .border_color(theme.border.opacity(0.6))
                    .child(header)
                    .child(installed_line)
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .w_full()
                            .child(div().flex_1().min_w_0())
                            .child(
                                Button::new(format!("npm-remove-{name}"))
                                    .danger()
                                    .label("Remove")
                                    .with_size(Size::Small)
                                    .on_click({
                                        let view = view.clone();
                                        let name = name.clone();
                                        move |_, window, cx| {
                                            view.update(cx, |panel, cx| {
                                                panel.remove_package(&name, window, cx);
                                            });
                                        }
                                    }),
                            ),
                    )
                    .child(
                        h_flex().w_full().child(
                            Button::new(format!("npm-more-info-{name}"))
                                .ghost()
                                .xsmall()
                                .icon(IconName::Info)
                                .label("More info")
                                .on_click({
                                    let view = view.clone();
                                    let name = name.clone();
                                    move |_, _window, cx| {
                                        view.update(cx, |panel, cx| {
                                            panel.fetch_details(name.clone(), cx);
                                        });
                                    }
                                }),
                        ),
                    );
                list = list.child(card);
            }
        }
        list.into_any_element()
    }
}

fn build_installed_page(project: &crate::NpmProject, pages: &PageViews) -> SettingPage {
    let count = project.installed.len();
    let loading = project.installed_loading;
    SettingPage::new("Installed")
        .resettable(false)
        .icon(Icon::new(IconName::Inbox))
        .description("Packages installed in this workspace.")
        .group(
            SettingGroup::new()
                .title("Installed packages")
                .item(embed_view(pages.installed.clone())),
        )
        .sidebar_badge(move |_, cx| count_badge(count, loading, cx))
}

// ── Updates ────────────────────────────────────────────────────────────────

pub(super) struct UpdatesPage {
    panel: WeakEntity<NpmManagerPanel>,
}

impl UpdatesPage {
    fn new(panel: WeakEntity<NpmManagerPanel>) -> Self {
        Self { panel }
    }
}

impl Render for UpdatesPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(view) = self.panel.upgrade() else {
            return div().into_any_element();
        };
        let panel = view.read(cx);
        let theme = cx.theme();
        let project = panel.active();
        let outdated = &project.outdated;
        let outdated_loading = project.outdated_loading;

        if outdated_loading {
            return div().into_any_element();
        }

        let mut list = v_flex().gap_1p5();
        if outdated.is_empty() {
            list = list.child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child("All packages are up to date."),
            );
        } else {
            list = list.child(
                h_flex().gap_1().children(
                    [
                        ("npm-update-all-safe", "Update All Safe", "safe"),
                        ("npm-update-all-patch", "Update All Patch", "patch"),
                    ]
                    .map(|(id, label, ty)| {
                        let view = view.clone();
                        Button::new(id)
                            .label(label)
                            .with_size(Size::Small)
                            .on_click(move |_, window, cx| {
                                view.update(cx, |panel, cx| {
                                    panel.update_all(ty, window, cx);
                                });
                            })
                    }),
                ),
            );
            for pkg in outdated {
                let name = pkg.name.clone();
                let current = pkg.current.clone();
                let latest = pkg.latest.clone();
                let kind = classify_update(&pkg.current, &pkg.latest);
                // Red is reserved for the Vulnerabilities page — "major"
                // here doesn't mean "broken", just "bigger diff to
                // review", matching `dotnet_panel`/`node_panel`'s corrected
                // scheme (and this page's own Vulnerabilities badge below).
                let (kind_color, kind_label) = match kind {
                    UpdateKind::Major => (theme.warning, "major"),
                    UpdateKind::Minor => (theme.info, "minor"),
                    UpdateKind::Patch => (theme.muted_foreground, "patch"),
                    UpdateKind::Current => (theme.muted_foreground, "same"),
                };

                // Card header: the package name (clickable → details), then
                // the update-kind badge in the same slot as the search cards'
                // compat badge.
                let mut header = h_flex().gap_2().items_center().w_full();
                header = header.child(
                    div()
                        .id(format!("npm-outdated-{name}"))
                        .text_sm()
                        .font_semibold()
                        .text_color(theme.foreground)
                        .child(name.clone())
                        .cursor_pointer()
                        .on_click({
                            let view = view.clone();
                            let name = name.clone();
                            move |_, _window, cx| {
                                view.update(cx, |panel, cx| {
                                    panel.fetch_details(name.clone(), cx);
                                });
                            }
                        }),
                );
                header = header.child(
                    div()
                        .px_1p5()
                        .py_0p5()
                        .rounded_md()
                        .bg(kind_color.opacity(0.2))
                        .text_color(kind_color)
                        .text_xs()
                        .child(kind_label),
                );

                // "update" line: `[current → latest]`, per the card spec.
                let update_line = h_flex()
                    .gap_1()
                    .items_center()
                    .w_full()
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child("update"),
                    )
                    .child(
                        div()
                            .px_1p5()
                            .py_0p5()
                            .rounded_md()
                            .bg(theme.border.opacity(0.5))
                            .text_color(theme.foreground)
                            .text_xs()
                            .child(format!("{current} → {latest}")),
                    );

                let card = v_flex()
                    .gap_1()
                    .w_full()
                    .p_2()
                    .rounded_md()
                    .border_1()
                    .border_color(theme.border.opacity(0.6))
                    .child(header)
                    .child(update_line)
                    .child(
                        h_flex()
                            .gap_2()
                            .items_center()
                            .w_full()
                            .child(div().flex_1().min_w_0())
                            .child(
                                Button::new(format!("npm-update-{name}"))
                                    .label("Update")
                                    .with_size(Size::Small)
                                    .on_click({
                                        let view = view.clone();
                                        let name = name.clone();
                                        let latest = latest.clone();
                                        move |_, window, cx| {
                                            view.update(cx, |panel, cx| {
                                                panel.update_pkg(&name, &latest, window, cx);
                                            });
                                        }
                                    }),
                            ),
                    )
                    .child(
                        h_flex().w_full().child(
                            Button::new(format!("npm-more-info-{name}"))
                                .ghost()
                                .xsmall()
                                .icon(IconName::Info)
                                .label("More info")
                                .on_click({
                                    let view = view.clone();
                                    let name = name.clone();
                                    move |_, _window, cx| {
                                        view.update(cx, |panel, cx| {
                                            panel.fetch_details(name.clone(), cx);
                                        });
                                    }
                                }),
                        ),
                    );
                list = list.child(card);
            }
        }
        list.into_any_element()
    }
}

fn build_updates_page(project: &crate::NpmProject, pages: &PageViews) -> SettingPage {
    let count = project.outdated.len();
    let loading = project.outdated_loading;
    SettingPage::new("Updates")
        .resettable(false)
        .icon(Icon::new(IconName::ArrowUp))
        .description("Packages behind their latest version.")
        .group(
            SettingGroup::new()
                .title("Outdated packages")
                .item(embed_view(pages.updates.clone())),
        )
        .sidebar_badge(move |_, cx| count_badge(count, loading, cx))
}

// ── Vulnerabilities ────────────────────────────────────────────────────────

pub(super) struct VulnPage {
    panel: WeakEntity<NpmManagerPanel>,
}

impl VulnPage {
    fn new(panel: WeakEntity<NpmManagerPanel>) -> Self {
        Self { panel }
    }
}

impl Render for VulnPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(view) = self.panel.upgrade() else {
            return div().into_any_element();
        };
        let panel = view.read(cx);
        let theme = cx.theme();
        let project = panel.active();
        let audit = &project.audit;
        let audit_loading = project.audit_loading;

        if audit_loading {
            return div().into_any_element();
        }

        let mut list = v_flex().gap_1p5();
        if audit.is_empty() {
            list = list.child(
                div()
                    .text_xs()
                    .text_color(theme.success)
                    .child("No known vulnerabilities for this package set."),
            );
        } else {
            for vuln in audit {
                let package = vuln.package.clone();
                let severity = vuln.severity.clone();
                let title = vuln.title.clone();
                let fixed_in = vuln.fixed_in.clone();
                let severity_color = match severity.as_str() {
                    "critical" | "high" => theme.danger,
                    "moderate" => theme.warning,
                    _ => theme.muted_foreground,
                };
                let mut row = h_flex()
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
                            .child(severity.clone()),
                    )
                    .child(
                        div()
                            .id(format!("npm-vuln-{package}"))
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .cursor_pointer()
                            .text_xs()
                            .text_color(theme.foreground)
                            .child(package.clone())
                            .on_click({
                                let view = view.clone();
                                let package = package.clone();
                                move |_, _window, cx| {
                                    view.update(cx, |panel, cx| {
                                        panel.fetch_details(package.clone(), cx);
                                    });
                                }
                            }),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(title.clone()),
                    );
                if let Some(version) = &fixed_in {
                    row = row.child(
                        Button::new(format!("npm-fix-{package}"))
                            .primary()
                            .label(format!("Fix → {version}"))
                            .with_size(Size::Small)
                            .on_click({
                                let view = view.clone();
                                let package = package.clone();
                                let version = version.clone();
                                move |_, window, cx| {
                                    view.update(cx, |panel, cx| {
                                        panel.update_pkg(&package, &version, window, cx);
                                    });
                                }
                            }),
                    );
                }
                list = list.child(row);
            }
        }
        list.into_any_element()
    }
}

fn build_vuln_page(project: &crate::NpmProject, pages: &PageViews) -> SettingPage {
    let count = project.audit.len();
    let loading = project.audit_loading;
    SettingPage::new("Vulnerabilities")
        .resettable(false)
        .icon(Icon::new(IconName::TriangleAlert))
        .description("Findings from the latest `npm audit` run.")
        .group(
            SettingGroup::new()
                .title("npm audit findings")
                .item(embed_view(pages.vuln.clone())),
        )
        .sidebar_badge(move |_, cx| count_badge(count, loading, cx))
}

// ── Shared ─────────────────────────────────────────────────────────────────

fn count_badge(count: usize, loading: bool, cx: &App) -> AnyElement {
    let theme = cx.theme();
    h_flex()
        .child(
            div()
                .when(count == 0 && !loading, |this| this.size_0())
                .when(count > 0, |this| {
                    this.flex()
                        .bg(theme.primary)
                        .rounded_full()
                        .px_1p5()
                        .min_w_3p5()
                        .h_4()
                        .items_center()
                        .justify_center()
                        .text_xs()
                        .text_color(white())
                        .child(count.to_string())
                })
                .when(count == 0 && loading, |this| {
                    this.child(Spinner::new().with_size(Size::XSmall))
                }),
        )
        .into_any_element()
}
