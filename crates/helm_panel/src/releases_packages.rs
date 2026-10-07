//! Helm's release screens for a repository: releases (list and create),
//! packages with their versions, and tags.

use super::*;

impl HelmPanel {
    /// Loads `self.selected_repo`'s releases.
    pub(super) fn load_releases(&mut self, cx: &mut Context<Self>) {
        self.load_section(
            cx,
            |this| &mut this.releases,
            |repo, gh_state| async move {
                gh_list_releases(repo.owner.login, repo.name, &gh_state).await
            },
        );
    }

    /// Loads `self.selected_repo`'s packages (owner-scoped: either the repo's
    /// org or the user's own account).
    pub(super) fn load_packages(&mut self, cx: &mut Context<Self>) {
        if self.selected_repo.is_none() {
            return;
        }
        self.package_versions.clear();
        self.package_versions_error = None;
        self.expanded_package = None;
        self.load_for_repo(
            cx,
            |repo, gh_state| async move { gh_list_packages(repo.owner.login, &gh_state).await },
            |this, packages| this.packages = packages,
        );
    }

    /// Expands `pkg` to show its versions (loading them on first tap) — a
    /// second tap on the same package collapses it again.
    pub(super) fn toggle_package_versions(&mut self, owner: String, pkg: Package, cx: &mut Context<Self>) {
        if let Some(expanded) = self.expanded_package.as_deref() {
            if expanded == pkg.name {
                self.expanded_package = None;
                self.package_versions.clear();
                self.package_versions_error = None;
                cx.notify();
                return;
            }
        }
        self.expanded_package = Some(pkg.name.clone());
        self.package_versions.clear();
        self.package_versions_error = None;
        cx.notify();
        let gh_state = self.gh_state.clone();
        cx.spawn(async move |this, cx| {
            let result = on_tokio(async move {
                gh_list_package_versions(owner, pkg.package_type, pkg.name.clone(), &gh_state).await
            })
            .await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(versions) => this.package_versions = versions,
                    Err(e) => this.package_versions_error = Some(e),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Loads `self.selected_repo`'s tags.
    pub(super) fn load_tags(&mut self, cx: &mut Context<Self>) {
        self.load_for_repo(
            cx,
            |repo, gh_state| async move {
                gh_list_tags(repo.owner.login, repo.name, &gh_state).await
            },
            |this, tags| this.tags = tags,
        );
    }

    /// Opens the "Create release" modal.
    pub(super) fn open_create_release_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_workspace_modal(HelmModalKind::CreateRelease, window, cx);
    }

    pub(super) fn handle_create_release(
        &mut self,
        tag_name: String,
        title: String,
        body: String,
        draft: bool,
        prerelease: bool,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.run_action(
            HelmAction::CreateRelease {
                tag_name,
                title,
                body,
                draft,
                prerelease,
            },
            true,
            cx,
        );
    }

    /// The Releases tab — list of tags with name/body preview, draft and
    /// prerelease badges, and asset download summary; rows open in the
    /// browser.
    pub(super) fn render_releases(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let muted_foreground = cx.theme().muted_foreground;
        let foreground = cx.theme().foreground;
        let warning = cx.theme().warning;

        let header = h_flex()
            .items_center()
            .justify_between()
            .px_3()
            .py_2()
            .child(
                div()
                    .text_sm()
                    .font_semibold()
                    .text_color(foreground)
                    .child("Releases"),
            )
            .child(
                Button::new("releases-create")
                    .ghost()
                    .xsmall()
                    .icon(IconName::Plus)
                    .label("New release")
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_create_release_dialog(window, cx)
                    })),
            );

        if self.releases.state == LoadState::Loading {
            return v_flex()
                .child(header)
                .child(div().h_px().w_full().bg(cx.theme().border))
                .child(
                    v_flex()
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
                                        .child("Loading releases…"),
                                ),
                        ),
                )
                .into_any_element();
        }

        if self.releases.state == LoadState::Error {
            return v_flex()
                .child(header)
                .child(div().h_px().w_full().bg(cx.theme().border))
                .child(
                    v_flex()
                        .gap_3()
                        .p_4()
                        .child(
                            div()
                                .text_sm()
                                .text_color(muted_foreground)
                                .child("Failed to load releases"),
                        )
                        .child(
                            Button::new("releases-retry")
                                .outline()
                                .label("Retry")
                                .on_click(cx.listener(|this, _, _, cx| this.load_releases(cx))),
                        ),
                )
                .into_any_element();
        }

        if self.releases.items.is_empty() {
            return v_flex()
                .child(header)
                .child(div().h_px().w_full().bg(cx.theme().border))
                .child(
                    v_flex()
                        .flex_1()
                        .items_center()
                        .justify_center()
                        .p_4()
                        .child(
                            div()
                                .text_sm()
                                .text_color(muted_foreground)
                                .child("No releases yet"),
                        ),
                )
                .into_any_element();
        }

        let releases_len = self.releases.items.len();
        let releases_cursor = self.releases.cursor;
        let releases_urls: Vec<String> =
            self.releases.items.iter().map(|r| r.html_url.clone()).collect();
        let releases_list = v_flex()
            .id("helm-releases-list")
            .track_focus(&self.releases.focus)
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, window, cx| {
                window.focus(&this.releases.focus, cx);
            }))
            .key_context("HelmRowList")
            .on_action(cx.listener(move |this, _: &SelectNextRow, _, cx| {
                this.releases.cursor = step_selected(this.releases.cursor, releases_len, true);
                cx.notify();
            }))
            .on_action(cx.listener(move |this, _: &SelectPrevRow, _, cx| {
                this.releases.cursor =
                    step_selected(this.releases.cursor, releases_len, false);
                cx.notify();
            }))
            .on_action(cx.listener(move |this, _: &OpenSelectedRow, _, cx| {
                let Some(url) = this.releases.cursor.and_then(|ix| releases_urls.get(ix))
                else {
                    return;
                };
                cx.open_url(url);
            }))
            .py_1()
            .children(self.releases.items.iter().enumerate().map(|(ix, release)| {
                let tag = release.tag_name.clone();
                let title = release
                    .name
                    .clone()
                    .filter(|n| !n.is_empty())
                    .unwrap_or_else(|| tag.clone());
                let draft = release.draft;
                let prerelease = release.prerelease;
                let body_preview = release.body.as_deref().map(|b| {
                    let mut trimmed: String = b.chars().take(120).collect();
                    if b.chars().count() > 120 {
                        trimmed.push('…');
                    }
                    trimmed
                });
                let asset_count = release.assets.len();
                let url = release.html_url.clone();
                ListItem::new(format!("helm-release-{}", release.tag_name))
                    .selected(releases_cursor == Some(ix))
                    .child(
                        v_flex()
                            .gap_0p5()
                            .min_w_0()
                            .child(
                                h_flex()
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        div()
                                            .text_sm()
                                            .font_semibold()
                                            .text_color(foreground)
                                            .child(title),
                                    )
                                    .when(draft, |row| {
                                        row.child(
                                            div()
                                                .px_1p5()
                                                .py_0p5()
                                                .rounded_full()
                                                .text_xs()
                                                .bg(muted_foreground.opacity(0.15))
                                                .text_color(muted_foreground)
                                                .child("draft"),
                                        )
                                    })
                                    .when(prerelease, |row| {
                                        row.child(
                                            div()
                                                .px_1p5()
                                                .py_0p5()
                                                .rounded_full()
                                                .text_xs()
                                                .bg(warning.opacity(0.15))
                                                .text_color(warning)
                                                .child("pre-release"),
                                        )
                                    }),
                            )
                            .when_some(body_preview, |col, body| {
                                col.child(
                                    div()
                                        .truncate()
                                        .text_xs()
                                        .text_color(muted_foreground)
                                        .child(body),
                                )
                            })
                            .when(asset_count > 0, |col| {
                                col.child(div().text_xs().text_color(muted_foreground).child(
                                    format!(
                                        "{asset_count} asset{}",
                                        if asset_count == 1 { "" } else { "s" }
                                    ),
                                ))
                            }),
                    )
                    .suffix(move |_, _| {
                        Icon::new(IconName::ExternalLink)
                            .xsmall()
                            .text_color(muted_foreground)
                    })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.releases.cursor = Some(ix);
                        cx.open_url(&url);
                    }))
            }));

        v_flex()
            .child(header)
            .child(div().h_px().w_full().bg(cx.theme().border))
            .child(releases_list)
            .into_any_element()
    }

    /// The Packages tab — owner-scoped package list; tapping a package
    /// expands its versions inline.
    pub(super) fn render_packages(&self, cx: &mut Context<Self>) -> impl IntoElement {
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
                                .child("Loading packages…"),
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
                        .child("Failed to load packages"),
                )
                .child(
                    Button::new("packages-retry")
                        .outline()
                        .label("Retry")
                        .on_click(cx.listener(|this, _, _, cx| this.load_packages(cx))),
                )
                .into_any_element();
        }

        if self.packages.is_empty() {
            return v_flex()
                .flex_1()
                .items_center()
                .justify_center()
                .p_4()
                .child(
                    div()
                        .text_sm()
                        .text_color(muted_foreground)
                        .child("No packages found"),
                )
                .into_any_element();
        }

        let owner = self
            .selected_repo
            .as_ref()
            .map(|r| r.owner.login.clone())
            .unwrap_or_default();
        let expanded = self.expanded_package.clone();
        let version_error = self.package_versions_error.clone();
        let view = cx.entity();
        let packages_len = self.packages.len();
        let packages_cursor = self.packages_list_cursor;
        let packages_for_open = self.packages.clone();
        let owner_for_open = owner.clone();

        v_flex()
            .id("helm-packages-list")
            .track_focus(&self.packages_list_focus)
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, window, cx| {
                window.focus(&this.packages_list_focus, cx);
            }))
            .key_context("HelmRowList")
            .on_action(cx.listener(move |this, _: &SelectNextRow, _, cx| {
                this.packages_list_cursor = step_selected(this.packages_list_cursor, packages_len, true);
                cx.notify();
            }))
            .on_action(cx.listener(move |this, _: &SelectPrevRow, _, cx| {
                this.packages_list_cursor =
                    step_selected(this.packages_list_cursor, packages_len, false);
                cx.notify();
            }))
            .on_action(cx.listener(move |this, _: &OpenSelectedRow, _, cx| {
                let Some(pkg) = this.packages_list_cursor.and_then(|ix| packages_for_open.get(ix))
                else {
                    return;
                };
                this.toggle_package_versions(owner_for_open.clone(), pkg.clone(), cx);
            }))
            .py_1()
            .children(self.packages.iter().enumerate().map(|(ix, pkg)| {
                let pkg_name = pkg.name.clone();
                let is_expanded = expanded.as_deref() == Some(pkg_name.as_str());
                let is_selected = packages_cursor == Some(ix);
                let pkg_type = pkg.package_type.clone();
                let pkg_vis = pkg.visibility.clone();
                let pkg_desc = pkg.description.clone();
                let package_clone = pkg.clone();
                let owner_clone = owner.clone();
                let view = view.clone();

                let row = ListItem::new(format!("helm-package-{pkg_name}"))
                    .selected(is_selected)
                    .child(
                        v_flex()
                            .gap_0p5()
                            .min_w_0()
                            .child(
                                h_flex()
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        div()
                                            .text_sm()
                                            .font_semibold()
                                            .text_color(foreground)
                                            .child(pkg_name.clone()),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(muted_foreground)
                                            .child(pkg_type.clone()),
                                    ),
                            )
                            .when_some(pkg_desc, |col, desc| {
                                col.child(
                                    div()
                                        .truncate()
                                        .text_xs()
                                        .text_color(muted_foreground)
                                        .child(desc),
                                )
                            }),
                    )
                    .suffix({
                        let pkg_vis = pkg_vis.clone();
                        move |_, _| {
                            h_flex()
                                .items_center()
                                .gap_2()
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(muted_foreground)
                                        .child(pkg_vis.clone()),
                                )
                                .child(
                                    Icon::new(if is_expanded {
                                        IconName::ChevronDown
                                    } else {
                                        IconName::ChevronRight
                                    })
                                    .xsmall()
                                    .text_color(muted_foreground),
                                )
                        }
                    })
                    .on_click({
                        let owner_clone = owner_clone.clone();
                        let package_clone = package_clone.clone();
                        move |_, _window, cx| {
                            view.update(cx, |this, cx| {
                                this.packages_list_cursor = Some(ix);
                                this.toggle_package_versions(
                                    owner_clone.clone(),
                                    package_clone.clone(),
                                    cx,
                                );
                            });
                        }
                    });

                if is_expanded {
                    let versions = if self.package_versions.is_empty() && version_error.is_some() {
                        v_flex()
                            .px_4()
                            .py_1()
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(muted_foreground)
                                    .child("Failed to load versions"),
                            )
                            .into_any_element()
                    } else if self.package_versions.is_empty() {
                        v_flex()
                            .px_4()
                            .py_1()
                            .child(
                                h_flex()
                                    .gap_2()
                                    .items_center()
                                    .child(Spinner::new().small())
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(muted_foreground)
                                            .child("Loading versions…"),
                                    ),
                            )
                            .into_any_element()
                    } else {
                        v_flex()
                            .children(self.package_versions.iter().map(|version| {
                                h_flex()
                                    .items_center()
                                    .justify_between()
                                    .gap_2()
                                    .px_4()
                                    .py_1()
                                    .child(
                                        div()
                                            .text_sm()
                                            .text_color(foreground)
                                            .child(version.name.clone()),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(muted_foreground)
                                            .child(short_date(&version.created_at)),
                                    )
                            }))
                            .into_any_element()
                    };
                    v_flex()
                        .child(row)
                        .child(div().h_px().w_full().bg(border))
                        .child(versions)
                        .into_any_element()
                } else {
                    row.into_any_element()
                }
            }))
            .into_any_element()
    }

    /// The Tags screen.
    pub(super) fn render_tags(&self, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some(el) = self.activity_list_states(
            "Loading tags…",
            "Failed to load tags",
            "No tags found",
            self.tags.is_empty(),
            |this, cx| this.load_tags(cx),
            cx,
        ) {
            return el;
        }

        let foreground = cx.theme().foreground;
        let muted_foreground = cx.theme().muted_foreground;
        let tags_len = self.tags.len();
        let tags_cursor = self.tags_list_cursor;

        v_flex()
            .id("helm-tags-list")
            .track_focus(&self.tags_list_focus)
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, window, cx| {
                window.focus(&this.tags_list_focus, cx);
            }))
            .key_context("HelmRowList")
            .on_action(cx.listener(move |this, _: &SelectNextRow, _, cx| {
                this.tags_list_cursor = step_selected(this.tags_list_cursor, tags_len, true);
                cx.notify();
            }))
            .on_action(cx.listener(move |this, _: &SelectPrevRow, _, cx| {
                this.tags_list_cursor = step_selected(this.tags_list_cursor, tags_len, false);
                cx.notify();
            }))
            .py_1()
            .children(self.tags.iter().enumerate().map(|(ix, tag)| {
                let short_sha: String = tag.commit.sha.chars().take(7).collect();
                ListItem::new(format!("helm-tag-{}", tag.name))
                    .selected(tags_cursor == Some(ix))
                    .child(
                        div()
                            .text_sm()
                            .font_family("Cascadia Mono")
                            .text_color(foreground)
                            .child(tag.name.clone()),
                    )
                    .suffix(move |_, _| {
                        div().text_xs().text_color(muted_foreground).child(short_sha.clone())
                    })
                    .into_any_element()
            }))
            .into_any_element()
    }
}
