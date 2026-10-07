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
        self.load_section(
            cx,
            |this| &mut this.packages,
            |repo, gh_state| async move { gh_list_packages(repo.owner.login, &gh_state).await },
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
        self.load_section(
            cx,
            |this| &mut this.tags,
            |repo, gh_state| async move {
                gh_list_tags(repo.owner.login, repo.name, &gh_state).await
            },
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
        let foreground = cx.theme().foreground;

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

        self.list_screen(
            self.releases.status(),
            &self.releases_list,
            Some(header.into_any_element()),
            ListLabels {
                loading: "Loading releases…",
                error: "Failed to load releases",
                empty: "No releases yet",
            },
            |this, cx| this.load_releases(cx),
            cx,
        )
    }

    /// The Packages tab — owner-scoped package list; tapping a package
    /// expands its versions inline.
    pub(super) fn render_packages(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let muted_foreground = cx.theme().muted_foreground;
        let foreground = cx.theme().foreground;
        let border = cx.theme().border;

        if self.packages.state == LoadState::Loading {
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

        if self.packages.state == LoadState::Error {
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

        if self.packages.items.is_empty() {
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
        let packages_len = self.packages.items.len();
        let packages_cursor = self.packages.cursor;
        let packages_for_open = self.packages.items.clone();
        let owner_for_open = owner.clone();

        v_flex()
            .id("helm-packages-list")
            .track_focus(&self.packages.focus)
            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, window, cx| {
                window.focus(&this.packages.focus, cx);
            }))
            .key_context("HelmRowList")
            .on_action(cx.listener(move |this, _: &SelectNextRow, _, cx| {
                this.packages.cursor = step_selected(this.packages.cursor, packages_len, true);
                cx.notify();
            }))
            .on_action(cx.listener(move |this, _: &SelectPrevRow, _, cx| {
                this.packages.cursor =
                    step_selected(this.packages.cursor, packages_len, false);
                cx.notify();
            }))
            .on_action(cx.listener(move |this, _: &OpenSelectedRow, _, cx| {
                let Some(pkg) = this.packages.cursor.and_then(|ix| packages_for_open.get(ix))
                else {
                    return;
                };
                this.toggle_package_versions(owner_for_open.clone(), pkg.clone(), cx);
            }))
            .py_1()
            .children(self.packages.items.iter().enumerate().map(|(ix, pkg)| {
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
                                this.packages.cursor = Some(ix);
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
        self.list_screen(
            self.tags.status(),
            &self.tags_list,
            None,
            ListLabels {
                loading: "Loading tags…",
                error: "Failed to load tags",
                empty: "No tags found",
            },
            |this, cx| this.load_tags(cx),
            cx,
        )
    }
}

/// One row of the Tags screen: the tag's name and the short commit it
/// points at.
pub(super) fn tag_row(ix: usize, tag: &Tag, cx: &App) -> ListItem {
    let foreground = cx.theme().foreground;
    let muted_foreground = cx.theme().muted_foreground;
    let short_sha: String = tag.commit.sha.chars().take(7).collect();
    ListItem::new(("helm-tag", ix))
        .child(
            div()
                .text_sm()
                .font_family("Cascadia Mono")
                .text_color(foreground)
                .child(tag.name.clone()),
        )
        .suffix(move |_, _| {
            div()
                .text_xs()
                .text_color(muted_foreground)
                .child(short_sha.clone())
        })
}

/// The second line of a release's row: how many assets it has, then the
/// start of its notes.
///
/// Every row gets this line, with a placeholder when there is nothing to
/// say, because the list draws all of its rows at one height.
pub(super) fn release_summary(release: &Release) -> String {
    let assets = match release.assets.len() {
        0 => None,
        1 => Some("1 asset".to_string()),
        count => Some(format!("{count} assets")),
    };
    let notes = release
        .body
        .as_deref()
        .map(|body| body.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|body| !body.is_empty())
        .map(|body| {
            let mut preview: String = body.chars().take(120).collect();
            if body.chars().count() > 120 {
                preview.push('…');
            }
            preview
        });
    match (assets, notes) {
        (Some(assets), Some(notes)) => format!("{assets} · {notes}"),
        (Some(assets), None) => assets,
        (None, Some(notes)) => notes,
        (None, None) => "No release notes".to_string(),
    }
}

/// One row of the Releases screen: its title with draft and pre-release
/// badges, and a one-line summary.
pub(super) fn release_row(ix: usize, release: &Release, cx: &App) -> ListItem {
    let muted_foreground = cx.theme().muted_foreground;
    let foreground = cx.theme().foreground;
    let warning = cx.theme().warning;
    let title = release
        .name
        .clone()
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| release.tag_name.clone());
    let badge = |text: &'static str, color: gpui::Hsla| {
        div()
            .px_1p5()
            .py_0p5()
            .rounded_full()
            .text_xs()
            .bg(color.opacity(0.15))
            .text_color(color)
            .child(text)
    };
    ListItem::new(("helm-release", ix))
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
                                .truncate()
                                .text_sm()
                                .font_semibold()
                                .text_color(foreground)
                                .child(title),
                        )
                        .when(release.draft, |row| row.child(badge("draft", muted_foreground)))
                        .when(release.prerelease, |row| {
                            row.child(badge("pre-release", warning))
                        }),
                )
                .child(
                    div()
                        .truncate()
                        .text_xs()
                        .text_color(muted_foreground)
                        .child(release_summary(release)),
                ),
        )
        .suffix(move |_, _| {
            Icon::new(IconName::ExternalLink)
                .xsmall()
                .text_color(muted_foreground)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(body: Option<&str>, assets: usize) -> Release {
        let assets: Vec<serde_json::Value> = (0..assets)
            .map(|n| serde_json::json!({ "name": format!("asset-{n}.zip") }))
            .collect();
        serde_json::from_value(serde_json::json!({
            "id": 1,
            "tag_name": "v1.0.0",
            "name": "One",
            "body": body,
            "draft": false,
            "prerelease": false,
            "html_url": "https://github.com/o/r/releases/tag/v1.0.0",
            "assets": assets,
        }))
        .expect("a release the backend's type accepts")
    }

    #[test]
    fn a_release_row_always_has_a_second_line() {
        // The list draws every row at one height, so no row may drop the line.
        assert_eq!(release_summary(&release(None, 0)), "No release notes");
        assert_eq!(release_summary(&release(Some("   
  "), 0)), "No release notes");
        assert_eq!(release_summary(&release(None, 1)), "1 asset");
        assert_eq!(release_summary(&release(None, 3)), "3 assets");
        assert_eq!(release_summary(&release(Some("Fixes"), 0)), "Fixes");
        assert_eq!(release_summary(&release(Some("Fixes"), 2)), "2 assets · Fixes");
    }

    #[test]
    fn release_notes_are_flattened_to_one_line_and_cut_at_120() {
        assert_eq!(
            release_summary(&release(Some("## Changes

- one
- two"), 0)),
            "## Changes - one - two"
        );
        let long = "x".repeat(200);
        let summary = release_summary(&release(Some(&long), 0));
        assert_eq!(summary.chars().count(), 121);
        assert!(summary.ends_with('…'));
    }
}

