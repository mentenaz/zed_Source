use settings::{DockSide, RegisterSetting, Settings};

pub use settings::HelmAccountSource;

/// The Helm panel's user settings, read from the `"helm_panel"` settings key.
#[derive(Clone, Debug, PartialEq, RegisterSetting)]
pub struct HelmPanelSettings {
    pub button: bool,
    pub dock: DockSide,
    /// The login a workspace uses when it has not picked one.
    pub default_account: Option<String>,
    /// The accounts signed in to from Helm. This list is the only index of
    /// them: the credential store can look a token up by name but cannot
    /// list what it holds.
    pub accounts: Vec<HelmAccount>,
}

/// A GitHub account signed in to from Helm. Its token lives in the
/// credential store under [`crate::accounts::credential_url`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HelmAccount {
    pub login: String,
    /// GitHub's numeric id, once Helm has seen it (`None` for an account
    /// listed before ids were recorded, until its next sign-in check).
    pub id: Option<u64>,
    /// Logins it had before being renamed on GitHub.
    pub former_logins: Vec<String>,
    pub label: Option<String>,
    /// The colour chosen for it in settings ("#rrggbb"), if any.
    pub color: Option<String>,
    /// How it was signed in; accounts listed before this was recorded count
    /// as signed in through Helm's OAuth App.
    pub source: HelmAccountSource,
    /// The commit identity for workspaces using this account, once chosen.
    pub git_name: Option<String>,
    pub git_email: Option<String>,
}

impl HelmAccount {
    /// The label if there is one, else the login.
    pub fn display_name(&self) -> &str {
        self.label.as_deref().unwrap_or(&self.login)
    }
}

impl HelmPanelSettings {
    /// The account a workspace uses when it has not picked one: the default
    /// account if it is listed, else the only account.
    pub fn default_account(&self) -> Option<&HelmAccount> {
        if let Some(login) = &self.default_account
            && let Some(account) = self.account(login)
        {
            return Some(account);
        }
        match self.accounts.as_slice() {
            [only] => Some(only),
            _ => None,
        }
    }

    /// The listed account with this login, compared the way GitHub compares
    /// logins: ignoring case. A login the account had before it was renamed
    /// finds it too.
    pub fn account(&self, login: &str) -> Option<&HelmAccount> {
        self.accounts
            .iter()
            .find(|account| account.login.eq_ignore_ascii_case(login))
            .or_else(|| {
                self.accounts.iter().find(|account| {
                    account
                        .former_logins
                        .iter()
                        .any(|former| former.eq_ignore_ascii_case(login))
                })
            })
    }

    /// The colour `login`'s account is shown in: the one chosen in settings,
    /// else one picked from the login it was first listed under, so it stays
    /// the same after a rename.
    pub fn account_color(&self, login: &str) -> gpui::Hsla {
        match self.account(login) {
            Some(account) => helm_ui::account_color(
                account.former_logins.first().unwrap_or(&account.login),
                account.color.as_deref(),
            ),
            None => helm_ui::account_color(login, None),
        }
    }

    /// The listed account with GitHub's numeric id `id`.
    pub fn account_by_id(&self, id: u64) -> Option<&HelmAccount> {
        self.accounts.iter().find(|account| account.id == Some(id))
    }
}

impl Settings for HelmPanelSettings {
    fn from_settings(content: &settings::SettingsContent) -> Self {
        let panel = content.helm_panel.as_ref().unwrap();
        Self {
            button: panel.button.unwrap(),
            dock: panel.dock.unwrap(),
            default_account: panel.default_account.clone(),
            accounts: panel
                .accounts
                .iter()
                .flatten()
                .map(|account| HelmAccount {
                    login: account.login.clone(),
                    id: account.id,
                    former_logins: account.former_logins.clone(),
                    label: account.label.clone(),
                    color: account.color.clone(),
                    source: account.source.unwrap_or(HelmAccountSource::OauthApp),
                    git_name: account.git_name.clone(),
                    git_email: account.git_email.clone(),
                })
                .collect(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn account(login: &str, id: Option<u64>, former: &[&str]) -> HelmAccount {
        HelmAccount {
            login: login.to_string(),
            id,
            former_logins: former.iter().map(|login| login.to_string()).collect(),
            label: None,
            color: None,
            source: HelmAccountSource::OauthApp,
            git_name: None,
            git_email: None,
        }
    }

    #[test]
    fn a_renamed_account_is_found_by_its_old_login_and_its_id() {
        let settings = HelmPanelSettings {
            button: true,
            dock: DockSide::Right,
            default_account: None,
            accounts: vec![
                account("NewName", Some(42), &["OldName"]),
                account("other", None, &[]),
            ],
        };
        assert_eq!(settings.account("newname").unwrap().login, "NewName");
        assert_eq!(settings.account("oldname").unwrap().login, "NewName");
        assert_eq!(settings.account_by_id(42).unwrap().login, "NewName");
        assert!(settings.account_by_id(7).is_none());
        assert!(settings.account("nobody").is_none());
    }
}
