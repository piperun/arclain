use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MoveFileRule {
    pub pattern: String,
    pub target: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MoveRule {
    pub target_dir: String,
    pub use_date: bool,
    pub use_category: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PluginRuleTrigger {
    pub filename_pattern: Option<String>,
    pub has_file: Option<String>,
    pub extensions: Option<Vec<String>>,
    pub min_size: Option<u64>,
    pub max_size: Option<u64>,
    pub metadata_source: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PluginRuleActions {
    pub root_folder: Option<String>,
    pub move_files: Vec<MoveFileRule>,
    pub move_to: Option<MoveRule>,
    pub rename_pattern: Option<String>,
    pub organize_content: bool,
    pub delete_original: bool,
    pub use_standard_layout: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PluginRuleDefinition {
    pub name: String,
    pub category: String,
    pub description: Option<String>,
    pub trigger: PluginRuleTrigger,
    pub actions: PluginRuleActions,
}
