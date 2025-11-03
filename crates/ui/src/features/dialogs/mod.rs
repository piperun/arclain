// Dialog sub-modules - feature-first design
pub mod password;
pub mod file_edit;
pub mod preferences;
pub mod password_rules;
pub mod helpers;

// Re-export commonly used types for convenience
pub use password::{PasswordDialog, PasswordDialogResult, render_password_dialog};
pub use file_edit::{FileEditDialog, FileEditResult, render_file_edit_dialog};
pub use preferences::{
    PreferencesDialog, 
    PreferencesDialogResult, 
    EncryptedCrcPolicy,
    render_preferences_dialog
};
pub use password_rules::{
    PasswordRulesDialog, 
    PasswordRulesResult, 
    PasswordRule,
    render_password_rules_dialog
};