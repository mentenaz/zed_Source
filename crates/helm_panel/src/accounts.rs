//! The GitHub accounts Helm has signed in to: their tokens in the system
//! credential store, the list of them in settings, and an app-wide entity
//! that tells every Helm panel when an account signs in or out.
//!
//! Each token is stored under [`credential_url`] with the login as user
//! name. The `accounts` list in the `"helm_panel"` settings is the index of
//! them, since the credential store can only look an entry up by its name.

use anyhow::{Context as _, Result};
use gpui::{App, AppContext as _, AsyncApp, Entity, EventEmitter, Global};
use settings::HelmAccountContent;

/// The credential-store name of `login`'s token. Logins are compared
/// ignoring case on GitHub, so the name is lowercased.
pub fn credential_url(login: &str) -> String {
    format!("helm://github.com/{}", login.to_ascii_lowercase())
}

/// The stored token for `login`, if there is one.
pub async fn read_token(login: &str, cx: &AsyncApp) -> Result<Option<String>> {
    let provider = cx.update(|cx| zed_credentials_provider::global(cx));
    let Some((_, token)) = provider.read_credentials(&credential_url(login), cx).await? else {
        return Ok(None);
    };
    let token = String::from_utf8(token).context("the stored GitHub token is not text")?;
    Ok(Some(token))
}

/// Stores `token` as `login`'s, replacing any token stored before.
pub async fn write_token(login: &str, token: &str, cx: &AsyncApp) -> Result<()> {
    let provider = cx.update(|cx| zed_credentials_provider::global(cx));
    provider
        .write_credentials(&credential_url(login), login, token.as_bytes(), cx)
        .await
}

/// Deletes `login`'s stored token. The account stays listed, as needing
/// sign-in.
pub async fn delete_token(login: &str, cx: &AsyncApp) -> Result<()> {
    let provider = cx.update(|cx| zed_credentials_provider::global(cx));
    provider
        .delete_credentials(&credential_url(login), cx)
        .await
}

/// Adds `login` to the `accounts` list in settings if it is not listed yet.
/// The first account listed also becomes the default.
pub fn remember_account(login: &str, cx: &mut App) {
    let login = login.to_string();
    settings::update_settings_file(<dyn fs::Fs>::global(cx), cx, move |settings, _| {
        let panel = settings.helm_panel.get_or_insert_default();
        let accounts = panel.accounts.get_or_insert_default();
        if !accounts
            .iter()
            .any(|account| account.login.eq_ignore_ascii_case(&login))
        {
            accounts.push(HelmAccountContent {
                login: login.clone(),
                label: None,
            });
        }
        if panel.default_account.is_none() {
            panel.default_account = Some(login);
        }
    });
}

/// Something that happened to one account, for every Helm panel.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HelmAccountEvent {
    /// A token was stored for this login: a first sign-in, a sign-in again,
    /// or a new scope.
    SignedIn { login: String },
    /// This login's token was deleted.
    SignedOut { login: String },
}

impl HelmAccountEvent {
    pub fn login(&self) -> &str {
        match self {
            HelmAccountEvent::SignedIn { login } | HelmAccountEvent::SignedOut { login } => login,
        }
    }
}

/// The app-wide sender of [`HelmAccountEvent`]s. Every Helm panel, one per
/// workspace window, subscribes, so a sign-out in one window does not leave
/// the others using the deleted token.
pub struct HelmAccounts;

impl EventEmitter<HelmAccountEvent> for HelmAccounts {}

struct GlobalHelmAccounts(Entity<HelmAccounts>);

impl Global for GlobalHelmAccounts {}

impl HelmAccounts {
    pub fn global(cx: &mut App) -> Entity<Self> {
        if let Some(accounts) = cx.try_global::<GlobalHelmAccounts>() {
            return accounts.0.clone();
        }
        let accounts = cx.new(|_| HelmAccounts);
        cx.set_global(GlobalHelmAccounts(accounts.clone()));
        accounts
    }

    pub fn emit(event: HelmAccountEvent, cx: &mut App) {
        Self::global(cx).update(cx, |_, cx| cx.emit(event));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credential_url_ignores_login_case() {
        assert_eq!(credential_url("Mentenaz"), "helm://github.com/mentenaz");
        assert_eq!(credential_url("mentenaz"), credential_url("MENTENAZ"));
    }
}
