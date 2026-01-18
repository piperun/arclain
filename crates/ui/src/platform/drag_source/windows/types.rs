// use arclain_core::ArchiveEntry; // Unused
// use std::path::PathBuf; // Unused

/// State for batch extraction - extracted files are cached in a temp directory
pub struct ExtractionCache {
    /// Temp directory path (always valid)
    pub temp_dir_path: std::path::PathBuf,
    /// Optional RAII guard - if set, directory is deleted on drop. If None, it persists.
    pub _guard: Option<tempfile::TempDir>,
    /// Set to true once batch extraction is complete
    pub extracted: bool,
}

/// Entry for drag operation with both archive path and display path
#[derive(Debug, Clone)]
pub struct DragEntry {
    /// Full path in the archive (for extraction)
    pub archive_path: String,
    /// Display path for the file descriptor (relative to what user dragged)
    pub display_path: String,
    /// File size
    pub size: u64,
}
