use settings::{DockSide, RegisterSetting, Settings};

/// The Flows panel's user settings, read from the `"flows_panel"` settings key.
#[derive(Clone, Copy, Debug, PartialEq, RegisterSetting)]
pub struct FlowsPanelSettings {
    pub button: bool,
    pub dock: DockSide,
}

impl Settings for FlowsPanelSettings {
    fn from_settings(content: &settings::SettingsContent) -> Self {
        let panel = content.flows_panel.as_ref().unwrap();
        Self {
            button: panel.button.unwrap(),
            dock: panel.dock.unwrap(),
        }
    }
}
