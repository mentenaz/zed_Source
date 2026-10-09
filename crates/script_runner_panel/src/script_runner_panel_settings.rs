use settings::{DockPosition, RegisterSetting, Settings};

/// The script runner panel's user settings, read from the `"script_runner_panel"` settings key.
#[derive(Clone, Copy, Debug, PartialEq, RegisterSetting)]
pub struct ScriptRunnerPanelSettings {
    pub button: bool,
    pub dock: DockPosition,
}

impl Settings for ScriptRunnerPanelSettings {
    fn from_settings(content: &settings::SettingsContent) -> Self {
        let panel = content.script_runner_panel.as_ref().unwrap();
        Self {
            button: panel.button.unwrap(),
            dock: panel.dock.unwrap(),
        }
    }
}
