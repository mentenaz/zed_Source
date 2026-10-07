//! Helm's release screens for a repository: releases (list and create),
//! packages with their versions, and tags.

use super::*;

impl HelmPanel {
    /// Loads `self.selected_repo`'s releases.
    pub(super) fn load_releases(&mut self, cx: &mut Context<Self>) {
        self.load_releases_page(1, cx);
    }

    pub(super) fn load_releases_page(&mut self, page: u32, cx: &mut Context<Self>) {
        self.load_repo_page(cx, |this| &mut this.releases, page, |repo| {
            requests::releases(&repo.owner.login, &repo.name)
        });
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
                    Err(e) => this.package_versions_error = Some(e.to_string()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Loads `self.selected_repo`'s tags.
    pub(super) fn load_tags(&mut self, cx: &mut Context<Self>) {
        self.load_tags_page(1, cx);
    }

    pub(super) fn load_tags_page(&mut self, page: u32, cx: &mut Context<Self>) {
        self.load_repo_page(cx, |this| &mut this.tags, page, |repo| {
            requests::tags(&repo.owner.login, &repo.name)
        });
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
            self.releases
                .paged_status(|this, page, cx| this.load_releases_page(page, cx)),
            &self.releases_list,
            Some(header.into_any_element()),
            ListLabels {
                loading: "Loading releases…",
                error: "Failed to load releases",
                empty: "No releases yet",
            },
            |this, cx| this.load_releases_page(this.releases.page, cx),
            cx,
        )
    }

    /// The Packages tab — owner-scoped package list; tapping a package
    /// expands its versions inline.
    pub(super) fn render_packages(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let versions = self
            .expanded_package
            .clone()
            .map(|package| self.render_package_versions(package, cx).into_any_element());
        self.list_screen(
            self.packages.status(),
            &self.packages_list,
            versions,
            ListLabels {
                loading: "Loading packages…",
                error: "Failed to load packages",
                empty: "No packages found",
            },
            |this, cx| this.load_packages(cx),
            cx,
        )
    }

    /// The versions of the package that was opened, shown above the list.
    ///
    /// They used to unfold inside the package's own row. The list draws all
    /// of its rows at one height, so a row can no longer grow; the versions
    /// have their own block instead, with a close button.
    fn render_package_versions(&self, package: String, cx: &mut Context<Self>) -> impl IntoElement {
        let muted_foreground = cx.theme().muted_foreground;
        let foreground = cx.theme().foreground;

        let title = h_flex()
            .items_center()
            .justify_between()
            .gap_2()
            .px_3()
            .pt_2()
            .child(
                div()
                    .truncate()
                    .text_xs()
                    .font_semibold()
                    .text_color(muted_foreground)
                    .child(format!("Versions of {package}")),
            )
            .child(
                Button::new("helm-package-versions-close")
                    .ghost()
                    .xsmall()
                    .icon(IconName::Close)
                    .tooltip("Close versions")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.expanded_package = None;
                        this.package_versions.clear();
                        this.package_versions_error = None;
                        cx.notify();
                    })),
            );

        let body = if self.package_versions.is_empty() && self.package_versions_error.is_some() {
            div()
                .px_3()
                .py_1()
                .text_xs()
                .text_color(muted_foreground)
                .child("Failed to load versions")
                .into_any_element()
        } else if self.package_versions.is_empty() {
            h_flex()
                .gap_2()
                .items_center()
                .px_3()
                .py_1()
                .child(Spinner::new().small())
                .child(
                    div()
                        .text_xs()
                        .text_color(muted_foreground)
                        .child("Loading versions…"),
                )
                .into_any_element()
        } else {
            v_flex()
                .id("helm-package-versions")
                // A package can have a long history; the list of packages
                // below still needs room.
                .max_h(px(180.))
                .overflow_y_scroll()
                .children(self.package_versions.iter().map(|version| {
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

        v_flex().pb_2().child(title).child(body)
    }

    /// The Tags screen.
    pub(super) fn render_tags(&self, cx: &mut Context<Self>) -> impl IntoElement {
        self.list_screen(
            self.tags
                .paged_status(|this, page, cx| this.load_tags_page(page, cx)),
            &self.tags_list,
            None,
            ListLabels {
                loading: "Loading tags…",
                error: "Failed to load tags",
                empty: "No tags found",
            },
            |this, cx| this.load_tags_page(this.tags.page, cx),
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
    let title = release
        .name
        .clone()
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| release.tag_name.clone());
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
                        .when(release.draft, |row| row.child(chip("draft")))
                        .when(release.prerelease, |row| {
                            row.child(Pill::warning().xsmall().rounded_full().child("pre-release"))
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

/// One row of the Packages screen: name and type, then the description.
/// Every row has the second line (see `release_summary` for why). `open`
/// marks the package whose versions are showing above the list.
pub(super) fn package_row(ix: usize, package: &Package, open: bool, cx: &App) -> ListItem {
    let foreground = cx.theme().foreground;
    let muted_foreground = cx.theme().muted_foreground;
    let visibility = package.visibility.clone();
    let description = package
        .description
        .clone()
        .filter(|description| !description.trim().is_empty())
        .unwrap_or_else(|| "No description".to_string());
    ListItem::new(("helm-package", ix))
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
                                .child(package.name.clone()),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(muted_foreground)
                                .child(package.package_type.clone()),
                        ),
                )
                .child(
                    div()
                        .truncate()
                        .text_xs()
                        .text_color(muted_foreground)
                        .child(description),
                ),
        )
        .suffix(move |_, _| {
            h_flex()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .text_xs()
                        .text_color(muted_foreground)
                        .child(visibility.clone()),
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

