// Shared dialog utilities module

pub mod form_dialog;
pub mod helpers;
pub mod log_viewer;
pub mod progress;

// Re-export dialog types
pub use form_dialog::{DialogMode, FormDialog, FormDialogConfig, FormDialogResult};
pub use progress::{ExtractionProgressDialog, ExtractionStatus};
