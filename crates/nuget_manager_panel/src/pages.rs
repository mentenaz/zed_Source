//! The five `SettingPage`s of the NuGet manager tab.
//!
//! Each page is a *view shell*: a cheap `SettingPage` whose body is a single
//! `SettingItem::render` that embeds a dedicated child `Entity<...>` view. The
//! heavy rows live in those child views and are built — by reading the panel
//! live through a `WeakEntity<NuGetManagerPanel>` in each view's
//! `Render::render` — only for the page currently open in the `Settings`
//! widget, which renders just the active page's items.
//!
//! Building the pages therefore no longer clones the package lists up front,
//! so a re-render of the panel (e.g. the 60s auto-refresh) never traces through
//! every installed / outdated / vulnerability / search row the way the old
//! `build_*_page` functions did.

use dotnet_backend::{UpdateKind, fmt_downloads};
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

use crate::NuGetManagerPanel;

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
    pub(super) fn new(panel: WeakEntity<NuGetManagerPanel>, cx: &mut App) -> Self {
        PageViews {
            general: cx.new(|_| GeneralPage::new(panel.clone())),
            search: cx.new(|_| SearchPage::new(panel.clone())),
            installed: cx.new(|_| InstalledPage::new(panel.clone())),
            updates: cx.new(|_| UpdatesPage::new(panel.clone())),
            vuln: cx.new(|_| VulnPage::new(panel.clone())),
        }
    }
}

/// `panel`'s snapshot is only used for the sidebar badges — the views read the
/// rest of the state live, so `build_all` stays O(1) per page.
pub(super) fn build_all(panel: &NuGetManagerPanel, pages: &PageViews) -> Vec<SettingPage> {
    vec![
        build_general_page(pages),
        build_search_page(pages),
        build_installed_page(panel, pages),
        build_updates_page(panel, pages),
        build_vuln_page(panel, pages),
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
    panel: WeakEntity<NuGetManagerPanel>,
}

impl GeneralPage {
    fn new(panel: WeakEntity<NuGetManagerPanel>) -> Self {
        Self { panel }
    }
}

impl Render for GeneralPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(view) = self.panel.upgrade() else {
            return div().into_any_element();
        };
        let panel = view.read(cx);
        let theme = cx.theme();

        let busy = panel.running_action.is_some()
            || panel.installed_loading
            || panel.outdated_loading
            || panel.vulnerable_loading;
        let running_action = panel.running_action.clone();
        let error = panel.error.clone();
        let csproj = panel.csproj.clone();
        let dotnet_version = panel.dotnet_version.clone();

        v_flex()
            .gap_2()
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(
                        Button::new("nuget-reload")
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
            .child(
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
                            .child("Project"),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_xs()
                            .text_color(theme.foreground)
                            .child(csproj.clone().unwrap_or_default()),
                    ),
            )
            .child(
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
                            .child("dotnet"),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_xs()
                            .text_color(if dotnet_version.is_some() {
                                theme.success
                            } else {
                                theme.warning
                            })
                            .child(match &dotnet_version {
                                Some(v) => format!("version {v}"),
                                None => "not detected on PATH — CLI actions will fail".to_string(),
                            }),
                    ),
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
        .description("The .NET project NuGet commands target and the active SDK.")
        .group(
            SettingGroup::new()
                .title("Project")
                .description("The resolved `.csproj` this tab installs into.")
                .item(embed_view(pages.general.clone())),
        )
}

// ── Search ─────────────────────────────────────────────────────────────────

pub(super) struct SearchPage {
    panel: WeakEntity<NuGetManagerPanel>,
}

impl SearchPage {
    fn new(panel: WeakEntity<NuGetManagerPanel>) -> Self {
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
                        Button::new("nuget-search-go")
                            .icon(IconName::Search)
                            .label("Search")
                            .with_size(Size::Small)
                            .on_click({
                                let view = view.clone();
                                move |_, _window, cx| {
                                    view.update(cx, |panel, cx| {
                                        panel.search_page = 0;
                                        panel.search_nuget(cx);
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
                                .child("Searching nuget.org…"),
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
                        .child("Search the NuGet registry for a package to install.")
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
                        let id = result.id.clone();
                        let version = result.version.clone();
                        let description = result.description.clone().unwrap_or_default();
                        let downloads = fmt_downloads(result.total_downloads);

                        // Card header: the package name (clickable → details),
                        // then the lifetime download count on the right edge.
                        let mut header = h_flex().gap_2().items_center().w_full();
                        header = header.child(
                            div()
                                .id(format!("nuget-result-{id}"))
                                .text_sm()
                                .font_semibold()
                                .text_color(theme.foreground)
                                .child(id.clone())
                                .cursor_pointer()
                                .on_click({
                                    let view = view.clone();
                                    let id = id.clone();
                                    move |_, _window, cx| {
                                        view.update(cx, |panel, cx| {
                                            panel.fetch_details(id.clone(), cx);
                                        });
                                    }
                                }),
                        );
                        header = header.child(div().flex_1());
                        header = header.child(
                            div()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child(format!("{downloads} downloads")),
                        );

                        // "latest" line: `[id@version]`, per the card spec.
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
                                    .child(format!("{id}@{version}")),
                            );

                        let install_button = Button::new(format!("nuget-install-{id}"))
                            .label("Install")
                            .with_size(Size::Small)
                            .on_click({
                                let view = view.clone();
                                let id = id.clone();
                                let version = version.clone();
                                move |_, window, cx| {
                                    view.update(cx, |panel, cx| {
                                        panel.install_pkg(&id, &version, window, cx);
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
                                    Button::new(format!("nuget-more-info-{id}"))
                                        .ghost()
                                        .xsmall()
                                        .icon(IconName::Info)
                                        .label("More info")
                                        .on_click({
                                            let view = view.clone();
                                            let id = id.clone();
                                            move |_, _window, cx| {
                                                view.update(cx, |panel, cx| {
                                                    panel.fetch_details_and_readme(id.clone(), cx);
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
                                Button::new("nuget-search-more")
                                    .ghost()
                                    .label("Load more")
                                    .with_size(Size::Small)
                                    .on_click({
                                        let view = view.clone();
                                        move |_, _window, cx| {
                                            view.update(cx, |panel, cx| {
                                                panel.search_page += 1;
                                                panel.search_nuget(cx);
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
        .description("Find packages on nuget.org and install them.")
        .group(
            SettingGroup::new()
                .title("nuget.org")
                .item(embed_view(pages.search.clone())),
        )
}

// ── Installed ──────────────────────────────────────────────────────────────

pub(super) struct InstalledPage {
    panel: WeakEntity<NuGetManagerPanel>,
}

impl InstalledPage {
    fn new(panel: WeakEntity<NuGetManagerPanel>) -> Self {
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
        let installed = &panel.installed;
        let installed_loading = panel.installed_loading;

        if installed_loading {
            // Rows appear once the csproj refs are parsed; the sidebar badge
            // spins meanwhile.
            return div().into_any_element();
        }

        let mut list = v_flex().gap_1p5();
        if installed.is_empty() {
            list = list.child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child("No NuGet packages installed in this project."),
            );
        } else {
            for pkg in installed {
                let id = pkg.id.clone();
                let version = pkg.version.clone();

                let header = h_flex().gap_2().items_center().w_full().child(
                    div()
                        .id(format!("nuget-installed-{id}"))
                        .text_sm()
                        .font_semibold()
                        .text_color(theme.foreground)
                        .child(id.clone())
                        .cursor_pointer()
                        .on_click({
                            let view = view.clone();
                            let id = id.clone();
                            move |_, _window, cx| {
                                view.update(cx, |panel, cx| {
                                    panel.fetch_details(id.clone(), cx);
                                });
                            }
                        }),
                );

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
                            .child(format!("{id}@{version}")),
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
                                Button::new(format!("nuget-remove-{id}"))
                                    .danger()
                                    .label("Remove")
                                    .with_size(Size::Small)
                                    .on_click({
                                        let view = view.clone();
                                        let id = id.clone();
                                        move |_, window, cx| {
                                            view.update(cx, |panel, cx| {
                                                panel.remove_package(&id, window, cx);
                                            });
                                        }
                                    }),
                            ),
                    )
                    .child(
                        h_flex().w_full().child(
                            Button::new(format!("nuget-more-info-{id}"))
                                .ghost()
                                .xsmall()
                                .icon(IconName::Info)
                                .label("More info")
                                .on_click({
                                    let view = view.clone();
                                    let id = id.clone();
                                    move |_, _window, cx| {
                                        view.update(cx, |panel, cx| {
                                            panel.fetch_details(id.clone(), cx);
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

fn build_installed_page(panel: &NuGetManagerPanel, pages: &PageViews) -> SettingPage {
    let count = panel.installed.len();
    let loading = panel.installed_loading;
    SettingPage::new("Installed")
        .resettable(false)
        .icon(Icon::new(IconName::Inbox))
        .description("Packages referenced by the resolved project.")
        .group(
            SettingGroup::new()
                .title("Installed packages")
                .item(embed_view(pages.installed.clone())),
        )
        .sidebar_badge(move |_, cx| count_badge(count, loading, cx))
}

// ── Updates ────────────────────────────────────────────────────────────────

pub(super) struct UpdatesPage {
    panel: WeakEntity<NuGetManagerPanel>,
}

impl UpdatesPage {
    fn new(panel: WeakEntity<NuGetManagerPanel>) -> Self {
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
        let outdated = &panel.outdated;
        let outdated_loading = panel.outdated_loading;

        if outdated_loading {
            return div().into_any_element();
        }

        let mut list = v_flex().gap_1p5();
        if let Some(err) = &panel.outdated_error {
            // `dotnet list package` failed (usually "run dotnet restore" — the
            // CLI refuses to check a project with stale assets.json).
            list = list
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.danger)
                        .child(format!("Could not check for updates: {err}")),
                )
                .child(
                    h_flex().w_full().child(
                        Button::new("nuget-outdated-retry")
                            .ghost()
                            .label("Retry")
                            .with_size(Size::Small)
                            .on_click({
                                let view = view.clone();
                                move |_, _window, cx| {
                                    view.update(cx, |panel, cx| panel.reload_visible(cx));
                                }
                            }),
                    ),
                );
        } else if outdated.is_empty() {
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
                        ("nuget-update-all-safe", "Update All Safe", "safe"),
                        ("nuget-update-all-patch", "Update All Patch", "patch"),
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
                let id = pkg.id.clone();
                let installed = pkg.installed.clone();
                let latest = pkg.latest.clone();
                let kind = pkg.update_kind;
                let (kind_color, kind_label) = match kind {
                    UpdateKind::Major => (theme.danger, "major"),
                    UpdateKind::Minor => (theme.warning, "minor"),
                    UpdateKind::Patch => (theme.success, "patch"),
                };

                let mut header = h_flex().gap_2().items_center().w_full();
                header = header.child(
                    div()
                        .id(format!("nuget-outdated-{id}"))
                        .text_sm()
                        .font_semibold()
                        .text_color(theme.foreground)
                        .child(id.clone())
                        .cursor_pointer()
                        .on_click({
                            let view = view.clone();
                            let id = id.clone();
                            move |_, _window, cx| {
                                view.update(cx, |panel, cx| {
                                    panel.fetch_details(id.clone(), cx);
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
                            .child(format!("{installed} → {latest}")),
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
                                Button::new(format!("nuget-update-{id}"))
                                    .label("Update")
                                    .with_size(Size::Small)
                                    .on_click({
                                        let view = view.clone();
                                        let id = id.clone();
                                        let latest = latest.clone();
                                        move |_, window, cx| {
                                            view.update(cx, |panel, cx| {
                                                panel.update_pkg(&id, &latest, window, cx);
                                            });
                                        }
                                    }),
                            ),
                    )
                    .child(
                        h_flex().w_full().child(
                            Button::new(format!("nuget-more-info-{id}"))
                                .ghost()
                                .xsmall()
                                .icon(IconName::Info)
                                .label("More info")
                                .on_click({
                                    let view = view.clone();
                                    let id = id.clone();
                                    move |_, _window, cx| {
                                        view.update(cx, |panel, cx| {
                                            panel.fetch_details(id.clone(), cx);
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

fn build_updates_page(panel: &NuGetManagerPanel, pages: &PageViews) -> SettingPage {
    let count = panel.outdated.len();
    let loading = panel.outdated_loading;
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
    panel: WeakEntity<NuGetManagerPanel>,
}

impl VulnPage {
    fn new(panel: WeakEntity<NuGetManagerPanel>) -> Self {
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
        let vulnerable = &panel.vulnerable;
        let vulnerable_loading = panel.vulnerable_loading;

        if vulnerable_loading {
            return div().into_any_element();
        }

        let mut list = v_flex().gap_1p5();
        if let Some(err) = &panel.vulnerable_error {
            list = list
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.danger)
                        .child(format!("Could not check for vulnerabilities: {err}")),
                )
                .child(
                    h_flex().w_full().child(
                        Button::new("nuget-vuln-retry")
                            .ghost()
                            .label("Retry")
                            .with_size(Size::Small)
                            .on_click({
                                let view = view.clone();
                                move |_, _window, cx| {
                                    view.update(cx, |panel, cx| panel.reload_visible(cx));
                                }
                            }),
                    ),
                );
        } else if vulnerable.is_empty() {
            list = list.child(
                div()
                    .text_xs()
                    .text_color(theme.success)
                    .child("No known vulnerabilities for this package set."),
            );
        } else {
            for vuln in vulnerable {
                let package = vuln.id.clone();
                let severity = vuln.severity.clone();
                let advisory_url = vuln.advisory_url.clone();
                let severity_color = match severity.as_str() {
                    "critical" | "high" => theme.danger,
                    "moderate" => theme.warning,
                    _ => theme.muted_foreground,
                };
                let row = h_flex()
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
                            .id(format!("nuget-vuln-{package}"))
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
                            .child(advisory_url.clone()),
                    );
                list = list.child(row);
            }
        }
        list.into_any_element()
    }
}

fn build_vuln_page(panel: &NuGetManagerPanel, pages: &PageViews) -> SettingPage {
    let count = panel.vulnerable.len();
    let loading = panel.vulnerable_loading;
    SettingPage::new("Vulnerabilities")
        .resettable(false)
        .icon(Icon::new(IconName::TriangleAlert))
        .description("Findings from the latest `dotnet list package --vulnerable` run.")
        .group(
            SettingGroup::new()
                .title("Known vulnerabilities")
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
