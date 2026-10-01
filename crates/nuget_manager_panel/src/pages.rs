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
    AnyElement, App, AppContext as _, Context, Entity, FocusHandle, Hsla, InteractiveElement as _,
    IntoElement, MouseButton, ParentElement as _, Render, StatefulInteractiveElement as _,
    Styled as _, WeakEntity, Window, div, prelude::FluentBuilder as _,
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

use crate::{
    ActSelectedPackage, NuGetManagerPanel, OpenSelectedPackage, SelectNextPackage,
    SelectPrevPackage,
};

/// The key context each package-list page's own container sets — see
/// `nuget_manager_panel::init`'s `cx.bind_keys` for the up/down/enter/space
/// bindings scoped to it, and why they live there rather than in the JSON
/// keymap (mirrors `gpui_component::table::data_table`'s own `DataTable`
/// context for the same reason; see also `npm_manager_panel::pages`'s
/// identical `PACKAGE_LIST_CONTEXT`).
const PACKAGE_LIST_CONTEXT: &str = "NuGetPackageList";

/// Moves `selected` one row up (`forward: false`) or down (`forward: true`)
/// within a `len`-row list, wrapping at both ends — matches
/// `gpui_component::table::TableState`'s default `loop_selection` behavior
/// — and starting from the top row on the very first press.
fn step_selected(selected: Option<usize>, len: usize, forward: bool) -> Option<usize> {
    if len == 0 {
        return None;
    }
    let ix = selected.unwrap_or(0);
    Some(if forward { (ix + 1) % len } else { (ix + len - 1) % len })
}

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
            search: cx.new(|cx| SearchPage::new(panel.clone(), cx)),
            installed: cx.new(|cx| InstalledPage::new(panel.clone(), cx)),
            updates: cx.new(|cx| UpdatesPage::new(panel.clone(), cx)),
            vuln: cx.new(|cx| VulnPage::new(panel.clone(), cx)),
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
    focus_handle: FocusHandle,
    /// Index into `panel.search_results` — see `InstalledPage::selected`.
    /// Scoped to just the results list below the search box (not the whole
    /// page), so the search `Input`'s own arrow-key/Enter behavior while
    /// typing is untouched — see the results `v_flex`'s own
    /// `track_focus`/`key_context` below, not the page root.
    selected: Option<usize>,
}

impl SearchPage {
    fn new(panel: WeakEntity<NuGetManagerPanel>, cx: &mut Context<Self>) -> Self {
        Self { panel, focus_handle: cx.focus_handle(), selected: None }
    }

    fn select_next(&mut self, _: &SelectNextPackage, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(view) = self.panel.upgrade() else { return };
        let len = view.read(cx).search_results.len();
        self.selected = step_selected(self.selected, len, true);
        cx.notify();
    }

    fn select_prev(&mut self, _: &SelectPrevPackage, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(view) = self.panel.upgrade() else { return };
        let len = view.read(cx).search_results.len();
        self.selected = step_selected(self.selected, len, false);
        cx.notify();
    }

    fn open_selected(
        &mut self,
        _: &OpenSelectedPackage,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(view) = self.panel.upgrade() else { return };
        let Some(id) = self
            .selected
            .and_then(|ix| view.read(cx).search_results.get(ix))
            .map(|r| r.id.clone())
        else {
            return;
        };
        view.update(cx, |panel, cx| panel.fetch_details_and_readme(id, cx));
    }

    /// Space's action on this page: Install — not destructive, so unlike
    /// Remove this needs no confirmation, matching the existing Install
    /// button.
    fn act_on_selected(
        &mut self,
        _: &ActSelectedPackage,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(view) = self.panel.upgrade() else { return };
        let Some((id, version)) = self
            .selected
            .and_then(|ix| view.read(cx).search_results.get(ix))
            .map(|r| (r.id.clone(), r.version.clone()))
        else {
            return;
        };
        view.update(cx, |panel, cx| panel.install_pkg(&id, &version, window, cx));
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
        let selected = self.selected;

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
                    let mut list = v_flex()
                        .id("nuget-search-results-list")
                        .track_focus(&self.focus_handle)
                .on_mouse_down(MouseButton::Left, cx.listener(|this, _, window, cx| {
                    window.focus(&this.focus_handle, cx);
                }))
                        .key_context(PACKAGE_LIST_CONTEXT)
                        .on_action(cx.listener(Self::select_next))
                        .on_action(cx.listener(Self::select_prev))
                        .on_action(cx.listener(Self::open_selected))
                        .on_action(cx.listener(Self::act_on_selected))
                        .gap_2()
                        .w_full();
                    list = list.child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(format!("{search_total} packages found")),
                    );
                    for (ix, result) in results.iter().enumerate() {
                        let id = result.id.clone();
                        let version = result.version.clone();
                        let description = result.description.clone().unwrap_or_default();
                        let downloads = fmt_downloads(result.total_downloads);
                        let is_selected = selected == Some(ix);

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
                            .id(("nuget-search-card", ix))
                            .gap_1()
                            .w_full()
                            .p_2()
                            .rounded_md()
                            .border_1()
                            .border_color(if is_selected { theme.primary } else { theme.border.opacity(0.6) })
                            .when(is_selected, |card| card.bg(theme.primary.opacity(0.05)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.selected = Some(ix);
                                cx.notify();
                            }))
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
    focus_handle: FocusHandle,
    /// Index into `panel.installed` — the row `up`/`down`/`enter`/`space`
    /// act on. `None` until the list first gets focus (a click, or Tab).
    selected: Option<usize>,
}

impl InstalledPage {
    fn new(panel: WeakEntity<NuGetManagerPanel>, cx: &mut Context<Self>) -> Self {
        Self { panel, focus_handle: cx.focus_handle(), selected: None }
    }

    fn select_next(&mut self, _: &SelectNextPackage, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(view) = self.panel.upgrade() else { return };
        let len = view.read(cx).installed.len();
        self.selected = step_selected(self.selected, len, true);
        cx.notify();
    }

    fn select_prev(&mut self, _: &SelectPrevPackage, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(view) = self.panel.upgrade() else { return };
        let len = view.read(cx).installed.len();
        self.selected = step_selected(self.selected, len, false);
        cx.notify();
    }

    fn open_selected(
        &mut self,
        _: &OpenSelectedPackage,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(view) = self.panel.upgrade() else { return };
        let Some(id) = self
            .selected
            .and_then(|ix| view.read(cx).installed.get(ix))
            .map(|pkg| pkg.id.clone())
        else {
            return;
        };
        view.update(cx, |panel, cx| panel.fetch_details(id, cx));
    }

    /// Space's action on this page: Remove (confirmed — see
    /// `remove_package`'s own doc comment).
    fn act_on_selected(
        &mut self,
        _: &ActSelectedPackage,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(view) = self.panel.upgrade() else { return };
        let Some(id) = self
            .selected
            .and_then(|ix| view.read(cx).installed.get(ix))
            .map(|pkg| pkg.id.clone())
        else {
            return;
        };
        view.update(cx, |panel, cx| panel.remove_package(&id, window, cx));
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

        let selected = self.selected;
        let mut list = v_flex()
            .id("nuget-installed-list")
            .track_focus(&self.focus_handle)
                .on_mouse_down(MouseButton::Left, cx.listener(|this, _, window, cx| {
                    window.focus(&this.focus_handle, cx);
                }))
            .key_context(PACKAGE_LIST_CONTEXT)
            .on_action(cx.listener(Self::select_next))
            .on_action(cx.listener(Self::select_prev))
            .on_action(cx.listener(Self::open_selected))
            .on_action(cx.listener(Self::act_on_selected))
            .gap_1p5();
        if installed.is_empty() {
            list = list.child(
                div()
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .child("No NuGet packages installed in this project."),
            );
        } else {
            for (ix, pkg) in installed.iter().enumerate() {
                let id = pkg.id.clone();
                let version = pkg.version.clone();
                let is_selected = selected == Some(ix);

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
                    .id(("nuget-installed-card", ix))
                    .gap_1()
                    .w_full()
                    .p_2()
                    .rounded_md()
                    .border_1()
                    .border_color(if is_selected { theme.primary } else { theme.border.opacity(0.6) })
                    .when(is_selected, |card| card.bg(theme.primary.opacity(0.05)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.selected = Some(ix);
                        cx.notify();
                    }))
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
    focus_handle: FocusHandle,
    /// Index into `panel.outdated` — see `InstalledPage::selected`.
    selected: Option<usize>,
}

impl UpdatesPage {
    fn new(panel: WeakEntity<NuGetManagerPanel>, cx: &mut Context<Self>) -> Self {
        Self { panel, focus_handle: cx.focus_handle(), selected: None }
    }

    fn select_next(&mut self, _: &SelectNextPackage, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(view) = self.panel.upgrade() else { return };
        let len = view.read(cx).outdated.len();
        self.selected = step_selected(self.selected, len, true);
        cx.notify();
    }

    fn select_prev(&mut self, _: &SelectPrevPackage, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(view) = self.panel.upgrade() else { return };
        let len = view.read(cx).outdated.len();
        self.selected = step_selected(self.selected, len, false);
        cx.notify();
    }

    fn open_selected(
        &mut self,
        _: &OpenSelectedPackage,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(view) = self.panel.upgrade() else { return };
        let Some(id) = self
            .selected
            .and_then(|ix| view.read(cx).outdated.get(ix))
            .map(|pkg| pkg.id.clone())
        else {
            return;
        };
        view.update(cx, |panel, cx| panel.fetch_details(id, cx));
    }

    /// Space's action on this page: Update to latest — not destructive (a
    /// version bump), so unlike Remove this needs no confirmation, matching
    /// the existing Update button.
    fn act_on_selected(
        &mut self,
        _: &ActSelectedPackage,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(view) = self.panel.upgrade() else { return };
        let Some((id, latest)) = self
            .selected
            .and_then(|ix| view.read(cx).outdated.get(ix))
            .map(|pkg| (pkg.id.clone(), pkg.latest.clone()))
        else {
            return;
        };
        view.update(cx, |panel, cx| panel.update_pkg(&id, &latest, window, cx));
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

        let selected = self.selected;
        let mut list = v_flex()
            .id("nuget-updates-list")
            .track_focus(&self.focus_handle)
                .on_mouse_down(MouseButton::Left, cx.listener(|this, _, window, cx| {
                    window.focus(&this.focus_handle, cx);
                }))
            .key_context(PACKAGE_LIST_CONTEXT)
            .on_action(cx.listener(Self::select_next))
            .on_action(cx.listener(Self::select_prev))
            .on_action(cx.listener(Self::open_selected))
            .on_action(cx.listener(Self::act_on_selected))
            .gap_1p5();
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
            for (ix, pkg) in outdated.iter().enumerate() {
                let id = pkg.id.clone();
                let installed = pkg.installed.clone();
                let latest = pkg.latest.clone();
                let is_selected = selected == Some(ix);
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
                    .id(("nuget-outdated-card", ix))
                    .gap_1()
                    .w_full()
                    .p_2()
                    .rounded_md()
                    .border_1()
                    .border_color(if is_selected { theme.primary } else { theme.border.opacity(0.6) })
                    .when(is_selected, |card| card.bg(theme.primary.opacity(0.05)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.selected = Some(ix);
                        cx.notify();
                    }))
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
    focus_handle: FocusHandle,
    /// Index into `panel.vulnerable` — see `InstalledPage::selected`. No
    /// `act_on_selected`/`ActSelectedPackage` handler on this page: unlike
    /// npm's audit findings, `dotnet list package --vulnerable` doesn't
    /// report a fixed-in version, so there's no per-row Fix button to give
    /// Space a job here — it's simply left unhandled (falls through as a
    /// no-op) rather than wired to something that doesn't exist.
    selected: Option<usize>,
}

impl VulnPage {
    fn new(panel: WeakEntity<NuGetManagerPanel>, cx: &mut Context<Self>) -> Self {
        Self { panel, focus_handle: cx.focus_handle(), selected: None }
    }

    fn select_next(&mut self, _: &SelectNextPackage, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(view) = self.panel.upgrade() else { return };
        let len = view.read(cx).vulnerable.len();
        self.selected = step_selected(self.selected, len, true);
        cx.notify();
    }

    fn select_prev(&mut self, _: &SelectPrevPackage, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(view) = self.panel.upgrade() else { return };
        let len = view.read(cx).vulnerable.len();
        self.selected = step_selected(self.selected, len, false);
        cx.notify();
    }

    fn open_selected(
        &mut self,
        _: &OpenSelectedPackage,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(view) = self.panel.upgrade() else { return };
        let Some(id) = self
            .selected
            .and_then(|ix| view.read(cx).vulnerable.get(ix))
            .map(|vuln| vuln.id.clone())
        else {
            return;
        };
        view.update(cx, |panel, cx| panel.fetch_details(id, cx));
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

        let selected = self.selected;
        let mut list = v_flex()
            .id("nuget-vuln-list")
            .track_focus(&self.focus_handle)
                .on_mouse_down(MouseButton::Left, cx.listener(|this, _, window, cx| {
                    window.focus(&this.focus_handle, cx);
                }))
            .key_context(PACKAGE_LIST_CONTEXT)
            .on_action(cx.listener(Self::select_next))
            .on_action(cx.listener(Self::select_prev))
            .on_action(cx.listener(Self::open_selected))
            .gap_1p5();
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
            for (ix, vuln) in vulnerable.iter().enumerate() {
                let package = vuln.id.clone();
                let severity = vuln.severity.clone();
                let advisory_url = vuln.advisory_url.clone();
                let is_selected = selected == Some(ix);
                let severity_color = match severity.as_str() {
                    "critical" | "high" => theme.danger,
                    "moderate" => theme.warning,
                    _ => theme.muted_foreground,
                };
                let row = h_flex()
                    .id(("nuget-vuln-row", ix))
                    .gap_2()
                    .items_center()
                    .w_full()
                    .rounded_md()
                    .when(is_selected, |row| {
                        row.bg(theme.primary.opacity(0.08)).border_1().border_color(theme.primary)
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.selected = Some(ix);
                        cx.notify();
                    }))
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
        .sidebar_badge(move |_, cx| colored_count_badge(count, loading, cx.theme().danger))
}

// ── Shared ─────────────────────────────────────────────────────────────────

fn count_badge(count: usize, loading: bool, cx: &App) -> AnyElement {
    colored_count_badge(count, loading, cx.theme().primary)
}

/// Like [`count_badge`], but with the pill's fill color set by the caller —
/// used for the Vulnerabilities badge, which needs the error color rather
/// than the primary-theme color the Installed/Updates badges use.
fn colored_count_badge(count: usize, loading: bool, color: Hsla) -> AnyElement {
    h_flex()
        .child(
            div()
                .when(count == 0 && !loading, |this| this.size_0())
                .when(count > 0, |this| {
                    this.flex()
                        .bg(color)
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
