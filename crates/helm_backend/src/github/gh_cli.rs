//! Importing a token from the GitHub CLI (`gh`), as an optional way to sign
//! in. Nothing else in Helm uses `gh`: once a token is imported, it is
//! stored and used like any other, and `gh` is not asked again.
//!
//! To import, two documented `gh` commands are run: `gh auth status --json
//! hosts` to list the signed-in accounts (no tokens in its output), and
//! `gh auth token --user <login>` to copy one account's token. Neither
//! changes which account `gh` has active.
//!
//! [`gh_cli_sign_in`] also signs `gh` in to another account
//! (`gh auth login --web`), for an account `gh` does not have yet. That makes
//! the new account `gh`'s active one, which would change who every `git push`
//! on the machine goes out as when git's credential helper is `gh`, so the
//! previously active account is switched back afterwards.

use std::process::Stdio;

use futures::FutureExt as _;
use serde::Deserialize;
use tokio::io::{AsyncBufReadExt as _, BufReader};

const HOST: &str = "github.com";

/// An account the GitHub CLI is signed in to on github.com.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GhCliAccount {
    pub login: String,
    /// Whether it is `gh`'s active account.
    pub active: bool,
    /// Whether `gh` could use its token when asked (`state: success`). An
    /// expired or revoked one has to be signed in again in `gh` first.
    pub usable: bool,
    /// The token's scopes as `gh` reports them. Empty when `gh` does not
    /// know them.
    pub scopes: Vec<String>,
}

#[derive(Deserialize)]
struct StatusJson {
    #[serde(default)]
    hosts: std::collections::HashMap<String, Vec<StatusEntry>>,
}

#[derive(Deserialize)]
struct StatusEntry {
    #[serde(default)]
    login: String,
    #[serde(default)]
    active: bool,
    #[serde(default)]
    state: String,
    #[serde(default)]
    scopes: String,
}

fn gh_command() -> tokio::process::Command {
    #[cfg_attr(
        not(target_os = "windows"),
        expect(unused_mut, reason = "only the Windows creation flag changes it")
    )]
    let mut command = tokio::process::Command::new("gh");
    command
        .env("GH_PROMPT_DISABLED", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    #[cfg(target_os = "windows")]
    command.creation_flags(0x0800_0000);
    command
}

/// The accounts the GitHub CLI is signed in to on github.com. `Ok(None)`
/// when `gh` is not installed.
pub async fn gh_cli_accounts() -> Result<Option<Vec<GhCliAccount>>, String> {
    let output = match gh_command()
        .args(["auth", "status", "--json", "hosts", "--hostname", HOST])
        .output()
        .await
    {
        Ok(output) => output,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("Could not run the GitHub CLI: {error}")),
    };
    // `gh` exits with 1 when an account has a problem, but still prints the
    // JSON, so the output is read whatever the exit code.
    if output.stdout.is_empty() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        // Signed in to nothing: no accounts, not an error.
        if stderr.contains("not logged in") {
            return Ok(Some(Vec::new()));
        }
        return Err(format!("The GitHub CLI did not list its accounts: {}", stderr.trim()));
    }
    parse_accounts(&output.stdout).map(Some)
}

fn parse_accounts(json: &[u8]) -> Result<Vec<GhCliAccount>, String> {
    let status: StatusJson = serde_json::from_slice(json)
        .map_err(|error| format!("Could not read the GitHub CLI's accounts: {error}"))?;
    Ok(status
        .hosts
        .get(HOST)
        .into_iter()
        .flatten()
        .filter(|entry| !entry.login.is_empty())
        .map(|entry| GhCliAccount {
            login: entry.login.clone(),
            active: entry.active,
            usable: entry.state == "success",
            scopes: entry
                .scopes
                .split(',')
                .map(str::trim)
                .filter(|scope| !scope.is_empty())
                .map(str::to_string)
                .collect(),
        })
        .collect())
}

/// Copies `login`'s token from the GitHub CLI. The token is only returned,
/// never logged.
pub async fn gh_cli_token(login: &str) -> Result<String, String> {
    let output = gh_command()
        .args(["auth", "token", "--hostname", HOST, "--user", login])
        .output()
        .await
        .map_err(|error| format!("Could not run the GitHub CLI: {error}"))?;
    let token = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if !output.status.success() || token.is_empty() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "The GitHub CLI has no token for {login}: {}",
            stderr.trim()
        ));
    }
    Ok(token)
}

/// The one-time code (`XXXX-XXXX`) and the page to enter it on, if `line`
/// of `gh auth login` output has them. `gh` prints them as text, with no
/// machine-readable form, so they are picked out of the line.
pub fn parse_device_prompt(line: &str) -> (Option<String>, Option<String>) {
    let code = line
        .split(|c: char| !c.is_ascii_alphanumeric() && c != '-')
        .find(|word| {
            let bytes = word.as_bytes();
            bytes.len() == 9
                && bytes[4] == b'-'
                && bytes[..4].iter().all(u8::is_ascii_alphanumeric)
                && bytes[5..].iter().all(u8::is_ascii_alphanumeric)
                && word
                    .chars()
                    .any(|c| c.is_ascii_digit() || c.is_ascii_uppercase())
        })
        .map(str::to_string);
    let url = line.find("https://github.com/login/device").map(|start| {
        line[start..]
            .split(char::is_whitespace)
            .next()
            .unwrap_or_default()
            .trim_end_matches(['.', ')'])
            .to_string()
    });
    (code, url)
}

/// Why signing `gh` in did not finish.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GhCliSignInFailure {
    /// [`GhCliSignInCanceller::cancel`] was called; `gh` was stopped.
    Cancelled,
    Failed(String),
}

impl std::fmt::Display for GhCliSignInFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GhCliSignInFailure::Cancelled => write!(f, "The sign-in was cancelled."),
            GhCliSignInFailure::Failed(message) => write!(f, "{message}"),
        }
    }
}

/// Stops the [`gh_cli_sign_in`] it was made for. Dropping it does not.
pub struct GhCliSignInCanceller(tokio::sync::oneshot::Sender<()>);

impl GhCliSignInCanceller {
    pub fn cancel(self) {
        self.0.send(()).ok();
    }
}

/// The receiving end of a [`GhCliSignInCanceller`].
pub struct GhCliSignInCancelSignal(tokio::sync::oneshot::Receiver<()>);

pub fn gh_cli_sign_in_canceller() -> (GhCliSignInCanceller, GhCliSignInCancelSignal) {
    let (sender, receiver) = tokio::sync::oneshot::channel();
    (GhCliSignInCanceller(sender), GhCliSignInCancelSignal(receiver))
}

/// What [`gh_cli_sign_in`] did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GhCliSignedIn {
    /// The account `gh` signed in to.
    pub login: String,
    /// The account that was active in `gh` before, if it was another one.
    pub previous_active: Option<String>,
    /// Whether `gh` was switched back to `previous_active`.
    pub switched_back: bool,
}

/// Signs the GitHub CLI in to an account in the browser
/// (`gh auth login --web`), sending each line of `gh`'s output on `output`
/// (the one-time code and link are among them; see
/// [`parse_device_prompt`]). Afterwards `gh`'s previously active account is
/// made active again.
pub async fn gh_cli_sign_in(
    output: tokio::sync::mpsc::UnboundedSender<String>,
    cancel: GhCliSignInCancelSignal,
) -> Result<GhCliSignedIn, GhCliSignInFailure> {
    let before = gh_cli_accounts()
        .await
        .map_err(GhCliSignInFailure::Failed)?
        .ok_or_else(|| GhCliSignInFailure::Failed("The GitHub CLI is not installed.".into()))?;
    let previous_active = before
        .iter()
        .find(|account| account.active)
        .map(|account| account.login.clone());

    let mut command = gh_command();
    // With no stdin, `gh` prints the code and waits for the browser on its
    // own; it is not told to disable prompts, as it is for the other
    // commands, because it then refuses to start the browser flow.
    command.env_remove("GH_PROMPT_DISABLED").args([
        "auth",
        "login",
        "--hostname",
        HOST,
        "--git-protocol",
        "https",
        "--web",
        "--scopes",
        "repo,read:org,user",
    ]);
    let mut child = command.spawn().map_err(|error| {
        GhCliSignInFailure::Failed(format!("Could not run the GitHub CLI: {error}"))
    })?;

    let mut readers = Vec::new();
    if let Some(stdout) = child.stdout.take() {
        readers.push(tokio::spawn(forward_lines(stdout, output.clone())));
    }
    if let Some(stderr) = child.stderr.take() {
        readers.push(tokio::spawn(forward_lines(stderr, output)));
    }

    // A dropped canceller is not a cancel: then wait for `gh` alone.
    let cancelled = async move {
        if cancel.0.await.is_err() {
            futures::future::pending::<()>().await;
        }
    };
    let status = futures::select_biased! {
        status = child.wait().fuse() => status.map_err(|error| {
            GhCliSignInFailure::Failed(format!("Waiting for the GitHub CLI failed: {error}"))
        })?,
        () = cancelled.fuse() => {
            child.kill().await.ok();
            return Err(GhCliSignInFailure::Cancelled);
        }
    };
    let mut lines = Vec::new();
    for reader in readers {
        if let Ok(read) = reader.await {
            lines.extend(read);
        }
    }
    if !status.success() {
        let start = lines.len().saturating_sub(3);
        let detail = lines[start..].join("\n");
        return Err(GhCliSignInFailure::Failed(format!(
            "The GitHub CLI could not sign in.\n{detail}"
        )));
    }

    let after = gh_cli_accounts()
        .await
        .map_err(GhCliSignInFailure::Failed)?
        .unwrap_or_default();
    let login = after
        .iter()
        .find(|account| account.active)
        .map(|account| account.login.clone())
        .ok_or_else(|| {
            GhCliSignInFailure::Failed(
                "The GitHub CLI signed in, but did not say to which account.".into(),
            )
        })?;

    let previous_active =
        previous_active.filter(|previous| !previous.eq_ignore_ascii_case(&login));
    let switched_back = match &previous_active {
        Some(previous) => gh_command()
            .args(["auth", "switch", "--hostname", HOST, "--user", previous])
            .output()
            .await
            .is_ok_and(|output| output.status.success()),
        None => false,
    };
    Ok(GhCliSignedIn {
        login,
        previous_active,
        switched_back,
    })
}

/// Sends each line of `stream` on `output` and returns them.
async fn forward_lines(
    stream: impl tokio::io::AsyncRead + Unpin,
    output: tokio::sync::mpsc::UnboundedSender<String>,
) -> Vec<String> {
    let mut read = Vec::new();
    let mut lines = BufReader::new(stream).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        let line = line.trim_end().to_string();
        if line.is_empty() {
            continue;
        }
        output.send(line.clone()).ok();
        read.push(line);
    }
    read
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accounts_are_read_from_gh_auth_status_json() {
        let json = br#"{"hosts":{"github.com":[
            {"state":"success","active":true,"host":"github.com","login":"mentenaz",
             "tokenSource":"keyring","scopes":"gist, read:org, repo, user","gitProtocol":"https"},
            {"state":"error","active":false,"host":"github.com","login":"other",
             "tokenSource":"keyring","scopes":"","gitProtocol":"https"}
        ],"ghe.example.com":[{"state":"success","active":true,"login":"elsewhere"}]}}"#;
        let accounts = parse_accounts(json).unwrap();
        assert_eq!(
            accounts,
            vec![
                GhCliAccount {
                    login: "mentenaz".into(),
                    active: true,
                    usable: true,
                    scopes: vec!["gist".into(), "read:org".into(), "repo".into(), "user".into()],
                },
                GhCliAccount {
                    login: "other".into(),
                    active: false,
                    usable: false,
                    scopes: Vec::new(),
                },
            ]
        );
    }

    #[test]
    fn the_code_and_link_are_picked_out_of_gh_output() {
        assert_eq!(
            parse_device_prompt("! First copy your one-time code: 1A2B-3C4D"),
            (Some("1A2B-3C4D".into()), None)
        );
        assert_eq!(
            parse_device_prompt(
                "Open this URL to continue in your web browser: https://github.com/login/device"
            ),
            (None, Some("https://github.com/login/device".into()))
        );
        // Words that merely look like a code are not taken for one.
        assert_eq!(parse_device_prompt("re-auth with read-only scopes"), (None, None));
    }

    #[test]
    fn no_github_com_entry_means_no_accounts() {
        assert!(parse_accounts(br#"{"hosts":{}}"#).unwrap().is_empty());
        assert!(parse_accounts(b"not json").is_err());
    }
}
