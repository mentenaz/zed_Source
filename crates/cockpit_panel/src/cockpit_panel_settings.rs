use settings::{DockSide, RegisterSetting, Settings};

/// The cockpit panel's user settings, read from the `"cockpit_panel"` settings key.
#[derive(Clone, Copy, Debug, PartialEq, RegisterSetting)]
pub struct CockpitPanelSettings {
    pub button: bool,
    pub dock: DockSide,
}

impl Settings for CockpitPanelSettings {
    fn from_settings(content: &settings::SettingsContent) -> Self {
        let panel = content.cockpit_panel.as_ref().unwrap();
        Self {
            button: panel.button.unwrap(),
            dock: panel.dock.unwrap(),
        }
    }
}
