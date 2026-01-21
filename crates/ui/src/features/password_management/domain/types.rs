use std::path::PathBuf;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct PasswordDialog {
    pub show: bool,
    pub password: String,
    pub save_password: bool,
    pub error: String,
    pub target_path: Option<PathBuf>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PasswordRule {
    pub name: String,
    pub pattern: String,
    pub password: String,
    pub priority: u32,
    pub enabled: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RegexTestResult {
    pub file_path: String,
    pub matched: bool,
}

#[derive(Clone, PartialEq)]
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
