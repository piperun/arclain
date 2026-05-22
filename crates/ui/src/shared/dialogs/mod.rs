// Shared dialog utilities module

pub mod archive_error_dialog;
pub mod ask_each_time_drop;
pub mod close_tab_confirm;
pub mod form_dialog;
pub mod helpers;
pub mod lightbox;
pub mod merge_dialog;
pub mod progress;

// Re-export dialog types
pub use archive_error_dialog::{
    classify as classify_archive_error, render_archive_error_dialog, ArchiveErrorDialogState,
    ArchiveErrorKind,
};
pub use close_tab_confirm::{
    render_close_tab_confirm, CloseTabConfirmResult, CloseTabConfirmState,
};
pub use form_dialog::{DialogMode, FormDialog, FormDialogConfig, FormDialogResult};
pub use lightbox::{render_lightbox, LightboxResult, LightboxState};
pub use merge_dialog::{render_merge_dialog, MergeDialogResult, MergeDialogState};
pub use progress::{ExtractionProgressDialog, ExtractionStatus, ProgressDialogs};
