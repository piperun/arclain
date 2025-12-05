use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub mod engine;
pub mod organizer;
pub mod presets;

#[cfg(test)]
mod pruning_tests;

// Re-export commonly used types from organizer
pub use organizer::{
    check_archive_structure, execute_organization_plan, needs_better_compression,
    organize_archive, ArchiveStructure, GameMetadata, ScreenshotData,
};

/// A rule for organizing archives
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrganizationRule {
    pub id: Option<i64>, // Database ID
    pub name: String,
    pub description: Option<String>,
    pub category: String, // e.g., "DLSite", "Scene", "General"
    pub priority: i32,
    pub is_enabled: bool,
    pub is_system: bool, // Cannot be deleted
    pub trigger: RuleTrigger,
    pub actions: RuleActions,
}

impl Default for OrganizationRule {
    fn default() -> Self {
        Self {
            id: None,
            name: "New Rule".to_string(),
            description: None,
            category: "General".to_string(),
            priority: 0,
            is_enabled: true,
            is_system: false,
            trigger: RuleTrigger::default(),
            actions: RuleActions::default(),
        }
    }
}

/// Triggers that cause a rule to match
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RuleTrigger {
    /// Regex pattern to match the archive filename
    /// Capture groups can be used in metadata mapping
    pub filename_pattern: Option<String>,

    /// Check if the archive contains a specific file (glob pattern)
    pub has_file: Option<String>,

    /// Check if the archive has a specific extension
    pub extensions: Option<Vec<String>>,

    /// Minimum size in bytes
    pub min_size: Option<u64>,

    /// Maximum size in bytes
    pub max_size: Option<u64>,
}

/// Actions to perform when organizing
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RuleActions {
    /// The new name for the root folder (supports variable expansion)
    pub root_folder: Option<String>,

    /// List of file movement rules
    pub move_files: Vec<MoveFileRule>,

    /// Move the entire archive content to a specific target directory
    pub move_to: Option<MoveRule>,

    /// Rename pattern for files
    pub rename_pattern: Option<String>,

    /// Whether to organize the content (extract, move, repack)
    pub organize_content: bool,

    /// Whether to delete the original archive after organization
    pub delete_original: bool,

    /// If true, enforces the standard Game/Screenshots/Metadata layout.
    /// If true, `move_files` is IGNORED or used only as hints for what constitutes "Game Content".
    #[serde(default)]
    pub use_standard_layout: bool,
}

/// A rule for moving a specific file or group of files
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MoveFileRule {
    /// Glob pattern to match files inside the archive
    pub pattern: String,

    /// Target directory (relative to new root)
    pub target: String,
}

/// A rule for moving the entire archive content
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MoveRule {
    pub target_dir: String,
    pub use_date: bool,
    pub use_category: bool,
}

/// Metadata extracted from the archive or filename
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractedMetadata {
    pub fields: HashMap<String, String>,
}
