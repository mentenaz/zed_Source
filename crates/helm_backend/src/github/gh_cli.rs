//! Importing a token from the GitHub CLI (`gh`), as an optional way to sign
//! in. Nothing else in Helm uses `gh`: once a token is imported, it is
//! stored and used like any other, and `gh` is not asked again.
//!
//! Only two documented `gh` commands are run: `gh auth status --json hosts`
//! to list the signed-in accounts (no tokens in its output), and
//! `gh auth token --user <login>` to copy one account's token. Neither
//! changes which account `gh` has active.

use std::process::Stdio;

use serde::Deserialize;

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
    fn no_github_com_entry_means_no_accounts() {
        assert!(parse_accounts(br#"{"hosts":{}}"#).unwrap().is_empty());
        assert!(parse_accounts(b"not json").is_err());
    }
}
