//! NuGet manager panel — virtual workspace tab opened from the .NET panel's
//! "nuget mgr" quick action (the `nuget_manager_panel` counterpart to the
//! `npm_manager_panel`).
//!
//! This is a *workspace tab* (an [`Item`]), not a dock panel — it appears as a
//! tab in the active pane, mirroring the `npm_manager_panel`/`keymap_editor`
//! Item precedent, with the pure package logic living in `dotnet_backend`.
//!
//! Unlike the npm manager (which discovers one tab per Node project), NuGet is
//! single-project: the tab resolves the workspace's *first* `.csproj` and
//! operates on it alone — `dotnet list package` rejects directories holding
//! more than one project file, so there is no multi-project aggregate view.
//! Package install/update/remove commands run in the csproj's *parent*
//! directory (not the workspace root), matching how `dotnet add package`
//! resolves which project to modify. When no `.csproj` exists under the
//! workspace the panel shows a rescan-notice instead of a phantom package
//! list).
//!
//! The UI follows the settings-page layout from `gpui_component::setting`
//! (five `SettingPage`s — General, Search, Installed, Updates, Vulnerabilities
//! — rendered by a `Settings` widget), with a resizable details pane splitting
//! the right edge while a package is selected.
//!
//! Per the port plan:
//! - Version/parse/CLI work is delegated to `dotnet_backend` (no dotnet CLI
//!   handling in the panel itself); the registry HTTP calls live in `search.rs`
//!   and their JSON parsing in `dotnet_backend`, mirroring `npm_backend`.
//! - The Forge "Ecosystem security-findings aggregator" (a host `Dashboard`
//!   feed) is dropped — this Zed host has no Dashboard consumer, so the panel
//!   surfaces the local per-package `dotnet list package --vulnerable` list.

use std::path::Path;

use dotnet_backend::{
    DotnetProject, InstalledPackage, NugetPackageDetails, NugetSearchResult, OutdatedPackage,
    UpdateKind, VulnerablePackage, classify_update, list_outdated, list_vulnerable,
    read_installed_packages, resolve_csproj, scan_dotnet_projects,
};
use gpui::{
    App, AppContext, AsyncApp, Context, Entity, EventEmitter, FocusHandle, Focusable,
    InteractiveElement as _, IntoElement, ParentElement as _, Render, Styled as _, Subscription,
    Task, WeakEntity, Window, actions, div, px,
};
use gpui_component::{
    ActiveTheme as _, IconName, Sizable as _, Size, StyledExt as _,
    button::Button,
    h_flex,
    input::{InputEvent, InputState},
    resizable::{h_resizable, resizable_panel},
    setting::Settings,
    spinner::Spinner,
    v_flex,
};
use script_runner_panel::ScriptRunnerPanel;
use workspace::{Item, ItemId, SerializableItem, Workspace, WorkspaceId};

mod details;
mod pages;
mod search;

actions!(nuget_manager, [OpenNuGetManager, ReloadNuGetManager]);

/// Opens (or activates) the NuGet manager tab for the given workspace root, in
/// the given workspace. This is the seam the .NET panel's quick action calls.
pub fn open(
    project_root: String,
    workspace: &mut Workspace,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) -> Entity<NuGetManagerPanel> {
    let existing = workspace
        .active_pane()
        .read(cx)
        .items()
        .find_map(|item| item.downcast::<NuGetManagerPanel>());

    let panel = if let Some(existing) = existing {
        workspace.activate_item(&existing, true, true, window, cx);
        existing
    } else {
        let panel =
            cx.new(|cx| NuGetManagerPanel::new(project_root, workspace.weak_handle(), window, cx));
        workspace.add_item_to_active_pane(Box::new(panel.clone()), None, true, window, cx);
        panel
    };
    panel
}

pub fn init(cx: &mut App) {
    workspace::register_serializable_item::<NuGetManagerPanel>(cx);
    cx.observe_new(|workspace: &mut Workspace, _, _cx| {
        workspace.register_action(|workspace, _: &OpenNuGetManager, window, cx| {
            let root = workspace_root(workspace, cx);
            if !root.is_empty() {
                let panel = open(root, workspace, window, cx);
                panel.update(cx, |panel, cx| {
                    panel.reload_visible(cx);
                });
            }
        });
        workspace.register_action(|workspace, _: &ReloadNuGetManager, _window, cx| {
            if let Some(panel) = workspace
                .active_pane()
                .read(cx)
                .active_item()
                .and_then(|item| item.downcast::<NuGetManagerPanel>())
            {
                panel.update(cx, |panel, cx| {
                    panel.reload_visible(cx);
                });
            }
        });
    })
    .detach();
}

/// The workspace's first worktree's absolute path — the scan root the NuGet
/// manager tab resolves `.csproj` files under. Falls back to the process cwd
/// when no project worktree is open yet (the same resolution
/// `NodePanel`/`ScriptRunnerPanel` use), rather than silently refusing to
/// open.
fn workspace_root(workspace: &Workspace, cx: &Context<Workspace>) -> String {
    workspace
        .project()
        .read(cx)
        .worktrees(cx)
        .next()
        .map(|worktree| worktree.read(cx).abs_path().to_string_lossy().into_owned())
        .unwrap_or_else(|| {
            std::env::current_dir()
                .map(|p| p.display().to_string())
                .unwrap_or_default()
        })
}

pub struct NuGetManagerPanel {
    focus_handle: FocusHandle,
    workspace: WeakEntity<Workspace>,
    /// Active observer on the Script Runner dock panel, replaced on every run
    /// it streams for — fires when [`ScriptRunnerPanel`] notifies, which it
    /// does on each output line and when a run finishes (mirrors the npm
    /// manager's §4.2 wiring).
    run_subscription: Option<Subscription>,
    /// Subscription on the search input; `Enter` kicks off a fresh registry
    /// search (kept alive for the entity's lifetime, hence underscore).
    _search_sub: Subscription,
    /// The workspace scan root the tab was opened with.
    scan_root: String,
    /// The resolved `.csproj` the tab operates on. `None` while scanning and
    /// when no project was found (the render then shows a rescan notice).
    csproj: Option<String>,
    /// Directory dotnet commands run in — the csproj's parent when resolved,
    /// else the scan root. `dotnet list package` requires a directory that
    /// holds exactly one project, so this is never the workspace root when a
    /// csproj exists elsewhere in it.
    project_dir: String,
    project_name: String,
    scanning: bool,
    no_project_found: bool,
    /// `dotnet --version`, probed once during the initial scan (shown on the
    /// General page). Ligent when the SDK could not be found.
    dotnet_version: Option<String>,

    search_input: Entity<InputState>,
    search_results: Vec<NugetSearchResult>,
    search_loading: bool,
    search_total: usize,
    search_page: usize,
    search_error: Option<String>,

    /// Package currently shown in the details pane.
    selected: Option<String>,
    details: Option<NugetPackageDetails>,
    details_loading: bool,
    details_error: Option<String>,
    show_readme: bool,
    /// True when the most recent `fetch_details` call should default the
    /// details pane to the README view once the catalog lands (the search
    /// card's "More info" button). Falls back to the plain details page when
    /// the package ships no README.
    open_readme_on_load: bool,

    installed: Vec<InstalledPackage>,
    installed_loading: bool,
    outdated: Vec<OutdatedPackage>,
    outdated_loading: bool,
    outdated_error: Option<String>,
    vulnerable: Vec<VulnerablePackage>,
    vulnerable_loading: bool,
    vulnerable_error: Option<String>,

    error: Option<String>,
    running_action: Option<String>,
    /// True while `reload_visible`'s background refresh is still in flight, so
    /// the periodic auto-refresh timer and the manual reload never overlap.
    reload_in_flight: bool,
    /// The five page views the `Settings` widget embeds. Their `Render` reads
    /// the panel live, so `build_all` stays O(1) per page and only the active
    /// page's rows are ever built.
    pages: pages::PageViews,
}

/// How often the tab re-runs `read_installed_packages`/`list_outdated`/
/// `list_vulnerable` in the background while it's open, keeping the lists
/// fresh without any user action.
const AUTO_REFRESH_INTERVAL: std::time::Duration = std::time::Duration::from_secs(60);

impl NuGetManagerPanel {
    pub fn new(
        scan_root: String,
        workspace: WeakEntity<Workspace>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let search_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("Search nuget.org for a package…"));
        let _search_sub = cx.subscribe(&search_input, |this, _, event, cx| {
            if matches!(event, InputEvent::PressEnter { .. }) {
                this.search_page = 0;
                this.search_nuget(cx);
            }
        });

        // Probe the SDK and resolve the first csproj in the background so the
        // first frame isn't stalled on the (slow) `dotnet --version` probe and
        // the filesystem walk. Until it lands `scanning` shows a spinner.
        let this = cx.weak_entity();
        let scan_task = scan_root.clone();
        let scan_result_root = scan_root.clone();
        cx.spawn(async move |_: WeakEntity<Self>, cx: &mut AsyncApp| {
            let (detected, version) = futures::join!(
                cx.background_spawn(async move { scan_dotnet_projects(&scan_task, 0) }),
                cx.background_spawn(
                    async move { dotnet_backend::query_runtime("dotnet".into()).ok() }
                ),
            );
            let _ = cx.update(|app| {
                if let Some(panel) = this.upgrade() {
                    let _ = panel.update(app, |panel, cx| {
                        panel.finish_scan(scan_result_root, detected, version, cx);
                    });
                }
            });
        })
        .detach();

        // Periodic auto-refresh: while the panel is alive, re-run the package
        // lists every `AUTO_REFRESH_INTERVAL`, unless a reload (manual or
        // automatic) is already in flight or an install/update action is
        // running right now.
        let auto_refresh_this = cx.weak_entity();
        cx.spawn(async move |_: WeakEntity<Self>, cx: &mut AsyncApp| {
            loop {
                cx.background_executor().timer(AUTO_REFRESH_INTERVAL).await;
                let alive = cx.update(|app| {
                    if let Some(panel) = auto_refresh_this.upgrade() {
                        let _ = panel.update(app, |panel, cx| {
                            if !panel.reload_in_flight && panel.running_action.is_none() {
                                panel.reload_active(true, cx);
                            }
                        });
                        true
                    } else {
                        false
                    }
                });
                if !alive {
                    break;
                }
            }
        })
        .detach();

        let pages = {
            let panel = cx.weak_entity();
            pages::PageViews::new(panel, cx)
        };

        NuGetManagerPanel {
            focus_handle: cx.focus_handle(),
            workspace,
            run_subscription: None,
            _search_sub,
            scan_root: scan_root.clone(),
            csproj: None,
            project_dir: scan_root.clone(),
            project_name: fallback_name(&scan_root),
            scanning: true,
            no_project_found: false,
            dotnet_version: None,
            search_input,
            search_results: Vec::new(),
            search_loading: false,
            search_total: 0,
            search_page: 0,
            search_error: None,
            selected: None,
            details: None,
            details_loading: false,
            details_error: None,
            show_readme: false,
            open_readme_on_load: false,
            installed: Vec::new(),
            installed_loading: false,
            outdated: Vec::new(),
            outdated_loading: false,
            outdated_error: None,
            vulnerable: Vec::new(),
            vulnerable_loading: false,
            vulnerable_error: None,
            error: None,
            running_action: None,
            reload_in_flight: false,
            pages,
        }
    }

    /// Turns the initial (or re-)scan into the panel's project state and kicks
    /// off the first package-list load.
    fn finish_scan(
        &mut self,
        scan_root: String,
        detected: Vec<DotnetProject>,
        version: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let csproj = detected
            .iter()
            .find(|p| !p.is_solution)
            .map(|p| p.path.clone())
            // No scan hit (deep/large repos): fall back to the bounded
            // `resolve_csproj` which also handles a sln-without-csproj root.
            .or_else(|| resolve_csproj(&scan_root));

        self.csproj = csproj.clone();
        if let Some(csproj_path) = &csproj {
            self.project_name = Path::new(csproj_path)
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| fallback_name(&scan_root));
            self.project_dir = Path::new(csproj_path)
                .parent()
                .map(|d| d.to_string_lossy().into_owned())
                .unwrap_or_else(|| scan_root.clone());
        } else {
            self.project_dir = scan_root.clone();
            self.project_name = fallback_name(&scan_root);
        }
        self.dotnet_version = version.filter(|v| !v.trim().is_empty());
        self.scanning = false;
        self.no_project_found = csproj.is_none();
        self.reload_active(false, cx);
        cx.notify();
    }

    /// Re-runs the workspace-root scan. Used by the notice shown when no
    /// `.csproj` was found, so dropping a new project file in doesn't force an
    /// app restart (or a new tab).
    fn rescan(&mut self, cx: &mut Context<Self>) {
        let root = self.scan_root.clone();
        self.scanning = true;
        self.no_project_found = false;
        cx.notify();

        let this = cx.weak_entity();
        let scan_root_task = root.clone();
        cx.spawn(async move |_: WeakEntity<Self>, cx: &mut AsyncApp| {
            let (detected, version) = futures::join!(
                cx.background_spawn(async move { scan_dotnet_projects(&scan_root_task, 0) }),
                cx.background_spawn(
                    async move { dotnet_backend::query_runtime("dotnet".into()).ok() }
                ),
            );
            let _ = cx.update(|app| {
                if let Some(panel) = this.upgrade() {
                    let _ = panel.update(app, |panel, cx| {
                        panel.finish_scan(root, detected, version, cx);
                    });
                }
            });
        })
        .detach();
    }

    /// Directory dotnet CLI commands run in (see `project_dir`).
    fn project_dir(&self) -> String {
        self.project_dir.clone()
    }

    // ── Loading ─────────────────────────────────────────────────────────

    /// Reloads the package lists. `silent` is used by the periodic
    /// auto-refresh: the old lists stay on screen while the probes run, then
    /// the new data lands in a single notification — no spinner flash. Manual
    /// reloads pass `false` so the loading state is shown immediately.
    fn reload_visible(&mut self, cx: &mut Context<Self>) {
        self.reload_active(false, cx);
    }

    fn reload_active(&mut self, silent: bool, cx: &mut Context<Self>) {
        self.error = None;
        // Nothing to load while scanning or with no resolved csproj — the
        // panel is showing the rescan notice, not a package list.
        if self.scanning || self.reload_in_flight || self.no_project_found {
            return;
        }
        self.reload_in_flight = true;
        if !silent {
            self.installed_loading = true;
            self.outdated_loading = true;
            self.outdated_error = None;
            self.vulnerable_loading = true;
            self.vulnerable_error = None;
        }
        cx.notify();

        let csproj = self.csproj.clone();
        let dir = self.project_dir();

        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            // All three probes run concurrently, so the refresh waits on the
            // slowest one instead of serializing them end to end.
            let (installed, outdated, vulnerable) = futures::join!(
                cx.background_spawn({
                    let csproj = csproj.clone();
                    async move {
                        csproj
                            .map(|path| read_installed_packages(&path))
                            .unwrap_or_default()
                    }
                }),
                cx.background_spawn({
                    let dir = dir.clone();
                    async move { list_outdated(dir) }
                }),
                cx.background_spawn({
                    let dir = dir.clone();
                    async move { list_vulnerable(dir) }
                }),
            );

            let _ = cx.update(|app| {
                if let Some(panel) = this.upgrade() {
                    let _ = panel.update(app, |panel, cx| {
                        panel.installed = installed;
                        if !silent {
                            match outdated {
                                Ok(list) => panel.outdated = list,
                                Err(e) => panel.outdated_error = Some(e),
                            }
                            match vulnerable {
                                Ok(list) => panel.vulnerable = list,
                                Err(e) => panel.vulnerable_error = Some(e),
                            }
                        } else {
                            // Silent refresh: keep existing lists and just
                            // merge in whatever the probes produced, so a
                            // temporarily failed probe never nukes a page.
                            if let Ok(list) = outdated {
                                panel.outdated = list;
                            }
                            if let Ok(list) = vulnerable {
                                panel.vulnerable = list;
                            }
                        }
                        panel.installed_loading = false;
                        panel.outdated_loading = false;
                        panel.vulnerable_loading = false;
                        panel.reload_in_flight = false;
                        cx.notify();
                    });
                }
            });
        })
        .detach();
    }

    // ── Actions ─────────────────────────────────────────────────────────

    /// The live Script Runner dock panel for this workspace, if it's been
    /// loaded yet (`initialize_panels` creates it shortly after startup).
    fn runner(&self, cx: &App) -> Option<Entity<ScriptRunnerPanel>> {
        let workspace = self.workspace.upgrade()?;
        workspace.read(cx).panel::<ScriptRunnerPanel>(cx)
    }

    /// Installs a new package to an exact version (`dotnet add package`).
    fn install_pkg(
        &mut self,
        name: &str,
        version: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let command = dotnet_command(&["add", "package", name, "--version", version]);
        self.kick_run(&format!("install {name}"), command, window, cx);
    }

    /// Updates an installed package to the given version — `dotnet add
    /// package` with a version upgrades an existing reference in place.
    fn update_pkg(
        &mut self,
        name: &str,
        version: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.install_pkg(name, version, window, cx);
    }

    fn remove_package(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        let command = dotnet_command(&["remove", "package", name]);
        self.kick_run(&format!("remove {name}"), command, window, cx);
    }

    /// Bulk-updates every outdated package — or only the patch-only ones when
    /// `ty` is "patch", else every patch/minor — by running one
    /// `dotnet add package` per target in a single Script Runner run (chained
    /// with `&&`, which the pwsh shell the runner uses understands).
    fn update_all(&mut self, ty: &str, window: &mut Window, cx: &mut Context<Self>) {
        let moves: Vec<(String, String)> = self
            .outdated
            .iter()
            .filter(|o| {
                let kind = classify_update(&o.installed, &o.latest);
                match ty {
                    "patch" => kind == UpdateKind::Patch,
                    _ => matches!(kind, UpdateKind::Patch | UpdateKind::Minor),
                }
            })
            .map(|o| (o.id.clone(), o.latest.clone()))
            .collect();
        if moves.is_empty() {
            return;
        }
        let segments = moves
            .iter()
            .map(|(id, latest)| dotnet_command(&["add", "package", id, "--version", latest]))
            .collect::<Vec<_>>();
        self.kick_run("update all", segments.join(" && "), window, cx);
    }

    /// Runs a `dotnet` command for the active project, streaming its output
    /// through the live Script Runner dock panel (the npm manager's §4.2
    /// wiring) — opening that dock so the output is visible — and reloads the
    /// package lists when it finishes.
    ///
    /// Falls back to a silent background run with a spinner when the Script
    /// Runner dock panel isn't loaded yet (e.g. the very first click of a
    /// session, before `initialize_panels` has finished).
    fn kick_run(
        &mut self,
        label: &str,
        command: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.running_action.is_some() {
            return;
        }
        if self.no_project_found {
            self.error = Some("No .NET project found in this workspace.".into());
            cx.notify();
            return;
        }
        let dir = self.project_dir();
        self.running_action = Some(label.to_string());
        self.error = None;
        cx.notify();

        let Some(runner) = self.runner(cx) else {
            return self.background_run(command, cx);
        };

        // The runner can only stream one command at a time — never pile a run
        // onto an already-streaming one.
        if runner.read(cx).is_running() {
            self.running_action = None;
            self.error = Some("Script Runner is busy — wait for it to finish first.".into());
            cx.notify();
            return;
        }

        if let Some(workspace) = self.workspace.upgrade() {
            workspace.update(cx, |workspace, cx| {
                workspace.open_panel::<ScriptRunnerPanel>(window, cx);
            });
        }

        runner.update(cx, |runner, cx| runner.run_external(command, dir, cx));

        // Reload the lists when the run finishes: the runner notifies on every
        // output line AND on completion, so this fires exactly then.
        self.run_subscription = Some(cx.observe(&runner, |this, runner, cx| {
            if !runner.read(cx).is_running() {
                this.running_action = None;
                this.reload_visible(cx);
                cx.notify();
            }
        }));

        // A blindingly-fast run can finish between `run_external` above and
        // subscribing, which would miss the completion notify — re-check once.
        if !runner.read(cx).is_running() {
            self.running_action = None;
            self.reload_visible(cx);
            cx.notify();
        }
    }

    /// Fallback when no Script Runner dock panel exists yet: split the shell
    /// command into its `dotnet` invocations (guarding on the `&&`-chained
    /// bulk updates) and run each through the backend synchronously on a
    /// background task, showing only a spinner and an error on failure.
    fn background_run(&mut self, command: String, cx: &mut Context<Self>) {
        let dir = self.project_dir();
        let this = cx.weak_entity();
        cx.spawn(async move |_: WeakEntity<Self>, cx: &mut AsyncApp| {
            let result = cx
                .background_spawn(async move {
                    for segment in command.split(" && ") {
                        let mut parts = segment.split_whitespace();
                        // Every command this panel builds starts with `dotnet`.
                        let _dotnet = parts.next();
                        let args: Vec<String> = parts.map(String::from).collect();
                        dotnet_backend::run_dotnet_args(&args, &dir)?;
                    }
                    Ok::<(), String>(())
                })
                .await;
            let _ = cx.update(|app| {
                if let Some(panel) = this.upgrade() {
                    let _ = panel.update(app, |panel, cx| {
                        panel.running_action = None;
                        match result {
                            Ok(()) => panel.reload_visible(cx),
                            Err(e) => {
                                panel.error = Some(e);
                                cx.notify();
                            }
                        }
                    });
                }
            });
        })
        .detach();
    }
}

/// Builds a shell command string for a `dotnet` invocation.
fn dotnet_command(parts: &[&str]) -> String {
    let mut command = "dotnet".to_string();
    for part in parts {
        command.push(' ');
        command.push_str(part);
    }
    command
}

fn fallback_name(root: &str) -> String {
    Path::new(root)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "main".into())
}

impl EventEmitter<()> for NuGetManagerPanel {}

impl Focusable for NuGetManagerPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Item for NuGetManagerPanel {
    type Event = ();

    fn tab_content_text(&self, _detail: usize, _cx: &App) -> gpui::SharedString {
        "NuGet Manager".into()
    }

    fn tab_tooltip_text(&self, _: &App) -> Option<gpui::SharedString> {
        Some(format!("NuGet Manager — {}", self.project_dir).into())
    }
}

impl SerializableItem for NuGetManagerPanel {
    fn serialized_item_kind() -> &'static str {
        "nuget_manager_panel"
    }

    /// Persisting the tab just records its presence in the pane; the scan
    /// root is always the project's first worktree, so nothing extra is
    /// written and nothing needs cleaning up.
    fn cleanup(
        _workspace_id: WorkspaceId,
        _alive_items: Vec<ItemId>,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Task<anyhow::Result<()>> {
        Task::ready(Ok(()))
    }

    fn deserialize(
        project: gpui::Entity<project::Project>,
        workspace: WeakEntity<Workspace>,
        _workspace_id: WorkspaceId,
        _item_id: ItemId,
        window: &mut Window,
        cx: &mut App,
    ) -> Task<anyhow::Result<gpui::Entity<Self>>> {
        let root = project
            .read(cx)
            .worktrees(cx)
            .next()
            .map(|worktree| worktree.read(cx).abs_path().to_string_lossy().into_owned())
            .unwrap_or_else(|| {
                std::env::current_dir()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default()
            });

        window.spawn(cx, async move |cx| {
            cx.update(|window, cx| {
                Ok(cx.new(|cx| NuGetManagerPanel::new(root, workspace, window, cx)))
            })?
        })
    }

    /// Nothing is persisted beyond the item's presence in the pane.
    fn serialize(
        &mut self,
        _workspace: &mut Workspace,
        _item_id: ItemId,
        _closing: bool,
        _cx: &mut gpui::Context<Self>,
    ) -> Option<Task<anyhow::Result<()>>> {
        None
    }

    fn should_serialize(&self, _event: &Self::Event) -> bool {
        false
    }
}

impl Render for NuGetManagerPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let view = cx.entity();

        let body = if self.scanning {
            h_flex()
                .gap_2()
                .items_center()
                .p_4()
                .text_xs()
                .child(Spinner::new().with_size(Size::XSmall))
                .child(
                    div()
                        .text_color(theme.muted_foreground)
                        .child("Scanning for .NET projects…"),
                )
                .into_any_element()
        } else if self.no_project_found {
            // The scan finished but found no `.csproj` anywhere under the scan
            // root — show a clear notice instead of a phantom package list
            // with three empty (or error-swallowed) package lists.
            v_flex()
                .gap_2()
                .p_4()
                .text_xs()
                .child(
                    div()
                        .font_semibold()
                        .text_color(theme.muted_foreground)
                        .child("No .NET project found in this workspace."),
                )
                .child(
                    Button::new("nuget-rescan")
                        .icon(IconName::Redo2)
                        .label("Rescan")
                        .with_size(Size::Small)
                        .on_click({
                            let view = view.clone();
                            move |_, _, cx| {
                                view.update(cx, |panel, cx| panel.rescan(cx));
                            }
                        }),
                )
                .into_any_element()
        } else {
            let settings =
                Settings::new("nuget-package-manager").pages(pages::build_all(self, &self.pages));
            if self.selected.is_some() {
                h_resizable("nuget-details-split")
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
        };

        v_flex()
            .id("nuget-manager-panel")
            .track_focus(&self.focus_handle)
            .size_full()
            .pl_2()
            .bg(theme.background)
            .child(body)
    }
}
