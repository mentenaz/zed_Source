//! Cloning a repository with `git`, using the signed-in account's token.
//!
//! The token is passed to `git` as an `Authorization` header through
//! environment-based config (`GIT_CONFIG_COUNT` / `GIT_CONFIG_KEY_n` /
//! `GIT_CONFIG_VALUE_n`, git 2.31 or later), so it is not in the command
//! line, the URL, or the clone's `.git/config`. Credential helpers and
//! prompts are turned off for the clone, so a rejected token fails with a
//! message instead of opening a sign-in window or waiting for input.
//!
//! Output streams through `GhState::clone_tx` while the clone runs. Later
//! pushes and pulls in the clone use whatever credential helper git has;
//! Helm does not configure one.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use base64::{Engine as _, prelude::BASE64_STANDARD};
use futures::FutureExt as _;
use tokio::io::{AsyncRead, AsyncReadExt as _};

use super::types::{CloneEvent, GhState};

/// Why a clone did not finish.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CloneFailure {
    /// [`CloneCanceller::cancel`] was called. The partial clone was removed.
    Cancelled,
    Failed(String),
}

impl std::fmt::Display for CloneFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CloneFailure::Cancelled => write!(f, "The clone was cancelled."),
            CloneFailure::Failed(message) => write!(f, "{message}"),
        }
    }
}

/// Stops the clone it was made for. Dropping it does not.
pub struct CloneCanceller(tokio::sync::oneshot::Sender<()>);

impl CloneCanceller {
    pub fn cancel(self) {
        self.0.send(()).ok();
    }
}

/// The receiving end of a [`CloneCanceller`], handed to [`clone_repo`].
pub struct CloneCancelSignal(tokio::sync::oneshot::Receiver<()>);

/// A canceller and the signal it sends, for one clone.
pub fn clone_canceller() -> (CloneCanceller, CloneCancelSignal) {
    let (sender, receiver) = tokio::sync::oneshot::channel();
    (CloneCanceller(sender), CloneCancelSignal(receiver))
}

/// Clones `owner/name` into `target_path` with `git`, using the state's
/// token if there is one (public repositories clone without). Sends each
/// line of git's output, then [`CloneEvent::Done`], on `state.clone_tx`.
pub async fn clone_repo(
    full_name: String,
    target_path: String,
    cancel: CloneCancelSignal,
    state: &GhState,
) -> Result<(), CloneFailure> {
    let result = clone_inner(&full_name, Path::new(&target_path), cancel, state).await;
    let _ = state.clone_tx.send(CloneEvent::Done);
    result
}

async fn clone_inner(
    full_name: &str,
    target: &Path,
    cancel: CloneCancelSignal,
    state: &GhState,
) -> Result<(), CloneFailure> {
    if target
        .read_dir()
        .is_ok_and(|mut entries| entries.next().is_some())
    {
        return Err(CloneFailure::Failed(format!(
            "{} already exists and is not empty. Choose another folder.",
            target.display()
        )));
    }
    let created_target = !target.exists();

    let token = state.token().await;
    let mut command = git_clone_command(full_name, target, token.as_deref());
    let mut child = command.spawn().map_err(|error| {
        CloneFailure::Failed(if error.kind() == std::io::ErrorKind::NotFound {
            "Git is not installed or not on PATH. Install it from https://git-scm.com and \
             try again."
                .to_string()
        } else {
            format!("Could not start git: {error}")
        })
    })?;

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let tx = state.clone_tx.clone();
    let stdout_task = stdout.map(|out| tokio::spawn(forward_output(out, tx.clone())));
    let stderr_task = stderr.map(|err| tokio::spawn(forward_output(err, tx)));

    // A dropped canceller is not a cancel: then wait for git alone.
    let cancelled = async move {
        if cancel.0.await.is_err() {
            futures::future::pending::<()>().await;
        }
    };
    futures::select_biased! {
        status = child.wait().fuse() => {
            let status = status.map_err(|error| {
                CloneFailure::Failed(format!("Waiting for git failed: {error}"))
            })?;
            let mut messages = Vec::new();
            for task in [stdout_task, stderr_task].into_iter().flatten() {
                if let Ok(lines) = task.await {
                    messages.extend(lines);
                }
            }
            if status.success() {
                return Ok(());
            }
            if created_target {
                remove_partial_clone(target);
            }
            return Err(CloneFailure::Failed(failure_message(
                status.code(),
                &messages,
                token.is_some(),
            )));
        }
        () = cancelled.fuse() => {}
    }

    child.kill().await.ok();
    if created_target {
        remove_partial_clone(target);
    }
    Err(CloneFailure::Cancelled)
}

/// `git clone --progress https://github.com/<full_name>.git <target>`, with
/// the token (if any) as an `Authorization` header in environment config,
/// and no credential helpers or prompts.
fn git_clone_command(full_name: &str, target: &Path, token: Option<&str>) -> tokio::process::Command {
    let mut config: Vec<(&str, String)> = vec![
        // An empty value clears the helpers configured so far, including
        // `gh auth git-credential` and Git Credential Manager.
        ("credential.helper", String::new()),
        ("credential.https://github.com.helper", String::new()),
    ];
    if let Some(token) = token {
        let basic = BASE64_STANDARD.encode(format!("x-access-token:{token}"));
        config.push((
            "http.https://github.com/.extraHeader",
            format!("Authorization: Basic {basic}"),
        ));
    }

    let mut command = tokio::process::Command::new("git");
    command
        .arg("clone")
        .arg("--progress")
        .arg(format!("https://github.com/{full_name}.git"))
        .arg(target)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GCM_INTERACTIVE", "never")
        .env_remove("GIT_ASKPASS")
        .env_remove("SSH_ASKPASS")
        .env("GIT_CONFIG_COUNT", config.len().to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    for (index, (key, value)) in config.into_iter().enumerate() {
        command
            .env(format!("GIT_CONFIG_KEY_{index}"), key)
            .env(format!("GIT_CONFIG_VALUE_{index}"), value);
    }
    #[cfg(target_os = "windows")]
    command.creation_flags(0x0800_0000);
    command
}

/// Sends `output` on `tx` piece by piece and returns its finished lines.
/// `git --progress` rewrites one line with `\r`; those pieces are sent as
/// [`CloneEvent::Progress`] so the reader can replace the previous one.
async fn forward_output(
    mut output: impl AsyncRead + Unpin,
    tx: tokio::sync::broadcast::Sender<CloneEvent>,
) -> Vec<String> {
    let mut lines = Vec::new();
    let mut pending = Vec::new();
    let mut buffer = [0u8; 4096];
    loop {
        let read = match output.read(&mut buffer).await {
            Ok(0) | Err(_) => break,
            Ok(read) => read,
        };
        for &byte in &buffer[..read] {
            if byte != b'\r' && byte != b'\n' {
                pending.push(byte);
                continue;
            }
            let text = String::from_utf8_lossy(&pending).trim_end().to_string();
            pending.clear();
            if text.is_empty() {
                continue;
            }
            if byte == b'\r' {
                let _ = tx.send(CloneEvent::Progress(text));
            } else {
                let _ = tx.send(CloneEvent::Line(text.clone()));
                lines.push(text);
            }
        }
    }
    let text = String::from_utf8_lossy(&pending).trim_end().to_string();
    if !text.is_empty() {
        let _ = tx.send(CloneEvent::Line(text.clone()));
        lines.push(text);
    }
    lines
}

/// What to tell the user when git failed, from its own messages.
fn failure_message(code: Option<i32>, messages: &[String], had_token: bool) -> String {
    let detail = messages
        .iter()
        .filter(|line| line.starts_with("fatal:") || line.starts_with("error:"))
        .cloned()
        .collect::<Vec<_>>()
        .join("\n");
    let all = messages.join("\n").to_ascii_lowercase();
    let hint = if all.contains("repository not found")
        || all.contains("authentication failed")
        || all.contains("could not read username")
    {
        Some(if had_token {
            "The account this window uses may not have access to this repository, or its \
             organisation has not approved Helm."
        } else {
            "Sign in to GitHub in the Helm panel to clone private repositories."
        })
    } else {
        None
    };
    let mut message = match code {
        Some(code) => format!("git clone failed (exit code {code})."),
        None => "git clone failed.".to_string(),
    };
    if !detail.is_empty() {
        message.push('\n');
        message.push_str(&detail);
    }
    if let Some(hint) = hint {
        message.push('\n');
        message.push_str(hint);
    }
    message
}

/// Removes what a failed or cancelled clone left behind. Only called for a
/// folder the clone created itself.
fn remove_partial_clone(target: &Path) {
    let target: PathBuf = target.to_path_buf();
    if let Err(error) = std::fs::remove_dir_all(&target)
        && error.kind() != std::io::ErrorKind::NotFound
    {
        log::warn!(
            "helm: could not remove the partial clone at {}: {error}",
            target.display()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_of(command: &tokio::process::Command) -> Vec<(String, String)> {
        command
            .as_std()
            .get_envs()
            .filter_map(|(key, value)| {
                Some((
                    key.to_string_lossy().into_owned(),
                    value?.to_string_lossy().into_owned(),
                ))
            })
            .collect()
    }

    #[test]
    fn the_token_is_in_environment_config_not_in_the_arguments() {
        let command = git_clone_command("owner/repo", Path::new("C:/code/repo"), Some("gho_secret"));
        let args: Vec<String> = command
            .as_std()
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            args,
            ["clone", "--progress", "https://github.com/owner/repo.git", "C:/code/repo"]
        );
        assert!(args.iter().all(|arg| !arg.contains("gho_secret")));

        let env = env_of(&command);
        let value = |key: &str| {
            env.iter()
                .find(|(name, _)| name == key)
                .map(|(_, value)| value.clone())
        };
        assert_eq!(value("GIT_CONFIG_COUNT").as_deref(), Some("3"));
        assert_eq!(
            value("GIT_CONFIG_KEY_2").as_deref(),
            Some("http.https://github.com/.extraHeader")
        );
        let expected = BASE64_STANDARD.encode("x-access-token:gho_secret");
        assert_eq!(
            value("GIT_CONFIG_VALUE_2"),
            Some(format!("Authorization: Basic {expected}"))
        );
        assert_eq!(value("GIT_TERMINAL_PROMPT").as_deref(), Some("0"));
    }

    #[test]
    fn without_a_token_only_the_helpers_are_cleared() {
        let command = git_clone_command("owner/repo", Path::new("repo"), None);
        let env = env_of(&command);
        assert!(env.contains(&("GIT_CONFIG_COUNT".into(), "2".into())));
        assert!(env.iter().all(|(_, value)| !value.contains("Authorization")));
    }

    #[test]
    fn a_missing_repository_gets_an_access_hint() {
        let messages = vec![
            "Cloning into 'repo'...".to_string(),
            "remote: Repository not found.".to_string(),
            "fatal: repository 'https://github.com/owner/repo.git/' not found".to_string(),
        ];
        let message = failure_message(Some(128), &messages, true);
        assert!(message.contains("exit code 128"));
        assert!(message.contains("fatal: repository"));
        assert!(message.contains("may not have access"));
        assert!(!message.contains("Cloning into"));

        let signed_out = failure_message(Some(128), &messages, false);
        assert!(signed_out.contains("Sign in to GitHub"));
    }
}
