//! Helm's insight screens for a repository: traffic (views, clones, referrers,
//! popular paths) and security alerts (Dependabot and secret scanning).

use super::*;

impl HelmPanel {
    /// Loads the four traffic endpoints for `self.selected_repo` into one
    /// [`RepoTraffic`]. Views/clones come back `None` when GitHub has no
    /// traffic data yet (202/404); referrers/paths just stay empty on
    /// failure.
    pub(super) fn load_traffic(&mut self, cx: &mut Context<Self>) {
        self.load_for_repo(
            cx,
            |repo, gh_state| async move {
                let owner = repo.owner.login;
                let name = repo.name;
                let views = gh_get_traffic_views(owner.clone(), name.clone(), &gh_state)
                    .await
                    .ok();
                let clones = gh_get_traffic_clones(owner.clone(), name.clone(), &gh_state)
                    .await
                    .ok();
                let referrers = gh_get_traffic_referrers(owner.clone(), name.clone(), &gh_state)
                    .await
                    .unwrap_or_default();
                let paths = gh_get_traffic_paths(owner.clone(), name.clone(), &gh_state)
                    .await
                    .unwrap_or_default();
                Ok(RepoTraffic {
                    views,
                    clones,
                    referrers,
                    paths,
                })
            },
            |this, traffic| this.traffic = Some(traffic),
        );
    }

    /// Loads `self.selected_repo`'s dependabot and secret-scanning alerts for
    /// the Security screen — like `load_traffic`, each endpoint's failure is
    /// independent (a repo can have one feature enabled and not the other).
    pub(super) fn load_security(&mut self, cx: &mut Context<Self>) {
        let Some(repo) = self.selected_repo.clone() else {
            return;
        };
        // One request each, shown on one screen, so they load as a pair.
        self.dependabot_alerts.begin();
        self.secret_scanning_alerts.begin();
        cx.notify();
        let gh_state = self.gh_state.clone();
        cx.spawn(async move |this, cx| {
            let (dependabot, secret_scanning) = on_tokio(async move {
                let owner = repo.owner.login;
                let name = repo.name;
                let dependabot = gh_list_dependabot_alerts(owner.clone(), name.clone(), &gh_state)
                    .await
                    .unwrap_or_default();
                let secret_scanning = gh_list_secret_scanning_alerts(owner, name, &gh_state)
                    .await
                    .unwrap_or_default();
                (dependabot, secret_scanning)
            })
            .await;
            this.update(cx, |this, cx| {
                // Neither list fails the screen: an alert type that is turned
                // off, or that the token may not read, is shown as empty.
                this.dependabot_alerts.finish(Ok(dependabot));
                this.secret_scanning_alerts.finish(Ok(secret_scanning));
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The Traffic tab — view/clone totals plus the last week of daily data,
    /// then the top referrers and paths. An empty repo shows the "no traffic
    /// data yet" state (views/clones are `None` for 202/404 repos).
    pub(super) fn render_traffic(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let muted_foreground = cx.theme().muted_foreground;
        let foreground = cx.theme().foreground;
        let border = cx.theme().border;

        if self.load_state == LoadState::Loading {
            return v_flex()
                .flex_1()
                .items_center()
                .justify_center()
                .p_4()
                .child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(Spinner::new().small())
                        .child(
                            div()
                                .text_sm()
                                .text_color(muted_foreground)
                                .child("Loading traffic…"),
                        ),
                )
                .into_any_element();
        }

        if self.load_state == LoadState::Error {
            return v_flex()
                .gap_3()
                .p_4()
                .child(
                    div()
                        .text_sm()
                        .text_color(muted_foreground)
                        .child("Failed to load traffic"),
                )
                .child(
                    Button::new("traffic-retry")
                        .outline()
                        .label("Retry")
                        .on_click(cx.listener(|this, _, _, cx| this.load_traffic(cx))),
                )
                .into_any_element();
        }

        let traffic = self.traffic.clone().unwrap_or_default();
        let has_any = traffic.views.is_some()
            || traffic.clones.is_some()
            || !traffic.referrers.is_empty()
            || !traffic.paths.is_empty();
        if !has_any {
            return v_flex()
                .flex_1()
                .items_center()
                .justify_center()
                .p_4()
                .child(div().text_sm().text_color(muted_foreground).child(
                    "No traffic data yet — GitHub releases it for repositories with enough views.",
                ))
                .into_any_element();
        }

        let views_total = traffic.views.as_ref().map(|v| (v.count, v.uniques));
        let clones_total = traffic.clones.as_ref().map(|c| (c.count, c.uniques));

        let totals_row = h_flex()
            .items_center()
            .gap_4()
            .px_3()
            .py_2()
            .text_sm()
            .text_color(muted_foreground)
            .when_some(views_total, |row, (count, uniques)| {
                row.child(format!(
                    "Views {} · {} unique",
                    fmt_num(count),
                    fmt_num(uniques)
                ))
            })
            .when_some(clones_total, |row, (count, uniques)| {
                row.child(format!(
                    "Clones {} · {} unique",
                    fmt_num(count),
                    fmt_num(uniques)
                ))
            });

        let daily_label = |title: &str| {
            div()
                .px_3()
                .pt_2()
                .pb_1()
                .text_xs()
                .font_semibold()
                .text_color(muted_foreground)
                .child(title.to_string())
        };

        let mut col = v_flex()
            .child(totals_row)
            .child(div().h_px().w_full().bg(border));

        if let Some(views) = traffic.views.clone() {
            col = col.child(daily_label("Views (last 7 days)")).child(
                v_flex().children(
                    views
                        .views
                        .iter()
                        .rev()
                        .skip(views.views.len().saturating_sub(7))
                        .map(|day| {
                            h_flex()
                                .items_center()
                                .justify_between()
                                .gap_2()
                                .px_3()
                                .py_1()
                                .child(
                                    div()
                                        .text_sm()
                                        .text_color(foreground)
                                        .child(short_date(&day.timestamp)),
                                )
                                .child(div().text_xs().text_color(muted_foreground).child(format!(
                                    "{} · {} unique",
                                    fmt_num(day.count),
                                    fmt_num(day.uniques)
                                )))
                        }),
                ),
            );
        }

        if !traffic.referrers.is_empty() {
            col = col
                .child(daily_label("Top referrers"))
                .child(
                    v_flex().children(traffic.referrers.iter().take(10).map(|r| {
                        h_flex()
                            .items_center()
                            .justify_between()
                            .gap_2()
                            .px_3()
                            .py_1()
                            .child(
                                div()
                                    .truncate()
                                    .text_sm()
                                    .text_color(foreground)
                                    .child(r.referrer.clone()),
                            )
                            .child(div().text_xs().text_color(muted_foreground).child(format!(
                                "{} · {} unique",
                                fmt_num(r.count),
                                fmt_num(r.uniques)
                            )))
                    })),
                );
        }

        if !traffic.paths.is_empty() {
            col = col.child(daily_label("Top paths")).child(v_flex().children(
                traffic.paths.iter().take(10).map(|p| {
                    h_flex()
                        .items_center()
                        .justify_between()
                        .gap_2()
                        .px_3()
                        .py_1()
                        .child(
                            div()
                                .truncate()
                                .text_sm()
                                .font_family("Cascadia Mono")
                                .text_color(foreground)
                                .child(p.path.clone()),
                        )
                        .child(div().text_xs().text_color(muted_foreground).child(format!(
                            "{} · {} unique",
                            fmt_num(p.count),
                            fmt_num(p.uniques)
                        )))
                }),
            ));
        }

        col.into_any_element()
    }

    /// The Security screen — dependabot and secret-scanning alerts, in two
    /// sections. Both endpoints return raw JSON (no dedicated repo feature
    /// flag check is done here — a 404/disabled response just yields an
    /// empty list, same as `load_security`'s `unwrap_or_default`).
    pub(super) fn render_security(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let muted_foreground = cx.theme().muted_foreground;
        let foreground = cx.theme().foreground;
        let border = cx.theme().border;

        if self.dependabot_alerts.state == LoadState::Loading {
            return v_flex()
                .flex_1()
                .items_center()
                .justify_center()
                .p_4()
                .child(
                    h_flex()
                        .gap_2()
                        .items_center()
                        .child(Spinner::new().small())
                        .child(
                            div()
                                .text_sm()
                                .text_color(muted_foreground)
                                .child("Loading security alerts…"),
                        ),
                )
                .into_any_element();
        }

        let section_label = |title: &str| {
            div()
                .px_3()
                .pt_2()
                .pb_1()
                .text_xs()
                .font_semibold()
                .text_color(muted_foreground)
                .child(title.to_string())
        };

        // Both are read-only (no click action), so each gets its own
        // up/down + selection highlight and no `OpenSelectedRow` handler,
        // same reasoning as `render_branches`.
        let dependabot_len = self.dependabot_alerts.items.len();
        let dependabot_cursor = self.dependabot_alerts.cursor;
        let dependabot_rows = self.dependabot_alerts.items.iter().enumerate().map(|(i, alert)| {
            let package = alert
                .pointer("/dependency/package/name")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown package")
                .to_string();
            let severity = alert
                .pointer("/security_advisory/severity")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string();
            let state = alert
                .get("state")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            ListItem::new(format!("helm-dependabot-{i}"))
                .selected(dependabot_cursor == Some(i))
                .child(div().text_sm().text_color(foreground).child(package))
                .suffix(move |_, _| {
                    div()
                        .text_xs()
                        .text_color(muted_foreground)
                        .child(format!("{severity} · {state}"))
                })
        });

        let secret_len = self.secret_scanning_alerts.items.len();
        let secret_cursor = self.secret_scanning_alerts.cursor;
        let secret_rows = self.secret_scanning_alerts.items.iter().enumerate().map(|(i, alert)| {
            let secret_type = alert
                .get("secret_type_display_name")
                .or_else(|| alert.get("secret_type"))
                .and_then(|v| v.as_str())
                .unwrap_or("unknown secret")
                .to_string();
            let state = alert
                .get("state")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            ListItem::new(format!("helm-secret-scanning-{i}"))
                .selected(secret_cursor == Some(i))
                .child(div().text_sm().text_color(foreground).child(secret_type))
                .suffix(move |_, _| div().text_xs().text_color(muted_foreground).child(state.clone()))
        });

        v_flex()
            .child(section_label("Dependabot alerts"))
            .child(if self.dependabot_alerts.items.is_empty() {
                div()
                    .px_3()
                    .py_2()
                    .text_sm()
                    .text_color(muted_foreground)
                    .child("No open Dependabot alerts")
                    .into_any_element()
            } else {
                v_flex()
                    .id("helm-dependabot-list")
                    .track_focus(&self.dependabot_alerts.focus)
                    .on_mouse_down(MouseButton::Left, cx.listener(|this, _, window, cx| {
                        window.focus(&this.dependabot_alerts.focus, cx);
                    }))
                    .key_context("HelmRowList")
                    .on_action(cx.listener(move |this, _: &SelectNextRow, _, cx| {
                        this.dependabot_alerts.cursor =
                            step_selected(this.dependabot_alerts.cursor, dependabot_len, true);
                        cx.notify();
                    }))
                    .on_action(cx.listener(move |this, _: &SelectPrevRow, _, cx| {
                        this.dependabot_alerts.cursor =
                            step_selected(this.dependabot_alerts.cursor, dependabot_len, false);
                        cx.notify();
                    }))
                    .children(dependabot_rows)
                    .into_any_element()
            })
            .child(div().h_px().w_full().bg(border))
            .child(section_label("Secret scanning alerts"))
            .child(if self.secret_scanning_alerts.items.is_empty() {
                div()
                    .px_3()
                    .py_2()
                    .text_sm()
                    .text_color(muted_foreground)
                    .child("No open secret scanning alerts")
                    .into_any_element()
            } else {
                v_flex()
                    .id("helm-secret-scanning-list")
                    .track_focus(&self.secret_scanning_alerts.focus)
                    .on_mouse_down(MouseButton::Left, cx.listener(|this, _, window, cx| {
                        window.focus(&this.secret_scanning_alerts.focus, cx);
                    }))
                    .key_context("HelmRowList")
                    .on_action(cx.listener(move |this, _: &SelectNextRow, _, cx| {
                        this.secret_scanning_alerts.cursor =
                            step_selected(this.secret_scanning_alerts.cursor, secret_len, true);
                        cx.notify();
                    }))
                    .on_action(cx.listener(move |this, _: &SelectPrevRow, _, cx| {
                        this.secret_scanning_alerts.cursor =
                            step_selected(this.secret_scanning_alerts.cursor, secret_len, false);
                        cx.notify();
                    }))
                    .children(secret_rows)
                    .into_any_element()
            })
            .into_any_element()
    }
}
