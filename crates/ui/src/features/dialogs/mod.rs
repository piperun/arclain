// Dialog sub-modules - feature-first design
pub mod password;
pub mod file_edit;
pub mod preferences;
pub mod zip_pass_rules;
pub mod helpers;
pub mod progress;

// Re-export commonly used types for convenience
pub use password::{PasswordDialog, PasswordDialogResult, render_password_dialog};
pub use file_edit::{FileEditDialog, FileEditResult, render_file_edit_dialog};
pub use preferences::{
    EncryptedCrcPolicy,
};
pub use zip_pass_rules::{
    PasswordRulesDialog,
    PasswordRulesResult,
    PasswordRule,
    render_password_rules_dialog
};
pub use progress::{
    ExtractionProgressDialog,
    ExtractionStatus,
    ExtractionDialogResult,
    render_extraction_progress_dialog,
};