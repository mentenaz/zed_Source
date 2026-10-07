//! Helm's repository screens: the list with its filter, the repository detail
//! screen, and the create and edit dialogs.

use super::*;

impl HelmPanel {
    /// Navigate to the repo list, loading either the selected org's repos
    /// or the user's own (`"self"`) — mirrors the old TS
    /// `loadRepos(selectedOrg || "self")`.
    pub(super) fn open_repo_list(&mut self, cx: &mut Context<Self>) {
        let owner = self
            .selected_org
            .clone()
            .unwrap_or_else(|| "self".to_string());
        self.navigate_to(HelmScreen::RepoList, cx);
        self.load_repos(owner, cx);
    }

    pub(super) fn load_repos(&mut self, owner: String, cx: &mut Context<Self>) {
        self.load_section_with(
            cx,
            |this| &mut this.repos,
            |gh_state| async move { gh_get_repos(owner, &gh_state).await },
        );
    }

    /// User picked a repo from `RepoList` — mirrors the old TS
    /// `setSelectedRepo` + `setScreen("repo-detail")` pair. No extra fetch
    /// needed, the list response already has everything the detail screen
    /// shows.
    pub(super) fn select_repo(&mut self, repo: Repo, cx: &mut Context<Self>) {
        self.selected_repo = Some(repo);
        self.navigate_to(HelmScreen::RepoDetail, cx);
    }

    /// Creates a repo under the authenticated user's own account, or under the
    /// given org (`Some(owner)`) via `gh_create_repo`'s `"owner"` key — the
    /// key is stripped from the request body and rerouted to
    /// `POST /orgs/{org}/repos` under the hood. On success, jumps straight to
    /// the new repo's detail screen; on failure, surfaces the error via a
    /// notification since the Menu screen (where this is triggered from) has
    /// no dedicated place to show it.
    pub(super) fn handle_create_repo(
        &mut self,
        name: String,
        description: String,
        private: bool,
        owner: Option<String>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut opts = json!({
            "name": name,
            "description": description,
            "private": private,
        });
        if let Some(owner) = owner {
            opts["owner"] = json!(owner);
        }
        self.run_action(HelmAction::CreateRepo { opts }, true, cx);
    }

    /// Renames/updates description+homepage, toggles feature switches
    /// (issues/wiki/projects/discussions), optionally changes visibility,
    /// and updates topics for `self.selected_repo`.
    pub(super) fn handle_edit_repo(
        &mut self,
        name: String,
        description: String,
        homepage: String,
        topics: Vec<String>,
        private: bool,
        has_issues: bool,
        has_wiki: bool,
        has_projects: bool,
        has_discussions: bool,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(repo) = self.selected_repo.as_ref() else {
            return;
        };

        let mut changes = json!({
            "name": name,
            "description": description,
            "homepage": homepage,
            "has_issues": has_issues,
            "has_wiki": has_wiki,
            "has_projects": has_projects,
            "has_discussions": has_discussions,
        });
        // Only sent when the switch was actually flipped: a visibility change
        // is the one consequential field here, so an unrelated edit (or an
        // `internal` repo, which also reports `private: true`) must not
        // restate it.
        if private != repo.private {
            changes["private"] = json!(private);
        }

        self.run_action(HelmAction::EditRepo { changes, topics }, true, cx);
    }

    /// Opens the "Create repository" dialog — targets the authenticated
    /// user's own account by default; typing an org login in the
    /// "Organization" field creates it org-scoped instead (`gh_create_repo`
    /// reroutes the `"owner"` key to `POST /orgs/{org}/repos`).
    pub(super) fn open_create_repo_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_workspace_modal(HelmModalKind::CreateRepo, window, cx);
        return;

        /*
        let name = cx.new(|cx| InputState::new(window, cx).placeholder("my-new-repo"));
        let description =
            cx.new(|cx| InputState::new(window, cx).placeholder("Description (optional)"));
        let org = cx.new(|cx| {
            InputState::new(window, cx).placeholder("Organization (blank = your account)")
        });
        let private = Rc::new(Cell::new(true));
        let view = cx.entity();

        window.open_dialog(cx, move |dialog, _, _| {
            let name = name.clone();
            let description = description.clone();
            let org = org.clone();
            let private = private.clone();
            let view = view.clone();

            dialog
                .title("Create repository")
                .child(
                    v_flex()
                        .gap_3()
                        .child(Input::new(&name))
                        .child(Input::new(&description))
                        .child(Input::new(&org))
                        .child(
                            Switch::new("create-repo-private")
                                .label("Private")
                                .checked(private.get())
                                .on_click({
                                    let private = private.clone();
                                    move |checked: &bool, _, _| private.set(*checked)
                                }),
                        ),
                )
                .footer(
                    DialogFooter::new()
                        .child(
                            DialogClose::new()
                                .child(Button::new("cancel").outline().label("Cancel")),
                        )
                        .child(
                            DialogAction::new().child(
                                Button::new("create-repo-confirm").primary().label("Create"),
                            ),
                        ),
                )
                .on_ok(move |_, window, cx| {
                    let name_val = name.read(cx).value().to_string();
                    let description_val = description.read(cx).value().to_string();
                    let private_val = private.get();
                    let org_val = org.read(cx).value().to_string();
                    let org_val = (!org_val.trim().is_empty()).then(|| org_val.trim().to_string());
                    view.update(cx, |this, cx| {
                        this.handle_create_repo(
                            name_val,
                            description_val,
                            private_val,
                            org_val,
                            window,
                            cx,
                        );
                    });
                    true
                })
        });
        */
    }

    /// The repo list — mirrors the old `RepoListScreen`. Loads either the
    /// selected org's repos or the user's own, filterable by name. Rows
    /// route into `RepoDetail` via [`Self::select_repo`].
    pub(super) fn render_repo_list(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let search_row = div().px_3().py_2().child(Input::new(&self.repo_search));
        let owner = self
            .selected_org
            .clone()
            .unwrap_or_else(|| "self".to_string());
        self.list_screen(
            ListStatus {
                state: self.repos.state,
                error: self.repos.error.clone(),
                // `repos_shown` is the page of what the search box leaves,
                // worked out at the top of `render`.
                is_empty: self.repos_shown.is_empty(),
                pager: Some(Pager {
                    page: self.repos_page,
                    last_page: self.repos_last_page,
                    // Every repository is already here; another page is
                    // just another slice.
                    go: |this, page, cx| {
                        this.repos_page = page;
                        cx.notify();
                    },
                }),
            },
            &self.repos_list,
            Some(search_row.into_any_element()),
            ListLabels {
                loading: "Loading repositories…",
                error: "Failed to load repositories",
                empty: if self.repos.items.is_empty() {
                    "No repositories found"
                } else {
                    "No repositories match your search"
                },
            },
            move |this, cx| this.load_repos(owner.clone(), cx),
            cx,
        )
    }

    /// Repo detail: name/visibility, description, homepage, topics, clone
    /// URL + a Clone action that streams `gh repo clone` output and, on
    /// success, sets the clone as the new workspace root. The old app's
    /// inline-editable description/homepage is now the Settings/Edit dialog,
    /// there's no feature-toggle port, and Branches/Collaborators drill into
    /// real screens — so the read side plus Clone (with an optional custom
    /// target folder via [`Self::pick_clone_dir`]) is fully covered.
    pub(super) fn render_repo_detail(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let muted_foreground = cx.theme().muted_foreground;
        let foreground = cx.theme().foreground;
        let border = cx.theme().border;

        let Some(repo) = self.selected_repo.clone() else {
            return v_flex()
                .flex_1()
                .items_center()
                .justify_center()
                .p_4()
                .child(
                    div()
                        .text_sm()
                        .text_color(muted_foreground)
                        .child("No repo selected"),
                )
                .into_any_element();
        };

        let header_row = h_flex()
            .items_center()
            .justify_between()
            .gap_2()
            .px_3()
            .py_2()
            .child(
                h_flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .font_semibold()
                            .text_color(foreground)
                            .child(repo.name.clone()),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(muted_foreground)
                            .child(repo_vis_label(&repo)),
                    ),
            )
            .child(
                Button::new("helm-repo-edit")
                    .ghost()
                    .xsmall()
                    .icon(IconName::Settings)
                    .tooltip("Edit repository")
                    .on_click(
                        cx.listener(|this, _, window, cx| this.open_edit_repo_dialog(window, cx)),
                    ),
            );

        let topics_row = (!repo.topics.is_empty()).then(|| {
            h_flex()
                .flex_wrap()
                .gap_1()
                .px_3()
                .py_2()
                .children(repo.topics.iter().map(|t| {
                    div()
                        .px_2()
                        .py_0p5()
                        .rounded_full()
                        .text_xs()
                        .bg(cx.theme().muted)
                        .text_color(muted_foreground)
                        .child(t.clone())
                }))
        });

        let info_row = |label: &'static str, value: String| {
            v_flex()
                .gap_0p5()
                .px_3()
                .py_2()
                .child(div().text_xs().text_color(muted_foreground).child(label))
                .child(div().text_sm().text_color(foreground).child(value))
        };

        let description_row = info_row(
            "Description",
            repo.description.clone().unwrap_or_else(|| "—".to_string()),
        );
        let homepage_row = info_row(
            "Homepage",
            repo.homepage.clone().unwrap_or_else(|| "—".to_string()),
        );

        let copied = self.clone_url_copied;
        let clone_url_row = h_flex()
            .items_center()
            .justify_between()
            .gap_2()
            .px_3()
            .py_2()
            .child(
                v_flex()
                    .gap_0p5()
                    .min_w_0()
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted_foreground)
                            .child("Clone URL"),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(foreground)
                            .child(repo.clone_url.clone()),
                    ),
            )
            .child(
                Button::new("helm-repo-copy-clone-url")
                    .ghost()
                    .xsmall()
                    .icon(if copied {
                        IconName::Check
                    } else {
                        IconName::Copy
                    })
                    .tooltip(if copied { "Copied" } else { "Copy clone URL" })
                    .on_click(cx.listener(|this, _, _, cx| this.handle_copy_clone_url(cx))),
            );

        let workspace_root = self
            .workspace
            .update(cx, |workspace, cx| {
                workspace
                    .worktrees(cx)
                    .next()
                    .map(|worktree| worktree.read(cx).abs_path().to_path_buf())
            })
            .ok()
            .flatten();

        let clone_section = if workspace_root.is_some() {
            v_flex()
                .gap_2()
                .px_3()
                .py_2()
                .child(
                    Button::new("helm-repo-clone")
                        .primary()
                        .icon(IconName::Github)
                        .label("Clone repository")
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.open_clone_modal(window, cx)
                        })),
                )
                .into_any_element()
        } else {
            v_flex()
                .gap_1()
                .px_3()
                .py_2()
                .child(
                    div()
                        .text_sm()
                        .text_color(muted_foreground)
                        .child("Open a folder first (File → Open Folder) to clone into it."),
                )
                .child(
                    Button::new("helm-repo-clone")
                        .primary()
                        .icon(IconName::Github)
                        .label("Clone repository")
                        .disabled(true),
                )
                .into_any_element()
        };

        let nav_row = |id: &'static str, icon: IconName, label: &'static str| {
            ListItem::new(id)
                .child(
                    h_flex()
                        .items_center()
                        .gap_2()
                        .child(Icon::new(icon).xsmall().text_color(muted_foreground))
                        .child(div().text_color(foreground).child(label)),
                )
                .suffix(move |_, _| {
                    Icon::new(IconName::ChevronRight)
                        .xsmall()
                        .text_color(muted_foreground)
                })
        };

        v_flex()
            .child(header_row)
            .child(div().h_px().w_full().bg(border))
            .children(topics_row)
            .child(description_row)
            .child(homepage_row)
            .child(div().h_px().w_full().bg(border))
            .child(clone_url_row)
            .child(clone_section)
            .child(div().h_px().w_full().bg(border))
            .child(
                nav_row("helm-repo-branches", IconName::Git, "Branches").on_click(cx.listener(
                    |this, _, _, cx| {
                        this.navigate_to(HelmScreen::Branches, cx);
                        this.load_branches(cx);
                    },
                )),
            )
            .child(
                nav_row("helm-repo-collaborators", IconName::User, "Collaborators").on_click(
                    cx.listener(|this, _, _, cx| {
                        this.navigate_to(HelmScreen::Collaborators, cx);
                        this.load_collaborators(cx);
                    }),
                ),
            )
            .child(
                nav_row("helm-repo-issues", IconName::Inbox, "Issues").on_click(cx.listener(
                    |this, _, _, cx| {
                        this.navigate_to(HelmScreen::Issues, cx);
                        this.load_issues(cx);
                    },
                )),
            )
            .child(
                nav_row("helm-repo-pulls", IconName::Redo, "Pull requests").on_click(cx.listener(
                    |this, _, _, cx| {
                        this.navigate_to(HelmScreen::Pulls, cx);
                        this.load_pulls(cx);
                    },
                )),
            )
            .child(
                nav_row(
                    "helm-repo-releases",
                    IconName::GalleryVerticalEnd,
                    "Releases",
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.navigate_to(HelmScreen::Releases, cx);
                    this.load_releases(cx);
                })),
            )
            .child(
                nav_row("helm-repo-packages", IconName::HardDrive, "Packages").on_click(
                    cx.listener(|this, _, _, cx| {
                        this.navigate_to(HelmScreen::Packages, cx);
                        this.load_packages(cx);
                    }),
                ),
            )
            .child(
                nav_row("helm-repo-traffic", IconName::ChartPie, "Traffic").on_click(cx.listener(
                    |this, _, _, cx| {
                        this.navigate_to(HelmScreen::Traffic, cx);
                        this.load_traffic(cx);
                    },
                )),
            )
            .child(
                nav_row("helm-repo-commits", IconName::Git, "Commits").on_click(cx.listener(
                    |this, _, _, cx| {
                        this.navigate_to(HelmScreen::Commits, cx);
                        this.load_commits(cx);
                    },
                )),
            )
            .child(
                nav_row("helm-repo-actions", IconName::SquareTerminal, "Actions").on_click(
                    cx.listener(|this, _, _, cx| {
                        this.navigate_to(HelmScreen::WorkflowRuns, cx);
                        this.load_workflow_runs(cx);
                    }),
                ),
            )
            .child(
                nav_row("helm-repo-deployments", IconName::Globe, "Deployments").on_click(
                    cx.listener(|this, _, _, cx| {
                        this.navigate_to(HelmScreen::Deployments, cx);
                        this.load_deployments(cx);
                    }),
                ),
            )
            .child(
                nav_row("helm-repo-tags", IconName::Asterisk, "Tags").on_click(cx.listener(
                    |this, _, _, cx| {
                        this.navigate_to(HelmScreen::Tags, cx);
                        this.load_tags(cx);
                    },
                )),
            )
            .child(
                nav_row("helm-repo-security", IconName::TriangleAlert, "Security").on_click(
                    cx.listener(|this, _, _, cx| {
                        this.navigate_to(HelmScreen::Security, cx);
                        this.load_security(cx);
                    }),
                ),
            )
            .into_any_element()
    }

    /// Opens the "Edit repository" dialog, prefilled from `self.selected_repo`.
    pub(super) fn open_edit_repo_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(repo) = self.selected_repo.clone() else {
            return;
        };

        self.open_workspace_modal(HelmModalKind::EditRepo(repo), window, cx);
        return;

        /*
        let name = cx.new(|cx| InputState::new(window, cx).default_value(repo.name.clone()));
        let description = cx.new(|cx| {
            InputState::new(window, cx).default_value(repo.description.clone().unwrap_or_default())
        });
        let homepage = cx.new(|cx| {
            InputState::new(window, cx).default_value(repo.homepage.clone().unwrap_or_default())
        });
        let topics = cx.new(|cx| {
            InputState::new(window, cx)
                .default_value(repo.topics.join(", "))
                .placeholder("comma, separated, topics")
        });
        let has_issues = Rc::new(Cell::new(repo.has_issues));
        let has_wiki = Rc::new(Cell::new(repo.has_wiki));
        let has_projects = Rc::new(Cell::new(repo.has_projects));
        let has_discussions = Rc::new(Cell::new(repo.has_discussions));
        let view = cx.entity();

        window.open_dialog(cx, move |dialog, _, _| {
            let name = name.clone();
            let description = description.clone();
            let homepage = homepage.clone();
            let topics = topics.clone();
            let has_issues = has_issues.clone();
            let has_wiki = has_wiki.clone();
            let has_projects = has_projects.clone();
            let has_discussions = has_discussions.clone();
            let view = view.clone();

            dialog
                .title("Edit repository")
                .child(
                    v_flex()
                        .gap_3()
                        .child(Input::new(&name))
                        .child(Input::new(&description))
                        .child(Input::new(&homepage))
                        .child(Input::new(&topics))
                        .child(
                            Switch::new("edit-repo-has-issues")
                                .label("Issues")
                                .checked(has_issues.get())
                                .on_click({
                                    let has_issues = has_issues.clone();
                                    move |checked: &bool, _, _| has_issues.set(*checked)
                                }),
                        )
                        .child(
                            Switch::new("edit-repo-has-projects")
                                .label("Projects")
                                .checked(has_projects.get())
                                .on_click({
                                    let has_projects = has_projects.clone();
                                    move |checked: &bool, _, _| has_projects.set(*checked)
                                }),
                        )
                        .child(
                            Switch::new("edit-repo-has-wiki")
                                .label("Wiki")
                                .checked(has_wiki.get())
                                .on_click({
                                    let has_wiki = has_wiki.clone();
                                    move |checked: &bool, _, _| has_wiki.set(*checked)
                                }),
                        )
                        .child(
                            Switch::new("edit-repo-has-discussions")
                                .label("Discussions")
                                .checked(has_discussions.get())
                                .on_click({
                                    let has_discussions = has_discussions.clone();
                                    move |checked: &bool, _, _| has_discussions.set(*checked)
                                }),
                        ),
                )
                .footer(
                    DialogFooter::new()
                        .child(
                            DialogClose::new()
                                .child(Button::new("cancel").outline().label("Cancel")),
                        )
                        .child(
                            DialogAction::new()
                                .child(Button::new("edit-repo-confirm").primary().label("Save")),
                        ),
                )
                .on_ok(move |_, window, cx| {
                    let name_val = name.read(cx).value().to_string();
                    let description_val = description.read(cx).value().to_string();
                    let homepage_val = homepage.read(cx).value().to_string();
                    let topics_val: Vec<String> = topics
                        .read(cx)
                        .value()
                        .split(',')
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect();
                    let has_issues_val = has_issues.get();
                    let has_wiki_val = has_wiki.get();
                    let has_projects_val = has_projects.get();
                    let has_discussions_val = has_discussions.get();
                    view.update(cx, |this, cx| {
                        this.handle_edit_repo(
                            name_val,
                            description_val,
                            homepage_val,
                            topics_val,
                            has_issues_val,
                            has_wiki_val,
                            has_projects_val,
                            has_discussions_val,
                            window,
                            cx,
                        );
                    });
                    true
                })
        });
        */
    }
}

/// Which repositories the search box leaves, as positions in `repos`, in
/// their original order. An empty or blank query keeps them all. Matching
/// is on the repository's name, ignoring case.
pub(super) fn matching_repos(repos: &[Repo], query: &str) -> Vec<usize> {
    let query = query.trim().to_lowercase();
    repos
        .iter()
        .enumerate()
        .filter(|(_, repo)| query.is_empty() || repo.name.to_lowercase().contains(&query))
        .map(|(ix, _)| ix)
        .collect()
}

/// One row of the Repositories screen: the name and whether it is public,
/// private, internal or archived.
pub(super) fn repo_row(ix: usize, repo: &Repo, cx: &App) -> ListItem {
    let foreground = cx.theme().foreground;
    let muted_foreground = cx.theme().muted_foreground;
    let vis = repo_vis_label(repo);
    ListItem::new(("helm-repo", ix))
        .child(div().text_color(foreground).child(repo.name.clone()))
        .suffix(move |_, _| {
            h_flex()
                .items_center()
                .gap_2()
                .child(div().text_color(muted_foreground).child(vis))
                .child(
                    Icon::new(IconName::ChevronRight)
                        .xsmall()
                        .text_color(muted_foreground),
                )
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo(name: &str) -> Repo {
        serde_json::from_value(serde_json::json!({
            "id": 1,
            "name": name,
            "full_name": format!("me/{name}"),
            "owner": { "login": "me", "id": 1, "avatar_url": "", "type": "User" },
            "private": false,
            "description": null,
            "clone_url": "",
            "visibility": "public",
            "archived": false,
            "pushed_at": "",
            "has_issues": true,
            "has_wiki": true,
            "has_projects": true,
        }))
        .expect("a repository the backend's type accepts")
    }

    #[test]
    fn the_search_box_filters_by_name_and_keeps_order() {
        let repos = [repo("zed_Source"), repo("Forge.Scaffold.SDK"), repo("forge-templates")];
        assert_eq!(matching_repos(&repos, ""), vec![0, 1, 2]);
        assert_eq!(matching_repos(&repos, "   "), vec![0, 1, 2]);
        assert_eq!(matching_repos(&repos, "FORGE"), vec![1, 2]);
        assert_eq!(matching_repos(&repos, " zed "), vec![0]);
        assert!(matching_repos(&repos, "nothing").is_empty());
        assert!(matching_repos(&[], "zed").is_empty());
    }
}
