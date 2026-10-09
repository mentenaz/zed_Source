use settings::{DockSide, RegisterSetting, Settings};

/// The Node panel's user settings, read from the `"node_panel"` settings key.
#[derive(Clone, Copy, Debug, PartialEq, RegisterSetting)]
pub struct NodePanelSettings {
    pub button: bool,
    pub dock: DockSide,
}

impl Settings for NodePanelSettings {
    fn from_settings(content: &settings::SettingsContent) -> Self {
        let panel = content.node_panel.as_ref().unwrap();
        Self {
            button: panel.button.unwrap(),
            dock: panel.dock.unwrap(),
        }
    }
}
