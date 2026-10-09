use settings::{DockSide, RegisterSetting, Settings};

/// The Rust panel's user settings, read from the `"rust_panel"` settings key.
#[derive(Clone, Copy, Debug, PartialEq, RegisterSetting)]
pub struct RustPanelSettings {
    pub button: bool,
    pub dock: DockSide,
}

impl Settings for RustPanelSettings {
    fn from_settings(content: &settings::SettingsContent) -> Self {
        let panel = content.rust_panel.as_ref().unwrap();
        Self {
            button: panel.button.unwrap(),
            dock: panel.dock.unwrap(),
        }
    }
}
