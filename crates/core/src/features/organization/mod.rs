pub mod engine;
pub mod flatten_helper;
pub mod metrics;
pub mod organizer;
pub mod presets;
pub mod profile;
#[cfg(test)]
pub mod pruning_tests;
pub mod session;

pub mod checks;
pub mod downloads;
pub mod flatten;
pub mod metadata;
pub mod tasks;

pub use checks::*;
pub use metadata::{GameMetadata, ScreenshotData};
pub use organizer::*;
pub use profile::{list_archive_profiles, load_archive_profile, ArchiveFormat, ArchiveProfile};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct OrganizationRule {
    /// Database ID (0 for new rules)
    #[serde(default)]
    pub id: i64,
    pub name: String,
    pub priority: i32,
    pub is_enabled: bool,
    pub trigger: RuleTrigger,
    pub actions: RuleActions,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RuleTrigger {
    pub metadata_source: Option<String>,
    pub filename_pattern: Option<String>,
    pub has_file: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RuleActions {
    pub root_folder: Option<String>,
    /// Template for output archive name (uses template variables)
    #[serde(default)]
    pub output_name: Option<String>,
    pub move_files: Vec<MoveAction>,
    pub use_standard_layout: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MoveAction {
    pub pattern: String,
    pub target: String,
}
