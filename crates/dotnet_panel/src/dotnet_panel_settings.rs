use settings::{DockSide, RegisterSetting, Settings};

/// The .NET panel's user settings, read from the `"dotnet_panel"` settings key.
#[derive(Clone, Copy, Debug, PartialEq, RegisterSetting)]
pub struct DotnetPanelSettings {
    pub button: bool,
    pub dock: DockSide,
}

impl Settings for DotnetPanelSettings {
    fn from_settings(content: &settings::SettingsContent) -> Self {
        let panel = content.dotnet_panel.as_ref().unwrap();
        Self {
            button: panel.button.unwrap(),
            dock: panel.dock.unwrap(),
        }
    }
}
