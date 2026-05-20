//! Archive browser feature types

/// Actions that can be triggered by the archive browser UI
#[derive(Debug, Clone)]
pub enum Action {
    /// Navigate into a folder within the archive
    NavigateToFolder(String),
    /// Navigate to a specific path in the archive
    NavigateToPath(String),
    /// Open/preview a file from the archive
    OpenFile(String),
    /// Open a nested archive in a new tab
    OpenArchiveInTab(String),
    /// Edit a text file from the archive
    EditFile(String),
    /// Delete a file from the archive
    DeleteFile(String),
    /// Open the organize panel
    Organize,
    /// Metadata JSON received from a plugin
    Metadata(String),
    /// Extract a single file to default location
    Extract(String),
    /// Extract a file to a user-selected location
    ExtractTo(String),
    /// Copy the file path to clipboard
    CopyPath(String),
    /// Show file properties panel
    ShowProperties(String),
    /// Start drag-out operation (extract to temp, then OS drag)
    DragExtract(Vec<String>),
    /// Navigate back in history
    NavigateBack,
    /// Navigate forward in history
    NavigateForward,
    /// Navigate up one level
    NavigateUp,
    /// No action
    None,
}

// Re-export for backwards compatibility if needed
pub type ArchiveBrowserAction = Action;

// `BrowserViewState` was relocated to `core/tabs/view_state.rs` per the
// 2026-05-19 dependency audit (§2 + §5 medium #9): the type is per-tab
// state and was being imported by `core/tabs/tab_state.rs`, violating
// `core/ ⊥ features/`. Consumers in this feature now import from
// `crate::core::tabs::BrowserViewState`.
