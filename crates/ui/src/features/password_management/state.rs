use crate::shared::dialogs::{PasswordDialog, PasswordRulesDialog};
use std::path::PathBuf;

#[derive(Default)]
pub struct PasswordFeatureState {
    /// Main password unlock dialog
    pub password_dialog: PasswordDialog,
    
    /// Password rules management dialog
    pub password_rules_dialog: PasswordRulesDialog,
    
    /// Track if password rules have been loaded for current settings session
    pub password_rules_loaded: bool,
    
    /// Pending archive path waiting for password
    pub pending_archive_path: Option<PathBuf>,
    
    /// Pending file to edit after password unlock
    pub pending_edit_file: Option<String>,
    
    /// Pending file to open after password unlock
    pub pending_open_file: Option<String>,
}