// use arclain_core::ArchiveEntry; // Unused
// use std::path::PathBuf; // Unused

/// State for batch extraction - extracted files are cached in a temp directory
pub struct ExtractionCache {
    /// Temp directory where files are extracted (auto-cleaned on drop via tempfile)
    pub temp_dir: tempfile::TempDir,
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
