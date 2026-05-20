use crate::core::SettingsPage;
use crate::features::password_management::dialogs::zip_pass_rules::PasswordRule;
use crate::features::password_management::dialogs::PasswordRulesDialog;
use crate::shared::SharedState;

/// Top-level feature struct for password management, owning the
/// password-rules dialog/page state.
pub struct PasswordManagementFeature {
    pub password_rules_dialog: PasswordRulesDialog,
    /// Tracks the last settings page rendered, so the rules list can be
    /// resynced from `app_state.pass_rules` whenever the user (re-)enters
    /// the PasswordRules page. Previously lived on `SettingsFeature`.
    last_visited_page: Option<SettingsPage>,
}

impl PasswordManagementFeature {
    pub fn new(shared: &SharedState) -> Self {
        let rules = Self::collect_rules_from_state(shared);
        Self {
            password_rules_dialog: PasswordRulesDialog {
                rules,
                ..Default::default()
            },
            last_visited_page: None,
        }
    }

    /// If the page just transitioned to PasswordRules, reload the dialog's
    /// rules list from `app_state.pass_rules`. Mirrors the behavior that
    /// used to live in `SettingsFeature::render`.
    pub fn sync_on_page_change(&mut self, shared: &SharedState, page: &SettingsPage) {
        if *page == SettingsPage::PasswordRules && self.last_visited_page.as_ref() != Some(page) {
            self.password_rules_dialog.rules = Self::collect_rules_from_state(shared);
            tracing::debug!(
                "Reloaded {} password rules from app state",
                self.password_rules_dialog.rules.len()
            );
        }
        self.last_visited_page = Some(page.clone());
    }

    /// Returns true when the dialog's rules differ from the persisted
    /// rules in `app_state.pass_rules`. Used by the settings header to
    /// decide whether to mark the page dirty.
    pub fn is_dirty(&self, shared: &SharedState) -> bool {
        let state = shared.app_state.lock();
        if self.password_rules_dialog.rules.len() != state.pass_rules.len() {
            return true;
        }
        for (i, rule) in self.password_rules_dialog.rules.iter().enumerate() {
            let other = &state.pass_rules[i];
            if rule.name != other.name
                || rule.pattern != other.pattern
                || rule.password != other.password
                || rule.priority != other.priority
                || rule.enabled != other.enabled
            {
                return true;
            }
        }
        false
    }

    fn collect_rules_from_state(shared: &SharedState) -> Vec<PasswordRule> {
        let state = shared.app_state.lock();
        state
            .pass_rules
            .iter()
            .map(|r| PasswordRule {
                name: r.name.clone(),
                pattern: r.pattern.clone(),
                password: r.password.clone(),
                priority: r.priority,
                enabled: r.enabled,
            })
            .collect()
    }
}
