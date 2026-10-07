//! Cargo manager — the workspace tab opened from the Rust panel's "Package
//! Manager" quick action, for one crate's crates.io dependencies: what is
//! installed, what is behind the registry, which advisories apply, and
//! adding, removing, updating and re-versioning them through Cargo.
//!
//! Not a Forge port: written for this tree from
//! `docs/Rust_Manager_Design_Note.md`, in the shape of the other manager
//! tabs (`nuget_manager_panel` is the closest sibling). It is a workspace
//! tab (an [`Item`]), not a dock panel. The working-out lives in
//! `cargo_backend`; this crate is the UI, the HTTP requests that crate
//! leaves to its host ([`registry`]), and the wiring to the Script Runner.
//!
//! The UI follows the settings-page layout from `gpui_component::setting`
//! (`SettingPage`s rendered by a `Settings` widget), with a resizable
//! details pane on the right while a dependency is selected.
//!
//! Rules from the design note that shape this file:
//!
//! - **Nothing depends on rust-analyzer.** Everything is read from Cargo's
//!   own files and one cheap command, and refreshed by re-reading them when
//!   a `Cargo.toml` or `Cargo.lock` changes on disk. Nothing polls.
//! - **One Cargo action at a time**, and each one that changes a file is
//!   confirmed first, with what it will change spelled out: an update shows
//!   Cargo's own dry run, a removal says when the root manifest is edited
//!   too.
//! - **A dependency inherited from the workspace is never re-versioned from
//!   here.** `cargo add name@version` would write a literal version into
//!   the member and leave the root manifest alone; see
//!   `cargo_backend::declaration`.
//! - **The advisory scan is a button**, and until it has run the page says
//!   so. Unknown never looks like healthy.

use std::time::Instant;

use cargo_backend::{
    CrateInfo, Declaration, DependencyKind, DependencyList, FindingCounts, ListedDependency,
    LockChange, LockChangeKind, LockedPackage, Lockfile, Manifest, RemoveEffect, UpdateSpec,
    command_line, declaration, direct_dependencies, has_lockfile, is_cargo_project,
    load_workspace, merge_findings, query_cargo, query_rustc, read_lockfile,
    read_member_manifests, read_root_manifest, remove_args, remove_effect, update_args,
    update_dry_run,
};
use gpui::{
    App, AppContext as _, AsyncApp, Context, Entity, EventEmitter, FocusHandle, Focusable,
    InteractiveElement as _, IntoElement, KeyBinding, ParentElement as _, PromptLevel, Render,
    Styled as _, Subscription, Task, WeakEntity, Window, actions, div, px,
};
use gpui_component::{
    ActiveTheme as _, IconName, Sizable as _, Size, StyledExt as _,
    button::Button,
    h_flex,
    resizable::{h_resizable, resizable_panel},
    setting::Settings,
    spinner::Spinner,
    v_flex,
};
use project::Project;
use script_runner_panel::ScriptRunnerPanel;
use script_runner_panel::command::{check_package_name, check_version};
use search::{Readme, SearchState, add_command};
use workspace::{Item, ItemId, SerializableItem, Workspace, WorkspaceId};

mod details;
mod pages;
pub mod registry;
mod search;

use registry::{
    AdvisoryRecords, AdvisoryState, IndexCache, OutdatedRow, OutdatedState, ScanResult,
    SharedScans, fetch_index_entries, outdated_rows, publish_scan, run_advisory_scan, scan_key,
    shared_scan, stale_names, store_index_entries,
};

actions!(
    cargo_manager,
    [
        /// Opens the Cargo manager tab for the workspace's default crate.
        OpenCargoManager,
        /// Re-reads the workspace and lockfile in the active Cargo manager
        /// tab.
        ReloadCargoManager,
        /// Moves the selection down one row in whichever list page
        /// (Installed/Updates/Vulnerabilities) currently has focus.
        SelectNextPackage,
        /// Moves the selection up one row, same scope as `SelectNextPackage`.
        SelectPrevPackage,
        /// Opens the selected row: a dependency's details, or an advisory's
        /// page in the browser.
        OpenSelectedPackage,
        /// Runs the selected row's primary action for the page it's on:
        /// Remove (Installed) or Update (Updates). Both ask first. The
        /// Vulnerabilities page has no per-row action.
        ActSelectedPackage
    ]
);

/// Opens (or activates) the Cargo manager tab in `workspace`, for the Cargo
/// workspace at `root`. With a `crate_name` the tab switches to that crate;
/// without one it keeps the crate it has, or starts on the workspace's
/// default. This is the seam the Rust panel's quick action calls.
pub fn open(
    root: String,
    crate_name: Option<String>,
    workspace: &mut Workspace,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) -> Entity<CargoManagerPanel> {
    let existing = workspace
        .active_pane()
        .read(cx)
        .items()
        .find_map(|item| item.downcast::<CargoManagerPanel>());

    if let Some(existing) = existing {
        workspace.activate_item(&existing, true, true, window, cx);
        if let Some(name) = crate_name {
            existing.update(cx, |panel, cx| panel.show_crate(name, cx));
        }
        return existing;
    }

    let project = workspace.project().clone();
    let handle = workspace.weak_handle();
    let panel = cx.new(|cx| CargoManagerPanel::new(root, crate_name, handle, project, window, cx));
    workspace.add_item_to_active_pane(Box::new(panel.clone()), None, true, window, cx);
    panel
}

pub fn init(cx: &mut App) {
    workspace::register_serializable_item::<CargoManagerPanel>(cx);
    cx.observe_new(|workspace: &mut Workspace, _, _cx| {
        workspace.register_action(|workspace, _: &OpenCargoManager, window, cx| {
            let root = first_worktree_root(workspace.project(), cx);
            if !root.is_empty() {
                open(root, None, workspace, window, cx);
            }
        });
        workspace.register_action(|workspace, _: &ReloadCargoManager, _window, cx| {
            if let Some(panel) = workspace
                .active_pane()
                .read(cx)
                .active_item()
                .and_then(|item| item.downcast::<CargoManagerPanel>())
            {
                panel.update(cx, |panel, cx| panel.reload(cx));
            }
        });
    })
    .detach();

    // Scoped to `CargoPackageList` (each list page sets that key context on
    // its own container), the way the other manager tabs scope theirs.
    cx.bind_keys([
        KeyBinding::new("down", SelectNextPackage, Some(pages::PACKAGE_LIST_CONTEXT)),
        KeyBinding::new("up", SelectPrevPackage, Some(pages::PACKAGE_LIST_CONTEXT)),
        KeyBinding::new("enter", OpenSelectedPackage, Some(pages::PACKAGE_LIST_CONTEXT)),
        KeyBinding::new("space", ActSelectedPackage, Some(pages::PACKAGE_LIST_CONTEXT)),
    ]);
}

fn first_worktree_root(project: &Entity<Project>, cx: &App) -> String {
    project
        .read(cx)
        .worktrees(cx)
        .next()
        .map(|worktree| worktree.read(cx).abs_path().to_string_lossy().into_owned())
        .unwrap_or_default()
}

// ── Pure helpers ───────────────────────────────────────────────────────

/// Whether a changed file is one this tab reads — the trigger for reloading
/// without polling.
fn is_manifest_file(file_name: &str) -> bool {
    file_name == cargo_backend::MANIFEST_FILE || file_name == cargo_backend::LOCK_FILE
}

/// `cargo remove <name> -p <crate>`, with the flags for the table the
/// dependency is in.
///
/// `cargo_backend` validates what it builds; the names also go through
/// `script_runner_panel::command`, like every other value this fork places
/// into a Script Runner command (design rule 10).
fn remove_command(
    name: &str,
    member: &str,
    kind: DependencyKind,
    target: Option<&str>,
) -> Result<String, String> {
    check_package_name(name)?;
    check_package_name(member)?;
    Ok(command_line(&remove_args(name, member, kind, target)?))
}

/// `cargo add <name>@<version> -p <crate>`: rewrites the requirement of a
/// dependency the crate already has.
fn change_version_command(
    name: &str,
    version: &str,
    member: &str,
    kind: DependencyKind,
) -> Result<String, String> {
    add_command(name, Some(version), member, kind)
}

/// `cargo update a@1.0.0 b@2.1.0`. `Ok(None)` with nothing to update: there
/// is no such thing here as a bare `cargo update`.
fn update_command(specs: &[UpdateSpec]) -> Result<Option<String>, String> {
    for spec in specs {
        check_package_name(&spec.name)?;
        if let Some(locked) = &spec.locked {
            check_version(locked)?;
        }
    }
    Ok(update_args(specs, false)?.map(|args| command_line(&args)))
}

/// The packages an "Update all" covers: every row with a newer version
/// inside its declared range. Rows that need the requirement changed are
/// never included.
fn update_all_specs(rows: &[OutdatedRow]) -> Vec<UpdateSpec> {
    rows.iter()
        .filter(|row| row.in_range.is_some())
        .map(|row| UpdateSpec::new(&row.name, row.locked.as_deref()))
        .collect()
}

/// How many lockfile changes a confirmation lists before summarizing.
const LISTED_CHANGES: usize = 15;

/// Cargo's dry run as the text of a confirmation: one line per lockfile
/// entry that would change.
fn describe_changes(changes: &[LockChange]) -> String {
    let mut lines: Vec<String> = changes
        .iter()
        .take(LISTED_CHANGES)
        .map(|change| {
            let from = change.from.as_deref().unwrap_or_default();
            let to = change.to.as_deref().unwrap_or_default();
            match change.kind {
                LockChangeKind::Update => format!("{}  {from} \u{2192} {to}", change.name),
                LockChangeKind::Downgrade => {
                    format!("{}  {from} \u{2192} {to} (downgrade)", change.name)
                }
                LockChangeKind::Add => format!("{}  {to} (added)", change.name),
                LockChangeKind::Remove => format!("{}  {from} (removed)", change.name),
            }
        })
        .collect();
    if changes.len() > LISTED_CHANGES {
        lines.push(format!("and {} more", changes.len() - LISTED_CHANGES));
    }
    lines.push(String::new());
    lines.push("Only Cargo.lock changes. Nothing is compiled.".to_string());
    lines.join("\n")
}

/// The text of a removal's confirmation. `effect` is `None` when the
/// manifests could not be read, in which case the text says the root
/// manifest might change rather than staying silent about it.
fn describe_removal(member: &str, package: &str, effect: Option<RemoveEffect>) -> String {
    let mut text = format!("This edits {member}'s Cargo.toml and Cargo.lock. Nothing is compiled.");
    match effect {
        Some(effect) if effect.removes_workspace_entry => text.push_str(&format!(
            "\n\nIt also removes {package} from [workspace.dependencies] in the root \
             Cargo.toml, because no other crate in the workspace uses it."
        )),
        Some(_) => {}
        None => text.push_str(
            "\n\nThe workspace's manifests could not be read, so this may also remove the \
             entry from [workspace.dependencies] in the root Cargo.toml.",
        ),
    }
    text
}

// ── State ──────────────────────────────────────────────────────────────

/// Where the local load (workspace + lockfile + manifests) stands.
enum LoadState {
    Loading,
    /// The opened folder has no `Cargo.toml` at its root.
    NoManifest,
    Ready,
    Failed(String),
}

/// What one reload reads from disk.
struct Loaded {
    workspace: cargo_backend::Workspace,
    lockfile: Option<Lockfile>,
    root_manifest: Option<Manifest>,
    /// Every member's manifest by crate name; empty when they could not be
    /// read.
    manifests: Vec<(String, Manifest)>,
}

pub struct CargoManagerPanel {
    focus_handle: FocusHandle,
    workspace: WeakEntity<Workspace>,
    /// Observer on the Script Runner while it streams a command for this
    /// tab; fires on each output line and when the run finishes.
    run_subscription: Option<Subscription>,
    _project_subscription: Subscription,
    _scan_subscription: Subscription,

    /// The folder the tab was opened on: the project's first worktree.
    root: String,
    load: LoadState,
    cargo_workspace: Option<cargo_backend::Workspace>,
    lockfile: Option<Lockfile>,
    root_manifest: Option<Manifest>,
    manifests: Vec<(String, Manifest)>,
    crate_name: Option<String>,
    dependencies: DependencyList,

    rustc_version: Option<String>,
    cargo_version: Option<String>,

    index: IndexCache,
    outdated: OutdatedState,
    advisories: AdvisoryState,
    advisory_records: AdvisoryRecords,

    /// The dependency shown in the details pane.
    selected: Option<String>,
    /// The details pane's "compatible versions only" filter.
    compatible_only: bool,
    /// The README the details pane is showing in place of the details.
    readme: Readme,
    search: SearchState,

    /// Label of the Cargo command streaming in the Script Runner.
    running_action: Option<String>,
    /// What is being worked out before a confirmation (Cargo's dry run).
    checking: Option<String>,
    error: Option<String>,
    /// Something worth saying that is not an error.
    notice: Option<String>,

    // Dropping a task cancels it, so replacing one of these is how a newer
    // request supersedes an older one.
    load_task: Option<Task<()>>,
    outdated_task: Option<Task<()>>,
    advisory_task: Option<Task<()>>,
    confirm_task: Option<Task<()>>,
    details_task: Option<Task<()>>,
    readme_task: Option<Task<()>>,

    pages: pages::PageViews,
}

impl CargoManagerPanel {
    pub fn new(
        root: String,
        crate_name: Option<String>,
        workspace: WeakEntity<Workspace>,
        project: Entity<Project>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        // Reload when a manifest or the lockfile changes on disk, however it
        // changed (this tab, a terminal, a git checkout).
        let project_subscription = cx.subscribe(
            &project,
            |this: &mut Self, _, event: &project::Event, cx| {
                if let project::Event::WorktreeUpdatedEntries(_, changes) = event {
                    let touched = changes.iter().any(|(path, _, change)| {
                        !matches!(change, project::PathChange::Loaded)
                            && path.file_name().is_some_and(is_manifest_file)
                    });
                    if touched {
                        this.reload(cx);
                    }
                }
            },
        );

        let scan_subscription =
            cx.observe_global::<SharedScans>(|this: &mut Self, cx| this.adopt_shared_scan(cx));

        let search = SearchState::new(window, cx);
        let pages = pages::PageViews::new(cx.weak_entity(), cx);
        let mut panel = CargoManagerPanel {
            focus_handle: cx.focus_handle(),
            workspace,
            run_subscription: None,
            _project_subscription: project_subscription,
            _scan_subscription: scan_subscription,
            root,
            load: LoadState::Loading,
            cargo_workspace: None,
            lockfile: None,
            root_manifest: None,
            manifests: Vec::new(),
            crate_name,
            dependencies: DependencyList::default(),
            rustc_version: None,
            cargo_version: None,
            index: IndexCache::new(),
            outdated: OutdatedState::Idle,
            advisories: AdvisoryState::NotScanned,
            advisory_records: AdvisoryRecords::new(),
            selected: None,
            compatible_only: true,
            readme: Readme::Closed,
            search,
            running_action: None,
            checking: None,
            error: None,
            notice: None,
            load_task: None,
            outdated_task: None,
            advisory_task: None,
            confirm_task: None,
            details_task: None,
            readme_task: None,
            pages,
        };
        panel.detect_toolchain(cx);
        panel.reload(cx);
        panel
    }

    // ── Accessors for the pages ──

    fn selected_crate(&self) -> Option<&CrateInfo> {
        self.cargo_workspace
            .as_ref()?
            .find(self.crate_name.as_deref()?)
    }

    fn outdated_rows(&self) -> Vec<OutdatedRow> {
        outdated_rows(
            &self.dependencies,
            &self.index,
            self.rustc_version.as_deref(),
        )
    }

    /// The first listed row for `package`. A package declared in two tables
    /// has two rows; the first is the one actions on the package as a whole
    /// (update, details) go by.
    fn listed(&self, package: &str) -> Option<&ListedDependency> {
        self.dependencies
            .listed
            .iter()
            .find(|row| row.name == package)
    }

    /// How the selected crate's manifest writes `row`. `None` when the
    /// manifests could not be read: unknown, which no action may treat as
    /// either answer.
    fn declaration_of(&self, row: &ListedDependency) -> Option<Declaration> {
        let name = self.crate_name.as_deref()?;
        let (_, manifest) = self.manifests.iter().find(|(member, _)| member == name)?;
        declaration(
            self.root_manifest.as_ref()?,
            manifest,
            &row.name,
            row.kind,
            row.target.as_deref(),
        )
    }

    /// Whether a Cargo action is running or being prepared (design rule 1:
    /// one at a time).
    fn is_busy(&self) -> bool {
        self.running_action.is_some() || self.checking.is_some()
    }

    fn vulnerability_count(&self) -> Option<usize> {
        match &self.advisories {
            AdvisoryState::Done { findings, .. } => {
                Some(FindingCounts::of(findings).vulnerabilities)
            }
            _ => None,
        }
    }

    // ── Loading ──

    fn detect_toolchain(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let (cargo, rustc) = cx
                .background_spawn(async { (query_cargo(), query_rustc()) })
                .await;
            this.update(cx, |this, cx| {
                this.cargo_version = cargo.ok();
                this.rustc_version = rustc.ok();
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Switches the tab to another crate of the same workspace.
    fn show_crate(&mut self, name: String, cx: &mut Context<Self>) {
        if self.crate_name.as_deref() == Some(name.as_str()) {
            return;
        }
        self.selected = None;
        if self.cargo_workspace.is_some() {
            self.select_crate(name, cx);
        } else {
            // Still loading: the load picks this up when it lands.
            self.crate_name = Some(name);
        }
    }

    /// Re-reads the workspace, lockfile and manifests from disk. Local and
    /// cheap: no network, nothing compiled.
    fn reload(&mut self, cx: &mut Context<Self>) {
        if self.root.is_empty() {
            self.load = LoadState::NoManifest;
            cx.notify();
            return;
        }
        if self.cargo_workspace.is_none() {
            self.load = LoadState::Loading;
        }
        cx.notify();

        let root = self.root.clone();
        self.load_task = Some(cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let result = cx
                .background_spawn(async move {
                    if !is_cargo_project(&root) {
                        return Ok(None);
                    }
                    let workspace = load_workspace(&root)?;
                    let lockfile = if has_lockfile(&workspace.root) {
                        Some(read_lockfile(&workspace.root)?)
                    } else {
                        None
                    };
                    // A manifest that can't be read doesn't stop the lists
                    // from loading; it leaves "inherited or literal" unknown,
                    // and the actions that need it say so.
                    let root_manifest = read_root_manifest(&workspace).ok();
                    let manifests = read_member_manifests(&workspace).unwrap_or_default();
                    Ok::<_, String>(Some(Loaded {
                        workspace,
                        lockfile,
                        root_manifest,
                        manifests,
                    }))
                })
                .await;

            this.update(cx, |this, cx| {
                match result {
                    Ok(Some(loaded)) => {
                        // Keep the crate across a reload when it still
                        // exists; otherwise fall back to the default.
                        let selection = this
                            .crate_name
                            .take()
                            .filter(|name| loaded.workspace.find(name).is_some())
                            .or_else(|| {
                                loaded
                                    .workspace
                                    .default_crate()
                                    .map(|krate| krate.name.clone())
                            });
                        this.cargo_workspace = Some(loaded.workspace);
                        this.lockfile = loaded.lockfile;
                        this.root_manifest = loaded.root_manifest;
                        this.manifests = loaded.manifests;
                        this.load = LoadState::Ready;
                        match selection {
                            Some(name) => this.select_crate(name, cx),
                            None => this.dependencies = DependencyList::default(),
                        }
                    }
                    Ok(None) => {
                        this.load = LoadState::NoManifest;
                        this.cargo_workspace = None;
                        this.lockfile = None;
                        this.dependencies = DependencyList::default();
                    }
                    Err(error) => this.load = LoadState::Failed(error),
                }
                cx.notify();
            })
            .ok();
        }));
    }

    fn select_crate(&mut self, name: String, cx: &mut Context<Self>) {
        let changed = self.crate_name.as_deref() != Some(name.as_str());
        self.crate_name = Some(name);
        self.dependencies = self
            .selected_crate()
            .map(|krate| direct_dependencies(krate, self.lockfile.as_ref()))
            .unwrap_or_default();
        // The details pane follows a dependency, and it may be gone now.
        if self.selected.as_deref().is_some_and(|package| {
            self.listed(package).is_none() && self.search.result(package).is_none()
        }) {
            self.selected = None;
        }

        // A scan's findings belong to one crate at one lockfile state. Any
        // reload or reselection puts the page back to "not scanned".
        if changed || matches!(self.advisories, AdvisoryState::Done { .. }) {
            self.advisory_task = None;
            self.advisories = AdvisoryState::NotScanned;
        }
        // Unless that same set of packages has already been scanned.
        self.adopt_shared_scan(cx);
        self.check_outdated(cx);
        cx.notify();
    }

    /// Looks up, in the crates.io sparse index, every listed dependency that
    /// isn't already cached and fresh.
    fn check_outdated(&mut self, cx: &mut Context<Self>) {
        let names = stale_names(&self.dependencies, &self.index, Instant::now());
        if names.is_empty() {
            self.outdated_task = None;
            self.outdated = if self.dependencies.listed.is_empty() {
                OutdatedState::Idle
            } else {
                OutdatedState::Done { failed: 0 }
            };
            return;
        }

        self.outdated = OutdatedState::Checking;
        let client = cx.http_client();
        self.outdated_task = Some(cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let results = fetch_index_entries(&client, names).await;
            this.update(cx, |this, cx| {
                let failed = store_index_entries(&mut this.index, results);
                this.outdated = OutdatedState::Done { failed };
                cx.notify();
            })
            .ok();
        }));
    }

    /// Shows the result of a scan already run for exactly these packages,
    /// here or in the crate's other view, instead of "not scanned yet".
    fn adopt_shared_scan(&mut self, cx: &mut Context<Self>) {
        if matches!(self.advisories, AdvisoryState::Scanning) {
            return;
        }
        let (Some(krate), Some(lockfile)) = (self.selected_crate(), &self.lockfile) else {
            return;
        };
        let reachable: Vec<LockedPackage> = lockfile
            .reachable_crates_io_packages(&krate.name, &krate.version)
            .into_iter()
            .cloned()
            .collect();
        if let Some(result) = shared_scan(scan_key(&reachable), cx) {
            self.advisories = result.into_state();
            cx.notify();
        }
    }

    /// Asks OSV about every crates.io package reachable from the crate,
    /// fetches the records it names, and merges them into findings.
    fn scan_advisories(&mut self, cx: &mut Context<Self>) {
        let Some(krate) = self.selected_crate() else {
            return;
        };
        let Some(lockfile) = &self.lockfile else {
            self.advisories = AdvisoryState::Failed(
                "There is no Cargo.lock yet, so there are no exact versions to check. \
                 Build the project once to create it."
                    .to_string(),
            );
            cx.notify();
            return;
        };
        let reachable: Vec<LockedPackage> = lockfile
            .reachable_crates_io_packages(&krate.name, &krate.version)
            .into_iter()
            .cloned()
            .collect();
        let scanned = reachable.len();
        let key = scan_key(&reachable);

        self.advisories = AdvisoryState::Scanning;
        cx.notify();

        let client = cx.http_client();
        let cached = self.advisory_records.clone();
        self.advisory_task = Some(cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let outcome = run_advisory_scan(&client, &reachable, cached).await;
            this.update(cx, |this, cx| {
                match outcome {
                    Ok((hits, records)) => {
                        let known: Vec<_> =
                            records.values().map(|(_, record)| record.clone()).collect();
                        let findings = merge_findings(&hits, &known);
                        this.advisory_records = records;
                        let result = ScanResult {
                            findings,
                            scanned,
                            truncated: hits.iter().any(|hit| hit.truncated),
                        };
                        // Shared, so the same crate's other view shows it too.
                        publish_scan(key, result.clone(), cx);
                        this.advisories = result.into_state();
                    }
                    Err(error) => this.advisories = AdvisoryState::Failed(error),
                }
                cx.notify();
            })
            .ok();
        }));
    }

    // ── Actions ──

    fn reject(&mut self, error: String, cx: &mut Context<Self>) {
        self.error = Some(error);
        self.notice = None;
        cx.notify();
    }

    /// Removes the dependency in row `ix` of the Installed list, after a
    /// confirmation that says whether the root manifest changes too.
    fn remove_dependency(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if self.is_busy() {
            return;
        }
        let (Some(row), Some(member)) =
            (self.dependencies.listed.get(ix).cloned(), self.crate_name.clone())
        else {
            return;
        };
        let command = match remove_command(&row.name, &member, row.kind, row.target.as_deref()) {
            Ok(command) => command,
            Err(error) => return self.reject(error, cx),
        };
        let effect = self.root_manifest.as_ref().filter(|_| !self.manifests.is_empty()).map(
            |root| {
                remove_effect(
                    root,
                    &self.manifests,
                    &member,
                    &row.name,
                    row.kind,
                    row.target.as_deref(),
                )
            },
        );

        let answer = window.prompt(
            PromptLevel::Warning,
            &format!("Remove {} from {member}?", row.name),
            Some(&format!(
                "Runs: {command}\n\n{}",
                describe_removal(&member, &row.name, effect)
            )),
            &["Remove", "Cancel"],
            cx,
        );
        let label = format!("remove {}", row.name);
        self.confirm_task = Some(cx.spawn_in(window, async move |this, cx| {
            if answer.await != Ok(0) {
                return;
            }
            this.update_in(cx, |this, window, cx| this.kick_run(&label, command, window, cx))
                .ok();
        }));
    }

    /// Updates one dependency within its declared range (`cargo update`).
    fn update_dependency(&mut self, package: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(row) = self.listed(package) else {
            return;
        };
        let specs = vec![UpdateSpec::new(&row.name, row.locked_version.as_deref())];
        self.confirm_update(specs, format!("update {package}"), window, cx);
    }

    /// Updates every dependency that has a newer version inside its declared
    /// range, in one command that names each of them.
    fn update_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let specs = update_all_specs(&self.outdated_rows());
        self.confirm_update(specs, "update all".to_string(), window, cx);
    }

    /// Asks Cargo what the update would change (a dry run), shows that as
    /// the confirmation, and runs the update if it is accepted.
    ///
    /// The dry run is the confirmation because updating one package
    /// routinely moves others with it, and because the version Cargo settles
    /// on can be lower than the newest the requirement allows.
    fn confirm_update(
        &mut self,
        specs: Vec<UpdateSpec>,
        label: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.is_busy() {
            return;
        }
        let Some(workspace_root) = self.cargo_workspace.as_ref().map(|w| w.root.clone()) else {
            return;
        };
        let command = match update_command(&specs) {
            Ok(Some(command)) => command,
            Ok(None) => return,
            Err(error) => return self.reject(error, cx),
        };

        self.checking = Some("Asking Cargo what would change\u{2026}".to_string());
        self.error = None;
        self.notice = None;
        cx.notify();

        self.confirm_task = Some(cx.spawn_in(window, async move |this, cx| {
            let dry_run = cx
                .background_spawn(async move { update_dry_run(&workspace_root, &specs) })
                .await;
            let answer = this
                .update_in(cx, |this, window, cx| {
                    this.checking = None;
                    cx.notify();
                    match dry_run {
                        Err(error) => {
                            this.error = Some(error);
                            None
                        }
                        Ok(changes) if changes.is_empty() => {
                            this.notice = Some(
                                "Cargo would change nothing: another package in the workspace \
                                 holds this version where it is."
                                    .to_string(),
                            );
                            None
                        }
                        Ok(changes) => Some(window.prompt(
                            PromptLevel::Info,
                            &format!("Run {command}?"),
                            Some(&describe_changes(&changes)),
                            &["Update", "Cancel"],
                            cx,
                        )),
                    }
                })
                .ok()
                .flatten();
            let Some(answer) = answer else {
                return;
            };
            if answer.await != Ok(0) {
                return;
            }
            this.update_in(cx, |this, window, cx| this.kick_run(&label, command, window, cx))
                .ok();
        }));
    }

    /// Rewrites a dependency's requirement to `version` (`cargo add
    /// name@version`), for a version outside the declared range.
    ///
    /// Only for a requirement written in the crate's own manifest. On an
    /// inherited dependency the same command would write a literal version
    /// over `workspace = true`, so it is refused here whatever the caller
    /// showed.
    fn change_version(
        &mut self,
        package: &str,
        version: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.is_busy() {
            return;
        }
        let (Some(row), Some(member)) = (self.listed(package).cloned(), self.crate_name.clone())
        else {
            return;
        };
        match self.declaration_of(&row) {
            Some(Declaration::Literal) => {}
            Some(Declaration::Inherited) => {
                return self.reject(
                    format!(
                        "{package} is inherited from the workspace. Its version is set in \
                         [workspace.dependencies] in the root Cargo.toml, which no Cargo \
                         command edits."
                    ),
                    cx,
                );
            }
            None => {
                return self.reject(
                    format!("Could not read how {member} declares {package}."),
                    cx,
                );
            }
        }
        if row.target.is_some() {
            return self.reject(
                format!(
                    "{package} is declared for one target only. Change its version in \
                     Cargo.toml by hand."
                ),
                cx,
            );
        }
        let command = match change_version_command(package, version, &member, row.kind) {
            Ok(command) => command,
            Err(error) => return self.reject(error, cx),
        };

        let answer = window.prompt(
            PromptLevel::Info,
            &format!("Change {package} to {version}?"),
            Some(&format!(
                "Runs: {command}\n\nThis rewrites the requirement in {member}'s Cargo.toml \
                 and updates Cargo.lock. Nothing is compiled."
            )),
            &["Change", "Cancel"],
            cx,
        );
        let label = format!("change {package}");
        self.confirm_task = Some(cx.spawn_in(window, async move |this, cx| {
            if answer.await != Ok(0) {
                return;
            }
            this.update_in(cx, |this, window, cx| this.kick_run(&label, command, window, cx))
                .ok();
        }));
    }

    fn runner(&self, cx: &App) -> Option<Entity<ScriptRunnerPanel>> {
        self.workspace
            .upgrade()?
            .read(cx)
            .panel::<ScriptRunnerPanel>(cx)
    }

    /// Runs a Cargo command from the workspace root in the Script Runner,
    /// and re-reads the manifests and lockfile when it finishes (design rule
    /// 3: refresh by re-reading files, never by waiting on the analyzer).
    fn kick_run(&mut self, label: &str, command: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.running_action.is_some() {
            return;
        }
        let Some(workspace_root) = self.cargo_workspace.as_ref().map(|w| w.root.clone()) else {
            return;
        };
        let Some(runner) = self.runner(cx) else {
            return self.reject("The Script Runner panel isn't available yet.".into(), cx);
        };
        // The runner streams one command at a time.
        if runner.read(cx).is_running() {
            return self.reject(
                "Script Runner is busy \u{2014} wait for it to finish first.".into(),
                cx,
            );
        }

        self.running_action = Some(label.to_string());
        self.error = None;
        self.notice = None;
        cx.notify();

        if let Some(workspace) = self.workspace.upgrade() {
            workspace.update(cx, |workspace, cx| {
                workspace.open_panel::<ScriptRunnerPanel>(window, cx);
            });
        }
        runner.update(cx, |runner, cx| runner.run_external(command, workspace_root, cx));

        // The runner notifies on every output line and on completion.
        self.run_subscription = Some(cx.observe(&runner, |this, runner, cx| {
            if !runner.read(cx).is_running() && this.running_action.take().is_some() {
                this.reload(cx);
            }
        }));
        // A very fast run can finish before the observer is attached.
        if !runner.read(cx).is_running() && self.running_action.take().is_some() {
            self.reload(cx);
        }
    }
}

impl EventEmitter<()> for CargoManagerPanel {}

impl Focusable for CargoManagerPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Item for CargoManagerPanel {
    type Event = ();

    fn tab_content_text(&self, _detail: usize, _cx: &App) -> gpui::SharedString {
        "Cargo Manager".into()
    }

    fn tab_tooltip_text(&self, _: &App) -> Option<gpui::SharedString> {
        Some(match &self.crate_name {
            Some(name) => format!("Cargo Manager \u{2014} {name}").into(),
            None => "Cargo Manager".into(),
        })
    }
}

impl SerializableItem for CargoManagerPanel {
    fn serialized_item_kind() -> &'static str {
        "cargo_manager_panel"
    }

    /// Persisting the tab just records its presence in the pane; the root is
    /// always the project's first worktree, so nothing extra is written and
    /// nothing needs cleaning up.
    fn cleanup(
        _workspace_id: WorkspaceId,
        _alive_items: Vec<ItemId>,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Task<anyhow::Result<()>> {
        Task::ready(Ok(()))
    }

    fn deserialize(
        project: Entity<Project>,
        workspace: WeakEntity<Workspace>,
        _workspace_id: WorkspaceId,
        _item_id: ItemId,
        window: &mut Window,
        cx: &mut App,
    ) -> Task<anyhow::Result<Entity<Self>>> {
        let root = first_worktree_root(&project, cx);
        Task::ready(Ok(
            cx.new(|cx| CargoManagerPanel::new(root, None, workspace, project, window, cx))
        ))
    }

    /// Nothing is persisted beyond the item's presence in the pane.
    fn serialize(
        &mut self,
        _workspace: &mut Workspace,
        _item_id: ItemId,
        _closing: bool,
        _cx: &mut Context<Self>,
    ) -> Option<Task<anyhow::Result<()>>> {
        None
    }

    fn should_serialize(&self, _event: &Self::Event) -> bool {
        false
    }
}

impl Render for CargoManagerPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let view = cx.entity();

        let reload_button = |label: &'static str| {
            Button::new("cargo-manager-reload")
                .icon(IconName::Redo2)
                .label(label)
                .with_size(Size::Small)
                .on_click({
                    let view = view.clone();
                    move |_, _, cx| view.update(cx, |panel, cx| panel.reload(cx))
                })
        };

        let body = match &self.load {
            LoadState::Loading => h_flex()
                .gap_2()
                .items_center()
                .p_4()
                .text_xs()
                .child(Spinner::new().with_size(Size::XSmall))
                .child(
                    div()
                        .text_color(theme.muted_foreground)
                        .child("Reading the workspace\u{2026}"),
                )
                .into_any_element(),
            LoadState::NoManifest => v_flex()
                .gap_2()
                .p_4()
                .text_xs()
                .child(
                    div()
                        .font_semibold()
                        .text_color(theme.muted_foreground)
                        .child("Cargo.toml not found in this folder."),
                )
                .child(reload_button("Look again"))
                .into_any_element(),
            LoadState::Failed(error) => v_flex()
                .gap_2()
                .p_4()
                .text_xs()
                .child(div().text_color(theme.danger).child(error.clone()))
                .child(reload_button("Retry"))
                .into_any_element(),
            LoadState::Ready => {
                let settings = Settings::new("cargo-package-manager")
                    .pages(pages::build_all(self, &self.pages));
                if self.selected.is_some() {
                    h_resizable("cargo-details-split")
                        .child(
                            resizable_panel()
                                .child(v_flex().flex_1().min_w_0().h_full().child(settings)),
                        )
                        .child(
                            resizable_panel()
                                .size(px(400.0))
                                .size_range(px(280.0)..px(900.0))
                                .flex_none()
                                .child(details::details_pane(self, &view, cx)),
                        )
                        .into_any_element()
                } else {
                    settings.into_any_element()
                }
            }
        };

        v_flex()
            .id("cargo-manager-panel")
            .track_focus(&self.focus_handle)
            .size_full()
            .pl_2()
            .bg(theme.background)
            .child(body)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cargo_backend::{RustCompat, UpdateKind, UpdateTarget, Version};

    fn target(version: &str) -> UpdateTarget {
        UpdateTarget {
            version: Version::parse(version).unwrap(),
            kind: UpdateKind::Patch,
            rust: RustCompat::Compatible,
        }
    }

    fn row(name: &str, locked: &str, in_range: Option<&str>, out_of_range: Option<&str>) -> OutdatedRow {
        OutdatedRow {
            name: name.to_string(),
            locked: Some(locked.to_string()),
            locked_yanked: false,
            in_range: in_range.map(target),
            out_of_range: out_of_range.map(target),
        }
    }

    #[test]
    fn only_cargo_files_trigger_a_reload() {
        assert!(is_manifest_file("Cargo.toml"));
        assert!(is_manifest_file("Cargo.lock"));
        assert!(!is_manifest_file("cargo.toml"));
        assert!(!is_manifest_file("Cargo.toml.bak"));
        assert!(!is_manifest_file("main.rs"));
    }

    #[test]
    fn remove_commands_name_the_table() {
        assert_eq!(
            remove_command("serde", "app", DependencyKind::Normal, None).as_deref(),
            Ok("cargo remove serde -p app")
        );
        assert_eq!(
            remove_command("tempfile", "app", DependencyKind::Dev, None).as_deref(),
            Ok("cargo remove tempfile -p app --dev")
        );
        assert_eq!(
            remove_command("windows", "app", DependencyKind::Normal, Some("x86_64-pc-windows-msvc"))
                .as_deref(),
            Ok("cargo remove windows -p app --target x86_64-pc-windows-msvc")
        );
    }

    #[test]
    fn commands_refuse_shell_syntax() {
        assert!(remove_command("serde; calc", "app", DependencyKind::Normal, None).is_err());
        assert!(remove_command("serde", "app && calc", DependencyKind::Normal, None).is_err());
        assert!(remove_command("libc", "app", DependencyKind::Normal, Some("cfg(unix)")).is_err());
        assert!(change_version_command("serde", "2.0.0 | more", "app", DependencyKind::Normal).is_err());
        assert!(change_version_command("--git", "2.0.0", "app", DependencyKind::Normal).is_err());
        assert!(update_command(&[UpdateSpec::new("serde && calc", Some("1.0.0"))]).is_err());
        assert!(update_command(&[UpdateSpec::new("serde", Some("1.0.0 && calc"))]).is_err());
    }

    #[test]
    fn version_changes_keep_the_dependency_in_its_table() {
        assert_eq!(
            change_version_command("serde", "2.0.0", "app", DependencyKind::Normal).as_deref(),
            Ok("cargo add serde@2.0.0 -p app")
        );
        assert_eq!(
            change_version_command("tempfile", "4.0.0", "app", DependencyKind::Dev).as_deref(),
            Ok("cargo add tempfile@4.0.0 -p app --dev")
        );
    }

    #[test]
    fn an_update_always_names_its_packages() {
        assert_eq!(update_command(&[]), Ok(None));
        assert_eq!(
            update_command(&[UpdateSpec::new("clap", Some("4.5.49"))]).unwrap().as_deref(),
            Some("cargo update clap@4.5.49")
        );
    }

    #[test]
    fn update_all_covers_in_range_rows_only() {
        let rows = [
            row("clap", "4.5.49", Some("4.6.1"), None),
            // Only a version outside the range: needs the requirement
            // changed, so "Update all" leaves it alone.
            row("windows-registry", "0.6.1", None, Some("0.100.0")),
            row("serde", "1.0.210", Some("1.0.229"), Some("2.0.0")),
        ];
        let specs = update_all_specs(&rows);
        assert_eq!(
            update_command(&specs).unwrap().as_deref(),
            Some("cargo update clap@4.5.49 serde@1.0.210")
        );

        // Nothing in range: no command, rather than a bare `cargo update`.
        let none = [row("windows-registry", "0.6.1", None, Some("0.100.0"))];
        assert_eq!(update_command(&update_all_specs(&none)), Ok(None));
    }

    fn change(kind: LockChangeKind, name: &str, from: Option<&str>, to: Option<&str>) -> LockChange {
        LockChange {
            kind,
            name: name.to_string(),
            from: from.map(str::to_string),
            to: to.map(str::to_string),
        }
    }

    #[test]
    fn a_confirmation_lists_every_lockfile_change() {
        let text = describe_changes(&[
            change(LockChangeKind::Add, "anstream", None, Some("1.0.0")),
            change(LockChangeKind::Update, "clap", Some("4.5.49"), Some("4.6.1")),
            change(LockChangeKind::Remove, "old", Some("0.3.1"), None),
            change(LockChangeKind::Downgrade, "time", Some("0.3.41"), Some("0.3.36")),
        ]);
        assert!(text.contains("anstream  1.0.0 (added)"), "{text}");
        assert!(text.contains("clap  4.5.49 \u{2192} 4.6.1"), "{text}");
        assert!(text.contains("old  0.3.1 (removed)"), "{text}");
        assert!(text.contains("time  0.3.41 \u{2192} 0.3.36 (downgrade)"), "{text}");
        assert!(text.contains("Only Cargo.lock changes"), "{text}");
    }

    #[test]
    fn a_long_confirmation_is_summarized_not_cut_off_silently() {
        let changes: Vec<LockChange> = (0..40)
            .map(|n| change(LockChangeKind::Update, &format!("crate{n}"), Some("1.0.0"), Some("1.0.1")))
            .collect();
        let text = describe_changes(&changes);
        assert!(text.contains("crate14"), "{text}");
        assert!(!text.contains("crate15"), "{text}");
        assert!(text.contains("and 25 more"), "{text}");
    }

    #[test]
    fn a_removal_says_when_the_root_manifest_changes() {
        let plain = describe_removal("app", "serde", Some(RemoveEffect::default()));
        assert!(plain.contains("app's Cargo.toml"), "{plain}");
        assert!(!plain.contains("root"), "{plain}");

        let last = describe_removal(
            "app",
            "serde",
            Some(RemoveEffect { removes_workspace_entry: true }),
        );
        assert!(last.contains("removes serde from [workspace.dependencies]"), "{last}");

        // Unknown is said out loud, not passed off as "nothing else changes".
        let unknown = describe_removal("app", "serde", None);
        assert!(unknown.contains("may also remove"), "{unknown}");
    }
}
