//! Offers, once per project directory, to run `npm install` when a workspace
//! is opened whose root has a `package.json` but no `node_modules` yet.
//! Applies no matter how the folder was opened (drag-and-drop, File → Open
//! Folder, a fresh git clone, …) since the hook is the generic
//! `Workspace` lifecycle, not any particular open path.

use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::rc::Rc;

use db::kvp::KeyValueStore;
use gpui::{App, AppContext as _, AsyncApp, Context, Entity, WeakEntity};
use project::{Event as ProjectEvent, Worktree};
use workspace::{Toast, Workspace, notifications::NotificationId};

pub fn init(cx: &mut App) {
    cx.observe_new(|workspace: &mut Workspace, _, cx| {
        maybe_offer_install(workspace, cx);
    })
    .detach();
}

/// The workspace's first worktree isn't always attached the moment the
/// `Workspace` entity is created (e.g. a project still being loaded), so we
/// fall back to waiting for the first `WorktreeAdded` event rather than
/// assuming `worktrees(cx)` is non-empty here.
fn maybe_offer_install(workspace: &mut Workspace, cx: &mut Context<Workspace>) {
    let first_worktree = workspace.worktrees(cx).next();
    if let Some(worktree) = first_worktree {
        check_worktree(workspace, &worktree, cx);
        return;
    }

    let project = workspace.project().clone();
    let handled = Rc::new(Cell::new(false));
    cx.subscribe(&project, move |workspace, project, event, cx| {
        if handled.get() {
            return;
        }
        let ProjectEvent::WorktreeAdded(id) = event else {
            return;
        };
        let Some(worktree) = project.read(cx).worktree_for_id(*id, cx) else {
            return;
        };
        handled.set(true);
        check_worktree(workspace, &worktree, cx);
    })
    .detach();
}

fn check_worktree(
    workspace: &mut Workspace,
    worktree: &Entity<Worktree>,
    cx: &mut Context<Workspace>,
) {
    let root = worktree.read(cx).abs_path();
    if !needs_install(&root) {
        return;
    }

    let key = prompt_key(&root);
    if KeyValueStore::global(cx)
        .read_kvp(&key)
        .ok()
        .flatten()
        .is_some()
    {
        return;
    }

    // "Ask once" means the marker goes down the moment the prompt is shown,
    // not only if the user accepts it — otherwise dismissing the toast would
    // just re-prompt on the next open.
    let kvp = KeyValueStore::global(cx);
    cx.background_spawn(async move {
        kvp.write_kvp(key, "1".to_string()).await.ok();
    })
    .detach();

    let install_root = root.to_path_buf();
    let workspace_handle = cx.weak_entity();
    workspace.show_toast(
        Toast::new(
            NotificationId::unique::<NpmInstallPrompt>(),
            "package.json found — install dependencies?",
        )
        .on_click("Run npm install", move |_, cx| {
            run_npm_install(install_root.clone(), workspace_handle.clone(), cx);
        }),
        cx,
    );
}

enum NpmInstallPrompt {}

fn run_npm_install(root: PathBuf, workspace: WeakEntity<Workspace>, cx: &mut App) {
    cx.spawn(async move |cx: &mut AsyncApp| {
        let result = gpui_tokio::Tokio::spawn_result(cx, install(root)).await;
        let message = match result {
            Ok(()) => "Installed dependencies".to_string(),
            Err(err) => format!("npm install failed: {err}"),
        };
        workspace
            .update(cx, |workspace, cx| {
                workspace.show_toast(
                    Toast::new(NotificationId::unique::<NpmInstallResult>(), message),
                    cx,
                );
            })
            .ok();
    })
    .detach();
}

enum NpmInstallResult {}

async fn install(root: PathBuf) -> anyhow::Result<()> {
    #[cfg(target_os = "windows")]
    let npm_bin = "npm.cmd";
    #[cfg(not(target_os = "windows"))]
    let npm_bin = "npm";

    let mut command = tokio::process::Command::new(npm_bin);
    command
        .arg("install")
        .current_dir(&root as &Path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(target_os = "windows")]
    command.creation_flags(0x0800_0000);

    let output = command.output().await?;
    if output.status.success() {
        Ok(())
    } else {
        Err(anyhow::anyhow!(
            String::from_utf8_lossy(&output.stderr).into_owned()
        ))
    }
}

/// Whether `root` looks like a JavaScript project whose dependencies were
/// never installed: it has a `package.json` file and no `node_modules`.
fn needs_install(root: &Path) -> bool {
    root.join("package.json").is_file() && !root.join("node_modules").exists()
}

/// The key-value store key recording that the install prompt was already
/// shown for `root`, so it is offered once per project directory.
fn prompt_key(root: &Path) -> String {
    format!("npm-bootstrap-asked:{}", root.display())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_is_offered_only_for_uninstalled_projects() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();

        // Not a JavaScript project at all.
        assert!(!needs_install(root));

        std::fs::write(root.join("package.json"), "{}").unwrap();
        assert!(needs_install(root));

        // Dependencies already installed.
        std::fs::create_dir(root.join("node_modules")).unwrap();
        assert!(!needs_install(root));
    }

    #[test]
    fn a_directory_named_package_json_does_not_count() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("package.json")).unwrap();
        assert!(!needs_install(dir.path()));
    }

    #[test]
    fn a_node_modules_file_still_counts_as_installed() {
        // `exists`, not `is_dir`: a symlink or stray file named
        // `node_modules` means something else is managing it.
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("package.json"), "{}").unwrap();
        std::fs::write(dir.path().join("node_modules"), "").unwrap();
        assert!(!needs_install(dir.path()));
    }

    #[test]
    fn missing_directories_need_nothing() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!needs_install(&dir.path().join("does-not-exist")));
    }

    #[test]
    fn prompt_key_is_distinct_per_project() {
        let a = prompt_key(Path::new("/work/a"));
        let b = prompt_key(Path::new("/work/b"));
        assert!(a.starts_with("npm-bootstrap-asked:"));
        assert!(a.ends_with("a"));
        assert_ne!(a, b);
        assert_eq!(a, prompt_key(Path::new("/work/a")));
    }
}
