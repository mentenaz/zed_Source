//! The local half of the Git panel's "Using Helm" create flow: wiring the
//! current workspace to a repository `gh_create_repo` just created on
//! GitHub. The API call can't do any of this itself — its `auto_init`
//! commit would make pushing the workspace's existing files fail — so it
//! runs here, after GitHub confirms the repository: README/.gitignore,
//! `git init`, the initial commit, the remote, and the first push.

use std::path::Path;

use super::*;

/// The commit used when the dialog's message field was left blank.
pub(super) const DEFAULT_COMMIT_MESSAGE: &str = "Initial commit";

/// What the Create dialog offers in [`HelmModalKind::CreateRepo`]'s
/// `attach_local` mode, parked on the panel while `gh_create_repo` runs and
/// handed to [`HelmPanel::attach_created_repo`] once GitHub confirms the
/// repository.
#[derive(Clone)]
pub(super) struct CreateRepoAttach {
    /// The initial commit's message; blank falls back to
    /// [`DEFAULT_COMMIT_MESSAGE`].
    pub(super) message: String,
    /// Write a `README.md` when the workspace has none.
    pub(super) readme: bool,
    /// Write a default `.gitignore` when the workspace has none.
    pub(super) gitignore: bool,
}

/// A failed step of the local wiring, tagged with whether the repository
/// had already been initialized — a failure before that point left the
/// workspace untouched, one after it left a usable local repository (and
/// possibly a remote) behind.
struct AttachFailure {
    local_repo: bool,
    message: String,
}

impl HelmPanel {
    /// The follow-up to a `CreateRepo { attach_local: true }` run: creates
    /// the requested local files, initializes the workspace's first
    /// directory worktree as a Git repository, commits everything (the
    /// workspace was necessarily uninitialized — that's the state the Git
    /// panel's "Using Helm" button shows in), points `origin` at the new
    /// GitHub repository, and pushes. Each outcome is one toast; a failure
    /// keeps whatever succeeded so far, so the push can simply be retried
    /// from the Git panel.
    pub(super) fn attach_created_repo(
        &mut self,
        repo: Repo,
        attach: CreateRepoAttach,
        cx: &mut Context<Self>,
    ) {
        let root = self
            .workspace
            .update(cx, |workspace, cx| {
                workspace
                    .worktrees(cx)
                    .find(|worktree| !worktree.read(cx).is_single_file())
                    .map(|worktree| worktree.read(cx).abs_path().to_path_buf())
            })
            .ok()
            .flatten();
        let Some(root) = root else {
            self.notify(
                format!(
                    "Created {} on GitHub, but this window has no folder to attach it to.",
                    repo.name
                ),
                cx,
            );
            return;
        };

        let fs = <dyn fs::Fs>::global(cx);
        let name = repo.name;
        let url = repo.clone_url;
        let description = repo.description.unwrap_or_default();
        // The task owns its own copy — the toast below names the repository
        // after the future has consumed the originals.
        let created = name.clone();
        cx.spawn(async move |this, cx| {
            let result = on_tokio(async move {
                attach_workspace(&*fs, &root, &name, &description, &url, &attach).await
            })
            .await;
            this.update(cx, |this, cx| match result {
                Ok(()) => {
                    this.notify(
                        format!("Created {created} and pushed this workspace to GitHub"),
                        cx,
                    );
                }
                Err(AttachFailure {
                    local_repo,
                    message,
                }) => {
                    let kept = if local_repo {
                        " — the local repository was kept; push it from the Git panel to finish."
                    } else {
                        ""
                    };
                    this.notify(format!("{message}{kept}"), cx);
                }
            })
            .ok();
        })
        .detach();
    }
}

/// Runs every local step, in order, stopping at the first failure. The
/// caller words the outcome based on [`AttachFailure::local_repo`].
async fn attach_workspace(
    fs: &dyn fs::Fs,
    root: &Path,
    name: &str,
    description: &str,
    url: &str,
    attach: &CreateRepoAttach,
) -> Result<(), AttachFailure> {
    // An explicit `main` so the GitHub default branch matches what Zed's
    // own `git init` produces; gits too old to know `--initial-branch`
    // get a plain init, whose branch the push below adopts either way. The
    // first error is deliberately dropped in favor of the fallback's —
    // on an old git it's just the unknown-flag usage text.
    if git(&["init", "--initial-branch=main"], root).await.is_err() {
        git(&["init"], root)
            .await
            .map_err(|message| AttachFailure {
                local_repo: false,
                message,
            })?;
    }

    if attach.readme {
        let path = root.join("README.md");
        if !fs.is_file(&path).await {
            fs.write(&path, readme_content(name, description).as_bytes())
                .await
                .map_err(|error| AttachFailure {
                    local_repo: true,
                    message: format!("Writing README.md failed: {error}"),
                })?;
        }
    }
    if attach.gitignore {
        let path = root.join(".gitignore");
        if !fs.is_file(&path).await {
            fs.write(&path, DEFAULT_GITIGNORE.as_bytes())
                .await
                .map_err(|error| AttachFailure {
                    local_repo: true,
                    message: format!("Writing .gitignore failed: {error}"),
                })?;
        }
    }

    git(&["add", "-A"], root).await.map_err(local_failure)?;
    // `--allow-empty` so a workspace whose files are all ignored still ends
    // up with the initial commit the push needs.
    git(
        &["commit", "--allow-empty", "-m", commit_message(&attach.message)],
        root,
    )
    .await
    .map_err(local_failure)?;
    // A re-run (a first attempt that failed at the push) may already have
    // the remote — move it to the just-created repository rather than
    // failing.
    if git(&["remote", "add", "origin", url], root).await.is_err() {
        git(&["remote", "set-url", "origin", url], root)
            .await
            .map_err(local_failure)?;
    }
    git(&["push", "-u", "origin", "HEAD"], root)
        .await
        .map_err(local_failure)
}

fn local_failure(message: String) -> AttachFailure {
    AttachFailure {
        local_repo: true,
        message,
    }
}

/// The commit the initial commit uses — a blank field falls back to
/// [`DEFAULT_COMMIT_MESSAGE`], so `git commit` can't fail on an empty
/// message.
fn commit_message(entered: &str) -> &str {
    match entered.trim() {
        "" => DEFAULT_COMMIT_MESSAGE,
        trimmed => trimmed,
    }
}

/// The `README.md` written when the workspace has none: the repository's
/// name as its heading, with its description under it when one was given.
fn readme_content(name: &str, description: &str) -> String {
    if description.trim().is_empty() {
        format!("# {name}\n")
    } else {
        format!("# {name}\n\n{}\n", description.trim())
    }
}

/// What the gitignore switch writes when the workspace has none — the OS
/// and tooling junk that should never land in a fresh repository. A real
/// template picker (GitHub's `gitignore_template` list) can replace this
/// later without touching the flow around it.
const DEFAULT_GITIGNORE: &str = "\
# OS
.DS_Store
Thumbs.db
desktop.ini

# Editors
.vscode/
.idea/

# Dependencies and build output
node_modules/
target/
dist/
build/
__pycache__/
*.pyc
.venv/
venv/

# Logs
*.log

# Environment
.env
.env.local
";

/// Runs `git <args>` in `cwd`, folding git's own stderr into the error, so
/// a failed step's toast carries git's actual explanation (missing author
/// identity, rejected push, …). Stdin is null — inherited from `output()` —
/// so a step can never hang waiting for input; credential helpers like the
/// Git Credential Manager run out-of-process regardless.
async fn git(args: &[&str], cwd: &Path) -> Result<(), String> {
    let output = tokio::process::Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .await
        .map_err(|error| format!("Running `git {}` failed: {error}", args[0]))?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let detail = if stderr.trim().is_empty() {
        String::from_utf8_lossy(&output.stdout)
    } else {
        stderr
    };
    let detail = detail.trim();
    if detail.is_empty() {
        Err(format!("`git {}` failed", args[0]))
    } else {
        Err(format!("`git {}` failed: {detail}", args[0]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blank_commit_message_falls_back() {
        assert_eq!(commit_message(""), DEFAULT_COMMIT_MESSAGE);
        assert_eq!(commit_message("   \t"), DEFAULT_COMMIT_MESSAGE);
        assert_eq!(commit_message(" first pass "), "first pass");
    }

    #[test]
    fn readme_leads_with_the_name() {
        assert_eq!(readme_content("AniCraft", ""), "# AniCraft\n");
        assert_eq!(
            readme_content("AniCraft", "  A toy. "),
            "# AniCraft\n\nA toy.\n"
        );
    }

    #[test]
    fn default_gitignore_covers_the_usual_junk() {
        for pattern in [".DS_Store", "node_modules/", ".env", "target/"] {
            assert!(
                DEFAULT_GITIGNORE.contains(pattern),
                "{pattern} missing from the default .gitignore"
            );
        }
    }
}
