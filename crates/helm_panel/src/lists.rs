//! The list widget of every list screen, built once when the panel is.
//!
//! Each function here says three things about one screen's list: where its
//! rows are, which function draws a row, and what a click or `enter` on a
//! row does. See `list_view.rs` for what a [`ListView`] is.

use super::*;

pub(super) fn menu_list(window: &mut Window, cx: &mut Context<HelmPanel>) -> ListView {
    ListView::sectioned(
        Vec::new(),
        |panel, _| panel.menu_rows().len(),
        |panel, ix, cx| {
            Some(profile::menu_row(
                ix.row,
                panel.menu_rows().get(ix.row)?,
                cx,
            ))
        },
        |this, ix, window, cx| {
            if let Some(id) = this.menu_rows().get(ix.row).map(|row| row.id) {
                this.open_menu_row(id, window, cx);
            }
        },
        window,
        cx,
    )
}

pub(super) fn profile_menu_list(window: &mut Window, cx: &mut Context<HelmPanel>) -> ListView {
    ListView::sectioned(
        Vec::new(),
        |panel, _| panel.profile_rows().len(),
        |panel, ix, cx| {
            Some(profile::profile_row(
                ix.row,
                panel.profile_rows().get(ix.row)?,
                cx,
            ))
        },
        |this, ix, window, cx| {
            if let Some(id) = this.profile_rows().get(ix.row).map(|row| row.id) {
                this.open_profile_row(id, window, cx);
            }
        },
        window,
        cx,
    )
}

pub(super) fn org_logins_list(window: &mut Window, cx: &mut Context<HelmPanel>) -> ListView {
    ListView::new(
        |panel| &panel.org_logins,
        orgs::org_row,
        |this, ix, _, cx| {
            if let Some(org) = this.org_logins.items.get(ix).cloned() {
                this.select_org(org, cx);
            }
        },
        window,
        cx,
    )
}

pub(super) fn repos_list(window: &mut Window, cx: &mut Context<HelmPanel>) -> ListView {
    ListView::sectioned(
        Vec::new(),
        |panel, _| panel.repos_shown.len(),
        |panel, ix, cx| {
            let repo = panel.repos.items.get(*panel.repos_shown.get(ix.row)?)?;
            Some(repos::repo_row(ix.row, repo, cx))
        },
        |this, ix, _, cx| {
            let repo = this
                .repos_shown
                .get(ix.row)
                .and_then(|position| this.repos.items.get(*position))
                .cloned();
            if let Some(repo) = repo {
                this.select_repo(repo, cx);
            }
        },
        window,
        cx,
    )
}

pub(super) fn github_search_list(window: &mut Window, cx: &mut Context<HelmPanel>) -> ListView {
    ListView::new(
        |panel| &panel.github_search_results,
        repos::search_repo_row,
        |this, ix, _, cx| {
            if let Some(repo) = this.github_search_results.items.get(ix).cloned() {
                this.select_repo(repo, cx);
            }
        },
        window,
        cx,
    )
}

pub(super) fn repo_sections_list(window: &mut Window, cx: &mut Context<HelmPanel>) -> ListView {
    ListView::sectioned(
        Vec::new(),
        |_, _| repos::repo_sections().len(),
        |_, ix, cx| {
            let (_, icon, label) = repos::repo_sections().into_iter().nth(ix.row)?;
            Some(repos::repo_section_row(ix.row, icon, label, cx))
        },
        |this, ix, _, cx| {
            if let Some((screen, _, _)) = repos::repo_sections().into_iter().nth(ix.row) {
                this.open_repo_section(screen, cx);
            }
        },
        window,
        cx,
    )
}

/// Read-only. A row needs the repository's default branch to
/// mark it, which the panel knows and the row does not.
pub(super) fn branches_list(window: &mut Window, cx: &mut Context<HelmPanel>) -> ListView {
    ListView::new(
        |panel| &panel.branches,
        {
            let panel: WeakEntity<HelmPanel> = cx.weak_entity();
            move |ix: usize, branch: &Branch, cx: &App| {
                let default_branch: String = panel
                    .upgrade()
                    .and_then(|panel| {
                        let repo = panel.read(cx).selected_repo.as_ref()?;
                        Some(repo.default_branch.clone())
                    })
                    .unwrap_or_default();
                branch_row(ix, branch, &default_branch, cx)
            }
        },
        |_, _, _, _| {},
        window,
        cx,
    )
}

pub(super) fn collaborators_list(window: &mut Window, cx: &mut Context<HelmPanel>) -> ListView {
    ListView::new(
        |panel| &panel.collaborators,
        {
            let panel: WeakEntity<HelmPanel> = cx.weak_entity();
            move |ix: usize, collab: &Collaborator, cx: &App| {
                collaborators::collaborator_row(ix, collab, &panel, cx)
            }
        },
        |this, ix, _, cx| {
            if let Some(collab) = this.collaborators.items.get(ix) {
                let login = collab.login.clone();
                this.open_user_profile(login, cx);
            }
        },
        window,
        cx,
    )
}

pub(super) fn issues_list(window: &mut Window, cx: &mut Context<HelmPanel>) -> ListView {
    ListView::new(
        |panel| &panel.issues,
        issue_row,
        |this, ix, _, cx| {
            if let Some(issue) = this.issues.items.get(ix).cloned() {
                this.open_issue_detail(issue, cx);
            }
        },
        window,
        cx,
    )
}

pub(super) fn pulls_list(window: &mut Window, cx: &mut Context<HelmPanel>) -> ListView {
    ListView::new(
        |panel| &panel.pulls,
        pull_row,
        |this, ix, _, cx| {
            if let Some(pr) = this.pulls.items.get(ix).cloned() {
                this.open_pr_detail(pr, cx);
            }
        },
        window,
        cx,
    )
}

pub(super) fn releases_list(window: &mut Window, cx: &mut Context<HelmPanel>) -> ListView {
    ListView::new(
        |panel| &panel.releases,
        release_row,
        |this, ix, _, cx| {
            if let Some(release) = this.releases.items.get(ix) {
                cx.open_url(&release.html_url);
            }
        },
        window,
        cx,
    )
}

pub(super) fn packages_list(window: &mut Window, cx: &mut Context<HelmPanel>) -> ListView {
    ListView::sectioned(
        Vec::new(),
        |panel, _| panel.packages.items.len(),
        |panel, ix, cx| {
            let package = panel.packages.items.get(ix.row)?;
            let open = panel.expanded_package.as_deref() == Some(package.name.as_str());
            Some(releases_packages::package_row(ix.row, package, open, cx))
        },
        // Opens the package's versions above the list, or closes
        // them if they are the ones showing.
        |this, ix, _, cx| {
            let package = this.packages.items.get(ix.row).cloned();
            let owner = this
                .selected_repo
                .as_ref()
                .map(|repo| repo.owner.login.clone());
            if let (Some(package), Some(owner)) = (package, owner) {
                this.toggle_package_versions(owner, package, cx);
            }
        },
        window,
        cx,
    )
}

pub(super) fn commits_list(window: &mut Window, cx: &mut Context<HelmPanel>) -> ListView {
    ListView::new(
        |panel| &panel.commits,
        commit_row,
        |_, _, _, _| {},
        window,
        cx,
    )
}

pub(super) fn workflow_runs_list(window: &mut Window, cx: &mut Context<HelmPanel>) -> ListView {
    ListView::new(
        |panel| &panel.workflow_runs,
        workflow_run_row,
        |this, ix, window, cx| {
            if let Some(run) = this.workflow_runs.items.get(ix).cloned() {
                this.select_workflow_run(run, window, cx);
            }
        },
        window,
        cx,
    )
}

pub(super) fn deployments_list(window: &mut Window, cx: &mut Context<HelmPanel>) -> ListView {
    ListView::new(
        |panel| &panel.deployments,
        activity::deployment_row,
        |_, _, _, _| {},
        window,
        cx,
    )
}

/// Tags are read-only: nothing happens on `enter` or a click.
pub(super) fn tags_list(window: &mut Window, cx: &mut Context<HelmPanel>) -> ListView {
    ListView::new(|panel| &panel.tags, tag_row, |_, _, _, _| {}, window, cx)
}

/// Read-only: an alert has no detail screen to open.
pub(super) fn security_list(window: &mut Window, cx: &mut Context<HelmPanel>) -> ListView {
    ListView::sectioned(
        vec!["Dependabot alerts", "Secret scanning alerts"],
        |panel, section| match section {
            0 => panel.dependabot_alerts.items.len(),
            _ => panel.secret_scanning_alerts.items.len(),
        },
        |panel, ix, cx| match ix.section {
            0 => Some(insights::dependabot_row(
                ix.row,
                panel.dependabot_alerts.items.get(ix.row)?,
                cx,
            )),
            _ => Some(insights::secret_scanning_row(
                ix.row,
                panel.secret_scanning_alerts.items.get(ix.row)?,
                cx,
            )),
        },
        |_, _, _, _| {},
        window,
        cx,
    )
}

pub(super) fn invitations_list(window: &mut Window, cx: &mut Context<HelmPanel>) -> ListView {
    {
        let row_panel: WeakEntity<HelmPanel> = cx.weak_entity();
        ListView::sectioned(
            vec!["Organizations", "Repositories"],
            |panel, section| match section {
                0 => panel.org_invitations.len(),
                _ => panel.invitations.items.len(),
            },
            move |panel, ix, cx| match ix.section {
                0 => Some(invitations::org_invitation_row(
                    ix.row,
                    panel.org_invitations.get(ix.row)?,
                    &row_panel,
                    cx,
                )),
                _ => Some(invitations::repo_invitation_row(
                    ix.row,
                    panel.invitations.items.get(ix.row)?,
                    &row_panel,
                    cx,
                )),
            },
            // Nothing on a click or `enter`: see
            // `invitations::invitation_buttons`.
            |_, _, _, _| {},
            window,
            cx,
        )
    }
}
