// Dialog state for Password Rules
use std::path::PathBuf;
use super::types::{PasswordRule, RegexTestResult};

pub struct PasswordRulesDialog {
    pub show: bool,
    pub rules: Vec<PasswordRule>,
    pub editing_index: Option<usize>,
    pub edit_name: String,
    pub edit_pattern: String,
    pub edit_password: String,
    pub edit_priority: String,
    pub edit_enabled: bool,
    pub error: String,
    pub show_regex_tester: bool,
    pub regex_test_pattern: String,
    pub regex_test_folder: Option<PathBuf>,
    pub regex_test_results: Vec<RegexTestResult>,
}

impl Default for PasswordRulesDialog {
    fn default() -> Self {
        Self {
            show: false,
            rules: Vec::new(),
            editing_index: None,
            edit_name: String::new(),
            edit_pattern: String::new(),
            edit_password: String::new(),
            edit_priority: "10".to_string(),
            edit_enabled: true,
            error: String::new(),
            show_regex_tester: false,
            regex_test_pattern: String::new(),
            regex_test_folder: None,
            regex_test_results: Vec::new(),
        }
    }
}