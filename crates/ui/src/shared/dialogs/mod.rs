// Shared dialog utilities module

pub mod form_dialog;
pub mod helpers;
pub mod lightbox;
pub mod log_viewer;
pub mod merge_dialog;
pub mod progress;

// Re-export dialog types
pub use form_dialog::{DialogMode, FormDialog, FormDialogConfig, FormDialogResult};
pub use lightbox::{render_lightbox, LightboxResult, LightboxState};
pub use merge_dialog::{render_merge_dialog, MergeDialogResult, MergeDialogState};
pub use progress::{ExtractionProgressDialog, ExtractionStatus};
