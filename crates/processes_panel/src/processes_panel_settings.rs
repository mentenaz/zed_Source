use settings::{RegisterSetting, Settings};

/// The processes panel's user settings, read from the `"processes_panel"` settings key.
#[derive(Clone, Copy, Debug, PartialEq, RegisterSetting)]
pub struct ProcessesPanelSettings {
    pub button: bool,
}

impl Settings for ProcessesPanelSettings {
    fn from_settings(content: &settings::SettingsContent) -> Self {
        let panel = content.processes_panel.as_ref().unwrap();
        Self {
            button: panel.button.unwrap(),
        }
    }
}
