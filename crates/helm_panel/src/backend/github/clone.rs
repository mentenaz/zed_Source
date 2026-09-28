//! `gh repo clone` (was `github.rs::gh_clone_repo` in the Tauri backend).
//!
//! Phase 1 clones with `gh repo clone`, streaming output lines through
//! `GhState::clone_tx` (replaces the Tauri `window.emit("gh-clone-line", …)`
//! calls) so the progress screen can show them live instead of only after the
//! whole clone finishes. Phase 2, when `run_npm` is set, runs `npm install`
//! in the target directory the same way, emitting a `NpmStart` event first so
//! the progress screen knows to switch phases and start polling
//! `node_modules` (that polling itself lives panel-side, same as TS).

use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, BufReader as TokioBufReader};

use super::gh_cmd;
use super::types::{CloneEvent, GhState};

pub async fn gh_clone_repo(
    full_name: String,
    target_path: String,
    run_npm: bool,
    state: &GhState,
) -> Result<(), String> {
    let result = clone_inner(full_name, target_path, run_npm, state).await;
    let _ = state.clone_tx.send(CloneEvent::Done);
    result
}

async fn clone_inner(
    full_name: String,
    target_path: String,
    run_npm: bool,
    state: &GhState,
) -> Result<(), String> {
    // ── Phase 1: git clone via gh CLI ───────────────────────────────────────
    let mut child = gh_cmd()
        .arg("repo")
        .arg("clone")
        .arg(&full_name)
        .arg(&target_path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Failed to spawn gh clone: {}", e))?;

    let err_lines: std::sync::Arc<std::sync::Mutex<Vec<String>>> = Default::default();

    if let Some(out) = child.stdout.take() {
        let tx = state.clone_tx.clone();
        tokio::spawn(async move {
            let reader = TokioBufReader::new(out);
            let mut lines = reader.lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let _ = tx.send(CloneEvent::Line(line));
            }
        });
    }
    if let Some(err) = child.stderr.take() {
        let tx = state.clone_tx.clone();
        let captured = err_lines.clone();
        tokio::spawn(async move {
            let reader = TokioBufReader::new(err);
            let mut lines = reader.lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let _ = tx.send(CloneEvent::Line(line.clone()));
                if let Ok(mut buf) = captured.lock() {
                    buf.push(line);
                }
            }
        });
    }

    let status = child
        .wait()
        .await
        .map_err(|e| format!("Clone wait error: {}", e))?;
    if !status.success() {
        let code = status.code().unwrap_or(-1);
        let stderr_log = err_lines
            .lock()
            .ok()
            .map(|b| b.join("\n"))
            .filter(|s| !s.is_empty());
        let detail = match stderr_log {
            Some(log) => format!("gh clone exited with code {}\nstderr:\n{}", code, log),
            None => format!("gh clone exited with code {}", code),
        };
        return Err(detail);
    }

    // ── Phase 2: npm install (optional) ─────────────────────────────────────
    if run_npm {
        let _ = state.clone_tx.send(CloneEvent::NpmStart);

        #[cfg(target_os = "windows")]
        let npm_bin = "npm.cmd";
        #[cfg(not(target_os = "windows"))]
        let npm_bin = "npm";

        let mut npm = tokio::process::Command::new(npm_bin);
        npm.arg("install")
            .current_dir(&target_path)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(target_os = "windows")]
        npm.creation_flags(0x0800_0000);

        match npm.spawn() {
            Err(e) => {
                let _ = state.clone_tx.send(CloneEvent::Line(format!(
                    "npm install failed to start: {}",
                    e
                )));
            }
            Ok(mut npm_child) => {
                if let Some(out) = npm_child.stdout.take() {
                    let tx = state.clone_tx.clone();
                    tokio::spawn(async move {
                        let reader = TokioBufReader::new(out);
                        let mut lines = reader.lines();
                        while let Ok(Some(line)) = lines.next_line().await {
                            let _ = tx.send(CloneEvent::Line(line));
                        }
                    });
                }
                if let Some(err) = npm_child.stderr.take() {
                    let tx = state.clone_tx.clone();
                    tokio::spawn(async move {
                        let reader = TokioBufReader::new(err);
                        let mut lines = reader.lines();
                        while let Ok(Some(line)) = lines.next_line().await {
                            let _ = tx.send(CloneEvent::Line(line));
                        }
                    });
                }
                let _ = npm_child.wait().await;
            }
        }
    }

    Ok(())
}
