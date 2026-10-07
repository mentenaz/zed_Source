//! The `SettingPage`s of the Cargo manager tab: General, Installed, Updates
//! and Vulnerabilities.
//!
//! Each page is a *view shell*, as in the other manager tabs: a cheap
//! `SettingPage` whose body is a single `SettingItem::render` embedding a
//! child view. The rows are built in that view's own `Render::render`, by
//! reading the panel live through a `WeakEntity`, so only the page that is
//! open is ever built.
//!
//! The three list pages share one view type, [`ListPage`], told apart by
//! [`ListKind`]: they differ in what a row is, not in how a list of rows is
//! focused, stepped through and acted on.

use cargo_backend::{
    Declaration, DependencyKind, Finding, FindingCounts, FindingKind, RustCompat, Severity,
    UpdateTarget,
};
use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, FocusHandle, Hsla, InteractiveElement as _,
    IntoElement, MouseButton, ParentElement as _, Render, Stateful, StatefulInteractiveElement as _,
    Styled as _, WeakEntity, Window, div, prelude::FluentBuilder as _,
};
use gpui_component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _, Size, StyledExt as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    setting::{SettingGroup, SettingItem, SettingPage},
    spinner::Spinner,
    tag::Tag,
    v_flex, white,
};

use crate::registry::{AdvisoryState, OutdatedRow, OutdatedState};
use crate::{
    ActSelectedPackage, CargoManagerPanel, OpenSelectedPackage, SelectNextPackage,
    SelectPrevPackage,
};

/// The key context each list page's container sets — see
/// `cargo_manager_panel::init` for the bindings scoped to it.
pub(super) const PACKAGE_LIST_CONTEXT: &str = "CargoPackageList";

/// Moves `selected` one row up (`forward: false`) or down (`forward: true`)
/// within a `len`-row list, wrapping at both ends, and starting from the
/// nearest end on the very first press.
fn step_selected(selected: Option<usize>, len: usize, forward: bool) -> Option<usize> {
    if len == 0 {
        return None;
    }
    Some(match selected {
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

/// The page views created alongside the panel.
pub(super) struct PageViews {
    general: Entity<GeneralPage>,
    installed: Entity<ListPage>,
    updates: Entity<ListPage>,
    advisories: Entity<ListPage>,
}

impl PageViews {
    pub(super) fn new(panel: WeakEntity<CargoManagerPanel>, cx: &mut App) -> Self {
        let list = |kind: ListKind, cx: &mut App| {
            let panel = panel.clone();
            cx.new(|cx| ListPage {
                panel,
                kind,
                focus_handle: cx.focus_handle(),
                selected: None,
            })
        };
        PageViews {
            general: cx.new(|_| GeneralPage {
                panel: panel.clone(),
            }),
            installed: list(ListKind::Installed, cx),
            updates: list(ListKind::Updates, cx),
            advisories: list(ListKind::Advisories, cx),
        }
    }
}

/// `panel`'s snapshot is only used for the sidebar badges — the views read
/// the rest of the state live.
pub(super) fn build_all(panel: &CargoManagerPanel, pages: &PageViews) -> Vec<SettingPage> {
    let installed = panel.dependencies.listed.len();
    let (outdated, checking) = match panel.outdated {
        OutdatedState::Done { .. } => (Some(panel.outdated_rows().len()), false),
        OutdatedState::Checking => (None, true),
        OutdatedState::Idle => (None, false),
    };
    let vulnerabilities = panel.vulnerability_count();
    let scanning = matches!(panel.advisories, AdvisoryState::Scanning);

    vec![
        SettingPage::new("General")
            .default_open(true)
            .resettable(false)
            .icon(Icon::new(IconName::Settings))
            .description("The crate this tab manages and the toolchain it is checked against.")
            .group(
                SettingGroup::new()
                    .title("Crate")
                    .description("Pick another crate in the Rust panel, then open its Package Manager.")
                    .item(embed_view(pages.general.clone())),
            ),
        SettingPage::new("Installed")
            .resettable(false)
            .icon(Icon::new(IconName::Inbox))
            .description("The crate's direct dependencies from crates.io, with the version each is locked to.")
            .group(
                SettingGroup::new()
                    .title("Dependencies")
                    .item(embed_view(pages.installed.clone())),
            )
            .sidebar_badge(move |_, cx| count_badge(Some(installed), false, cx.theme().primary)),
        SettingPage::new("Updates")
            .resettable(false)
            .icon(Icon::new(IconName::ArrowUp))
            .description("Dependencies with a newer version on crates.io.")
            .group(
                SettingGroup::new()
                    .title("Behind the registry")
                    .item(embed_view(pages.updates.clone())),
            )
            .sidebar_badge(move |_, cx| count_badge(outdated, checking, cx.theme().primary)),
        SettingPage::new("Vulnerabilities")
            .resettable(false)
            .icon(Icon::new(IconName::TriangleAlert))
            .description("Advisories from OSV.dev for every crates.io package this crate pulls in.")
            .group(
                SettingGroup::new()
                    .title("Advisories")
                    .item(embed_view(pages.advisories.clone())),
            )
            // Warning colour, and no number at all until a scan has run:
            // "not scanned" must not read as zero.
            .sidebar_badge(move |_, cx| count_badge(vulnerabilities, scanning, cx.theme().warning)),
    ]
}

/// A `SettingItem` that simply embeds a page view.
fn embed_view<T>(view: Entity<T>) -> SettingItem
where
    T: Render + 'static,
{
    SettingItem::render(move |_options, _window, _cx| view.clone().into_any_element())
}

/// A count pill for the sidebar. `None` shows nothing (or a spinner while
/// `loading`): an unknown count is never drawn as a zero.
fn count_badge(count: Option<usize>, loading: bool, color: Hsla) -> AnyElement {
    let count = count.filter(|count| *count > 0);
    h_flex()
        .child(
            div()
                .when(count.is_none() && !loading, |this| this.size_0())
                .when_some(count, |this, count| {
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
                .when(count.is_none() && loading, |this| {
                    this.child(Spinner::new().with_size(Size::XSmall))
                }),
        )
        .into_any_element()
}

// ── Shared row pieces ──────────────────────────────────────────────────

fn muted(cx: &App, text: impl Into<String>) -> gpui::Div {
    div()
        .text_xs()
        .text_color(cx.theme().muted_foreground)
        .child(text.into())
}

fn busy_line(cx: &App, text: impl Into<String>) -> gpui::Div {
    h_flex()
        .gap_2()
        .items_center()
        .child(Spinner::new().with_size(Size::XSmall))
        .child(muted(cx, text))
}

fn mono(text: impl Into<String>) -> gpui::Div {
    div().font_family("Cascadia Mono").child(text.into())
}

/// The three minimum-Rust-version states (design decision 6): too new is an
/// error naming the version needed, not declared is a neutral marker, and
/// compatible says nothing.
pub(super) fn rust_tag(rust: &RustCompat) -> Option<Tag> {
    match rust {
        RustCompat::Compatible => None,
        RustCompat::TooNew { needs } => {
            Some(Tag::danger().xsmall().child(format!("needs Rust {needs}")))
        }
        RustCompat::NotDeclared => Some(
            Tag::secondary()
                .xsmall()
                .outline()
                .child("Rust version not declared"),
        ),
    }
}

fn severity_tag(finding: &Finding) -> Option<Tag> {
    let tag = match (finding.severity, finding.kind) {
        (Some(Severity::Critical), _) => Tag::danger().child("critical"),
        (Some(Severity::High), _) => Tag::danger().outline().child("high"),
        (Some(Severity::Moderate), _) => Tag::warning().child("moderate"),
        (Some(Severity::Low), _) => Tag::info().outline().child("low"),
        // Unknown severity on a vulnerability is a warning, never neutral.
        (None, FindingKind::Vulnerability) => Tag::warning().outline().child("unknown"),
        // A notice without a severity simply has none.
        (None, _) => return None,
    };
    Some(tag.xsmall())
}

/// The summary line above the findings: each kind with its own count, so an
/// abandoned crate is never counted as a security hole.
fn counts_line(counts: FindingCounts) -> String {
    let part = |count: usize, one: &str, many: &str| {
        format!("{count} {}", if count == 1 { one } else { many })
    };
    [
        part(counts.vulnerabilities, "vulnerability", "vulnerabilities"),
        part(counts.unsound, "unsound", "unsound"),
        part(counts.unmaintained, "unmaintained", "unmaintained"),
        part(counts.notices, "other notice", "other notices"),
    ]
    .join(" \u{00b7} ")
}

// ── General ────────────────────────────────────────────────────────────

pub(super) struct GeneralPage {
    panel: WeakEntity<CargoManagerPanel>,
}

fn info_row(cx: &App, label: &str, value: String, color: Hsla) -> impl IntoElement {
    h_flex()
        .items_start()
        .gap_2()
        .w_full()
        .child(
            div()
                .w_32()
                .flex_none()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(label.to_string()),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .text_xs()
                .text_color(color)
                .child(value),
        )
}

impl Render for GeneralPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(view) = self.panel.upgrade() else {
            return div().into_any_element();
        };
        let panel = view.read(cx);
        let theme = cx.theme();

        let working = panel.is_busy()
            || matches!(panel.outdated, OutdatedState::Checking)
            || matches!(panel.advisories, AdvisoryState::Scanning);
        let krate = panel.selected_crate();
        let toolchain = |version: &Option<String>| match version {
            Some(version) => (version.clone(), theme.foreground),
            None => ("not detected on PATH".to_string(), theme.warning),
        };
        let (rustc, rustc_color) = toolchain(&panel.rustc_version);
        let (cargo, cargo_color) = toolchain(&panel.cargo_version);

        let mut status = v_flex().gap_1().text_xs();
        if let Some(action) = &panel.running_action {
            status = status.child(
                div()
                    .text_color(theme.foreground)
                    .child(format!("Running: {action}\u{2026} (output in the Script Runner)")),
            );
        }
        if let Some(checking) = &panel.checking {
            status = status.child(div().text_color(theme.foreground).child(checking.clone()));
        }
        if let Some(notice) = &panel.notice {
            status = status.child(div().text_color(theme.muted_foreground).child(notice.clone()));
        }
        if let Some(error) = &panel.error {
            status = status.child(div().text_color(theme.danger).child(error.clone()));
        }
        if panel.manifests.is_empty() {
            status = status.child(div().text_color(theme.warning).child(
                "The workspace's manifests could not be read, so whether a dependency is \
                 inherited from the workspace is unknown. Version changes are disabled.",
            ));
        }

        v_flex()
            .gap_2()
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(
                        Button::new("cargo-reload")
                            .icon(IconName::Redo2)
                            .label("Refresh")
                            .with_size(Size::Small)
                            .tooltip("Re-read Cargo.toml and Cargo.lock from disk")
                            .on_click({
                                let view = view.clone();
                                move |_, _, cx| view.update(cx, |panel, cx| panel.reload(cx))
                            }),
                    )
                    .when(working, |row| row.child(Spinner::new().with_size(Size::XSmall))),
            )
            .child(info_row(
                cx,
                "Crate",
                match krate {
                    Some(krate) => format!("{} {}", krate.name, krate.version),
                    None => "none".to_string(),
                },
                theme.foreground,
            ))
            .child(info_row(
                cx,
                "Manifest",
                krate.map(|krate| krate.manifest_path.clone()).unwrap_or_default(),
                theme.foreground,
            ))
            .child(info_row(
                cx,
                "Workspace",
                panel
                    .cargo_workspace
                    .as_ref()
                    .map(|workspace| format!("{} ({} crates)", workspace.root, workspace.crates.len()))
                    .unwrap_or_default(),
                theme.foreground,
            ))
            .child(info_row(
                cx,
                "Cargo.lock",
                if panel.lockfile.is_some() {
                    "found".to_string()
                } else {
                    "not found: installed versions are unknown".to_string()
                },
                if panel.lockfile.is_some() {
                    theme.foreground
                } else {
                    theme.warning
                },
            ))
            .child(info_row(cx, "rustc", rustc, rustc_color))
            .child(info_row(cx, "cargo", cargo, cargo_color))
            .child(status)
            .into_any_element()
    }
}

// ── The list pages ─────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Eq)]
enum ListKind {
    Installed,
    Updates,
    Advisories,
}

pub(super) struct ListPage {
    panel: WeakEntity<CargoManagerPanel>,
    kind: ListKind,
    focus_handle: FocusHandle,
    /// The row `up`/`down`/`enter`/`space` act on. `None` until the list
    /// first gets focus.
    selected: Option<usize>,
}

impl ListPage {
    fn row_count(&self, panel: &CargoManagerPanel) -> usize {
        match self.kind {
            ListKind::Installed => panel.dependencies.listed.len(),
            ListKind::Updates => panel.outdated_rows().len(),
            ListKind::Advisories => match &panel.advisories {
                AdvisoryState::Done { findings, .. } => findings.len(),
                _ => 0,
            },
        }
    }

    fn step(&mut self, forward: bool, cx: &mut Context<Self>) {
        let Some(view) = self.panel.upgrade() else {
            return;
        };
        let len = self.row_count(view.read(cx));
        self.selected = step_selected(self.selected, len, forward);
        cx.notify();
    }

    fn select_next(&mut self, _: &SelectNextPackage, _: &mut Window, cx: &mut Context<Self>) {
        self.step(true, cx);
    }

    fn select_prev(&mut self, _: &SelectPrevPackage, _: &mut Window, cx: &mut Context<Self>) {
        self.step(false, cx);
    }

    /// Enter: a dependency's details, or an advisory's page in the browser
    /// (a finding is usually about a transitive package, which has no
    /// details here).
    fn open_selected(&mut self, _: &OpenSelectedPackage, _: &mut Window, cx: &mut Context<Self>) {
        let (Some(view), Some(ix)) = (self.panel.upgrade(), self.selected) else {
            return;
        };
        let panel = view.read(cx);
        let package = match self.kind {
            ListKind::Installed => panel.dependencies.listed.get(ix).map(|row| row.name.clone()),
            ListKind::Updates => panel.outdated_rows().get(ix).map(|row| row.name.clone()),
            ListKind::Advisories => {
                if let AdvisoryState::Done { findings, .. } = &panel.advisories
                    && let Some(finding) = findings.get(ix)
                {
                    cx.open_url(&finding.url);
                }
                None
            }
        };
        if let Some(package) = package {
            view.update(cx, |panel, cx| panel.open_details(package, cx));
        }
    }

    /// Space: Remove on Installed, Update on Updates. Both confirm first.
    fn act_on_selected(
        &mut self,
        _: &ActSelectedPackage,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (Some(view), Some(ix)) = (self.panel.upgrade(), self.selected) else {
            return;
        };
        match self.kind {
            ListKind::Installed => {
                view.update(cx, |panel, cx| panel.remove_dependency(ix, window, cx));
            }
            ListKind::Updates => {
                let row = view.read(cx).outdated_rows().into_iter().nth(ix);
                if let Some(row) = row.filter(|row| row.in_range.is_some()) {
                    view.update(cx, |panel, cx| panel.update_dependency(&row.name, window, cx));
                }
            }
            ListKind::Advisories => {}
        }
    }

    /// The focusable container the rows go in.
    fn list(&self, id: &'static str, cx: &mut Context<Self>) -> Stateful<gpui::Div> {
        v_flex()
            .id(id)
            .track_focus(&self.focus_handle)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, window, cx| window.focus(&this.focus_handle, cx)),
            )
            .key_context(PACKAGE_LIST_CONTEXT)
            .on_action(cx.listener(Self::select_next))
            .on_action(cx.listener(Self::select_prev))
            .on_action(cx.listener(Self::open_selected))
            .on_action(cx.listener(Self::act_on_selected))
            .gap_1p5()
            .w_full()
    }

    /// One row's card: bordered, highlighted when selected, selected on
    /// click.
    fn card(&self, ix: usize, cx: &mut Context<Self>) -> Stateful<gpui::Div> {
        let is_selected = self.selected == Some(ix);
        let theme = cx.theme();
        v_flex()
            .id(("cargo-row", ix))
            .gap_1()
            .w_full()
            .p_2()
            .rounded_md()
            .border_1()
            .border_color(if is_selected {
                theme.primary
            } else {
                theme.border.opacity(0.6)
            })
            .when(is_selected, |card| card.bg(theme.primary.opacity(0.05)))
            .on_click(cx.listener(move |this, _, _, cx| {
                this.selected = Some(ix);
                cx.notify();
            }))
    }
}

/// A dependency's name, opening its details when clicked.
fn name_link(
    id: String,
    name: &str,
    view: &Entity<CargoManagerPanel>,
    cx: &App,
) -> Stateful<gpui::Div> {
    let package = name.to_string();
    let view = view.clone();
    div()
        .id(id)
        .text_sm()
        .font_semibold()
        .font_family("Cascadia Mono")
        .text_color(cx.theme().foreground)
        .cursor_pointer()
        .child(name.to_string())
        .on_click(move |_, _, cx| {
            view.update(cx, |panel, cx| panel.open_details(package.clone(), cx));
        })
}

impl Render for ListPage {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(view) = self.panel.upgrade() else {
            return div().into_any_element();
        };
        match self.kind {
            ListKind::Installed => self.render_installed(&view, cx).into_any_element(),
            ListKind::Updates => self.render_updates(&view, cx).into_any_element(),
            ListKind::Advisories => self.render_advisories(&view, cx).into_any_element(),
        }
    }
}

impl ListPage {
    fn render_installed(
        &self,
        view: &Entity<CargoManagerPanel>,
        cx: &mut Context<Self>,
    ) -> Stateful<gpui::Div> {
        let mut list = self.list("cargo-installed-list", cx);
        let panel = view.read(cx);
        let busy = panel.is_busy();

        if panel.dependencies.listed.is_empty() {
            list = list.child(muted(cx, "This crate has no crates.io dependencies."));
        }
        if panel.lockfile.is_none() && !panel.dependencies.listed.is_empty() {
            list = list.child(muted(cx, "No Cargo.lock yet: installed versions are unknown."));
        }

        let rows: Vec<_> = panel
            .dependencies
            .listed
            .iter()
            .map(|row| {
                let yanked = panel
                    .index
                    .get(&row.name)
                    .is_some_and(|cached| {
                        row.status(&cached.versions, panel.rustc_version.as_deref()).locked_yanked
                    });
                (row.clone(), panel.declaration_of(row), yanked)
            })
            .collect();
        let hidden = panel.dependencies.hidden;

        for (ix, (row, declared, yanked)) in rows.into_iter().enumerate() {
            let mut header = h_flex().gap_2().items_center().w_full().child(name_link(
                format!("cargo-installed-{ix}"),
                &row.name,
                view,
                cx,
            ));
            if row.kind != DependencyKind::Normal {
                header = header.child(Tag::secondary().xsmall().child(row.kind.label()));
            }
            if row.optional {
                header = header.child(Tag::secondary().xsmall().outline().child("optional"));
            }
            if let Some(target) = &row.target {
                header = header.child(Tag::secondary().xsmall().outline().child(target.clone()));
            }
            if declared == Some(Declaration::Inherited) {
                header = header.child(Tag::info().xsmall().outline().child("workspace"));
            }
            header = header.child(div().flex_1()).child(
                // Unknown stays unknown: never a guessed version.
                mono(row.locked_version.clone().unwrap_or_else(|| "unknown".into()))
                    .text_xs()
                    .text_color(cx.theme().foreground),
            );

            let mut card = self.card(ix, cx).child(header).child(
                h_flex()
                    .gap_1()
                    .items_center()
                    .child(muted(cx, "requires"))
                    .child(mono(row.requirement.clone()).text_xs().text_color(cx.theme().muted_foreground)),
            );
            if yanked {
                // An installed yanked version usually signals a bug or a
                // security problem (design decision 7).
                card = card.child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().warning)
                        .child("The locked version has been yanked from crates.io."),
                );
            }
            card = card.child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .w_full()
                    .child(
                        Button::new(("cargo-details", ix))
                            .ghost()
                            .xsmall()
                            .icon(IconName::Info)
                            .label("Versions")
                            .on_click({
                                let view = view.clone();
                                let package = row.name.clone();
                                move |_, _, cx| {
                                    view.update(cx, |panel, cx| {
                                        panel.open_details(package.clone(), cx);
                                    });
                                }
                            }),
                    )
                    .child(div().flex_1())
                    .child(
                        Button::new(("cargo-remove", ix))
                            .danger()
                            .label("Remove")
                            .with_size(Size::Small)
                            .disabled(busy)
                            .on_click({
                                let view = view.clone();
                                move |_, window, cx| {
                                    view.update(cx, |panel, cx| {
                                        panel.remove_dependency(ix, window, cx);
                                    });
                                }
                            }),
                    ),
            );
            list = list.child(card);
        }

        if hidden > 0 {
            // The omission is stated, not silent (design decision 10).
            list = list.child(muted(
                cx,
                format!("{hidden} local and git dependencies not shown."),
            ));
        }
        list
    }

    fn render_updates(
        &self,
        view: &Entity<CargoManagerPanel>,
        cx: &mut Context<Self>,
    ) -> Stateful<gpui::Div> {
        let mut list = self.list("cargo-updates-list", cx);
        let panel = view.read(cx);
        let busy = panel.is_busy();

        let failed = match panel.outdated {
            OutdatedState::Idle => return list.child(muted(cx, "Nothing to check.")),
            OutdatedState::Checking => {
                return list.child(busy_line(cx, "Checking crates.io\u{2026}"));
            }
            OutdatedState::Done { failed } => failed,
        };
        if failed > 0 {
            list = list.child(div().text_xs().text_color(cx.theme().warning).child(format!(
                "{failed} lookup(s) failed \u{2014} offline? Results below may be incomplete."
            )));
        }

        let rows: Vec<(OutdatedRow, Option<Declaration>, bool)> = panel
            .outdated_rows()
            .into_iter()
            .map(|row| {
                let listed = panel.listed(&row.name);
                let declared = listed.and_then(|listed| panel.declaration_of(listed));
                let targeted = listed.is_some_and(|listed| listed.target.is_some());
                (row, declared, targeted)
            })
            .collect();
        if rows.is_empty() {
            return list.child(muted(cx, "Everything checked is up to date."));
        }

        let in_range = rows.iter().filter(|(row, ..)| row.in_range.is_some()).count();
        list = list.child(
            h_flex()
                .gap_2()
                .items_center()
                .child(
                    Button::new("cargo-update-all")
                        .label(format!("Update all ({in_range})"))
                        .with_size(Size::Small)
                        .disabled(busy || in_range == 0)
                        .tooltip("One cargo update naming every dependency with a newer version inside its declared range")
                        .on_click({
                            let view = view.clone();
                            move |_, window, cx| {
                                view.update(cx, |panel, cx| panel.update_all(window, cx));
                            }
                        }),
                )
                .child(
                    Button::new("cargo-recheck")
                        .ghost()
                        .label("Check again")
                        .with_size(Size::Small)
                        .on_click({
                            let view = view.clone();
                            move |_, _, cx| {
                                view.update(cx, |panel, cx| {
                                    panel.index.clear();
                                    panel.check_outdated(cx);
                                    cx.notify();
                                });
                            }
                        }),
                ),
        );

        for (ix, (row, declared, targeted)) in rows.into_iter().enumerate() {
            let header = h_flex()
                .gap_2()
                .items_center()
                .w_full()
                .child(name_link(format!("cargo-outdated-{ix}"), &row.name, view, cx))
                .child(div().flex_1())
                .child(
                    mono(row.locked.clone().unwrap_or_else(|| "unknown".into()))
                        .text_xs()
                        .text_color(cx.theme().muted_foreground),
                );
            let mut card = self.card(ix, cx).child(header);
            if row.locked_yanked {
                card = card.child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().warning)
                        .child("The locked version has been yanked from crates.io."),
                );
            }

            // "Up to": the newest this crate's requirement allows. Another
            // package in the workspace can hold Cargo lower, which the
            // confirmation's dry run shows.
            if let Some(target) = &row.in_range {
                card = card.child(
                    target_line(cx, "up to", target).child(div().flex_1()).child(
                        Button::new(("cargo-update", ix))
                            .label("Update")
                            .with_size(Size::Small)
                            .disabled(busy)
                            .tooltip("cargo update: changes Cargo.lock only")
                            .on_click({
                                let view = view.clone();
                                let package = row.name.clone();
                                move |_, window, cx| {
                                    view.update(cx, |panel, cx| {
                                        panel.update_dependency(&package, window, cx);
                                    });
                                }
                            }),
                    ),
                );
            }
            if let Some(target) = &row.out_of_range {
                let version = target.version.to_string();
                let line = target_line(cx, "newer", target).child(div().flex_1());
                card = card.child(match (declared, targeted) {
                    (Some(Declaration::Literal), false) => line.child(
                        Button::new(("cargo-change", ix))
                            .label(format!("Change to {version}"))
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
                    ),
                    // No Cargo command edits `[workspace.dependencies]`, so
                    // there is a note here instead of a button.
                    (Some(Declaration::Inherited), _) => {
                        line.child(muted(cx, "set in the root Cargo.toml"))
                    }
                    (Some(Declaration::Literal), true) => {
                        line.child(muted(cx, "target-specific: edit Cargo.toml"))
                    }
                    (None, _) => line.child(muted(cx, "needs the requirement changed")),
                });
            }
            list = list.child(card);
        }
        list
    }

    fn render_advisories(
        &self,
        view: &Entity<CargoManagerPanel>,
        cx: &mut Context<Self>,
    ) -> Stateful<gpui::Div> {
        let mut list = self.list("cargo-advisories-list", cx);
        let panel = view.read(cx);

        let scanning = matches!(panel.advisories, AdvisoryState::Scanning);
        list = list.child(
            h_flex().child(
                Button::new("cargo-advisory-scan")
                    .label(match panel.advisories {
                        AdvisoryState::NotScanned => "Scan",
                        _ => "Scan again",
                    })
                    .with_size(Size::Small)
                    .disabled(scanning)
                    .tooltip("Ask OSV.dev about every crates.io package this crate pulls in")
                    .on_click({
                        let view = view.clone();
                        move |_, _, cx| view.update(cx, |panel, cx| panel.scan_advisories(cx))
                    }),
            ),
        );

        let (findings, scanned, truncated) = match &panel.advisories {
            AdvisoryState::NotScanned => return list.child(muted(cx, "Not scanned yet.")),
            AdvisoryState::Scanning => return list.child(busy_line(cx, "Asking OSV.dev\u{2026}")),
            AdvisoryState::Failed(error) => {
                return list.child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().danger)
                        .child(error.clone()),
                );
            }
            AdvisoryState::Done {
                findings,
                scanned,
                truncated,
            } => (findings.clone(), *scanned, *truncated),
        };

        if findings.is_empty() {
            list = list.child(muted(
                cx,
                format!("No advisories for the {scanned} packages checked."),
            ));
        } else {
            list = list.child(muted(cx, counts_line(FindingCounts::of(&findings))));
        }

        let mut heading = None;
        for (ix, finding) in findings.iter().enumerate() {
            if heading != Some(finding.kind) {
                heading = Some(finding.kind);
                list = list.child(
                    div()
                        .pt_1()
                        .text_xs()
                        .font_semibold()
                        .text_color(cx.theme().foreground)
                        .child(match finding.kind {
                            FindingKind::Vulnerability => "Vulnerabilities",
                            FindingKind::Unsound => "Unsound",
                            FindingKind::Unmaintained => "Unmaintained",
                            FindingKind::Notice => "Notices",
                        }),
                );
            }

            let url = finding.url.clone();
            let mut head = h_flex().gap_2().items_center().w_full();
            if let Some(tag) = severity_tag(finding) {
                head = head.child(tag);
            }
            head = head
                .child(
                    mono(format!("{} {}", finding.package, finding.version))
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_xs()
                        .text_color(cx.theme().foreground),
                )
                .child(
                    mono(finding.id.clone())
                        .id(("cargo-advisory-link", ix))
                        .text_xs()
                        .text_color(cx.theme().primary)
                        .cursor_pointer()
                        .on_click(move |_, _, cx| cx.open_url(&url)),
                );

            let mut card = self.card(ix, cx).child(head);
            if let Some(summary) = &finding.summary {
                card = card.child(muted(cx, summary.clone()));
            }
            if finding.details_missing {
                card = card.child(
                    div()
                        .text_xs()
                        .text_color(cx.theme().warning)
                        .child("Details could not be fetched."),
                );
            }
            if !finding.fixed_in.is_empty() {
                card = card.child(muted(cx, format!("Fixed in {}", finding.fixed_in.join(", "))));
            }
            list = list.child(card);
        }

        if truncated {
            list = list.child(div().text_xs().text_color(cx.theme().warning).child(
                "Some packages had more advisories than were returned; this list is incomplete.",
            ));
        }
        // The scan reads the lockfile, which covers every platform.
        list.child(muted(
            cx,
            format!(
                "Checked {scanned} packages from Cargo.lock, for every platform. A finding may \
                 concern a package that is never built here. Local and git dependencies are \
                 not checked."
            ),
        ))
    }
}

/// `up to 1.2.3 [patch] [needs Rust 1.99]` — one update target.
fn target_line(cx: &App, prefix: &str, target: &UpdateTarget) -> gpui::Div {
    let mut line = h_flex()
        .gap_1()
        .items_center()
        .w_full()
        .child(muted(cx, prefix.to_string()))
        .child(
            mono(target.version.to_string())
                .text_xs()
                .text_color(cx.theme().foreground),
        )
        .child(Tag::secondary().xsmall().child(target.kind.label()));
    if let Some(tag) = rust_tag(&target.rust) {
        line = line.child(tag);
    }
    line
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn step_selected_starts_at_the_nearest_end() {
        assert_eq!(step_selected(None, 3, true), Some(0));
        assert_eq!(step_selected(None, 3, false), Some(2));
    }

    #[test]
    fn step_selected_wraps_at_both_ends() {
        assert_eq!(step_selected(Some(0), 3, true), Some(1));
        assert_eq!(step_selected(Some(2), 3, true), Some(0));
        assert_eq!(step_selected(Some(0), 3, false), Some(2));
        assert_eq!(step_selected(Some(0), 1, true), Some(0));
    }

    #[test]
    fn step_selected_handles_empty_and_shrunken_lists() {
        assert_eq!(step_selected(None, 0, true), None);
        assert_eq!(step_selected(Some(4), 0, false), None);
        // A selection left over from a longer list still lands in range.
        assert!(step_selected(Some(9), 3, true).is_some_and(|ix| ix < 3));
        assert!(step_selected(Some(9), 3, false).is_some_and(|ix| ix < 3));
    }

    #[test]
    fn counts_keep_each_kind_apart() {
        let counts = FindingCounts {
            vulnerabilities: 26,
            unsound: 5,
            unmaintained: 10,
            notices: 0,
        };
        assert_eq!(
            counts_line(counts),
            "26 vulnerabilities \u{00b7} 5 unsound \u{00b7} 10 unmaintained \u{00b7} 0 other notices"
        );
        let one = FindingCounts {
            vulnerabilities: 1,
            unsound: 0,
            unmaintained: 0,
            notices: 1,
        };
        assert!(counts_line(one).starts_with("1 vulnerability "));
        assert!(counts_line(one).ends_with("1 other notice"));
    }
}
