use settings::{DockSide, RegisterSetting, Settings};

/// The Python panel's user settings, read from the `"python_panel"` settings key.
#[derive(Clone, Copy, Debug, PartialEq, RegisterSetting)]
pub struct PythonPanelSettings {
    pub button: bool,
    pub dock: DockSide,
}

impl Settings for PythonPanelSettings {
    fn from_settings(content: &settings::SettingsContent) -> Self {
        let panel = content.python_panel.as_ref().unwrap();
        Self {
            button: panel.button.unwrap(),
            dock: panel.dock.unwrap(),
        }
    }
}
