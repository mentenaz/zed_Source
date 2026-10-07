//! The Search page: finding a crate on crates.io and adding it, and reading
//! a crate's README.
//!
//! Adding is the one action whose effect depends on a file the crate does
//! not own. If the root manifest's `[workspace.dependencies]` has the
//! package, Cargo writes `name.workspace = true`; if that table exists
//! without it, Cargo writes a literal version into the crate, which is not
//! how such a workspace declares dependencies. The confirmation says which
//! of the two will happen before anything runs (design decision 5).

use std::time::{Duration, Instant};

use cargo_backend::{
    AddEffect, DependencyKind, SearchResult, add_args, add_effect, check_crate_name,
    command_line, fmt_count, next_search_url, search_url,
};
use gpui::{
    AnyElement, AppContext as _, AsyncApp, Context, Entity, InteractiveElement as _, IntoElement,
    ParentElement as _, PromptLevel, Render, StatefulInteractiveElement as _, Styled as _,
    Subscription, Task, WeakEntity, Window, div,
};
use gpui_component::{
    ActiveTheme as _, Disableable as _, IconName, Sizable as _, Size, StyledExt as _,
    button::{Button, ButtonVariants as _},
    h_flex,
    input::{Input, InputEvent, InputState},
    spinner::Spinner,
    switch::Switch,
    tag::Tag,
    v_flex,
};
use script_runner_panel::command::{check_package_name, check_version};

use crate::CargoManagerPanel;
use crate::registry::{fetch_index_entries, fetch_readme, fetch_search, store_index_entries};

/// The least time between two search requests. crates.io asks API users to
/// keep to about one request a second.
const SEARCH_INTERVAL: Duration = Duration::from_secs(1);

/// Where the README shown in the details pane stands.
pub(crate) enum Readme {
    /// The pane shows the details, not a README.
    Closed,
    Loading,
    /// The README as rendered HTML, which is how crates.io serves it.
    Loaded(String),
    Failed(String),
}

pub(crate) struct SearchState {
    pub(crate) input: Entity<InputState>,
    pub(crate) results: Vec<SearchResult>,
    /// How many crates match in all; `results` holds the pages loaded so far.
    pub(crate) total: usize,
    next_page: Option<String>,
    pub(crate) loading: bool,
    pub(crate) error: Option<String>,
    /// A search has finished, so an empty list means "nothing found" rather
    /// than "nothing asked yet".
    pub(crate) searched: bool,
    /// Add to `[dev-dependencies]` instead of `[dependencies]`.
    pub(crate) as_dev: bool,
    last_request: Option<Instant>,
    task: Option<Task<()>>,
    _subscription: Subscription,
}

impl SearchState {
    pub(crate) fn new(window: &mut Window, cx: &mut Context<CargoManagerPanel>) -> Self {
        let input =
            cx.new(|cx| InputState::new(window, cx).placeholder("Search crates.io for a crate\u{2026}"));
        let subscription = cx.subscribe(
            &input,
            |this: &mut CargoManagerPanel, _, event: &InputEvent, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    this.search(false, cx);
                }
            },
        );
        SearchState {
            input,
            results: Vec::new(),
            total: 0,
            next_page: None,
            loading: false,
            error: None,
            searched: false,
            as_dev: false,
            last_request: None,
            task: None,
            _subscription: subscription,
        }
    }

    pub(crate) fn has_more(&self) -> bool {
        self.next_page.is_some()
    }

    pub(crate) fn result(&self, package: &str) -> Option<&SearchResult> {
        self.results.iter().find(|result| result.name == package)
    }
}

// ── Pure helpers ───────────────────────────────────────────────────────

/// `cargo add <name>[@<version>] -p <crate>`, with `--dev` or `--build` for
/// the other tables. Validated twice over, like the tab's other commands:
/// by `cargo_backend`, and by `script_runner_panel::command` (design rule
/// 10).
pub(crate) fn add_command(
    name: &str,
    version: Option<&str>,
    member: &str,
    kind: DependencyKind,
) -> Result<String, String> {
    check_package_name(name)?;
    check_package_name(member)?;
    if let Some(version) = version {
        check_version(version)?;
    }
    Ok(command_line(&add_args(name, version, member, kind)?))
}

/// The version to pass to `cargo add`, given what adding will do.
///
/// When the workspace already declares the package, no version is passed:
/// with one, Cargo writes that literal version into the crate instead of
/// `workspace = true`, which is the opposite of what the confirmation
/// promises.
fn version_to_add(effect: Option<AddEffect>, version: Option<&str>) -> Option<&str> {
    match effect {
        Some(AddEffect::Inherits) => None,
        _ => version,
    }
}

/// The text of an add's confirmation, and whether it is a warning.
///
/// `effect` is `None` when the root manifest could not be read; the text
/// then says what is unknown instead of promising either outcome.
fn describe_add(member: &str, package: &str, effect: Option<AddEffect>) -> (bool, String) {
    let tail = format!("This edits {member}'s Cargo.toml and Cargo.lock. Nothing is compiled.");
    match effect {
        Some(AddEffect::Inherits) => (
            false,
            format!(
                "The workspace already declares {package}, so Cargo writes \
                 `{package}.workspace = true` and {member} uses the workspace's version.\n\n{tail}"
            ),
        ),
        Some(AddEffect::LiteralOutsideWorkspaceTable) => (
            true,
            format!(
                "This workspace keeps its dependencies' versions in [workspace.dependencies] \
                 in the root Cargo.toml, and {package} is not there. Cargo will write a version \
                 into {member}'s own Cargo.toml instead.\n\nTo follow the workspace's \
                 convention, cancel, add {package} to the root manifest by hand, and add it \
                 again here.\n\n{tail}"
            ),
        ),
        Some(AddEffect::Literal) => (false, tail),
        None => (
            true,
            format!(
                "The root Cargo.toml could not be read, so it is not known whether Cargo will \
                 write `workspace = true` or a version of its own into {member}'s manifest.\
                 \n\n{tail}"
            ),
        ),
    }
}

// ── The panel's side ───────────────────────────────────────────────────

impl CargoManagerPanel {
    /// Searches crates.io for what is in the search box, or with `more`
    /// loads the next page of the current results.
    pub(crate) fn search(&mut self, more: bool, cx: &mut Context<Self>) {
        let url = if more {
            let Some(url) = self.search.next_page.as_deref().and_then(next_search_url) else {
                return;
            };
            url
        } else {
            let query = self.search.input.read(cx).value().trim().to_string();
            if query.is_empty() {
                return;
            }
            search_url(&query)
        };
        if self.search.loading {
            return;
        }

        // Keep to the request rate crates.io asks for, by waiting out the
        // rest of the interval rather than dropping the search.
        let wait = self
            .search
            .last_request
            .map(|last| SEARCH_INTERVAL.saturating_sub(last.elapsed()))
            .unwrap_or_default();
        self.search.loading = true;
        self.search.error = None;
        cx.notify();

        let client = cx.http_client();
        self.search.task = Some(cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            if !wait.is_zero() {
                cx.background_executor().timer(wait).await;
            }
            let page = fetch_search(&client, &url).await;
            this.update(cx, |this, cx| {
                let search = &mut this.search;
                search.last_request = Some(Instant::now());
                search.loading = false;
                search.searched = true;
                match page {
                    Ok(page) => {
                        if more {
                            search.results.extend(page.results);
                        } else {
                            search.results = page.results;
                        }
                        search.total = page.total;
                        search.next_page = page.next_page;
                    }
                    Err(error) => {
                        search.error = Some(format!("Could not search crates.io: {error}"));
                    }
                }
                cx.notify();
            })
            .ok();
        }));
    }

    /// Opens the details pane on `package`. For a crate that is not a
    /// dependency yet, its versions are looked up in the index first, so the
    /// pane can show each one's minimum Rust version before anything is
    /// added.
    pub(crate) fn open_details(&mut self, package: String, cx: &mut Context<Self>) {
        self.readme = Readme::Closed;
        self.readme_task = None;
        if !self.index.contains_key(&package) && check_crate_name(&package).is_ok() {
            let client = cx.http_client();
            let names = vec![package.clone()];
            self.details_task = Some(cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
                let results = fetch_index_entries(&client, names).await;
                this.update(cx, |this, cx| {
                    store_index_entries(&mut this.index, results);
                    cx.notify();
                })
                .ok();
            }));
        }
        self.selected = Some(package);
        cx.notify();
    }

    pub(crate) fn close_details(&mut self, cx: &mut Context<Self>) {
        self.selected = None;
        self.readme = Readme::Closed;
        self.readme_task = None;
        cx.notify();
    }

    /// Fetches the README of `package` at `version` and shows it in the
    /// details pane.
    pub(crate) fn open_readme(&mut self, package: String, version: String, cx: &mut Context<Self>) {
        self.readme = Readme::Loading;
        cx.notify();
        let client = cx.http_client();
        self.readme_task = Some(cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let readme = fetch_readme(&client, &package, &version).await;
            this.update(cx, |this, cx| {
                this.readme = match readme {
                    Ok(html) if !html.trim().is_empty() => Readme::Loaded(html),
                    Ok(_) => Readme::Failed(format!("{package} {version} has no README.")),
                    Err(error) => Readme::Failed(format!("Could not load the README: {error}")),
                };
                cx.notify();
            })
            .ok();
        }));
    }

    pub(crate) fn close_readme(&mut self, cx: &mut Context<Self>) {
        self.readme = Readme::Closed;
        self.readme_task = None;
        cx.notify();
    }

    /// Adds `package` to the crate (`cargo add`), after a confirmation that
    /// says what Cargo will write. With no `version` Cargo picks the newest.
    pub(crate) fn add_dependency(
        &mut self,
        package: &str,
        version: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.is_busy() {
            return;
        }
        let Some(member) = self.crate_name.clone() else {
            return;
        };
        let kind = if self.search.as_dev {
            DependencyKind::Dev
        } else {
            DependencyKind::Normal
        };
        // `cargo add` on a dependency the crate already has rewrites it
        // instead; that is the Updates page's job, with its own checks.
        if self
            .dependencies
            .listed
            .iter()
            .any(|row| row.name == package && row.kind == kind)
        {
            return self.reject(
                format!(
                    "{member} already depends on {package}. To change its version, use the \
                     Updates page or the version list."
                ),
                cx,
            );
        }

        let effect = self.root_manifest.as_ref().map(|root| add_effect(root, package));
        let command = match add_command(package, version_to_add(effect, version), &member, kind) {
            Ok(command) => command,
            Err(error) => return self.reject(error, cx),
        };
        let (warn, detail) = describe_add(&member, package, effect);

        let answer = window.prompt(
            if warn {
                PromptLevel::Warning
            } else {
                PromptLevel::Info
            },
            &format!("Add {package} to {member}?"),
            Some(&format!("Runs: {command}\n\n{detail}")),
            &[if warn { "Add anyway" } else { "Add" }, "Cancel"],
            cx,
        );
        let label = format!("add {package}");
        self.confirm_task = Some(cx.spawn_in(window, async move |this, cx| {
            if answer.await != Ok(0) {
                return;
            }
            this.update_in(cx, |this, window, cx| this.kick_run(&label, command, window, cx))
                .ok();
        }));
    }
}

// ── The page ───────────────────────────────────────────────────────────

pub(crate) struct SearchView {
    pub(crate) panel: WeakEntity<CargoManagerPanel>,
}

impl Render for SearchView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(view) = self.panel.upgrade() else {
            return div().into_any_element();
        };
        let panel = view.read(cx);
        let theme = cx.theme();
        let search = &panel.search;
        let busy = panel.is_busy();
        let muted = |text: String| div().text_xs().text_color(theme.muted_foreground).child(text);

        let controls = v_flex()
            .gap_2()
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(Input::new(&search.input).xsmall().w_full()),
                    )
                    .child(
                        Button::new("cargo-search-go")
                            .icon(IconName::Search)
                            .label("Search")
                            .with_size(Size::Small)
                            .disabled(search.loading)
                            .on_click({
                                let view = view.clone();
                                move |_, _, cx| view.update(cx, |panel, cx| panel.search(false, cx))
                            }),
                    ),
            )
            .child(
                Switch::new("cargo-add-as-dev")
                    .checked(search.as_dev)
                    .label("Add as a dev-dependency")
                    .with_size(Size::Small)
                    .on_click({
                        let view = view.clone();
                        move |checked, _, cx| {
                            let checked = *checked;
                            view.update(cx, |panel, cx| {
                                panel.search.as_dev = checked;
                                cx.notify();
                            });
                        }
                    }),
            );

        let mut list = v_flex().gap_1p5().w_full();
        if let Some(error) = &search.error {
            list = list.child(div().text_xs().text_color(theme.danger).child(error.clone()));
        }
        if search.loading && search.results.is_empty() {
            list = list.child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .child(Spinner::new().with_size(Size::XSmall))
                    .child(muted("Searching crates.io\u{2026}".to_string())),
            );
        } else if search.results.is_empty() && search.error.is_none() {
            list = list.child(muted(if search.searched {
                "No crate matches.".to_string()
            } else {
                "Search crates.io for a crate to add.".to_string()
            }));
        } else if !search.results.is_empty() {
            list = list.child(muted(format!(
                "{} of {} crates",
                search.results.len(),
                search.total.max(search.results.len())
            )));
        }

        for (ix, result) in search.results.iter().enumerate() {
            let installed = panel.listed(&result.name).is_some();
            let mut header = h_flex()
                .gap_2()
                .items_center()
                .w_full()
                .child(
                    div()
                        .id(("cargo-result-name", ix))
                        .text_sm()
                        .font_semibold()
                        .font_family("Cascadia Mono")
                        .text_color(theme.foreground)
                        .cursor_pointer()
                        .child(result.name.clone())
                        .on_click({
                            let view = view.clone();
                            let package = result.name.clone();
                            move |_, _, cx| {
                                view.update(cx, |panel, cx| panel.open_details(package.clone(), cx));
                            }
                        }),
                )
                .child(Tag::secondary().xsmall().child(result.version.clone()));
            if result.exact_match {
                header = header.child(Tag::info().xsmall().outline().child("exact match"));
            }
            if installed {
                header = header.child(Tag::success().xsmall().outline().child("installed"));
            }
            header = header.child(div().flex_1()).child(muted(match result.recent_downloads {
                Some(recent) => format!(
                    "{} downloads \u{00b7} {} recent",
                    fmt_count(result.downloads),
                    fmt_count(recent)
                ),
                None => format!("{} downloads", fmt_count(result.downloads)),
            }));

            let card = v_flex()
                .gap_1()
                .w_full()
                .p_2()
                .rounded_md()
                .border_1()
                .border_color(theme.border.opacity(0.6))
                .child(header)
                .child(muted(
                    result
                        .description
                        .clone()
                        .unwrap_or_else(|| "No description.".to_string()),
                ))
                .child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .w_full()
                        .child(
                            Button::new(("cargo-result-details", ix))
                                .ghost()
                                .xsmall()
                                .icon(IconName::Info)
                                .label("Details")
                                .on_click({
                                    let view = view.clone();
                                    let package = result.name.clone();
                                    move |_, _, cx| {
                                        view.update(cx, |panel, cx| {
                                            panel.open_details(package.clone(), cx);
                                        });
                                    }
                                }),
                        )
                        .child(div().flex_1())
                        .child(
                            Button::new(("cargo-result-add", ix))
                                .label("Add")
                                .with_size(Size::Small)
                                .disabled(busy)
                                .tooltip("cargo add: Cargo picks the newest version")
                                .on_click({
                                    let view = view.clone();
                                    let package = result.name.clone();
                                    move |_, window, cx| {
                                        view.update(cx, |panel, cx| {
                                            panel.add_dependency(&package, None, window, cx);
                                        });
                                    }
                                }),
                        ),
                );
            list = list.child(card);
        }

        if search.has_more() && !search.results.is_empty() {
            list = list.child(
                h_flex().child(
                    Button::new("cargo-search-more")
                        .ghost()
                        .label(if search.loading {
                            "Loading\u{2026}"
                        } else {
                            "Load more"
                        })
                        .with_size(Size::Small)
                        .disabled(search.loading)
                        .on_click({
                            let view = view.clone();
                            move |_, _, cx| view.update(cx, |panel, cx| panel.search(true, cx))
                        }),
                ),
            );
        }

        let page: AnyElement = v_flex().gap_2().child(controls).child(list).into_any_element();
        page
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_commands_name_the_crate_and_the_table() {
        assert_eq!(
            add_command("serde", None, "app", DependencyKind::Normal).as_deref(),
            Ok("cargo add serde -p app")
        );
        assert_eq!(
            add_command("tempfile", None, "app", DependencyKind::Dev).as_deref(),
            Ok("cargo add tempfile -p app --dev")
        );
        assert_eq!(
            add_command("serde", Some("1.0.229"), "app", DependencyKind::Normal).as_deref(),
            Ok("cargo add serde@1.0.229 -p app")
        );
    }

    #[test]
    fn add_commands_refuse_shell_syntax() {
        assert!(add_command("serde; calc", None, "app", DependencyKind::Normal).is_err());
        assert!(add_command("serde", None, "app | more", DependencyKind::Normal).is_err());
        assert!(add_command("serde", Some("1.0.0 && calc"), "app", DependencyKind::Normal).is_err());
        assert!(add_command("--git", None, "app", DependencyKind::Normal).is_err());
        assert!(add_command("", None, "app", DependencyKind::Normal).is_err());
    }

    #[test]
    fn no_version_is_passed_for_a_package_the_workspace_declares() {
        // With a version, Cargo would write it into the crate instead of
        // `workspace = true`.
        assert_eq!(version_to_add(Some(AddEffect::Inherits), Some("1.0.0")), None);
        assert_eq!(
            version_to_add(Some(AddEffect::Literal), Some("1.0.0")),
            Some("1.0.0")
        );
        assert_eq!(
            version_to_add(Some(AddEffect::LiteralOutsideWorkspaceTable), Some("1.0.0")),
            Some("1.0.0")
        );
        assert_eq!(version_to_add(None, Some("1.0.0")), Some("1.0.0"));
        assert_eq!(version_to_add(Some(AddEffect::Literal), None), None);
    }

    #[test]
    fn an_add_says_what_cargo_will_write() {
        let (warn, text) = describe_add("app", "serde", Some(AddEffect::Inherits));
        assert!(!warn);
        assert!(text.contains("serde.workspace = true"), "{text}");

        // Decision 5: warn and ask before leaving the workspace's convention.
        let (warn, text) =
            describe_add("app", "tokio", Some(AddEffect::LiteralOutsideWorkspaceTable));
        assert!(warn);
        assert!(text.contains("[workspace.dependencies]"), "{text}");
        assert!(text.contains("tokio is not there"), "{text}");

        let (warn, text) = describe_add("app", "tokio", Some(AddEffect::Literal));
        assert!(!warn);
        assert!(text.contains("app's Cargo.toml"), "{text}");

        // Unknown is said out loud, and treated as a warning.
        let (warn, text) = describe_add("app", "tokio", None);
        assert!(warn);
        assert!(text.contains("could not be read"), "{text}");
    }
}
