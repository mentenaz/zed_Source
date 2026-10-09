//! Helm's release screens for a repository: releases (list and create),
//! packages with their versions, and tags.

use super::*;

impl HelmPanel {
    /// Loads `self.selected_repo`'s releases.
    pub(super) fn load_releases(&mut self, cx: &mut Context<Self>) {
        self.load_releases_page(1, cx);
    }

    pub(super) fn load_releases_page(&mut self, page: u32, cx: &mut Context<Self>) {
        self.load_repo_page(
            cx,
            |this| &mut this.releases,
            page,
            |repo| requests::releases(&repo.owner.login, &repo.name),
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
    pub(super) fn toggle_package_versions(
        &mut self,
        owner: String,
        pkg: Package,
        cx: &mut Context<Self>,
    ) {
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
        self.load_repo_page(
            cx,
            |this| &mut this.tags,
            page,
            |repo| requests::tags(&repo.owner.login, &repo.name),
        );
    }

    /// Opens the "Create release" modal.
    pub(super) fn open_create_release_dialog(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
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
            self.packages.status_for::<HelmPanel>(),
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
            div()
                .id("helm-package-versions")
                // A package can have a long history; the list of packages
                // below still needs room.
                .max_h(px(180.))
                .overflow_y_scroll()
                .px_3()
                .py_1()
                .child(
                    DescriptionList::horizontal()
                        .bordered(false)
                        .columns(1)
                        .label_width(relative(0.6))
                        .children(self.package_versions.iter().map(|version| {
                            DescriptionItem::new(version.name.clone())
                                .value(short_date(&version.created_at))
                        })),
                )
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
