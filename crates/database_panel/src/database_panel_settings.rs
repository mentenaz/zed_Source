use settings::{RegisterSetting, Settings};

/// The database panel's user settings, read from the `"database_panel"` settings key.
#[derive(Clone, Copy, Debug, PartialEq, RegisterSetting)]
pub struct DatabasePanelSettings {
    pub button: bool,
}

impl Settings for DatabasePanelSettings {
    fn from_settings(content: &settings::SettingsContent) -> Self {
        let panel = content.database_panel.as_ref().unwrap();
        Self {
            button: panel.button.unwrap(),
        }
    }
}
