//! Helm's collaborators screen for a repository: the list, changing a
//! collaborator's permission, and the add and remove dialogs.

use super::*;

/// GitHub's collaborator permission levels, in ascending order — used both
/// as the API's `permission` value and as the label shown in the dropdown.
pub(super) const COLLABORATOR_PERMISSIONS: [&str; 5] = ["pull", "triage", "push", "maintain", "admin"];

impl HelmPanel {
    /// Loads the collaborator list for `self.selected_repo` — mirrors
    /// `load_repos`'s shape.
    pub(super) fn load_collaborators(&mut self, cx: &mut Context<Self>) {
        self.load_collaborators_page(1, cx);
    }

    pub(super) fn load_collaborators_page(&mut self, page: u32, cx: &mut Context<Self>) {
        self.load_repo_page(cx, |this| &mut this.collaborators, page, |repo| {
            requests::collaborators(&repo.owner.login, &repo.name)
        });
    }

    /// Sets `login`'s permission on `self.selected_repo` — GitHub's
    /// add-collaborator endpoint doubles as the update-permission endpoint,
    /// so this is also how an existing collaborator's role is changed —
    /// then refreshes the list.
    pub(super) fn handle_set_collaborator_permission(
        &mut self,
        login: String,
        permission: String,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.run_action(
            HelmAction::SetCollaboratorPermission { login, permission },
            true,
            cx,
        );
    }

    /// Removes `login` as a collaborator on `self.selected_repo`, then
    /// refreshes the list.
    pub(super) fn handle_remove_collaborator(
        &mut self,
        login: String,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.run_action(HelmAction::RemoveCollaborator(login), true, cx);
    }

    /// The collaborators list — add/remove and per-row permission changes.
    pub(super) fn render_collaborators(&self, cx: &mut Context<Self>) -> impl IntoElement {
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
                    .child("Collaborators"),
            )
            .child(
                Button::new("collaborators-add")
                    .ghost()
                    .xsmall()
                    .icon(IconName::Plus)
                    .label("Add collaborator")
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_add_collaborator_dialog(window, cx)
                    })),
            );

        self.list_screen(
            self.collaborators
                .paged_status(|this, page, cx| this.load_collaborators_page(page, cx)),
            &self.collaborators_list,
            Some(header.into_any_element()),
            ListLabels {
                loading: "Loading collaborators…",
                error: "Failed to load collaborators",
                empty: "No collaborators",
            },
            |this, cx| this.load_collaborators_page(this.collaborators.page, cx),
            cx,
        )
    }

    /// Opens the "Add collaborator" dialog: a username and one of GitHub's
    /// permission levels.
    pub(super) fn open_add_collaborator_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_workspace_modal(HelmModalKind::AddCollaborator, window, cx);
    }

    /// Asks before removing `login` from `self.selected_repo`'s
    /// collaborators.
    pub(super) fn open_remove_collaborator_confirm(
        &mut self,
        login: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_workspace_modal(HelmModalKind::RemoveCollaborator(login), window, cx);
    }
}

/// One row of the Collaborators screen: avatar and login, a dropdown to
/// change the permission, and a remove button.
///
/// The dropdown and the button act on the panel, which a row is not given
/// (it is drawn by the list, not by the panel), so they reach it through
/// `panel`.
pub(super) fn collaborator_row(
    ix: usize,
    collab: &Collaborator,
    panel: &WeakEntity<HelmPanel>,
    cx: &App,
) -> ListItem {
    let foreground = cx.theme().foreground;
    let login = collab.login.clone();
    let role_name = collab.role_name.clone();
    let collab_id = collab.id;
    let panel = panel.clone();

    ListItem::new(("helm-collaborator", ix))
        .child(
            h_flex()
                .items_center()
                .gap_2()
                .child(
                    Avatar::new()
                        .src(collab.avatar_url.clone())
                        .name(login.clone())
                        .with_size(px(48.)),
                )
                .child(div().text_color(foreground).child(login.clone())),
        )
        .suffix(move |_, _| {
            let role_panel = panel.clone();
            let role_login = login.clone();
            let remove_panel = panel.clone();
            let remove_login = login.clone();

            h_flex()
                .items_center()
                .gap_2()
                .child(
                    Button::new(("helm-collaborator-role", collab_id))
                        .ghost()
                        .xsmall()
                        .label(role_name.clone())
                        .dropdown_menu(move |menu, _, _| {
                            COLLABORATOR_PERMISSIONS.iter().fold(menu, |menu, perm| {
                                let role_panel = role_panel.clone();
                                let role_login = role_login.clone();
                                let perm = perm.to_string();
                                menu.item(PopupMenuItem::new(perm.clone()).on_click(
                                    move |_, window, cx| {
                                        role_panel
                                            .update(cx, |this, cx| {
                                                this.handle_set_collaborator_permission(
                                                    role_login.clone(),
                                                    perm.clone(),
                                                    window,
                                                    cx,
                                                );
                                            })
                                            .ok();
                                    },
                                ))
                            })
                        }),
                )
                .child(
                    Button::new(("helm-collaborator-remove", collab_id))
                        .ghost()
                        .xsmall()
                        .icon(IconName::Delete)
                        .tooltip("Remove collaborator")
                        .on_click(move |_, window, cx| {
                            remove_panel
                                .update(cx, |this, cx| {
                                    this.open_remove_collaborator_confirm(
                                        remove_login.clone(),
                                        window,
                                        cx,
                                    );
                                })
                                .ok();
                        }),
                )
        })
}
