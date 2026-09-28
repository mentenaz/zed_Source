use std::path::PathBuf;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use settings_macros::{MergeFrom, with_fallible_options};

/// Settings for the Flows feature (the workflow designer and dashboard).
///
/// Flows live in `workflows_dir` under each workspace root. The workflow
/// engine renders them as `.flow.json` files; the designer panel renders
/// the canvas and executes runs.
#[with_fallible_options]
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, MergeFrom, Default, PartialEq)]
pub struct FlowsSettingsContent {
    /// The directory, relative to the workspace root, where Flow files
    /// (`.flow.json`) are stored.
    ///
    /// Default: `.zed/workflows`
    pub workflows_dir: Option<PathBuf>,
}