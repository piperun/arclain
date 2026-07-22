//! File list action types

pub use crate::shared::models::file_entry::{
    parse_ratio_pct, parse_size_to_bytes, FileEntry, SortColumn, SortState,
};

/// Actions that can be triggered from the file list
#[derive(Debug, Clone)]
pub enum FileListAction {
    Navigate(String),         // display-relative folder navigation path
    Edit(String),             // stable archive-root path
    Delete(String),           // stable archive-root path
    Open(String),             // stable archive-root path
    Extract(String),          // stable archive-root path
    ExtractTo(String),        // stable archive-root path
    CopyPath(String),         // stable archive-root path
    ShowProperties(String),   // stable archive-root path
    DragStarted(Vec<String>), // stable archive-root paths
}
