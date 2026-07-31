use std::path::PathBuf;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct PasswordDialog {
    pub show: bool,
    pub password: String,
    pub save_password: bool,
    pub error: String,
    pub target_path: Option<PathBuf>,
    // The pre-2026-05-20 `pending_tab_id: Option<TabId>` field is gone.
    // After the B3 reframed migration, the dialog lives on the
    // `TabState` that triggered the prompt (see `TabState::password_dialog`),
    // so the routing is implicit: the unlock result lands on the tab
    // whose dialog the user interacted with.
}

#[derive(Clone, PartialEq)]
pub struct PasswordRule {
    /// Name of the stored rule this draft originated from. `None` means a
    /// newly-added row that has never been persisted.
    pub original_name: Option<String>,
    pub name: String,
    pub pattern: String,
    /// Only a replacement typed during this edit session. The stored
    /// password is never projected into frontend state.
    pub replacement_password: String,
    pub password_configured: bool,
    pub priority: u32,
    pub enabled: bool,
}

impl std::fmt::Debug for PasswordRule {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PasswordRule")
            .field("original_name", &self.original_name)
            .field("name", &self.name)
            .field("pattern", &self.pattern)
            .field(
                "replacement_password_configured",
                &!self.replacement_password.is_empty(),
            )
            .field("password_configured", &self.password_configured)
            .field("priority", &self.priority)
            .field("enabled", &self.enabled)
            .finish()
    }
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

impl PasswordRulesDialog {
    pub fn can_save_edit(&self) -> bool {
        let preserves_stored_password = self
            .editing_index
            .and_then(|index| self.rules.get(index))
            .is_some_and(|rule| rule.original_name.is_some() && rule.password_configured);

        !self.edit_pattern.trim().is_empty()
            && (preserves_stored_password || !self.edit_password.is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stored_rule() -> PasswordRule {
        PasswordRule {
            original_name: Some("stored".to_string()),
            name: "stored".to_string(),
            pattern: ".*".to_string(),
            replacement_password: String::new(),
            password_configured: true,
            priority: 10,
            enabled: true,
        }
    }

    #[test]
    fn an_existing_configured_rule_can_be_edited_without_retyping_its_password() {
        let mut dialog = PasswordRulesDialog::default();
        dialog.rules.push(stored_rule());
        dialog.editing_index = Some(0);
        dialog.edit_pattern = "renamed-pattern".to_string();

        assert!(dialog.can_save_edit());
    }

    #[test]
    fn a_new_rule_cannot_be_added_without_a_password() {
        let mut dialog = PasswordRulesDialog::default();
        dialog.edit_pattern = "new-pattern".to_string();

        assert!(!dialog.can_save_edit());
    }

    #[test]
    fn an_unsaved_row_does_not_gain_password_preservation_by_being_reedited() {
        let mut rule = stored_rule();
        rule.original_name = None;
        let mut dialog = PasswordRulesDialog::default();
        dialog.rules.push(rule);
        dialog.editing_index = Some(0);
        dialog.edit_pattern = "new-pattern".to_string();

        assert!(!dialog.can_save_edit());
    }

    #[test]
    fn a_rule_draft_does_not_print_its_replacement_password() {
        let mut rule = stored_rule();
        rule.replacement_password = "replacement-secret-2a3f".to_string();

        assert!(!format!("{rule:?}").contains("replacement-secret-2a3f"));
    }
}
