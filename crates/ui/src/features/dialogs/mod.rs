// Dialog sub-modules - feature-first design
pub mod file_edit;
pub mod helpers;
pub mod password;
pub mod preferences;
pub mod progress;
pub mod zip_pass_rules;

// Re-export commonly used types for convenience
pub use file_edit::{render_file_edit_dialog, FileEditDialog, FileEditResult};
pub use password::{render_password_dialog, PasswordDialog, PasswordDialogResult};
pub use preferences::EncryptedCrcPolicy;
pub use progress::{
    render_extraction_progress_dialog, ExtractionDialogResult, ExtractionProgressDialog,
    ExtractionStatus,
};
pub use zip_pass_rules::{
    render_password_rules_dialog, PasswordRule, PasswordRulesDialog, PasswordRulesResult,
};
