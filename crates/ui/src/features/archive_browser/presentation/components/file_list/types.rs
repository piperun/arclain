//! File list action types

pub use crate::shared::models::file_entry::{
    parse_ratio_pct, parse_size_to_bytes, FileEntry, SortColumn, SortState,
};

/// Actions that can be triggered from the file list
#[derive(Debug, Clone)]
pub enum FileListAction {
    Navigate(String),
    Edit(String),      // full path (relative to current path will be resolved by caller)
    Delete(String),    // same as above
    Open(String),      // double-click open file
    Extract(String),   // Extract single file
    ExtractTo(String), // Extract to custom location
    CopyPath(String),  // Copy path to clipboard
    ShowProperties(String), // Show properties panel
    DragStarted(Vec<String>), // File(s) dragged - extract to temp and start OS drag
}
