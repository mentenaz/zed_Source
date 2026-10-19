use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use settings_macros::{MergeFrom, with_fallible_options};

use crate::{DockPosition, DockSide};

/// Settings for a sidebar-shaped panel, which can sit in the left or right
/// dock.
#[with_fallible_options]
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema, MergeFrom)]
pub struct SidePanelSettingsContent {
    /// Whether to show the panel button in the status bar.
    ///
    /// Default: true
    pub button: Option<bool>,
    /// Where to dock the panel.
    ///
    /// Default: left
    pub dock: Option<DockSide>,
}

/// Settings for the Helm (GitHub) panel.
#[with_fallible_options]
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema, MergeFrom)]
pub struct HelmPanelSettingsContent {
    /// Whether to show the panel button in the status bar.
    ///
    /// Default: true
    pub button: Option<bool>,
    /// Where to dock the panel.
    ///
    /// Default: left
    pub dock: Option<DockSide>,
    /// The GitHub login a workspace uses when it has not picked one.
    ///
    /// Default: none (the only account, if there is exactly one)
    pub default_account: Option<String>,
    /// The GitHub accounts signed in to from Helm. The tokens themselves are
    /// kept in the system credential store, never here, so an account listed
    /// on a machine without its token shows as needing sign-in.
    ///
    /// Default: []
    pub accounts: Option<Vec<HelmAccountContent>>,
}

/// A GitHub account signed in to from Helm.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HelmAccountContent {
    /// The GitHub login.
    pub login: String,
    /// A name to show for the account instead of the login.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// How the account was signed in. Decides how a missing scope is added.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<HelmAccountSource>,
}

/// How a Helm account's token was obtained.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum HelmAccountSource {
    /// Helm's own OAuth App ("Login with GitHub").
    OauthApp,
    /// Copied once from the GitHub CLI ("Use GitHub CLI").
    GhCli,
    /// A pasted personal access token.
    Token,
}

/// Settings for a panel that can sit in any of the three docks.
#[with_fallible_options]
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema, MergeFrom)]
pub struct DockablePanelSettingsContent {
    /// Whether to show the panel button in the status bar.
    ///
    /// Default: true
    pub button: Option<bool>,
    /// Where to dock the panel.
    ///
    /// Default: bottom
    pub dock: Option<DockPosition>,
}

/// Settings for a panel whose layout only works in the bottom dock.
#[with_fallible_options]
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, JsonSchema, MergeFrom)]
pub struct BottomPanelSettingsContent {
    /// Whether to show the panel button in the status bar.
    ///
    /// Default: true
    pub button: Option<bool>,
}
