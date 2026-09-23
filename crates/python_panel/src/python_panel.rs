//! Python panel — runtime detection, multi-project switching, environment/
//! venv info, installed/outdated/vulnerable package lists, framework
//! detection, quick actions, and a live process list.
//!
//! Brought up to the same level of detail as `dotnet_panel` (Projects,
//! Packages, Outdated, Vulnerabilities sections) — previously this panel
//! only showed a package *count*, no per-project switching, and no
//! outdated/vulnerability visibility at all. The multi-project scanner
//! (`python_backend::scan_python_projects`/`DetectedPythonProject`/
//! `EnvMarker`) and the vulnerability-scan HTTP plumbing
//! (`python_manager_panel::fetch_all_vulnerabilities`) are both shared with
//! `python_manager_panel` rather than duplicated — this panel already
//! depends on that crate for its "Package Manager" quick-action button, so
//! reusing its vulnerability fetcher doesn't add a new dependency edge.
//!
//! Ported standalone (no host-app wiring yet): the source panel depended on
//! a host `AppState` for four things, none of which exist here:
//!
//! - The initial/live-updating workspace root (`state.workspace_root`,
//!   `state.workspace_tx`) — this panel just uses its own process cwd
//!   instead, same as the other standalone-first ports.
//! - A live Python-process list, fed by subscribing to the Cockpit panel's
//!   own system-metrics tick broadcast and filtering it to "Python"
//!   (`state.tick_tx`). Reimplemented here as this panel's own lightweight
//!   `sysinfo` poll filtered by process name instead of cross-panel
//!   plumbing — see `spawn_processes_poll`.
//! - `run_in_script_runner` — the framework/entry-point quick-action
//!   buttons' "run this command" hook. `zed::zed::initialize_panels` hands
//!   this panel a `WeakEntity<ScriptRunnerPanel>` once both panels have
//!   loaded (see `set_script_runner`), so `dispatch_script` sends runs
//!   there for real — mirrors `node_panel`'s equivalent wiring.
//! - `open_python_manager` — re-added: opens the `python_manager_panel`
//!   workspace tab (installed/outdated/vulnerabilities, exact-name PyPI
//!   lookup, install/uninstall/update through the Script Runner), mirroring
//!   `node_panel`'s "Package Manager"/`dotnet_panel`'s "NuGet Manager"
//!   buttons. `open_task_chain_wizard` stays dropped — unrelated feature,
//!   no near-term port planned.
//!
//! Vulnerability scanning is on-demand (a button, not automatic on every
//! project switch) — same disclosed tradeoff `python_manager_panel`
//! documents: PyPI has no bulk-audit endpoint, so a scan is one HTTP
//! request per installed package.
//!
//! The actual detection/scanning/parsing logic lives in `python_backend`,
//! ported alongside this panel — see that crate's own doc comment.

use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use anyhow::Result;
use gpui::{
    Action, App, AppContext as _, AsyncApp, AsyncWindowContext, Context, Entity, EventEmitter,
    FocusHandle, Focusable, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _,
    Pixels, Render, StatefulInteractiveElement as _, Styled as _, WeakEntity, Window, actions, div,
    prelude::FluentBuilder as _, px, svg,
};
use gpui_component::{
    ActiveTheme as _, Disableable as _, Icon, IconName, Sizable as _,
    button::{Button, ButtonVariants as _},
    collapsible::Collapsible,
    h_flex,
    label::Label,
    spinner::Spinner,
    tag::Tag,
    v_flex,
};
use python_backend::{
    DetectedPythonProject, PyPiVulnerability, PythonFramework, PythonOutdatedPkg, PythonPackage,
    UpdateKind, count_requirements, detect_framework, find_missing_deps, list_outdated,
    list_packages, query_pip, query_python, scan_python_project, scan_python_projects,
};
use script_runner_panel::ScriptRunnerPanel;
use sysinfo::System;
use workspace::{
    Workspace,
    dock::{DockPosition, Panel, PanelEvent},
};

/// How often the local Python-process poll re-scans the system.
const PYTHON_PROCS_TICK_INTERVAL: Duration = Duration::from_secs(2);

/// One Python-related process row — a process whose name contains "python"
/// (case-insensitive).
struct PythonProcess {
    name: String,
    pid: u32,
    cpu_percent: f32,
    memory_mb: f64,
}

/// A background-loaded list with its three UI states — mirrors
/// `dotnet_panel::PackagesState`.
enum PackagesState<T> {
    Loading,
    Ready(Vec<T>),
    Error(String),
}

// ── Panel ──────────────────────────────────────────────────────────────

actions!(
    python_panel,
    [
        /// Toggles focus on the Python panel.
        ToggleFocus
    ]
);

/// Registers the Python panel's actions on every workspace. Call once at
/// app startup, alongside the other panels' `init` functions.
pub fn init(cx: &mut App) {
    cx.observe_new(|workspace: &mut Workspace, _, _| {
        workspace.register_action(|workspace, _: &ToggleFocus, window, cx| {
            workspace.toggle_panel_focus::<PythonPanel>(window, cx);
        });
    })
    .detach();
}

pub struct PythonPanel {
    focus_handle: FocusHandle,
    workspace: WeakEntity<Workspace>,
    /// Where script runs (framework/entry-point quick actions, bulk
    /// updates) get dispatched. Handed in by `zed::zed::initialize_panels`
    /// once both this panel and the Script Runner panel are loaded — see
    /// `set_script_runner`.
    script_runner: Option<WeakEntity<ScriptRunnerPanel>>,

    /// Scan root — every detected sub-project lives under this.
    cwd: String,
    projects: Vec<DetectedPythonProject>,
    /// The currently active project's directory — packages/outdated/
    /// vulnerabilities/framework are all scoped to this, not always `cwd`.
    /// `None` when no sub-project was detected (falls back to `cwd` itself
    /// via `active_project_dir`).
    selected_project: Option<String>,
    scanning: bool,

    /// Interpreter resolved for the active project: its own venv when one
    /// exists, else the first working `python`/`python3` on PATH.
    python_exe: Option<String>,
    /// True when `python_exe` is a project-local venv interpreter rather
    /// than a global fallback.
    using_venv: bool,

    py_ver: Option<String>,
    pip_ver: Option<String>,
    venv_path: Option<String>,
    framework: Option<PythonFramework>,
    entry_points: Vec<(String, String)>, // (file, dir)

    installed_pkgs: Vec<PythonPackage>,
    outdated: PackagesState<PythonOutdatedPkg>,

    /// Known vulnerabilities per installed package name, populated by
    /// `scan_vulnerabilities`. Flat map (not `PackagesState`) since a scan
    /// is user-triggered, not part of the automatic project-load flow.
    vulns: HashMap<String, Vec<PyPiVulnerability>>,
    vuln_scanning: bool,
    /// `None`/`false` until the first scan completes — distinguishes
    /// "never scanned" from "scanned, zero found" in the UI.
    vuln_scanned: bool,
    vuln_scan_error: Option<String>,

    /// First `requirements.txt` found by the last project reload, if any —
    /// kept so [`Self::recompute_missing_deps`] can be re-run whenever
    /// either half of its inputs (this or `installed_pkgs`) changes,
    /// without re-scanning the whole project.
    requirements_path: Option<String>,
    /// `count_requirements`'s result for `requirements_path`.
    requirements_count: usize,
    missing_deps: Vec<String>,

    python_procs: Vec<PythonProcess>,

    open: HashMap<String, bool>,
    _procs_poll: gpui::Task<()>,
}

/// Displays a path by its final component (folder/file name), falling back
/// to the full path when it has none — shared by the dock panels' dashboard
/// accessors.
fn path_file_name(dir: &str) -> String {
    std::path::Path::new(dir)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| dir.to_string())
}

impl PythonPanel {
    /// Detected Python version, for a future dashboard-style "runtime
    /// versions" summary (the source panel's Cockpit dashboard exposed this
    /// the same way, via a plain accessor).
    pub fn python_version(&self) -> Option<&str> {
        self.py_ver.as_deref()
    }

    /// Every detected Python sub-project, for the Cockpit dashboard's
    /// Runtimes section (matches `node_panel`/`dotnet_panel`'s equivalent).
    pub fn detected_projects(&self) -> &[DetectedPythonProject] {
        &self.projects
    }

    /// Whether a vulnerability scan has completed at least once for the
    /// active project — Python's scan is on-demand (a button), unlike
    /// Node/`.NET`'s automatic-on-load scan, so the dashboard needs to tell
    /// "never scanned" apart from "scanned, zero found".
    pub fn vulnerabilities_scanned(&self) -> bool {
        self.vuln_scanned
    }

    /// Loads the panel for a workspace, following the same
    /// `WeakEntity<Workspace>` + `AsyncWindowContext` convention as the
    /// other dock panels' `load` functions (see `initialize_panels` in
    /// `zed::zed`), so it can be added to the dock alongside them.
    pub async fn load(
        workspace: WeakEntity<Workspace>,
        mut cx: AsyncWindowContext,
    ) -> Result<Entity<Self>> {
        workspace.update_in(&mut cx, |workspace, window, cx| {
            PythonPanel::new(workspace, window, cx)
        })
    }

    pub fn new(
        _workspace: &mut Workspace,
        _window: &mut Window,
        cx: &mut Context<Workspace>,
    ) -> Entity<Self> {
        let workspace = cx.entity().downgrade();

        cx.new(|cx| {
            let cwd = std::env::current_dir()
                .map(|p| p.display().to_string())
                .unwrap_or_default();

            let mut open = HashMap::new();
            // Keep all sections open by default
            open.insert("Projects".to_string(), true);
            open.insert("Environment".to_string(), true);
            open.insert("Framework".to_string(), true);
            open.insert("Quick Actions".to_string(), true);
            open.insert("Packages".to_string(), true);
            open.insert("Outdated".to_string(), true);
            open.insert("Vulnerabilities".to_string(), true);
            open.insert("Processes".to_string(), true);

            let mut panel = PythonPanel {
                focus_handle: cx.focus_handle(),
                workspace,
                script_runner: None,
                cwd: cwd.clone(),
                projects: Vec::new(),
                selected_project: None,
                scanning: false,
                python_exe: None,
                using_venv: false,
                py_ver: None,
                pip_ver: None,
                venv_path: None,
                framework: None,
                entry_points: Vec::new(),
                installed_pkgs: Vec::new(),
                outdated: PackagesState::Ready(Vec::new()),
                vulns: HashMap::new(),
                vuln_scanning: false,
                vuln_scanned: false,
                vuln_scan_error: None,
                requirements_path: None,
                requirements_count: 0,
                missing_deps: Vec::new(),
                python_procs: Vec::new(),
                open,
                _procs_poll: Self::spawn_processes_poll(cx),
            };

            if !cwd.is_empty() {
                panel.scan_projects(cx);
            }

            panel
        })
    }

    fn is_open(&self, key: &str, default: bool) -> bool {
        *self.open.get(key).unwrap_or(&default)
    }

    fn toggle(&mut self, key: &str) {
        let e = self.open.entry(key.to_string()).or_insert(true);
        *e = !*e;
    }

    /// The directory quick actions/reloads operate on: the selected
    /// sub-project when one was detected, else the whole scan root.
    fn active_project_dir(&self) -> String {
        self.selected_project
            .clone()
            .unwrap_or_else(|| self.cwd.clone())
    }

    /// Called once by `zed::zed::initialize_panels` after both this panel
    /// and the Script Runner panel have loaded, so `dispatch_script` has
    /// somewhere to send runs — mirrors `node_panel::set_script_runner`.
    pub fn set_script_runner(&mut self, script_runner: WeakEntity<ScriptRunnerPanel>) {
        self.script_runner = Some(script_runner);
    }

    /// Sends `command` to the Script Runner panel, run from `cwd`.
    fn dispatch_script(
        &mut self,
        command: &str,
        cwd: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(script_runner) = self.script_runner.as_ref().and_then(|w| w.upgrade()) else {
            return;
        };
        // The runner can only stream one command at a time — see
        // `node_panel::dispatch_script`'s identical guard for why.
        if script_runner.read(cx).is_running() {
            return;
        }
        // Surface the run: reveal the Script Runner dock panel. Uses the
        // entity-agnostic `App::defer` (not `WeakEntity::update_in`, and
        // not `Context::defer_in`) — both of those re-lease `self`
        // (PythonPanel) to invoke their callback, and `reveal_panel` reads
        // every docked panel's zoom state including this one, which
        // panics with a "double lease" error if it's still leased. See
        // `node_panel::dispatch_script`'s identical fix for the full
        // writeup.
        let workspace = self.workspace.clone();
        let window_handle = window.window_handle();
        cx.defer(move |app| {
            let _ = app.update_window(window_handle, |_, window, cx| {
                if let Some(workspace) = workspace.upgrade() {
                    workspace.update(cx, |workspace, cx| {
                        workspace.reveal_panel::<ScriptRunnerPanel>(window, cx);
                    });
                }
            });
        });

        let command = command.to_string();
        script_runner.update(cx, |panel, cx| {
            panel.run_external(command, cwd, cx);
        });
    }

    /// Bulk-updates every outdated package whose risk is `Patch` (or
    /// `Patch`/`Minor` combined, when `ty` isn't `"patch"`) — one
    /// `pip install --upgrade "name==latest"` per target, chained with
    /// `&&` in a single Script Runner run. Mirrors
    /// `dotnet_panel::update_all`.
    fn update_all(&mut self, ty: &str, window: &mut Window, cx: &mut Context<Self>) {
        let PackagesState::Ready(list) = &self.outdated else {
            return;
        };
        let moves: Vec<(String, String)> = list
            .iter()
            .filter(|o| match ty {
                "patch" => o.kind == UpdateKind::Patch,
                _ => matches!(o.kind, UpdateKind::Patch | UpdateKind::Minor),
            })
            .map(|o| (o.name.clone(), o.latest_version.clone()))
            .collect();
        if moves.is_empty() {
            return;
        }
        let exe = self
            .python_exe
            .clone()
            .unwrap_or_else(|| "python".to_string());
        let segments = moves
            .iter()
            .map(|(name, latest)| format!("{exe} -m pip install --upgrade \"{name}=={latest}\""))
            .collect::<Vec<_>>();
        let dir = self.active_project_dir();
        self.dispatch_script(&segments.join(" && "), dir, window, cx);
    }

    /// Fires one `https://pypi.org/pypi/<name>/<version>/json` request per
    /// installed package (capped concurrency, no bulk endpoint exists) via
    /// `python_manager_panel::fetch_all_vulnerabilities` — reused as-is
    /// rather than duplicated, since this panel already depends on that
    /// crate.
    fn scan_vulnerabilities(&mut self, cx: &mut Context<Self>) {
        if self.vuln_scanning || self.installed_pkgs.is_empty() {
            return;
        }
        let pkgs: Vec<(String, String)> = self
            .installed_pkgs
            .iter()
            .map(|p| (p.name.clone(), p.version.clone()))
            .collect();
        self.vuln_scanning = true;
        self.vuln_scan_error = None;
        cx.notify();

        let http_client = cx.http_client();
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let result = python_manager_panel::fetch_all_vulnerabilities(&http_client, pkgs).await;

            let _ = cx.update(|app| {
                if let Some(panel) = this.upgrade() {
                    let _ = panel.update(app, |panel, cx| {
                        panel.vuln_scanning = false;
                        panel.vuln_scanned = true;
                        match result {
                            Ok(map) => {
                                panel.vulns = map;
                                panel.vuln_scan_error = None;
                            }
                            Err(e) => panel.vuln_scan_error = Some(e),
                        }
                        cx.notify();
                    });
                }
            });
        })
        .detach();
    }

    /// Refreshes `python_procs` from a locally-owned `sysinfo::System` every
    /// `PYTHON_PROCS_TICK_INTERVAL`, filtered to process names containing
    /// "python" — replaces subscribing to a host cockpit panel's broadcast
    /// tick (see the module doc).
    fn spawn_processes_poll(cx: &mut Context<Self>) -> gpui::Task<()> {
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let mut sys = System::new_all();
            loop {
                sys.refresh_cpu_all();
                sys.refresh_processes(sysinfo::ProcessesToUpdate::All, true);
                let cpu_count = sys.cpus().len() as f32;
                let procs: Vec<PythonProcess> = sys
                    .processes()
                    .iter()
                    .filter(|(_, p)| p.name().to_string_lossy().to_lowercase().contains("python"))
                    .map(|(pid, p)| PythonProcess {
                        name: p.name().to_string_lossy().to_string(),
                        pid: pid.as_u32(),
                        cpu_percent: p.cpu_usage() / cpu_count,
                        memory_mb: p.memory() as f64 / 1_048_576.0,
                    })
                    .collect();

                let alive = this
                    .update(cx, |this, cx| {
                        this.python_procs = procs;
                        cx.notify();
                    })
                    .is_ok();
                if !alive {
                    break;
                }

                cx.background_executor()
                    .timer(PYTHON_PROCS_TICK_INTERVAL)
                    .await;
            }
        })
    }

    /// Diffs `requirements_path` against `installed_pkgs` into
    /// `missing_deps`. Cheap enough (a single file read + set diff) to run
    /// synchronously.
    fn recompute_missing_deps(&mut self) {
        self.missing_deps =
            find_missing_deps(self.requirements_path.as_deref(), &self.installed_pkgs)
                .unwrap_or_default();
    }

    /// Scans `cwd` for sub-projects, picks (or keeps) the selected one, then
    /// reloads everything scoped to it. The initial entry point — replaces
    /// the old disjoint `detect_python`/`load_packages`/`load_pip_version`/
    /// `scan_project` tasks with one project-aware flow, mirroring
    /// `dotnet_panel::scan_projects` and `python_manager_panel`'s
    /// `schedule_load`.
    fn scan_projects(&mut self, cx: &mut Context<Self>) {
        if self.cwd.is_empty() {
            self.projects.clear();
            return;
        }
        self.scanning = true;
        cx.notify();

        let root = self.cwd.clone();
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let projects = cx
                .background_spawn(async move { scan_python_projects(&root, 0) })
                .await;
            let _ = cx.update(|app| {
                if let Some(panel) = this.upgrade() {
                    let _ = panel.update(app, |panel, cx| {
                        panel.projects = projects;
                        if panel.selected_project.is_none() {
                            panel.selected_project = panel.projects.first().map(|p| p.path.clone());
                        }
                        panel.reload_project_data(cx);
                    });
                }
            });
        })
        .detach();
    }

    /// Selects a different detected project and rescopes everything to it.
    fn select_project(&mut self, path: String, cx: &mut Context<Self>) {
        self.selected_project = Some(path);
        self.reload_project_data(cx);
    }

    /// Re-derives everything scoped to `active_project_dir()`: the resolved
    /// interpreter (project venv, else global `python`/`python3`), Python/
    /// pip versions, framework/entry points, installed + outdated packages,
    /// and the missing-deps diff. One background task, mirroring
    /// `python_manager_panel::schedule_load`'s consolidated shape rather
    /// than the several separately-racing tasks this panel used to run.
    fn reload_project_data(&mut self, cx: &mut Context<Self>) {
        self.scanning = true;
        self.outdated = PackagesState::Loading;
        // Vulnerabilities are keyed by installed-package name/version and
        // become stale the moment the active project (and therefore its
        // installed set) changes — clear them rather than show a scan
        // result for a different project's packages.
        self.vulns.clear();
        self.vuln_scanned = false;
        self.vuln_scan_error = None;
        cx.notify();

        let scan_root = self.active_project_dir();
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let scan_root_for_task = scan_root.clone();
            let result = cx
                .background_spawn(async move {
                    let scan = scan_python_project(&scan_root_for_task).ok();
                    let venv_exe = scan.as_ref().and_then(|s| s.venvs.first().cloned());

                    let (exe, using_venv) = if let Some(venv) = venv_exe {
                        (Some(venv), true)
                    } else {
                        let mut found = None;
                        for candidate in ["python", "python3"] {
                            if query_python(candidate).is_ok() {
                                found = Some(candidate.to_string());
                                break;
                            }
                        }
                        (found, false)
                    };

                    let (py_ver, pip_ver, installed, outdated) = match &exe {
                        Some(exe) => (
                            query_python(exe).ok(),
                            query_pip(exe).ok(),
                            list_packages(exe).unwrap_or_default(),
                            list_outdated(exe),
                        ),
                        None => (None, None, Vec::new(), Ok(Vec::new())),
                    };

                    let framework = scan
                        .as_ref()
                        .and_then(|s| detect_framework(&s.requirements, &s.entry_points));
                    let entry_points = scan
                        .as_ref()
                        .map(|s| s.entry_points.clone())
                        .unwrap_or_default();
                    let requirements_path =
                        scan.as_ref().and_then(|s| s.requirements.first().cloned());
                    let requirements_count =
                        count_requirements(requirements_path.as_deref()).unwrap_or(0);

                    (
                        exe,
                        using_venv,
                        py_ver,
                        pip_ver,
                        installed,
                        outdated,
                        framework,
                        entry_points,
                        requirements_path,
                        requirements_count,
                    )
                })
                .await;

            let (
                exe,
                using_venv,
                py_ver,
                pip_ver,
                installed,
                outdated,
                framework,
                entry_points,
                requirements_path,
                requirements_count,
            ) = result;

            let _ = cx.update(|app| {
                if let Some(panel) = this.upgrade() {
                    let _ = panel.update(app, |panel, cx| {
                        panel.scanning = false;
                        panel.python_exe = exe;
                        panel.using_venv = using_venv;
                        panel.venv_path = if using_venv {
                            panel.python_exe.clone()
                        } else {
                            None
                        };
                        panel.py_ver = py_ver;
                        panel.pip_ver = pip_ver;
                        panel.installed_pkgs = installed;
                        panel.outdated = match outdated {
                            Ok(list) => PackagesState::Ready(list),
                            Err(e) => PackagesState::Error(e),
                        };
                        panel.framework = framework;
                        panel.entry_points = entry_points;
                        panel.requirements_path = requirements_path;
                        panel.requirements_count = requirements_count;
                        panel.recompute_missing_deps();
                        cx.notify();
                    });
                }
            });
        })
        .detach();
    }
}

impl Focusable for PythonPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<PanelEvent> for PythonPanel {}

impl Panel for PythonPanel {
    fn persistent_name() -> &'static str {
        "Python Panel"
    }

    fn panel_key() -> &'static str {
        "PythonPanel"
    }

    fn position(&self, _window: &Window, _cx: &App) -> DockPosition {
        DockPosition::Left
    }

    fn position_is_valid(&self, position: DockPosition) -> bool {
        matches!(position, DockPosition::Left)
    }

    fn set_position(
        &mut self,
        _position: DockPosition,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        // Fixed to the left dock — see `position_is_valid`.
    }

    fn default_size(&self, _window: &Window, _cx: &App) -> Pixels {
        px(280.)
    }

    fn icon(&self, _window: &Window, _cx: &App) -> Option<ui::IconName> {
        Some(ui::IconName::Python)
    }

    fn icon_tooltip(&self, _window: &Window, _cx: &App) -> Option<&'static str> {
        Some("Python")
    }

    fn toggle_action(&self) -> Box<dyn Action> {
        Box::new(ToggleFocus)
    }

    fn activation_priority(&self) -> u32 {
        10
    }
}

impl Render for PythonPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();

        let panel_header = div()
            .flex()
            .flex_col()
            .flex_shrink_0()
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .px_3()
                    .py_2()
                    .child(
                        svg()
                            .path("icons/python.svg")
                            .size(px(16.0))
                            .text_color(theme.foreground),
                    )
                    .child(
                        div()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_sm()
                            .text_color(theme.foreground)
                            .child("Python"),
                    ),
            )
            .child(div().h(px(1.0)).w_full().bg(theme.border));

        if self.py_ver.is_none() && !self.scanning {
            return div()
                .id("python-panel-empty")
                .track_focus(&self.focus_handle(cx))
                .flex()
                .flex_col()
                .h_full()
                .bg(theme.background)
                .child(panel_header)
                .child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .items_center()
                        .justify_center()
                        .p_4()
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .child("Python not found in PATH"),
                )
                .into_any_element();
        }

        let projects_section = section_container(
            cx,
            "Projects",
            self.projects.len(),
            self.is_open("Projects", true),
            self.render_projects(cx),
        );

        let env_section = section_container(
            cx,
            "Environment",
            0,
            self.is_open("Environment", true),
            self.render_environment(),
        );

        let framework_section = self.framework.as_ref().map(|_| {
            section_container(
                cx,
                "Framework",
                0,
                self.is_open("Framework", true),
                self.render_framework(),
            )
        });

        let actions_section = section_container(
            cx,
            "Quick Actions",
            0,
            self.is_open("Quick Actions", true),
            self.render_quick_actions(cx),
        );

        let packages_section = section_container(
            cx,
            "Packages",
            self.installed_pkgs.len(),
            self.is_open("Packages", true),
            self.render_packages(cx),
        );

        let outdated_section = section_container(
            cx,
            "Outdated",
            self.outdated_count(),
            self.is_open("Outdated", true),
            self.render_outdated(cx),
        );

        let vulnerabilities_section = section_container(
            cx,
            "Vulnerabilities",
            self.vulnerable_count(),
            self.is_open("Vulnerabilities", true),
            self.render_vulnerabilities(cx),
        );

        let processes_section = section_container(
            cx,
            "Processes",
            self.python_procs.len(),
            self.is_open("Processes", true),
            self.render_processes(cx),
        );

        let mut body = div()
            .flex()
            .flex_col()
            .child(self.render_version_info(cx))
            .child(projects_section)
            .child(env_section);

        if let Some(fw_section) = framework_section {
            body = body.child(fw_section);
        }

        body = body
            .child(actions_section)
            .child(packages_section)
            .child(outdated_section)
            .child(vulnerabilities_section)
            .child(processes_section);

        div()
            .id("python-panel")
            .track_focus(&self.focus_handle(cx))
            .flex()
            .flex_col()
            .w_full()
            .h_full()
            .overflow_hidden()
            .bg(theme.background)
            .child(panel_header)
            .child(
                div()
                    .id("python-scroll")
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .overflow_y_scroll()
                    .child(body),
            )
            .into_any_element()
    }
}

// ── Render sections ───────────────────────────────────────────────────

impl PythonPanel {
    /// Outdated-package count, also used by the Cockpit dashboard's
    /// Runtimes section (see `dashboard_panel`).
    pub fn outdated_count(&self) -> usize {
        match &self.outdated {
            PackagesState::Ready(list) => list.len(),
            _ => 0,
        }
    }

    /// Vulnerable-package count, summed across every installed package's
    /// findings. `0` when a scan hasn't run yet — see
    /// `vulnerabilities_scanned` to distinguish "not scanned" from
    /// "scanned, none found". Also used by the Cockpit dashboard's
    /// Runtimes section (see `dashboard_panel`).
    pub fn vulnerable_count(&self) -> usize {
        self.vulns.values().map(|v| v.len()).sum()
    }

    /// Every vulnerability finding, keyed by installed package name. Only
    /// packages with at least one finding appear in the map — the Cockpit
    /// dashboard's Security section renders per-package, per-finding rows
    /// from this.
    pub fn vulnerabilities(&self) -> &HashMap<String, Vec<PyPiVulnerability>> {
        &self.vulns
    }

    /// Whether a vulnerability scan is currently in flight.
    pub fn vulnerabilities_scanning(&self) -> bool {
        self.vuln_scanning
    }

    /// The scan's error message, if the last scan failed.
    pub fn vulnerabilities_scan_error(&self) -> Option<&str> {
        self.vuln_scan_error.as_deref()
    }

    /// The active scan target's display label — the selected sub-project's
    /// folder name when one was detected, else the scan root's.
    pub fn active_project_label(&self) -> String {
        let dir = self.selected_project.as_ref().unwrap_or(&self.cwd);
        path_file_name(dir)
    }

    /// Re-runs the on-demand vulnerability scan, driven by the Cockpit
    /// dashboard's Security section (`scan_vulnerabilities` does the work;
    /// it no-ops while a scan is in flight or no packages are installed).
    pub fn rescan_vulnerabilities(&mut self, cx: &mut Context<Self>) {
        self.scan_vulnerabilities(cx);
    }

    fn render_version_info(&self, cx: &Context<Self>) -> impl IntoElement {
        // `pip_ver` is `pip --version`'s raw stdout, e.g. "pip 26.1.2 from
        // C:\Program Files\WindowsApps\...  (python 3.13)" — the install
        // path/interpreter suffix doesn't fit this compact header line, so
        // just keep the "pip <version>" prefix, up to " from ".
        let pip_short = self
            .pip_ver
            .as_deref()
            .map(|v| v.split(" from ").next().unwrap_or(v).trim().to_string());

        let text = match (&self.py_ver, &pip_short) {
            (Some(py), Some(pip)) => format!("Python {}  |  {}", py, pip),
            (Some(py), None) => format!("Python {}", py),
            _ => "Python not detected".to_string(),
        };

        div()
            .flex()
            .items_center()
            .justify_between()
            .px_3()
            .py_2()
            .border_b_1()
            .border_color(cx.theme().border)
            .child(
                div()
                    .font_family("Cascadia Mono")
                    .text_xs()
                    .text_color(cx.theme().primary)
                    .child(text),
            )
    }

    fn render_projects(&self, cx: &Context<Self>) -> impl IntoElement {
        let mut col = div().flex().flex_col();

        if self.scanning && self.projects.is_empty() {
            col = col.child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .px_3()
                    .py_2()
                    .child(Spinner::new().xsmall())
                    .child(
                        div()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child("Scanning\u{2026}"),
                    ),
            );
        } else if self.projects.is_empty() {
            col = col.child(
                div()
                    .px_3()
                    .py_2()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child("No Python sub-projects found — using the scan root"),
            );
        } else {
            for proj in &self.projects {
                let selected = self.selected_project.as_deref() == Some(&proj.path);
                let color = if selected {
                    cx.theme().primary
                } else {
                    cx.theme().foreground
                };
                let proj_path = proj.path.clone();
                let marker_label = proj.marker.file_name();
                col = col.child(
                    div()
                        .id(format!("py-proj-{}", proj.path))
                        .flex()
                        .items_center()
                        .gap_2()
                        .px_3()
                        .py_0p5()
                        .text_xs()
                        .cursor_pointer()
                        .when(selected, |d| d.bg(cx.theme().list_active))
                        .hover(|d| d.bg(cx.theme().list_hover))
                        .on_click(cx.listener(move |this, _e, _w, cx| {
                            this.select_project(proj_path.clone(), cx);
                        }))
                        .child(
                            div()
                                .flex_1()
                                .truncate()
                                .font_family("Cascadia Mono")
                                .text_color(color)
                                .child(proj.name.clone()),
                        )
                        .child(Tag::secondary().xsmall().child(marker_label)),
                );
            }
        }

        col
    }

    fn render_environment(&self) -> impl IntoElement {
        let mut content = v_flex().w_full().gap_1();

        if !self.missing_deps.is_empty() {
            content = content.child(div().w_full().flex().items_center().gap_1().child(
                Tag::danger().xsmall().child(format!(
                    "{} missing from requirements.txt",
                    self.missing_deps.len()
                )),
            ));
        }
        if self.requirements_count > 0 {
            content = content.child(div().w_full().child(
                Label::new(format!("{} in requirements.txt", self.requirements_count)).text_xs(),
            ));
        }

        if let Some(venv) = &self.venv_path {
            content = content.child(
                div()
                    .w_full()
                    .child(Label::new("venv").text_xs().secondary(venv.as_str())),
            );
        } else if self.python_exe.is_some() {
            content = content.child(div().w_full().child(
                Label::new("Using the global interpreter (no project venv found)").text_xs(),
            ));
        }

        if self.missing_deps.is_empty() && self.requirements_count == 0 && self.venv_path.is_none()
        {
            content = content.child(
                div()
                    .w_full()
                    .child(Label::new("No environment details for this project").text_xs()),
            );
        }

        content.w_full().px_3().py_2()
    }

    fn render_framework(&self) -> impl IntoElement {
        if let Some(fw) = &self.framework {
            h_flex()
                .gap_2()
                .items_center()
                .px_3()
                .py_2()
                .child(div().text_base().child(fw.icon.clone()))
                .child(Tag::primary().child(format!("{} detected", fw.name)))
                .into_any_element()
        } else {
            div().into_any_element()
        }
    }

    fn render_quick_actions(&self, cx: &Context<Self>) -> impl IntoElement {
        let mut row = div().flex().flex_row().flex_wrap().gap_1().px_3().py_1();

        // Framework quick action button: runs the framework's dev command
        // in the active project's directory via the Script Runner panel.
        if let Some(fw) = &self.framework {
            let label = fw.name.to_lowercase();
            let cmd = fw.dev_command.clone();
            let cwd = self.active_project_dir();
            row = row.child(
                Button::new(format!("py-quick-{label}"))
                    .secondary()
                    .xsmall()
                    .label(label)
                    .tooltip(format!("Run \"{cmd}\""))
                    .on_click(cx.listener(move |this, _e, window, cx| {
                        this.dispatch_script(&cmd, cwd.clone(), window, cx);
                    })),
            );
        }

        // Entry point buttons: run `python <file>` from that entry point's
        // own directory, matching the source's `dispatch(&cmd, dir)`.
        for (idx, (file, dir)) in self.entry_points.iter().enumerate() {
            let filename = Path::new(file)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(file)
                .to_string();
            let cmd = format!("python {file}");
            let cwd = dir.clone();
            row = row.child(
                Button::new(format!("py-quick-entry-{idx}"))
                    .secondary()
                    .xsmall()
                    .label(filename)
                    .tooltip(format!("Run \"{cmd}\""))
                    .on_click(cx.listener(move |this, _e, window, cx| {
                        this.dispatch_script(&cmd, cwd.clone(), window, cx);
                    })),
            );
        }

        // `open_python_manager` re-added: opens the Python Package Manager
        // workspace tab rooted on the active project. Mirrors
        // `node_panel`'s "Package Manager" button exactly — uses the
        // `Window` already handed to this click handler rather than
        // `WeakEntity::update_in`'s own window lookup.
        row = row.child(
            Button::new("py-quick-pymgr")
                .secondary()
                .xsmall()
                .label("Package Manager")
                .tooltip("Open Python Package Manager")
                .on_click(cx.listener(|this, _e, window, cx| {
                    if let Some(workspace) = this.workspace.upgrade() {
                        let root = this.active_project_dir();
                        workspace.update(cx, |workspace, cx| {
                            if !root.is_empty() {
                                python_manager_panel::open(root, workspace, window, cx);
                            }
                        });
                    }
                })),
        );

        // Show scanning state or no actions message
        if self.framework.is_none() && self.entry_points.is_empty() {
            if self.scanning {
                row = row.child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .px_3()
                        .py_2()
                        .child(Spinner::new().xsmall())
                        .child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child("Scanning\u{2026}"),
                        ),
                );
            } else {
                row = row.child(
                    div()
                        .px_3()
                        .py_2()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child("No framework or entry points detected"),
                );
            }
        }

        row
    }

    fn render_packages(&self, cx: &Context<Self>) -> impl IntoElement {
        let mut col = div().flex().flex_col();

        if self.installed_pkgs.is_empty() {
            col = col.child(
                div()
                    .px_3()
                    .py_2()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child("No installed packages"),
            );
        } else {
            for pkg in &self.installed_pkgs {
                col = col.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .px_3()
                        .py_0p5()
                        .text_xs()
                        .child(
                            div()
                                .flex_1()
                                .truncate()
                                .font_family("Cascadia Mono")
                                .text_color(cx.theme().foreground)
                                .child(pkg.name.clone()),
                        )
                        .child(
                            div()
                                .font_family("Cascadia Mono")
                                .text_color(cx.theme().muted_foreground)
                                .child(format!("@{}", pkg.version)),
                        ),
                );
            }
        }

        col
    }

    fn render_outdated(&self, cx: &Context<Self>) -> impl IntoElement {
        let mut col = div().flex().flex_col();

        match &self.outdated {
            PackagesState::Loading => {
                col = col.child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .px_3()
                        .py_2()
                        .child(Spinner::new().xsmall())
                        .child(
                            div()
                                .text_xs()
                                .text_color(cx.theme().muted_foreground)
                                .child("Checking for updates\u{2026}"),
                        ),
                );
            }
            PackagesState::Error(err) => {
                col = col.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .px_3()
                        .py_1()
                        .text_xs()
                        .child(
                            div()
                                .flex_1()
                                .truncate()
                                .text_color(cx.theme().danger)
                                .child(err.clone()),
                        ),
                );
            }
            PackagesState::Ready(list) => {
                if list.is_empty() {
                    col = col.child(
                        div()
                            .px_3()
                            .py_2()
                            .text_xs()
                            .text_color(cx.theme().muted_foreground)
                            .child("All packages up to date"),
                    );
                } else {
                    col = col.child(
                        h_flex()
                            .gap_1()
                            .flex_wrap()
                            .items_center()
                            .px_3()
                            .py_1()
                            .child(
                                Button::new("py-update-all-minor")
                                    .secondary()
                                    .xsmall()
                                    .label("Update all (minor)")
                                    .on_click(cx.listener(|this, _e, window, cx| {
                                        this.update_all("minor", window, cx);
                                    })),
                            )
                            .child(
                                Button::new("py-update-all-patch")
                                    .secondary()
                                    .xsmall()
                                    .label("Update all (patch)")
                                    .on_click(cx.listener(|this, _e, window, cx| {
                                        this.update_all("patch", window, cx);
                                    })),
                            ),
                    );

                    for pkg in list {
                        // Red is reserved for the Vulnerabilities section —
                        // "major" here doesn't mean "broken", just "bigger
                        // diff to review", so it gets amber instead. Minor
                        // and patch use two distinct, quieter colors rather
                        // than both reading as "fine" in the same shade.
                        let (kind_color, kind_label) = match pkg.kind {
                            UpdateKind::Major => (cx.theme().warning, "major"),
                            UpdateKind::Minor => (cx.theme().info, "minor"),
                            UpdateKind::Patch => (cx.theme().muted_foreground, "patch"),
                        };
                        col = col.child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .px_3()
                                .py_0p5()
                                .text_xs()
                                .child(
                                    div()
                                        .flex_1()
                                        .truncate()
                                        .font_family("Cascadia Mono")
                                        .text_color(cx.theme().foreground)
                                        .child(pkg.name.clone()),
                                )
                                .child(
                                    div()
                                        .flex_shrink_0()
                                        .font_family("Cascadia Mono")
                                        .text_color(cx.theme().muted_foreground)
                                        .child(format!(
                                            "{} \u{2192} {}",
                                            pkg.version, pkg.latest_version
                                        )),
                                )
                                .child(
                                    div()
                                        .rounded_md()
                                        .px_1()
                                        .py_0p5()
                                        .bg(kind_color.opacity(0.2))
                                        .text_color(kind_color)
                                        .child(kind_label),
                                ),
                        );
                    }
                }
            }
        }

        col
    }

    fn render_vulnerabilities(&self, cx: &Context<Self>) -> impl IntoElement {
        let mut col = div().flex().flex_col();

        col = col.child(
            h_flex()
                .gap_2()
                .items_center()
                .px_3()
                .py_1()
                .child(
                    Button::new("py-scan-vulns")
                        .secondary()
                        .xsmall()
                        .label(if self.vuln_scanning {
                            "Scanning\u{2026}"
                        } else {
                            "Scan for vulnerabilities"
                        })
                        .disabled(self.vuln_scanning || self.installed_pkgs.is_empty())
                        .on_click(cx.listener(|this, _e, _w, cx| {
                            this.scan_vulnerabilities(cx);
                        })),
                )
                .when(self.vuln_scanning, |row| row.child(Spinner::new().xsmall())),
        );

        if let Some(err) = &self.vuln_scan_error {
            col = col.child(
                div()
                    .px_3()
                    .py_1()
                    .text_xs()
                    .text_color(cx.theme().danger)
                    .child(err.clone()),
            );
        } else if self.vuln_scanned {
            if self.vulns.is_empty() {
                col = col.child(
                    div()
                        .px_3()
                        .py_2()
                        .text_xs()
                        .text_color(cx.theme().muted_foreground)
                        .child("No known vulnerabilities"),
                );
            } else {
                // No severity tiers here — PyPI's per-version advisory data
                // (unlike `dotnet list package --vulnerable`) doesn't carry
                // a severity field, only an OSV advisory id/summary/fixed-in
                // list. A known vulnerability is exactly what red is for.
                let mut names: Vec<&String> = self.vulns.keys().collect();
                names.sort();
                for name in names {
                    let Some(findings) = self.vulns.get(name) else {
                        continue;
                    };
                    for vuln in findings {
                        col = col.child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .px_3()
                                .py_0p5()
                                .text_xs()
                                .child(
                                    div()
                                        .flex_1()
                                        .truncate()
                                        .font_family("Cascadia Mono")
                                        .text_color(cx.theme().foreground)
                                        .child(format!("{name} — {}", vuln.id)),
                                )
                                .child(
                                    div()
                                        .rounded_md()
                                        .px_1()
                                        .py_0p5()
                                        .bg(cx.theme().danger.opacity(0.2))
                                        .text_color(cx.theme().danger)
                                        .child("vulnerable"),
                                ),
                        );
                        if let Some(summary) = &vuln.summary {
                            col = col.child(
                                div()
                                    .px_3()
                                    .pb_1()
                                    .text_xs()
                                    .text_color(cx.theme().muted_foreground)
                                    .child(summary.clone()),
                            );
                        }
                    }
                }
            }
        } else {
            col = col.child(
                div()
                    .px_3()
                    .py_2()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child("Not scanned yet"),
            );
        }

        col
    }

    fn render_processes(&self, cx: &Context<Self>) -> impl IntoElement {
        let mut col = div().flex().flex_col();

        if self.python_procs.is_empty() {
            col = col.child(
                div()
                    .px_3()
                    .py_2()
                    .text_xs()
                    .text_color(cx.theme().muted_foreground)
                    .child("No Python processes running"),
            );
        } else {
            col = col.child(
                div()
                    .flex()
                    .px_3()
                    .py_0p5()
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(cx.theme().muted_foreground)
                    .child(div().w_16().child("PID"))
                    .child(div().flex_1().child("Name"))
                    .child(div().w_16().text_right().child("CPU"))
                    .child(div().w_16().text_right().child("Mem")),
            );

            for proc in &self.python_procs {
                col = col.child(
                    div()
                        .flex()
                        .px_3()
                        .py_0p5()
                        .text_xs()
                        .text_color(cx.theme().foreground)
                        .child(
                            div()
                                .w_16()
                                .font_family("Cascadia Mono")
                                .child(proc.pid.to_string()),
                        )
                        .child(
                            div()
                                .flex_1()
                                .truncate()
                                .font_family("Cascadia Mono")
                                .child(proc.name.clone()),
                        )
                        .child(
                            div()
                                .w_16()
                                .text_right()
                                .font_family("Cascadia Mono")
                                .child(format!("{:.1}%", proc.cpu_percent)),
                        )
                        .child(
                            div()
                                .w_16()
                                .text_right()
                                .font_family("Cascadia Mono")
                                .child(format!("{:.0}MB", proc.memory_mb)),
                        ),
                );
            }
        }

        col
    }
}

// ── Section Container Helper ──────────────────────────────────────────

fn section_container(
    cx: &Context<PythonPanel>,
    title: &str,
    count: usize,
    open: bool,
    body: impl IntoElement,
) -> impl IntoElement {
    let key = title.to_string();

    let header = div()
        .id(format!("py-section-{key}"))
        .flex()
        .items_center()
        .gap_1()
        .px_3()
        .py_1()
        .text_xs()
        .cursor_pointer()
        .hover(|d| d.bg(cx.theme().list_hover))
        .child(
            Icon::new(if open {
                IconName::ChevronDown
            } else {
                IconName::ChevronRight
            })
            .xsmall()
            .text_color(cx.theme().muted_foreground),
        )
        .child(
            div()
                .flex_1()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(cx.theme().foreground)
                .child(title.to_string()),
        )
        .when(count > 0, |el| {
            el.child(Tag::secondary().xsmall().child(count.to_string()))
        })
        .on_click(cx.listener(move |this, _e, _w, _cx| {
            this.toggle(&key);
        }));

    Collapsible::new()
        .open(open)
        .child(header)
        .content(body)
        .flex()
        .flex_col()
        .w_full()
        .border_b_1()
        .border_color(cx.theme().border)
}
