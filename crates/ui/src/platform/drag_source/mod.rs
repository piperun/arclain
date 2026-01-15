//! Platform-specific drag source abstraction
//!
//! Enables dragging files from Arclain to external applications (Explorer, etc.)
//! Uses native Windows COM APIs for deferred extraction with IProgressDialog,
//! or `drag` crate fallback for non-Windows platforms.

#[cfg(target_os = "windows")]
pub mod stream;
#[cfg(target_os = "windows")]
pub mod windows_native;

use arclain_core::{ArchiveBackend, ArchiveEntry};
use std::path::PathBuf;
use std::sync::Arc;

/// Result of a drag operation
#[derive(Debug, Clone, PartialEq)]
pub enum DragResult {
    /// File was successfully dropped
    Dropped,
    /// User cancelled the drag
    Cancelled,
    /// Drop target didn't accept the file
    Rejected,
}

/// Error during drag operation
#[derive(Debug)]
pub enum DragError {
    /// Failed to extract file from archive
    ExtractionFailed(String),
    /// Platform-specific error
    PlatformError(String),
    /// No files provided
    NoFiles,
}

impl std::fmt::Display for DragError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DragError::ExtractionFailed(e) => write!(f, "Extraction failed: {}", e),
            DragError::PlatformError(e) => write!(f, "Platform error: {}", e),
            DragError::NoFiles => write!(f, "No files to drag"),
        }
    }
}

impl std::error::Error for DragError {}

/// Start a drag operation with the given files
///
/// This is a blocking operation that completes when the user drops.
/// Files should already exist on disk (extract to temp before calling).
///
/// # Arguments
/// * `window` - Window handle implementing `raw_window_handle::HasWindowHandle`
/// * `files` - Paths to files to drag (must exist on disk)
///
/// # Linux Note
/// On Linux with GTK, this requires the window to be a GTK window.
/// Since eframe uses winit, Linux drag-out may not work until winit adds GTK support.
#[cfg(not(target_os = "linux"))]
pub fn start_drag<W: raw_window_handle::HasWindowHandle>(
    window: &W,
    files: Vec<PathBuf>,
) -> Result<DragResult, DragError> {
    if files.is_empty() {
        return Err(DragError::NoFiles);
    }

    let item = drag::DragItem::Files(files);

    drag::start_drag(
        window,
        item,
        drag::Image::Raw(vec![]), // Empty image - use default cursor
        |result, _cursor_pos| {
            tracing::debug!("Drag completed: {:?}", result);
        },
        drag::Options::default(),
    )
    .map(|_| DragResult::Dropped)
    .map_err(|e| DragError::PlatformError(e.to_string()))
}

/// Start a deferred drag operation using native Windows APIs.
///
/// Files are extracted in a single batch operation to a temp directory,
/// with a native Windows IProgressDialog shown during extraction.
/// This is MUCH faster than extracting files one-by-one.
#[cfg(target_os = "windows")]
pub fn start_deferred_drag(
    backend: Arc<dyn ArchiveBackend>,
    archive_path: PathBuf,
    entries: Vec<ArchiveEntry>,
    password: Option<String>,
) -> Result<DragResult, DragError> {
    use windows_native::start_deferred_drag as native_start;

    match native_start(backend, archive_path, entries, password) {
        Ok(_effect) => Ok(DragResult::Dropped),
        Err(e) => Err(DragError::PlatformError(e)),
    }
}

#[cfg(not(target_os = "windows"))]
pub fn start_deferred_drag(
    _backend: Arc<dyn ArchiveBackend>,
    _archive_path: PathBuf,
    _entries: Vec<ArchiveEntry>,
    _password: Option<String>,
) -> Result<DragResult, DragError> {
    Err(DragError::PlatformError(
        "Deferred drag not supported on this platform. Please extract first.".into(),
    ))
}


/// Linux stub - drag-out not supported with winit on Linux yet
#[cfg(target_os = "linux")]
pub fn start_drag<W>(_window: &W, _files: Vec<PathBuf>) -> Result<DragResult, DragError> {
    Err(DragError::PlatformError(
        "Drag-out not yet supported on Linux with winit".into(),
    ))
}
