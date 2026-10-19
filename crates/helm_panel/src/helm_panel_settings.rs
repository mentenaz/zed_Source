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
    pub label: Option<String>,
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
    /// logins: ignoring case.
    pub fn account(&self, login: &str) -> Option<&HelmAccount> {
        self.accounts
            .iter()
            .find(|account| account.login.eq_ignore_ascii_case(login))
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
                    label: account.label.clone(),
                    source: account.source.unwrap_or(HelmAccountSource::OauthApp),
                    git_name: account.git_name.clone(),
                    git_email: account.git_email.clone(),
                })
                .collect(),
        }
    }
}
