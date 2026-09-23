//! The Processes panel — a live, sortable table of every process on this
//! machine (PID, name, CPU, memory, source, status), refreshed from a
//! locally-owned `sysinfo::System` on a 2-second timer, with a name/PID
//! search filter and a per-row right-click context menu: Adopt (mark as
//! managed by this panel), Release (stop managing), Kill (confirm-dialog
//! guarded, since it's irreversible).
//!
//! Ported standalone (no host-app wiring yet): the source panel held its
//! shared `sysinfo::System` and "managed" bookkeeping in a host `AppState`
//! (so other panels could also adopt/release processes and see the same
//! managed set); here both live directly on `ProcessesPanel` instead, since
//! nothing else exists yet to share them with.
//!
//! Structured after the `DataTableStory` template in `gpui-component`'s own
//! gallery: a `TableState<ProcessTableDelegate>` drives column layout,
//! sorting, selection and the `PopupMenu` context menu, while the
//! `TableDelegate` impl decides what each column shows.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Result;
use gpui::{
    Action, App, AppContext as _, AsyncWindowContext, ClickEvent, Context, Entity, EventEmitter,
    FocusHandle, Focusable, Hsla, InteractiveElement as _, IntoElement, ParentElement as _,
    Pixels, Render, Styled as _, Subscription, Task, TextAlign, WeakEntity, Window, actions, div,
    prelude::FluentBuilder as _,
};
use gpui_component::{
    ActiveTheme as _, Icon, IconName, Sizable as _, Size, StyledExt, StyleSized as _,
    button::{Button, ButtonCustomVariant, ButtonVariants as _},
    h_flex,
    input::{Input, InputEvent, InputState},
    menu::PopupMenu,
    table::{Column, ColumnFixed, ColumnSort, DataTable, TableDelegate, TableState},
    tag::Tag,
    v_flex,
};
use serde::Deserialize;
use sysinfo::{ProcessesToUpdate, System};
use workspace::{
    Workspace,
    dock::{DockPosition, Panel, PanelEvent},
};

/// How often the process table re-scans the system.
const PROCESSES_TICK_INTERVAL: Duration = Duration::from_secs(2);

/// A shared "adopted" set: which pids this panel considers managed, and
/// under what name/bucket — mirrors the source's `AppState::adopted`, just
/// owned by this panel instead of a host app state.
type Adopted = Arc<Mutex<HashMap<u32, (String, String)>>>;

/// The icon + title heading block with a separator underneath — the same
/// header row every ported `forge_shell` panel starts with.
fn panel_header(
    icon: IconName,
    title: &'static str,
    foreground: Hsla,
    border: Hsla,
) -> impl IntoElement {
    v_flex()
        .flex_shrink_0()
        .child(
            h_flex()
                .gap_2()
                .items_center()
                .px_3()
                .py_2()
                .child(Icon::new(icon).text_color(foreground))
                .child(
                    div()
                        .font_weight(gpui::FontWeight::SEMIBOLD)
                        .text_sm()
                        .text_color(foreground)
                        .child(title),
                ),
        )
        .child(div().h(gpui::px(1.0)).w_full().bg(border))
}

// ── Actions ────────────────────────────────────────────────────────────
// Dispatched by the table's right-click `PopupMenu` at runtime; the panel
// root registers an `.on_action` handler for each, mirroring how the
// `data_table_story` routes `ChangeSize` etc. up to its story root.

#[derive(Action, Clone, PartialEq, Eq, Deserialize)]
#[action(namespace = processes_panel, no_json)]
struct AdoptProcess(u32);

#[derive(Action, Clone, PartialEq, Eq, Deserialize)]
#[action(namespace = processes_panel, no_json)]
struct ReleaseProcess(u32);

#[derive(Action, Clone, PartialEq, Eq, Deserialize)]
#[action(namespace = processes_panel, no_json)]
struct KillProcess(u32);

/// One process row.
#[derive(Clone)]
struct ProcessEntry {
    pid: u32,
    name: String,
    cpu: f32,
    memory_mb: f64,
    source: String,
    status: String,
}

/// List all processes with a `Managed`/`External` source and run state.
fn list_processes(system: &Mutex<System>, adopted: &Adopted) -> Vec<ProcessEntry> {
    let mut sys = system.lock().unwrap();
    sys.refresh_processes(ProcessesToUpdate::All, true);
    let adopted = adopted.lock().unwrap();
    let cpu_count = sys.cpus().len() as f32;
    sys.processes()
        .iter()
        .map(|(pid, proc)| {
            let pid_u32 = pid.as_u32();
            ProcessEntry {
                pid: pid_u32,
                name: proc.name().to_string_lossy().to_string(),
                cpu: proc.cpu_usage() / cpu_count,
                memory_mb: proc.memory() as f64 / 1_048_576.0,
                source: if adopted.contains_key(&pid_u32) {
                    "Managed"
                } else {
                    "External"
                }
                .to_string(),
                status: match proc.status() {
                    sysinfo::ProcessStatus::Run => "Running",
                    _ => "Sleeping",
                }
                .to_string(),
            }
        })
        .collect()
}

/// Mark a process as managed by this panel under a named bucket.
fn adopt_process(adopted: &Adopted, pid: u32, name: String, bucket: String) -> Result<(), String> {
    adopted
        .lock()
        .map_err(|e| e.to_string())?
        .insert(pid, (name, bucket));
    Ok(())
}

/// Remove a process from the managed set.
fn release_process(adopted: &Adopted, pid: u32) -> Result<(), String> {
    adopted.lock().map_err(|e| e.to_string())?.remove(&pid);
    Ok(())
}

/// Kill a process by pid.
fn kill_process(system: &Mutex<System>, pid: u32) -> Result<(), String> {
    let sys = system.lock().map_err(|e| e.to_string())?;
    let target = sysinfo::Pid::from_u32(pid);
    sys.process(target)
        .ok_or_else(|| format!("Process {pid} not found"))?
        .kill();
    Ok(())
}

// ── Table delegate ─────────────────────────────────────────────────────

/// Backs `TableState`: owns the raw snapshot, the active filter, and the
/// sort column/direction, and publishes the visible rows (`entries`).
struct ProcessTableDelegate {
    /// Rows currently shown (filtered then sorted).
    entries: Vec<ProcessEntry>,
    /// Raw snapshot from the last poll (unfiltered, unsorted).
    all: Vec<ProcessEntry>,
    /// Lower-cased name/PID filter from the search box.
    filter: String,
    sort_col: usize,
    sort_dir: ColumnSort,
    columns: Vec<Column>,
}

impl ProcessTableDelegate {
    fn new() -> Self {
        Self {
            entries: Vec::new(),
            all: Vec::new(),
            filter: String::new(),
            sort_col: 1,
            sort_dir: ColumnSort::Ascending,
            columns: vec![
                Column::new("pid", "PID")
                    .width(70.)
                    .fixed(ColumnFixed::Left)
                    .sortable()
                    .resizable(true)
                    .min_width(50.),
                Column::new("name", "Name")
                    .width(200.)
                    .fixed(ColumnFixed::Left)
                    .sortable()
                    .max_width(320.),
                Column::new("cpu", "CPU %")
                    .width(80.)
                    .sortable()
                    .text_right()
                    .p_0(),
                Column::new("memory", "Memory")
                    .width(100.)
                    .sortable()
                    .text_right()
                    .p_0(),
                Column::new("source", "Source").width(110.).sortable(),
                Column::new("status", "Status").width(100.).sortable(),
                Column::new("actions", "")
                    .width(56.)
                    .resizable(false)
                    .movable(false)
                    .selectable(false),
            ],
        }
    }

    /// How many row entries are marked as managed by this panel.
    fn managed_count(&self) -> usize {
        self.entries.iter().filter(|e| e.source == "Managed").count()
    }

    /// Replace the raw snapshot, re-applying the filter and active sort.
    fn set_data(&mut self, all: Vec<ProcessEntry>) {
        self.all = all;
        self.apply();
    }

    fn set_filter(&mut self, filter: &str) {
        self.filter = filter.trim().to_lowercase();
        self.apply();
    }

    /// Rebuild `entries` from `all`: apply the name/PID filter, then the
    /// active column sort.
    fn apply(&mut self) {
        self.entries = self
            .all
            .iter()
            .filter(|e| {
                if self.filter.is_empty() {
                    return true;
                }
                e.name.to_lowercase().contains(&self.filter)
                    || e.pid.to_string().contains(&self.filter)
            })
            .cloned()
            .collect();

        let key = self
            .columns
            .get(self.sort_col)
            .map(|c| c.key.to_string())
            .unwrap_or_default();
        self.entries.sort_by(|a, b| match key.as_str() {
            "pid" => cmp_u32(a.pid, b.pid, self.sort_dir),
            "name" => cmp_str(&a.name, &b.name, self.sort_dir),
            "cpu" => cmp_f32(a.cpu, b.cpu, self.sort_dir),
            "memory" => cmp_f64(a.memory_mb, b.memory_mb, self.sort_dir),
            "source" => cmp_str(&a.source, &b.source, self.sort_dir),
            "status" => cmp_str(&a.status, &b.status, self.sort_dir),
            _ => std::cmp::Ordering::Equal,
        });
    }

    /// A full-height, vertically-centered cell that right-aligns for
    /// `text_right()` columns and applies the default cell size unless the
    /// column dropped its padding (`.p_0()`).
    fn value_cell(&self, col: &Column) -> gpui::Div {
        div()
            .h_full()
            .h_flex()
            .items_center()
            .when(col.paddings.is_some(), |this| this.table_cell_size(Size::default()))
            .when(col.align == TextAlign::Right, |this| this.justify_end())
    }
}

fn cmp_str(a: &str, b: &str, dir: ColumnSort) -> std::cmp::Ordering {
    let ord = a.cmp(b);
    if dir == ColumnSort::Descending { ord.reverse() } else { ord }
}

fn cmp_u32(a: u32, b: u32, dir: ColumnSort) -> std::cmp::Ordering {
    let ord = a.cmp(&b);
    if dir == ColumnSort::Descending { ord.reverse() } else { ord }
}

fn cmp_f32(a: f32, b: f32, dir: ColumnSort) -> std::cmp::Ordering {
    let ord = a.partial_cmp(&b).unwrap_or(std::cmp::Ordering::Equal);
    if dir == ColumnSort::Descending { ord.reverse() } else { ord }
}

fn cmp_f64(a: f64, b: f64, dir: ColumnSort) -> std::cmp::Ordering {
    let ord = a.partial_cmp(&b).unwrap_or(std::cmp::Ordering::Equal);
    if dir == ColumnSort::Descending { ord.reverse() } else { ord }
}

/// A compact colored badge for the Running/Sleeping status column.
fn status_tag(status: &str) -> Tag {
    match status {
        "Running" => Tag::success().outline().child(status.to_string()),
        _ => Tag::new().outline().child(status.to_string()),
    }
    .xsmall()
}

/// A compact colored badge for the Managed/External source column.
fn source_tag(source: &str) -> Tag {
    match source {
        "Managed" => Tag::primary().child(source.to_string()),
        _ => Tag::secondary().outline().child(source.to_string()),
    }
    .xsmall()
}

impl TableDelegate for ProcessTableDelegate {
    fn columns_count(&self, _: &App) -> usize {
        self.columns.len()
    }

    fn rows_count(&self, _: &App) -> usize {
        self.entries.len()
    }

    fn column(&self, col_ix: usize, _: &App) -> Column {
        self.columns.get(col_ix).cloned().unwrap_or_default()
    }

    fn render_th(
        &mut self,
        col_ix: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let col = self.column(col_ix, cx);
        div()
            .when(col.paddings.is_some(), |this| this.table_cell_size(Size::default()))
            .when(col.align == TextAlign::Right, |this| {
                this.h_flex().w_full().justify_end()
            })
            .child(col.name.clone())
    }

    fn render_tr(
        &mut self,
        row_ix: usize,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> gpui::Stateful<gpui::Div> {
        div().id(("process", row_ix))
    }

    fn render_td(
        &mut self,
        row_ix: usize,
        col_ix: usize,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        let Some(entry) = self.entries.get(row_ix) else {
            return div().child("--").into_any_element();
        };
        let Some(col) = self.columns.get(col_ix) else {
            return div().child("--").into_any_element();
        };

        match col.key.as_ref() {
            "pid" => self
                .value_cell(col)
                .text_color(cx.theme().muted_foreground)
                .child(entry.pid.to_string())
                .into_any_element(),
            "name" => self
                .value_cell(col)
                .font_medium()
                .child(div().truncate().child(entry.name.clone()))
                .into_any_element(),
            "cpu" => self
                .value_cell(col)
                .child(format!("{:.1}%", entry.cpu))
                .into_any_element(),
            "memory" => self
                .value_cell(col)
                .text_color(cx.theme().muted_foreground)
                .child(format!("{:.1} MB", entry.memory_mb))
                .into_any_element(),
            "source" => self
                .value_cell(col)
                .child(source_tag(&entry.source))
                .into_any_element(),
            "status" => self
                .value_cell(col)
                .child(status_tag(&entry.status))
                .into_any_element(),
            "actions" => {
                let pid = entry.pid;
                Button::new(("kill", pid))
                    .custom(ButtonCustomVariant::new(cx).foreground(cx.theme().danger))
                    .xsmall()
                    .icon(IconName::CircleX)
                    .tooltip(format!("Kill process {pid}"))
                    .on_click(move |_, window: &mut Window, cx: &mut App| {
                        window.dispatch_action(Box::new(KillProcess(pid)), cx);
                    })
                    .into_any_element()
            }
            _ => div().child("--").into_any_element(),
        }
    }

    fn context_menu(
        &mut self,
        row_ix: usize,
        menu: PopupMenu,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) -> PopupMenu {
        let Some(entry) = self.entries.get(row_ix) else {
            return menu;
        };
        let pid = entry.pid;
        let action: Box<dyn Action> = if entry.source == "Managed" {
            Box::new(ReleaseProcess(pid))
        } else {
            Box::new(AdoptProcess(pid))
        };

        let label = if entry.source == "Managed" {
            "Release (Stop Managing)"
        } else {
            "Adopt (Manage)"
        };
        menu.menu(label, action)
            .separator()
            .menu("Kill Process", Box::new(KillProcess(pid)))
    }

    fn perform_sort(
        &mut self,
        col_ix: usize,
        sort: ColumnSort,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) {
        self.sort_col = col_ix;
        self.sort_dir = sort;
        self.apply();
    }

    fn move_column(
        &mut self,
        col_ix: usize,
        to_ix: usize,
        _: &mut Window,
        _: &mut Context<TableState<Self>>,
    ) {
        let col = self.columns.remove(col_ix);
        self.columns.insert(to_ix, col);
        if self.sort_col == col_ix {
            self.sort_col = to_ix;
        }
    }

    fn render_empty(
        &mut self,
        _: &mut Window,
        cx: &mut Context<TableState<Self>>,
    ) -> impl IntoElement {
        v_flex()
            .size_full()
            .items_center()
            .justify_center()
            .gap_1()
            .p_4()
            .child(
                Icon::new(IconName::Cpu)
                    .size_16()
                    .text_color(cx.theme().muted_foreground),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .child(if self.all.is_empty() {
                        "No processes found"
                    } else {
                        "No processes match your search"
                    }),
            )
            .into_any_element()
    }
}

// ── Panel ──────────────────────────────────────────────────────────────

actions!(
    processes_panel,
    [
        /// Toggles focus on the Processes panel.
        ToggleFocus
    ]
);

/// Registers the Processes panel's actions on every workspace. Call once
/// at app startup, alongside the other panels' `init` functions.
pub fn init(cx: &mut App) {
    cx.observe_new(|workspace: &mut Workspace, _, _| {
        workspace.register_action(|workspace, _: &ToggleFocus, window, cx| {
            workspace.toggle_panel_focus::<ProcessesPanel>(window, cx);
        });
    })
    .detach();
}

pub struct ProcessesPanel {
    focus_handle: FocusHandle,
    system: Arc<Mutex<System>>,
    adopted: Adopted,
    table: Entity<TableState<ProcessTableDelegate>>,
    search: Entity<InputState>,
    _search_sub: Subscription,
    _poll_task: Task<()>,
    /// Set instead of a toast — `gpui_component`'s `push_notification`/
    /// `open_dialog` only work in a window whose outermost layer is
    /// `gpui_component::Root`, which Zed's real app window isn't; calling
    /// either there panics the whole process (a non-unwinding abort, not
    /// a catchable error) instead of failing gracefully. Every
    /// adopt/release/kill outcome goes through this inline banner and the
    /// inline kill-confirm below instead.
    error: Option<String>,
    /// The pid+name awaiting an inline "Kill this process?" confirmation
    /// (irreversible, so it's gated) — replaces the `window.open_dialog`
    /// modal for the same reason as `error` above.
    pending_kill: Option<(u32, String)>,
}

impl ProcessesPanel {
    /// Loads the panel for a workspace, following the same
    /// `WeakEntity<Workspace>` + `AsyncWindowContext` convention as the
    /// other dock panels' `load` functions (see `initialize_panels` in
    /// `zed::zed`), so it can be added to the dock alongside them.
    pub async fn load(
        workspace: WeakEntity<Workspace>,
        mut cx: AsyncWindowContext,
    ) -> Result<Entity<Self>> {
        workspace.update_in(&mut cx, |workspace, window, cx| {
            ProcessesPanel::new(workspace, window, cx)
        })
    }

    pub fn new(
        _workspace: &mut Workspace,
        window: &mut Window,
        cx: &mut Context<Workspace>,
    ) -> Entity<Self> {
        cx.new(|cx| {
            let table = cx.new(|cx| TableState::new(ProcessTableDelegate::new(), window, cx));

            let search =
                cx.new(|cx| InputState::new(window, cx).placeholder("Filter by name or PID…"));

            let table_for_filter = table.clone();
            let search_for_read = search.clone();
            let _search_sub =
                cx.subscribe(&search, move |_: &mut Self, _: Entity<InputState>, event: &InputEvent, cx: &mut Context<Self>| {
                    if matches!(event, InputEvent::Change) {
                        let query = search_for_read.read(cx).value().to_string();
                        table_for_filter.update(cx, |table, _| {
                            table.delegate_mut().set_filter(&query);
                        });
                        cx.notify();
                    }
                });

            let system = Arc::new(Mutex::new(System::new_all()));
            let adopted: Adopted = Arc::new(Mutex::new(HashMap::new()));

            let _poll_task = {
                let poll_system = system.clone();
                let poll_adopted = adopted.clone();
                cx.spawn(async move |this, cx| {
                    loop {
                        // The sysinfo scan blocks for a moment; only the
                        // resulting rows hop back to the main thread via
                        // `this.update`.
                        let processes = list_processes(&poll_system, &poll_adopted);
                        let alive = this
                            .update(cx, |this, cx| {
                                this.table.update(cx, |table, _| {
                                    table.delegate_mut().set_data(processes);
                                });
                                cx.notify();
                            })
                            .is_ok();
                        if !alive {
                            break;
                        }
                        cx.background_executor()
                            .timer(PROCESSES_TICK_INTERVAL)
                            .await;
                    }
                })
            };

            Self {
                focus_handle: cx.focus_handle(),
                system,
                adopted,
                table,
                search,
                _search_sub,
                _poll_task,
                error: None,
                pending_kill: None,
            }
        })
    }

    /// Update the table state now (used right after an adopt/release/kill
    /// so the UI reflects the change before the next poll tick).
    fn refresh_now(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let processes = list_processes(&self.system, &self.adopted);
        self.table.update(cx, |table, _| {
            table.delegate_mut().set_data(processes);
        });
        cx.notify();
    }

    /// Look up the process name for a pid from the visible rows (fallback to
    /// a "PID n" label when the row dropped out of the snapshot).
    fn process_name(&self, pid: u32, cx: &mut Context<Self>) -> String {
        self.table
            .read(cx)
            .delegate()
            .entries
            .iter()
            .find(|e| e.pid == pid)
            .map(|e| e.name.clone())
            .unwrap_or_else(|| format!("PID {pid}"))
    }

    fn on_refresh_click(&mut self, _: &ClickEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.refresh_now(window, cx);
    }

    fn on_adopt(&mut self, action: &AdoptProcess, window: &mut Window, cx: &mut Context<Self>) {
        let pid = action.0;
        let name = self.process_name(pid, cx);
        match adopt_process(&self.adopted, pid, name.clone(), "processes".to_string()) {
            Ok(()) => {
                self.refresh_now(window, cx);
                self.error = None;
            }
            Err(e) => self.error = Some(format!("Failed to adopt process {pid}: {e}")),
        }
        cx.notify();
    }

    fn on_release(&mut self, action: &ReleaseProcess, window: &mut Window, cx: &mut Context<Self>) {
        let pid = action.0;
        match release_process(&self.adopted, pid) {
            Ok(()) => {
                self.refresh_now(window, cx);
                self.error = None;
            }
            Err(e) => self.error = Some(format!("Failed to release process {pid}: {e}")),
        }
        cx.notify();
    }

    /// Arms the inline "Kill this process?" confirmation banner —
    /// irreversible, so it's gated behind an explicit confirmation rather
    /// than firing straight off the context menu / row button.
    fn on_kill(&mut self, action: &KillProcess, _window: &mut Window, cx: &mut Context<Self>) {
        let pid = action.0;
        let name = self.process_name(pid, cx);
        self.pending_kill = Some((pid, name));
        cx.notify();
    }

    fn on_cancel_kill_click(&mut self, cx: &mut Context<Self>) {
        self.pending_kill = None;
        cx.notify();
    }

    fn on_confirm_kill_click(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((pid, _name)) = self.pending_kill.take() else {
            return;
        };
        match kill_process(&self.system, pid) {
            Ok(()) => {
                self.refresh_now(window, cx);
                self.error = None;
            }
            Err(e) => self.error = Some(format!("Failed to kill process {pid}: {e}")),
        }
        cx.notify();
    }

    fn render_kill_confirm(&self, pid: u32, name: &str, cx: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .w_full()
            .flex_shrink_0()
            .items_center()
            .justify_between()
            .gap_2()
            .px_3()
            .py_2()
            .bg(cx.theme().danger.opacity(0.1))
            .border_b_1()
            .border_color(cx.theme().danger)
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().foreground)
                    .child(format!(
                        "Kill \u{201c}{name}\u{201d} (PID {pid})? This cannot be undone."
                    )),
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("cancel-kill")
                            .outline()
                            .xsmall()
                            .label("Cancel")
                            .on_click(cx.listener(|this, _, _, cx| this.on_cancel_kill_click(cx))),
                    )
                    .child(
                        Button::new("confirm-kill")
                            .danger()
                            .xsmall()
                            .label("Kill")
                            .on_click(cx.listener(|this, _, window, cx| this.on_confirm_kill_click(window, cx))),
                    ),
            )
    }

    fn render_footer(&self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let delegate = self.table.read(cx).delegate();
        let total = delegate.all.len();
        let visible = delegate.entries.len();
        let managed = delegate.managed_count();

        h_flex()
            .w_full()
            .min_h_9()
            .items_center()
            .justify_between()
            .gap_3()
            .px_3()
            .bg(cx.theme().muted.opacity(0.35))
            .text_xs()
            .text_color(cx.theme().muted_foreground)
            .child(format!(
                "{total} processes{}",
                if visible != total {
                    format!(" · {visible} shown")
                } else {
                    String::new()
                }
            ))
            .child(format!("{managed} managed"))
    }
}

impl Focusable for ProcessesPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<PanelEvent> for ProcessesPanel {}

impl Panel for ProcessesPanel {
    fn persistent_name() -> &'static str {
        "Processes Panel"
    }

    fn panel_key() -> &'static str {
        "ProcessesPanel"
    }

    fn position(&self, _window: &Window, _cx: &App) -> DockPosition {
        DockPosition::Right
    }

    fn position_is_valid(&self, position: DockPosition) -> bool {
        matches!(position, DockPosition::Right)
    }

    fn set_position(&mut self, _position: DockPosition, _window: &mut Window, _cx: &mut Context<Self>) {
        // Fixed to the right dock — see `position_is_valid`.
    }

    fn default_size(&self, _window: &Window, _cx: &App) -> Pixels {
        gpui::px(340.)
    }

    fn icon(&self, _window: &Window, _cx: &App) -> Option<ui::IconName> {
        Some(ui::IconName::Cpu)
    }

    fn icon_tooltip(&self, _window: &Window, _cx: &App) -> Option<&'static str> {
        Some("Processes")
    }

    fn toggle_action(&self) -> Box<dyn Action> {
        Box::new(ToggleFocus)
    }

    fn activation_priority(&self) -> u32 {
        9
    }
}

impl Render for ProcessesPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .id("processes-panel")
            .track_focus(&self.focus_handle(cx))
            .size_full()
            .bg(cx.theme().sidebar)
            .border_r_1()
            .border_color(cx.theme().border)
            .on_action(cx.listener(Self::on_adopt))
            .on_action(cx.listener(Self::on_release))
            .on_action(cx.listener(Self::on_kill))
            .child(panel_header(
                IconName::Cpu,
                "Processes",
                cx.theme().foreground,
                cx.theme().border,
            ))
            .when_some(self.pending_kill.clone(), |el, (pid, name)| {
                el.child(self.render_kill_confirm(pid, &name, cx))
            })
            .when_some(self.error.clone(), |el, error| {
                el.child(
                    div()
                        .px_3()
                        .py_1()
                        .text_xs()
                        .text_color(cx.theme().danger)
                        .child(error),
                )
            })
            .child(
                h_flex()
                    .gap_2()
                    .px_3()
                    .py_2()
                    .flex_shrink_0()
                    .child(Input::new(&self.search).xsmall().w_full())
                    .child(
                        Button::new("refresh-processes")
                            .ghost()
                            .xsmall()
                            .label("Refresh")
                            .tooltip("Refresh process list now")
                            .on_click(cx.listener(Self::on_refresh_click)),
                    ),
            )
            .child(
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .child(
                        DataTable::new(&self.table)
                            .large()
                            .stripe(true)
                            .bordered(true)
                            .scrollbar_visible(true, true),
                    ),
            )
            .child(self.render_footer(window, cx))
    }
}
