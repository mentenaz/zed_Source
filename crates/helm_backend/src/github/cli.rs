//! GitHub CLI auth commands (was part of `github.rs`).
//!
//! `gh auth` interactions. Streaming functions that previously emitted progress
//! via `window.emit` now broadcast each line through `GhState::auth_tx`.

use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, BufReader as TokioBufReader};

use super::GhState;
use super::gh_cmd;
use super::types::{AuthInfo, GhAuthEvent};

pub async fn gh_check_cli() -> Result<(), String> {
    match gh_cmd()
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
    {
        Ok(s) if s.success() => Ok(()),
        Ok(s) => Err(format!("gh exited with {}", s.code().unwrap_or(-1))),
        Err(e) => Err(format!("Failed to run gh: {}", e)),
    }
}

pub async fn gh_auth_status() -> Result<Option<AuthInfo>, String> {
    match gh_cmd().arg("auth").arg("status").output().await {
        Ok(out) if out.status.success() => {
            let raw = String::from_utf8_lossy(&out.stdout).to_string();
            let account = raw
                .lines()
                .find(|l| l.contains("Logged in to") && l.contains("account"))
                .and_then(|l| l.split_whitespace().last().map(|s| s.to_string()));
            let scopes = raw
                .lines()
                .find(|l| l.contains("Token scopes:"))
                .map(|l| l.replace("Token scopes:", ""))
                .map(|s| s.replace('"', ""))
                .map(|s| {
                    s.split(',')
                        .map(|p| p.trim().trim_matches('\'').to_string())
                        .filter(|p| !p.is_empty())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            if let Some(account) = account {
                Ok(Some(AuthInfo { account, scopes }))
            } else {
                Ok(None)
            }
        }
        Ok(out) => Err(format!(
            "gh auth status failed: {}",
            String::from_utf8_lossy(&out.stderr)
        )),
        Err(e) => Err(format!("Failed to run gh: {}", e)),
    }
}

/// Runs `gh auth login --web` and streams its progress lines through
/// `state.auth_tx` (subscribe before calling this so no lines are missed).
pub async fn gh_login(state: &GhState) -> Result<(), String> {
    let mut child = match gh_cmd()
        .arg("auth")
        .arg("login")
        .arg("--hostname")
        .arg("github.com")
        .arg("--git-protocol")
        .arg("https")
        .arg("--scopes")
        .arg("repo,read:org,user")
        .arg("--web")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => return Err(format!("Failed to start gh auth login: {}", e)),
    };

    if let Some(out) = child.stdout.take() {
        let tx = state.auth_tx.clone();
        tokio::spawn(async move {
            let reader = TokioBufReader::new(out);
            let mut lines = reader.lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let _ = tx.send(GhAuthEvent::Line(line));
            }
        });
    }
    if let Some(err) = child.stderr.take() {
        let tx = state.auth_tx.clone();
        tokio::spawn(async move {
            let reader = TokioBufReader::new(err);
            let mut lines = reader.lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let _ = tx.send(GhAuthEvent::Line(line));
            }
        });
    }

    let result = match child.wait().await {
        Ok(status) if status.success() => Ok(()),
        Ok(status) => Err(format!(
            "gh auth login exited with {}",
            status.code().unwrap_or(-1)
        )),
        Err(e) => Err(format!("Failed to run gh: {}", e)),
    };
    let _ = state.auth_tx.send(GhAuthEvent::Done);
    result
}

/// Spawn `gh auth refresh -s <scope>` and stream its device-code lines
/// through `state.auth_tx`.
async fn gh_refresh_scope(scope: &str, state: &GhState) -> Result<(), String> {
    let mut child = match gh_cmd()
        .arg("auth")
        .arg("refresh")
        .arg("-s")
        .arg(scope)
        .arg("--hostname")
        .arg("github.com")
        // Same as `gh_login`: no console to answer a prompt from, so `gh`
        // must print the device code instead of waiting on Enter.
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => return Err(format!("Failed to start gh auth refresh: {}", e)),
    };

    if let Some(out) = child.stdout.take() {
        let tx = state.auth_tx.clone();
        tokio::spawn(async move {
            let reader = TokioBufReader::new(out);
            let mut lines = reader.lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let _ = tx.send(GhAuthEvent::Line(line));
            }
        });
    }
    if let Some(err) = child.stderr.take() {
        let tx = state.auth_tx.clone();
        tokio::spawn(async move {
            let reader = TokioBufReader::new(err);
            let mut lines = reader.lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let _ = tx.send(GhAuthEvent::Line(line));
            }
        });
    }

    let result = match child.wait().await {
        Ok(status) if status.success() => {
            // A refresh issues a new token; drop the cached one so the next
            // API call picks up the token that actually carries the new
            // scope instead of replaying the old one.
            *state.token.write().await = None;
            // Answers remembered under the old sign-in are not this one's.
            state.forget_answers();
            Ok(())
        }
        Ok(status) => Err(format!(
            "gh auth refresh exited with {}",
            status.code().unwrap_or(-1)
        )),
        Err(e) => Err(format!("Failed to run gh: {}", e)),
    };
    let _ = state.auth_tx.send(GhAuthEvent::Done);
    result
}

/// Adds `scope` to the current login (`gh auth refresh -s <scope>`), keeping
/// the scopes it already has.
pub async fn gh_ensure_scope(scope: &str, state: &GhState) -> Result<(), String> {
    gh_refresh_scope(scope, state).await
}

pub async fn gh_logout(state: &GhState) -> Result<(), String> {
    match gh_cmd()
        .arg("auth")
        .arg("logout")
        .arg("--hostname")
        .arg("github.com")
        .output()
        .await
    {
        Ok(out) if out.status.success() => {
            *state.token.write().await = None;
            // Answers remembered under the old sign-in are not this one's.
            state.forget_answers();
            Ok(())
        }
        Ok(out) => Err(format!(
            "gh logout failed: {}",
            String::from_utf8_lossy(&out.stderr)
        )),
        Err(e) => Err(format!("Failed to run gh: {}", e)),
    }
}
