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
        self.load_section(
            cx,
            |this| &mut this.collaborators,
            |repo, gh_state| async move {
                gh_get_collaborators(repo.owner.login, repo.name, &gh_state).await
            },
        );
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
            self.collaborators.status(),
            &self.collaborators_list,
            Some(header.into_any_element()),
            ListLabels {
                loading: "Loading collaborators…",
                error: "Failed to load collaborators",
                empty: "No collaborators",
            },
            |this, cx| this.load_collaborators(cx),
            cx,
        )
    }

    /// Opens the "Add collaborator" dialog: a username input plus a
    /// permission dropdown, matching GitHub's own permission levels.
    pub(super) fn open_add_collaborator_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_workspace_modal(HelmModalKind::AddCollaborator, window, cx);
        return;

        /*
        let username = cx.new(|cx| InputState::new(window, cx).placeholder("GitHub username"));
        let permission = Rc::new(Cell::new(0usize));
        let view = cx.entity();

        window.open_dialog(cx, move |dialog, _, _| {
            let username = username.clone();
            let permission = permission.clone();
            let view = view.clone();
            let permission_label = COLLABORATOR_PERMISSIONS[permission.get()];

            dialog
                .title("Add collaborator")
                .child(
                    v_flex().gap_3().child(Input::new(&username)).child(
                        Button::new("add-collaborator-permission")
                            .outline()
                            .label(permission_label)
                            .dropdown_menu({
                                let permission = permission.clone();
                                move |menu, _, _| {
                                    COLLABORATOR_PERMISSIONS.iter().enumerate().fold(
                                        menu,
                                        |menu, (ix, perm)| {
                                            let permission = permission.clone();
                                            menu.item(
                                                PopupMenuItem::new(*perm)
                                                    .checked(ix == permission.get())
                                                    .on_click(move |_, window, _| {
                                                        permission.set(ix);
                                                        window.refresh();
                                                    }),
                                            )
                                        },
                                    )
                                }
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
                                Button::new("add-collaborator-confirm")
                                    .primary()
                                    .label("Add"),
                            ),
                        ),
                )
                .on_ok(move |_, window, cx| {
                    let login = username.read(cx).value().trim().to_string();
                    let perm = COLLABORATOR_PERMISSIONS[permission.get()].to_string();
                    if login.is_empty() {
                        return false;
                    }
                    view.update(cx, |this, cx| {
                        this.handle_set_collaborator_permission(login, perm, window, cx);
                    });
                    true
                })
        });
        */
    }

    /// Confirms removing `login` from `self.selected_repo`'s collaborators —
    /// same danger-variant `AlertDialog` shape as the editor's "Disregard
    /// changes" confirm.
    pub(super) fn open_remove_collaborator_confirm(
        &mut self,
        login: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_workspace_modal(HelmModalKind::RemoveCollaborator(login), window, cx);
        return;

        /*
        let view = cx.entity();

        window.open_alert_dialog(cx, move |alert, _, _| {
            let view = view.clone();
            let login = login.clone();

            alert
                .title("Remove collaborator?")
                .description(format!(
                    "{login} will lose access to this repository immediately."
                ))
                .button_props(
                    DialogButtonProps::default()
                        .ok_variant(ButtonVariant::Danger)
                        .ok_text("Remove")
                        .cancel_text("Cancel")
                        .show_cancel(true),
                )
                .on_ok(move |_, window, cx| {
                    let login = login.clone();
                    view.update(cx, |this, cx| {
                        this.handle_remove_collaborator(login, window, cx);
                    });
                    true
                })
        });
        */
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
