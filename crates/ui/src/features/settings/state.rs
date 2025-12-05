use crate::features::{dialogs, settings_content};
use crate::features::plugins::types::PluginsListState;

#[derive(Default)]
pub struct SettingsFeatureState {
    pub security_settings: settings_content::SecuritySettingsState,
    pub archives_settings: settings_content::ArchivesSettingsState,
    pub plugins_state: PluginsListState,
    pub password_rules_dialog: dialogs::PasswordRulesDialog,
    pub organization_rules_state: crate::features::organization::rules_page::OrganizationRulesState,
    pub password_rules_loaded: bool,
}