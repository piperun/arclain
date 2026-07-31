use crate::core::SettingsPage;
use crate::features::password_management::dialogs::zip_pass_rules::PasswordRule;
use crate::features::password_management::dialogs::PasswordRulesDialog;
use crate::shared::SharedState;

/// Top-level feature struct for password management, owning the
/// password-rules dialog/page state.
pub struct PasswordManagementFeature {
    pub password_rules_dialog: PasswordRulesDialog,
    persisted_rules: Vec<PasswordRule>,
    /// Tracks the last settings page rendered, so the rules list can be
    /// resynced through the application facade whenever the user (re-)enters
    /// the PasswordRules page. Previously lived on `SettingsFeature`.
    last_visited_page: Option<SettingsPage>,
}

impl PasswordManagementFeature {
    pub fn new(shared: &SharedState) -> Self {
        let mut feature = Self {
            password_rules_dialog: PasswordRulesDialog {
                ..Default::default()
            },
            persisted_rules: Vec::new(),
            last_visited_page: None,
        };
        if let Err(error) = feature.reload(shared) {
            feature.password_rules_dialog.error = error.summary;
        }
        feature
    }

    /// If the page just transitioned to PasswordRules, reload the dialog's
    /// rules list from `app_state.pass_rules`. Mirrors the behavior that
    /// used to live in `SettingsFeature::render`.
    pub fn sync_on_page_change(&mut self, shared: &SharedState, page: &SettingsPage) {
        if *page == SettingsPage::PasswordRules && self.last_visited_page.as_ref() != Some(page) {
            match self.reload(shared) {
                Ok(()) => tracing::debug!(
                    "Reloaded {} password-rule summaries from the application facade",
                    self.password_rules_dialog.rules.len()
                ),
                Err(error) => {
                    tracing::warn!("Failed to reload password rules: {}", error.summary);
                    self.password_rules_dialog.error = error.summary;
                }
            }
        }
        self.last_visited_page = Some(page.clone());
    }

    /// Returns true when the editable, non-secret draft differs from the
    /// last summaries successfully loaded or saved through the facade.
    pub fn is_dirty(&self) -> bool {
        self.password_rules_dialog.rules != self.persisted_rules
    }

    pub fn reload(
        &mut self,
        shared: &SharedState,
    ) -> Result<(), arclain_app::error::ApplicationError> {
        let facade = require_facade(shared)?;
        let summaries = shared
            .services
            .tokio_runtime
            .block_on(facade.password_rules())?;
        self.replace_with_summaries(summaries);
        Ok(())
    }

    pub fn save(
        &mut self,
        shared: &SharedState,
    ) -> Result<(), arclain_app::error::ApplicationError> {
        let facade = require_facade(shared)?;
        let edits = self
            .password_rules_dialog
            .rules
            .iter()
            .map(|rule| arclain_app::settings::PasswordRuleEditInput {
                original_name: rule.original_name.clone(),
                name: rule.name.clone(),
                pattern: rule.pattern.clone(),
                priority: rule.priority,
                enabled: rule.enabled,
                password: (!rule.replacement_password.is_empty()).then(|| {
                    arclain_app::challenge::SecretInput::new(rule.replacement_password.clone())
                }),
            })
            .collect();
        let summaries = shared
            .services
            .tokio_runtime
            .block_on(facade.replace_password_rules(edits))?;
        self.replace_with_summaries(summaries);
        Ok(())
    }

    fn replace_with_summaries(
        &mut self,
        summaries: Vec<arclain_app::settings::PasswordRuleSummary>,
    ) {
        let rules: Vec<_> = summaries
            .into_iter()
            .map(|summary| PasswordRule {
                original_name: Some(summary.name.clone()),
                name: summary.name,
                pattern: summary.pattern,
                replacement_password: String::new(),
                password_configured: summary.password_configured,
                priority: summary.priority,
                enabled: summary.enabled,
            })
            .collect();
        self.persisted_rules = rules.clone();
        self.password_rules_dialog.rules = rules;
        self.password_rules_dialog.error.clear();
    }
}

fn require_facade(
    shared: &SharedState,
) -> Result<&arclain_app::ArclainApp, arclain_app::error::ApplicationError> {
    shared.facade.as_ref().ok_or_else(|| {
        arclain_app::error::ApplicationError::new(
            arclain_app::error::ApplicationErrorKind::Unsupported,
            "password rules are unavailable right now",
        )
        .with_recoverability(arclain_app::error::Recoverability::Fatal)
    })
}
