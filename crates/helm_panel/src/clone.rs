//! Cloning a repository from Helm: the clone dialog, choosing the target
//! folder, streaming progress, and opening the result as a workspace.

use super::*;

impl HelmPanel {
    /// Copies the repo's clone URL and flips the icon to a checkmark for 2s
    /// — same pattern as `handle_copy_code` for the device-flow code.
    pub(super) fn handle_copy_clone_url(&mut self, cx: &mut Context<Self>) {
        let Some(repo) = self.selected_repo.as_ref() else {
            return;
        };
        cx.write_to_clipboard(ClipboardItem::new_string(repo.clone_url.clone()));
        self.clone_url_copied = true;
        cx.notify();

        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_secs(2)).await;
            this.update(cx, |this, cx| {
                this.clone_url_copied = false;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The exact path a clone of `self.selected_repo` would land at — the
    /// current workspace's first worktree root (or the user-picked
    /// `clone_target_dir`) joined with the repo's name. `None` when no
    /// folder is open yet or no repo is selected.
    pub(super) fn clone_target_path(&self, cx: &App) -> Option<String> {
        let repo = self.selected_repo.as_ref()?;
        let parent = match self.clone_target_dir.clone() {
            Some(dir) => dir,
            None => self
                .workspace
                .upgrade()?
                .read(cx)
                .worktrees(cx)
                .next()?
                .read(cx)
                .abs_path()
                .to_string_lossy()
                .into_owned(),
        };
        Some(
            std::path::Path::new(&parent)
                .join(&repo.name)
                .to_string_lossy()
                .into_owned(),
        )
    }

    /// Opens the Clone-repository overlay, resetting any previous
    /// attempt's progress/error/success state so it always starts fresh.
    pub(super) fn open_clone_modal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_clone_modal_with_file(None, window, cx);
    }

    pub(super) fn open_clone_modal_for_file(
        &mut self,
        file: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.open_clone_modal_with_file(file, window, cx);
    }

    fn open_clone_modal_with_file(
        &mut self,
        file: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.cloning = false;
        self.clone_lines.clear();
        self.clone_progress = None;
        self.clone_error = None;
        self.clone_succeeded_path = None;
        self.clone_file_to_open = file;
        self.open_workspace_modal(HelmModalKind::CloneRepo, window, cx);
    }

    /// Opens `path` (a just-completed clone) as a new workspace window — the
    /// Clone overlay's "Yes" button, called instead of doing this
    /// automatically so the user can decline and clone elsewhere without a
    /// second window popping up unasked.
    pub(super) fn handle_clone_open_workspace(
        &mut self,
        path: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.workspace
            .update(cx, |workspace, cx| {
                workspace
                    .open_workspace_for_paths(
                        workspace::OpenMode::NewWindow,
                        vec![std::path::PathBuf::from(&path)],
                        window,
                        cx,
                    )
                    .detach_and_log_err(cx);
            })
            .ok();
    }

    /// Clones `self.selected_repo` into [`Self::clone_target_path`] with
    /// `git`, as the account this panel uses, and opens the clone as a new
    /// workspace window. Installing dependencies is left to the new
    /// workspace (`npm_bootstrap` offers it when there is a `package.json`).
    /// No-ops if no folder is open yet (see `render_repo_detail`, which
    /// disables the Clone button in that case).
    pub(super) fn handle_clone(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(repo) = self.selected_repo.clone() else {
            return;
        };
        let Some(target_path) = self.clone_target_path(cx) else {
            return;
        };

        self.cloning = true;
        self.clone_lines.clear();
        self.clone_progress = None;
        self.clone_error = None;
        let file_to_open = self.clone_file_to_open.clone();
        let (canceller, cancel_signal) = clone_canceller();
        self.clone_canceller = Some(canceller);
        cx.notify();

        // Subscribe before starting the clone so no early lines are missed:
        // a broadcast channel only delivers what is sent after subscribing.
        let mut rx = self.gh_state.clone_tx.subscribe();
        cx.spawn_in(window, async move |this, cx| {
            loop {
                let (event, rx2) = on_tokio(async move {
                    let event = rx.recv().await;
                    (event, rx)
                })
                .await;
                rx = rx2;
                let alive = match event {
                    Ok(CloneEvent::Line(line)) => this.update(cx, |this, cx| {
                        this.clone_progress = None;
                        this.clone_lines.push(line);
                        cx.notify();
                    }),
                    Ok(CloneEvent::Progress(line)) => this.update(cx, |this, cx| {
                        this.clone_progress = Some(line);
                        cx.notify();
                    }),
                    Ok(CloneEvent::Done) => break,
                    // `Lagged` means missed events, not a dead channel —
                    // resync and keep listening; only `Closed` ends this loop.
                    Err(EventRecvError::Lagged(_)) => continue,
                    Err(EventRecvError::Closed) => break,
                };
                if alive.is_err() {
                    break;
                }
            }
        })
        .detach();

        let gh_state = self.gh_state.clone();
        let full_name = repo.full_name.clone();
        let target_for_git = target_path.clone();
        let repo_name = repo.name.clone();
        cx.spawn_in(window, async move |this, cx| {
            let result = on_tokio(async move {
                clone_repo(full_name, target_for_git, cancel_signal, &gh_state).await
            })
            .await;
            this.update_in(cx, |this, window, cx| {
                this.cloning = false;
                this.clone_canceller = None;
                this.clone_progress = None;
                match result {
                    Ok(()) => {
                        this.clone_lines
                            .push(format!("Cloned repository to {target_path}"));
                        this.notify(format!("Cloned {repo_name}"), cx);
                        let mut paths = vec![std::path::PathBuf::from(&target_path)];
                        if let Some(file) = file_to_open.as_deref()
                            && let Some(file_path) = safe_clone_file_path(&target_path, file)
                        {
                            paths.push(file_path);
                        }
                        this.workspace
                            .update(cx, |workspace, cx| {
                                workspace
                                    .open_workspace_for_paths(
                                        workspace::OpenMode::NewWindow,
                                        paths,
                                        window,
                                        cx,
                                    )
                                    .detach_and_log_err(cx);
                            })
                            .ok();
                        this.clone_file_to_open = None;
                    }
                    // Back to the start of the dialog, as if it had not run.
                    Err(CloneFailure::Cancelled) => {
                        this.clone_lines.clear();
                        this.notify(format!("Cancelled cloning {repo_name}"), cx);
                    }
                    Err(CloneFailure::Failed(error)) => this.clone_error = Some(error),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Stops the clone in progress; the half-cloned folder is removed.
    pub(super) fn cancel_clone(&mut self, cx: &mut Context<Self>) {
        if let Some(canceller) = self.clone_canceller.take() {
            canceller.cancel();
            self.clone_progress = Some("Cancelling…".into());
            cx.notify();
        }
    }

    /// Native folder picker for the Clone destination — stores the chosen
    /// parent directory in `clone_target_dir`; leaving it unset drags along
    /// the current workspace root. Cancelling the dialog keeps the previous
    /// choice (or the default).
    pub(super) fn pick_clone_dir(&mut self, cx: &mut Context<Self>) {
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Choose Clone Destination".into()),
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = receiver.await {
                if let Some(path) = paths.first() {
                    this.update(cx, |this, cx| {
                        this.clone_target_dir = Some(path.to_string_lossy().into_owned());
                        cx.notify();
                    })
                    .ok();
                }
            }
        })
        .detach();
    }
}

fn safe_clone_file_path(root: &str, file: &str) -> Option<std::path::PathBuf> {
    let file = std::path::Path::new(file);
    if file.as_os_str().is_empty()
        || file.is_absolute()
        || file
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
    {
        return None;
    }
    Some(std::path::Path::new(root).join(file))
}
