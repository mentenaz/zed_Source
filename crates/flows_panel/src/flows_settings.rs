use std::path::PathBuf;

use settings::{RegisterSetting, Settings};

/// The Flows feature's user settings, read from the `"flows"` settings key.
///
/// Default: workflows live in `.zed/workflows` under each workspace root.
#[derive(Clone, Debug, Default, RegisterSetting)]
pub struct FlowsSettings {
    /// The directory, relative to the workspace root, where Flow files
    /// (`.flow.json`) are stored and searched for. Created on first use.
    ///
    /// Default: `.zed/workflows`
    pub workflows_dir: PathBuf,
}

impl Settings for FlowsSettings {
    fn from_settings(content: &settings::SettingsContent) -> Self {
        let content = content.flows.clone().unwrap_or_default();
        Self {
            workflows_dir: content
                .workflows_dir
                .unwrap_or_else(|| PathBuf::from(".zed/workflows")),
        }
    }
}
