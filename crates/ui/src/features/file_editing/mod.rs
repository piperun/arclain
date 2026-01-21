pub mod application;
pub mod domain;
pub mod presentation;

pub use domain::types::{FileEditDialog, FileEditResult};
pub use presentation::views::edit_dialog::render_file_edit_dialog;
