use settings::{DockSide, RegisterSetting, Settings};

/// The Helm panel's user settings, read from the `"helm_panel"` settings key.
#[derive(Clone, Copy, Debug, PartialEq, RegisterSetting)]
pub struct HelmPanelSettings {
    pub button: bool,
    pub dock: DockSide,
}

impl Settings for HelmPanelSettings {
    fn from_settings(content: &settings::SettingsContent) -> Self {
        let panel = content.helm_panel.as_ref().unwrap();
        Self {
            button: panel.button.unwrap(),
            dock: panel.dock.unwrap(),
        }
    }
}
