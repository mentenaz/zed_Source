//! The Cockpit panel: system snapshot header (logical core count + uptime),
//! a collapsible "Metrics" section (circular CPU/RAM gauges), a "Cores"
//! section (per-core usage bars), a "Disks" section, a "History" section
//! (an area chart of recent CPU/RAM samples), and a "Network" section — see
//! `system_metrics.rs`. This panel owns its own `sysinfo::System` rather
//! than depending on any host application's state.
//!
//! `sysinfo` has no notion of history — each refresh only ever reports the
//! *current* reading, so "History" and "Network" are built by this panel:
//! every tick appends a sample to a capped `VecDeque`, which
//! `system_metrics::render_history`/`render_network` feed straight into an
//! `AreaChart`.

use std::collections::VecDeque;
use std::time::Duration;

use anyhow::Result;
use gpui::{
    Action, App, AppContext, AsyncWindowContext, Context, Entity, EventEmitter, FocusHandle,
    Focusable, InteractiveElement as _, IntoElement, ParentElement, Pixels, Render, Styled, Task,
    WeakEntity, Window, actions, px,
};
use gpui_component::{ActiveTheme, IconName, scroll::ScrollableElement as _, v_flex};
use sysinfo::{Disks, Networks, System};
use workspace::{
    Workspace,
    dock::{DockPosition, Panel, PanelEvent},
};

/// Bytes per second → kilobytes per second.
fn kbps(bytes_per_sec: f64) -> f64 {
    bytes_per_sec / 1024.
}

pub mod header;
mod system_metrics;

/// How often the CPU/RAM/uptime refresh task samples the system.
const METRICS_TICK_INTERVAL: Duration = Duration::from_secs(2);

/// How many samples `history` keeps — at `METRICS_TICK_INTERVAL` = 2s, 30
/// samples covers the last minute.
const HISTORY_LEN: usize = 30;

/// One CPU/RAM sample for the History area chart. `tick` is just a
/// monotonic sample counter — the chart's x-axis is hidden (this is a
/// sparkline, not a labeled timeline), so it only needs to be unique and
/// ordered, not a real timestamp.
#[derive(Clone)]
struct HistorySample {
    tick: u32,
    cpu: f64,
    ram: f64,
}

/// One disk's usage snapshot, refreshed alongside CPU/RAM every tick.
#[derive(Clone)]
struct DiskMetric {
    name: String,
    total: u64,
    available: u64,
}

/// One network throughput sample for the Network area chart — combined
/// download/upload rate (KB/s) summed across every non-loopback interface,
/// same `tick`-as-x-axis convention as `HistorySample`.
#[derive(Clone)]
struct NetSample {
    tick: u32,
    down_kbps: f64,
    up_kbps: f64,
}

actions!(
    cockpit_panel,
    [
        /// Toggles focus on the Cockpit panel.
        ToggleFocus,
        /// Opens the cross-ecosystem Dashboard tab. Dispatched by the
        /// header's "Dashboard" button; this crate has no opinion on what
        /// handles it (avoids a dependency on `dashboard_panel`, which
        /// embeds this panel and would make that a cycle) — see
        /// `dashboard_panel::init` for the actual handler.
        OpenDashboard
    ]
);

/// Registers the Cockpit panel's actions on every workspace. Call once at
/// app startup, alongside the other panels' `init` functions.
pub fn init(cx: &mut App) {
    cx.observe_new(|workspace: &mut Workspace, _, _| {
        workspace.register_action(|workspace, _: &ToggleFocus, window, cx| {
            workspace.toggle_panel_focus::<CockpitPanel>(window, cx);
        });
    })
    .detach();
}

pub struct CockpitPanel {
    focus_handle: FocusHandle,
    cores: usize,
    uptime: u64,
    cpu_usage: f32,
    ram_usage: f32,
    core_usage: Vec<f32>,
    disks: Vec<DiskMetric>,
    metrics_open: bool,
    cores_open: bool,
    disks_open: bool,
    history_open: bool,
    network_open: bool,
    history: VecDeque<HistorySample>,
    network: VecDeque<NetSample>,
    next_tick: u32,
    /// True when this panel is a mirror of another `CockpitPanel`, embedded in
    /// the Dashboard's "System" section. Such a panel runs no poller of its
    /// own (it copies the dock panel's samples — see `new_embedded`) and
    /// omits the header's "Dashboard" button.
    embedded: bool,
    _metrics_refresh: Task<()>,
}

impl CockpitPanel {
    /// Loads the panel for a workspace, following the same
    /// `WeakEntity<Workspace>` + `AsyncWindowContext` convention as the
    /// other dock panels' `load` functions (see `initialize_panels` in
    /// `zed::zed`), so it can be added to the dock alongside them.
    pub async fn load(
        workspace: WeakEntity<Workspace>,
        mut cx: AsyncWindowContext,
    ) -> Result<Entity<Self>> {
        workspace.update_in(&mut cx, |workspace, window, cx| {
            CockpitPanel::new(workspace, window, cx)
        })
    }

    pub fn new(
        _workspace: &mut Workspace,
        _window: &mut Window,
        cx: &mut Context<Workspace>,
    ) -> Entity<Self> {
        cx.new(|cx| {
            let cores = System::new_all().cpus().len();
            let metrics_refresh = Self::spawn_metrics_refresh(cx);
            Self {
                focus_handle: cx.focus_handle(),
                cores,
                uptime: System::uptime(),
                cpu_usage: 0.,
                ram_usage: 0.,
                core_usage: Vec::new(),
                disks: Vec::new(),
                metrics_open: true,
                cores_open: true,
                disks_open: true,
                history_open: true,
                network_open: true,
                history: VecDeque::with_capacity(HISTORY_LEN),
                network: VecDeque::with_capacity(HISTORY_LEN),
                next_tick: 0,
                embedded: false,
                _metrics_refresh: metrics_refresh,
            }
        })
    }

    /// Builds a lightweight mirror of `source` — the dock `CockpitPanel` — for
    /// embedding in the Dashboard's "System" section. It runs no `sysinfo`
    /// poller of its own (the dock panel already samples the system) and just
    /// copies `source`'s samples whenever it notifies, so both views stay in
    /// lockstep without a second poller. Its header omits the "Dashboard"
    /// button, which would be redundant inside the Dashboard itself.
    pub fn new_embedded(source: Entity<Self>, cx: &mut Context<Self>) -> Self {
        cx.observe(&source, |this, source, cx| {
            this.sync_from(source.read(cx));
            cx.notify();
        })
        .detach();

        let mut embedded = Self {
            focus_handle: cx.focus_handle(),
            cores: 0,
            uptime: 0,
            cpu_usage: 0.,
            ram_usage: 0.,
            core_usage: Vec::new(),
            disks: Vec::new(),
            metrics_open: true,
            cores_open: true,
            disks_open: true,
            history_open: true,
            network_open: true,
            history: VecDeque::with_capacity(HISTORY_LEN),
            network: VecDeque::with_capacity(HISTORY_LEN),
            next_tick: 0,
            embedded: true,
            _metrics_refresh: Task::ready(()),
        };
        embedded.sync_from(source.read(cx));
        embedded
    }

    /// Copies the sampled system data from `source`, leaving this panel's
    /// section open/closed state alone so the embedded mirror keeps its own
    /// collapse choices.
    fn sync_from(&mut self, source: &Self) {
        self.cores = source.cores;
        self.uptime = source.uptime;
        self.cpu_usage = source.cpu_usage;
        self.ram_usage = source.ram_usage;
        self.core_usage = source.core_usage.clone();
        self.disks = source.disks.clone();
        self.history = source.history.clone();
        self.network = source.network.clone();
    }

    fn toggle_metrics(&mut self, cx: &mut Context<Self>) {
        self.metrics_open = !self.metrics_open;
        cx.notify();
    }

    fn toggle_history(&mut self, cx: &mut Context<Self>) {
        self.history_open = !self.history_open;
        cx.notify();
    }

    fn toggle_cores(&mut self, cx: &mut Context<Self>) {
        self.cores_open = !self.cores_open;
        cx.notify();
    }

    fn toggle_disks(&mut self, cx: &mut Context<Self>) {
        self.disks_open = !self.disks_open;
        cx.notify();
    }

    fn toggle_network(&mut self, cx: &mut Context<Self>) {
        self.network_open = !self.network_open;
        cx.notify();
    }

    /// Push a new sample onto `history` and `network` under the same tick,
    /// dropping the oldest of each once full, then advance `next_tick`.
    /// Sharing one tick keeps the two charts' x-axes aligned to the same
    /// real moment. Rounded to 2 decimal places so the charts' hover
    /// tooltips read "45.23" instead of sysinfo's full float precision.
    fn push_metrics_sample(&mut self, cpu: f64, ram: f64, down_kbps: f64, up_kbps: f64) {
        let round2 = |v: f64| (v * 100.).round() / 100.;
        let tick = self.next_tick;

        if self.history.len() >= HISTORY_LEN {
            self.history.pop_front();
        }
        self.history.push_back(HistorySample {
            tick,
            cpu: round2(cpu),
            ram: round2(ram),
        });

        if self.network.len() >= HISTORY_LEN {
            self.network.pop_front();
        }
        self.network.push_back(NetSample {
            tick,
            down_kbps: round2(down_kbps),
            up_kbps: round2(up_kbps),
        });

        self.next_tick += 1;
    }

    /// Refresh uptime, CPU usage and RAM usage once every
    /// `METRICS_TICK_INTERVAL`, for as long as the panel is alive.
    /// `WeakEntity::update` returns `Err` once the entity is dropped, which
    /// ends the loop instead of ticking a timer forever in the background.
    ///
    /// CPU usage needs two samples `MINIMUM_CPU_UPDATE_INTERVAL` apart to be
    /// meaningful (sysinfo diffs the two), so each tick refreshes, waits,
    /// then refreshes again before reading `global_cpu_usage()`.
    fn spawn_metrics_refresh(cx: &mut Context<Self>) -> Task<()> {
        cx.spawn(async move |this, cx| {
            let mut sys = System::new_all();
            let mut disks = Disks::new_with_refreshed_list();
            let mut networks = Networks::new_with_refreshed_list();
            loop {
                sys.refresh_cpu_all();
                cx.background_executor()
                    .timer(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL)
                    .await;
                sys.refresh_cpu_all();
                sys.refresh_memory();
                disks.refresh(false);
                networks.refresh(false);

                let cpu_usage = sys.global_cpu_usage();
                let core_usage: Vec<f32> = sys.cpus().iter().map(|c| c.cpu_usage()).collect();
                let ram_usage = if sys.total_memory() > 0 {
                    sys.used_memory() as f32 / sys.total_memory() as f32 * 100.
                } else {
                    0.
                };
                let disk_metrics: Vec<DiskMetric> = disks
                    .iter()
                    .filter(|d| d.total_space() > 0)
                    .map(|d| DiskMetric {
                        name: d.mount_point().to_string_lossy().to_string(),
                        total: d.total_space(),
                        available: d.available_space(),
                    })
                    .collect();
                let (rx_bytes, tx_bytes) = networks
                    .iter()
                    .filter(|(name, _)| !name.to_lowercase().contains("loopback"))
                    .fold((0u64, 0u64), |(rx, tx), (_, data)| {
                        (rx + data.received(), tx + data.transmitted())
                    });
                let tick_secs = METRICS_TICK_INTERVAL.as_secs_f64();
                let down_kbps = kbps(rx_bytes as f64 / tick_secs);
                let up_kbps = kbps(tx_bytes as f64 / tick_secs);
                let uptime = System::uptime();

                let alive = this
                    .update(cx, |panel, cx| {
                        panel.cpu_usage = cpu_usage;
                        panel.ram_usage = ram_usage;
                        panel.core_usage = core_usage;
                        panel.disks = disk_metrics;
                        panel.uptime = uptime;
                        panel.push_metrics_sample(
                            cpu_usage as f64,
                            ram_usage as f64,
                            down_kbps,
                            up_kbps,
                        );
                        cx.notify();
                    })
                    .is_ok();
                if !alive {
                    break;
                }

                cx.background_executor().timer(METRICS_TICK_INTERVAL).await;
            }
        })
    }
}

impl Focusable for CockpitPanel {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<PanelEvent> for CockpitPanel {}

impl Panel for CockpitPanel {
    fn persistent_name() -> &'static str {
        "Cockpit Panel"
    }

    fn panel_key() -> &'static str {
        "CockpitPanel"
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
        px(260.)
    }

    fn icon(&self, _window: &Window, _cx: &App) -> Option<ui::IconName> {
        Some(ui::IconName::Cockpit)
    }

    fn icon_tooltip(&self, _window: &Window, _cx: &App) -> Option<&'static str> {
        Some("Cockpit")
    }

    fn toggle_action(&self) -> Box<dyn Action> {
        Box::new(ToggleFocus)
    }

    fn activation_priority(&self) -> u32 {
        4
    }
}

impl Render for CockpitPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .id("cockpit-panel")
            .track_focus(&self.focus_handle(cx))
            .size_full()
            .bg(cx.theme().sidebar)
            .border_r_1()
            .border_color(cx.theme().border)
            .child(header::cockpit_header(
                IconName::ForgeCockpit2,
                "Cockpit",
                cx.theme().foreground,
                cx.theme().border,
                !self.embedded,
                cx.listener(|_this, _e, window, cx| {
                    window.dispatch_action(Box::new(OpenDashboard), cx);
                }),
            ))
            .child(header::system_header(
                self.cores,
                self.uptime,
                cx.theme().primary,
                cx.theme().muted_foreground,
                cx.theme().border,
            ))
            .child(
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scrollbar()
                    .child(self.render_metrics(cx))
                    .child(self.render_history(cx))
                    .child(self.render_cores(cx))
                    .child(self.render_disks(cx))
                    .child(self.render_network(cx)),
            )
    }
}
