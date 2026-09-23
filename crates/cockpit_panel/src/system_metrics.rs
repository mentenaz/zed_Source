//! The collapsible "Metrics" section (circular CPU/RAM gauges) and the
//! "History" section (an area chart of recent CPU/RAM samples) underneath.

use gpui::{
    Context, Hsla, InteractiveElement as _, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement as _, Styled, div, linear_color_stop, linear_gradient, px, relative,
};
use gpui_component::{
    ActiveTheme, Icon, IconName, Sizable as _, Size, StyledExt,
    chart::AreaChart,
    collapsible::Collapsible,
    h_flex,
    progress::{Progress, ProgressCircle},
    v_flex,
};

use super::{CockpitPanel, DiskMetric, HistorySample, NetSample};

/// A single per-core usage bar: fill height tracks `pct`, colored `danger`
/// past 85% instead of `accent`, with the core's index underneath.
fn core_cell(
    index: usize,
    pct: f32,
    accent: Hsla,
    danger: Hsla,
    track: Hsla,
    label: Hsla,
) -> impl IntoElement {
    let fill = pct.clamp(0., 100.);
    let color = if pct > 85. { danger } else { accent };

    v_flex()
        .flex_shrink_0()
        .items_center()
        .gap_1()
        .w(px(22.))
        .child(
            v_flex()
                .justify_end()
                .w_full()
                .h_6()
                .rounded_sm()
                .bg(track)
                .overflow_hidden()
                .child(div().w_full().h(relative(fill / 100.)).bg(color)),
        )
        .child(
            div()
                .text_xs()
                .font_family("Cascadia Mono")
                .text_color(label)
                .child(index.to_string()),
        )
}

/// One disk usage row: mount-point label, a thin `Progress` bar, and a
/// "used / total" byte readout.
fn disk_row(
    name: SharedString,
    used: u64,
    total: u64,
    accent: Hsla,
    warning: Hsla,
    danger: Hsla,
    muted_foreground: Hsla,
) -> impl IntoElement {
    let pct = if total > 0 {
        used as f32 / total as f32 * 100.
    } else {
        0.
    };
    let color = if pct > 85. {
        danger
    } else if pct > 70. {
        warning
    } else {
        accent
    };

    h_flex()
        .items_center()
        .gap_2()
        .px_3()
        .py_1()
        .child(
            div()
                .w_8()
                .flex_shrink_0()
                .text_xs()
                .font_family("Cascadia Mono")
                .text_color(muted_foreground)
                .child(name.clone()),
        )
        .child(
            Progress::new(SharedString::from(format!("disk-progress-{name}")))
                .value(pct)
                .color(color)
                .with_size(Size::XSmall)
                .flex_1(),
        )
        .child(
            div()
                .flex_shrink_0()
                .text_xs()
                .font_family("Cascadia Mono")
                .text_color(muted_foreground)
                .child(format!("{} / {}", fmt_bytes(used), fmt_bytes(total))),
        )
}

/// Format bytes as `GB`/`MB`/`KB`.
fn fmt_bytes(bytes: u64) -> String {
    if bytes >= 1_073_741_824 {
        format!("{:.1} GB", bytes as f64 / 1_073_741_824.0)
    } else if bytes >= 1_048_576 {
        format!("{:.0} MB", bytes as f64 / 1_048_576.0)
    } else {
        format!("{:.0} KB", bytes as f64 / 1024.0)
    }
}

impl CockpitPanel {
    /// One circular gauge with its label underneath, e.g. the "CPU" or "RAM"
    /// column under the Metrics header. Colored by the same good/moderate/
    /// danger thresholds as the Disks section's `disk_row` (>85% danger,
    /// >70% warning/moderate, else the theme accent), so severity reads
    /// consistently across the whole panel.
    fn metric_gauge(
        &self,
        id: &'static str,
        label: &'static str,
        value: f32,
        accent: Hsla,
        warning: Hsla,
        danger: Hsla,
    ) -> impl IntoElement {
        let color = if value > 85. {
            danger
        } else if value > 70. {
            warning
        } else {
            accent
        };

        v_flex()
            .items_center()
            .gap_2()
            .child(
                ProgressCircle::new(id)
                    .value(value)
                    .color(color)
                    .size_20()
                    .child(
                        div()
                            .size_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .font_semibold()
                            .child(format!("{value:.0}%")),
                    ),
            )
            .child(div().text_sm().child(label))
    }

    /// The collapsible "Metrics" header, with the CPU/RAM gauge row as its
    /// content — hidden entirely while collapsed.
    pub(crate) fn render_metrics(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let open = self.metrics_open;
        let border = cx.theme().border;
        let muted_foreground = cx.theme().muted_foreground;
        let accent = cx.theme().primary;
        let warning = cx.theme().warning;
        let danger = cx.theme().danger;

        v_flex()
            .flex_shrink_0()
            .child(
                Collapsible::new()
                    .open(open)
                    .child(
                        h_flex()
                            .id("metrics-header")
                            .w_full()
                            .justify_between()
                            .items_center()
                            .px_3()
                            .py_2()
                            .child(div().font_semibold().child("Metrics"))
                            .child(
                                Icon::new(if open {
                                    IconName::ChevronDown
                                } else {
                                    IconName::ChevronRight
                                })
                                .xsmall()
                                .text_color(muted_foreground),
                            )
                            .on_click(cx.listener(|this, _, _, cx| this.toggle_metrics(cx))),
                    )
                    .content(
                        h_flex()
                            .w_full()
                            .justify_center()
                            .gap_8()
                            .pb_3()
                            .child(self.metric_gauge(
                                "cpu-gauge",
                                "CPU",
                                self.cpu_usage,
                                accent,
                                warning,
                                danger,
                            ))
                            .child(self.metric_gauge(
                                "ram-gauge",
                                "RAM",
                                self.ram_usage,
                                accent,
                                warning,
                                danger,
                            )),
                    ),
            )
            .child(div().h_px().w_full().bg(border))
    }

    /// The collapsible "Cores" header, with a wrapping grid of per-core
    /// usage bars as its content — hidden entirely while collapsed.
    pub(crate) fn render_cores(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let open = self.cores_open;
        let border = cx.theme().border;
        let muted_foreground = cx.theme().muted_foreground;
        let accent = cx.theme().primary;
        let danger = cx.theme().danger;
        let track = cx.theme().muted;

        v_flex()
            .flex_shrink_0()
            .child(
                Collapsible::new()
                    .open(open)
                    .child(
                        h_flex()
                            .id("cores-header")
                            .w_full()
                            .justify_between()
                            .items_center()
                            .px_3()
                            .py_2()
                            .child(
                                div()
                                    .font_semibold()
                                    .child(format!("Logical Cores: {}", self.core_usage.len())),
                            )
                            .child(
                                Icon::new(if open {
                                    IconName::ChevronDown
                                } else {
                                    IconName::ChevronRight
                                })
                                .xsmall()
                                .text_color(muted_foreground),
                            )
                            .on_click(cx.listener(|this, _, _, cx| this.toggle_cores(cx))),
                    )
                    .content(h_flex().flex_wrap().gap_1().px_3().pb_3().children(
                        self.core_usage.iter().enumerate().map(|(i, &pct)| {
                            core_cell(i, pct, accent, danger, track, muted_foreground)
                        }),
                    )),
            )
            .child(div().h_px().w_full().bg(border))
    }

    /// The collapsible "Disks" header, with a `Progress`-bar row per disk
    /// (mount point, thin usage bar, used/total bytes) as its content —
    /// hidden entirely while collapsed.
    pub(crate) fn render_disks(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let open = self.disks_open;
        let border = cx.theme().border;
        let muted_foreground = cx.theme().muted_foreground;
        let accent = cx.theme().primary;
        let warning = cx.theme().warning;
        let danger = cx.theme().danger;

        v_flex()
            .flex_shrink_0()
            .child(
                Collapsible::new()
                    .open(open)
                    .child(
                        h_flex()
                            .id("disks-header")
                            .w_full()
                            .justify_between()
                            .items_center()
                            .px_3()
                            .py_2()
                            .child(
                                div()
                                    .font_semibold()
                                    .child(format!("Disks: {}", self.disks.len())),
                            )
                            .child(
                                Icon::new(if open {
                                    IconName::ChevronDown
                                } else {
                                    IconName::ChevronRight
                                })
                                .xsmall()
                                .text_color(muted_foreground),
                            )
                            .on_click(cx.listener(|this, _, _, cx| this.toggle_disks(cx))),
                    )
                    .content(v_flex().gap_1().pb_3().children(self.disks.iter().map(
                        |d: &DiskMetric| {
                            let used = d.total.saturating_sub(d.available);
                            disk_row(
                                d.name.clone().into(),
                                used,
                                d.total,
                                accent,
                                warning,
                                danger,
                                muted_foreground,
                            )
                        },
                    ))),
            )
            .child(div().h_px().w_full().bg(border))
    }

    /// The collapsible "History" header, with an area chart of the last
    /// `super::HISTORY_LEN` CPU/RAM samples (one every
    /// `super::METRICS_TICK_INTERVAL`, pushed by `spawn_metrics_refresh`) as
    /// its content — hidden entirely while collapsed.
    pub(crate) fn render_history(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let open = self.history_open;
        let border = cx.theme().border;
        let muted_foreground = cx.theme().muted_foreground;
        let cpu_color = cx.theme().chart_1;
        let ram_color = cx.theme().chart_2;
        let background = cx.theme().background;
        let latest_tick = self.history.back().map(|s| s.tick).unwrap_or(0);
        let tick_secs = super::METRICS_TICK_INTERVAL.as_secs() as u32;

        v_flex()
            .flex_shrink_0()
            .child(
                Collapsible::new()
                    .open(open)
                    .child(
                        h_flex()
                            .id("history-header")
                            .w_full()
                            .justify_between()
                            .items_center()
                            .px_3()
                            .py_2()
                            .child(div().font_semibold().child("History"))
                            .child(
                                Icon::new(if open {
                                    IconName::ChevronDown
                                } else {
                                    IconName::ChevronRight
                                })
                                .xsmall()
                                .text_color(muted_foreground),
                            )
                            .on_click(cx.listener(|this, _, _, cx| this.toggle_history(cx))),
                    )
                    .content(
                        div().h(px(160.)).w_full().px_3().pb_3().child(
                            AreaChart::new(
                                self.history.iter().cloned().collect::<Vec<HistorySample>>(),
                            )
                            .x(move |d: &HistorySample| {
                                format!("-{}s", latest_tick.saturating_sub(d.tick) * tick_secs)
                            })
                            .y(|d: &HistorySample| d.cpu)
                            .stroke(cpu_color)
                            .fill(linear_gradient(
                                0.,
                                linear_color_stop(cpu_color.opacity(0.4), 1.),
                                linear_color_stop(background.opacity(0.3), 0.),
                            ))
                            .name("CPU")
                            .y(|d: &HistorySample| d.ram)
                            .stroke(ram_color)
                            .fill(linear_gradient(
                                0.,
                                linear_color_stop(ram_color.opacity(0.4), 1.),
                                linear_color_stop(background.opacity(0.3), 0.),
                            ))
                            .name("RAM")
                            .tick_margin(6)
                            .id("cockpit-history-chart"),
                        ),
                    ),
            )
            .child(div().h_px().w_full().bg(border))
    }

    /// The collapsible "Network" header, with an area chart of the last
    /// `super::HISTORY_LEN` combined download/upload rate samples (KB/s,
    /// summed across every non-loopback interface) as its content — hidden
    /// entirely while collapsed. Same chart pattern as `render_history`,
    /// just fed `NetSample` instead of `HistorySample`.
    pub(crate) fn render_network(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let open = self.network_open;
        let border = cx.theme().border;
        let muted_foreground = cx.theme().muted_foreground;
        let down_color = cx.theme().chart_3;
        let up_color = cx.theme().chart_4;
        let background = cx.theme().background;
        let latest_tick = self.network.back().map(|s| s.tick).unwrap_or(0);
        let tick_secs = super::METRICS_TICK_INTERVAL.as_secs() as u32;

        v_flex()
            .flex_shrink_0()
            .child(
                Collapsible::new()
                    .open(open)
                    .child(
                        h_flex()
                            .id("network-header")
                            .w_full()
                            .justify_between()
                            .items_center()
                            .px_3()
                            .py_2()
                            .child(div().font_semibold().child("Network"))
                            .child(
                                Icon::new(if open {
                                    IconName::ChevronDown
                                } else {
                                    IconName::ChevronRight
                                })
                                .xsmall()
                                .text_color(muted_foreground),
                            )
                            .on_click(cx.listener(|this, _, _, cx| this.toggle_network(cx))),
                    )
                    .content(
                        div().h(px(160.)).w_full().px_3().pb_3().child(
                            AreaChart::new(
                                self.network.iter().cloned().collect::<Vec<NetSample>>(),
                            )
                            .x(move |d: &NetSample| {
                                format!("-{}s", latest_tick.saturating_sub(d.tick) * tick_secs)
                            })
                            .y(|d: &NetSample| d.down_kbps)
                            .stroke(down_color)
                            .fill(linear_gradient(
                                0.,
                                linear_color_stop(down_color.opacity(0.4), 1.),
                                linear_color_stop(background.opacity(0.3), 0.),
                            ))
                            .name("Down (KB/s)")
                            .y(|d: &NetSample| d.up_kbps)
                            .stroke(up_color)
                            .fill(linear_gradient(
                                0.,
                                linear_color_stop(up_color.opacity(0.4), 1.),
                                linear_color_stop(background.opacity(0.3), 0.),
                            ))
                            .name("Up (KB/s)")
                            .tick_margin(6)
                            .id("cockpit-network-chart"),
                        ),
                    ),
            )
            .child(div().h_px().w_full().bg(border))
    }
}
