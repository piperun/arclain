use crate::features::password_management::dialogs;
use crate::features::plugins::domain::types::PluginsListState;

use crate::features::settings::domain::types as settings_content;

#[derive(Default)]
pub struct SettingsFeatureState {
    pub security_settings: settings_content::SecuritySettingsState,
    pub archives_settings: settings_content::ArchivesSettingsState,
    pub plugins_state: PluginsListState,
    pub password_rules_dialog: dialogs::PasswordRulesDialog,
    pub organization_rules_state:
        crate::features::organization::presentation::views::rules_page::RulesPage,

    pub password_rules_loaded: bool,
}
