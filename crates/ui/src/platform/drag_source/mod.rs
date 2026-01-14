//! Platform-specific drag source abstraction
//!
//! Enables dragging files from Arclain to external applications (Explorer, etc.)
//! Uses native Windows COM APIs for deferred extraction, or `drag` crate fallback.

#[cfg(target_os = "windows")]
pub mod stream;
#[cfg(target_os = "windows")]
pub mod windows_native;

use arclain_core::{ArchiveBackend, ArchiveEntry, ExtractionProgress};
use std::path::PathBuf;
use std::sync::Arc;

/// Callback for progress updates during drag extraction
pub type DragProgressCallback = Arc<dyn Fn(ExtractionProgress) + Send + Sync>;

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

#[cfg(target_os = "windows")]
pub fn start_deferred_drag(
    backend: Arc<dyn ArchiveBackend>,
    archive_path: PathBuf,
    entries: Vec<ArchiveEntry>,
    password: Option<String>,
) -> Result<DragResult, DragError> {
    start_deferred_drag_with_progress(backend, archive_path, entries, password, None)
}

/// Start a deferred drag operation with optional progress callback.
///
/// The progress callback is invoked during batch extraction to report progress.
/// This allows the UI to show an extraction progress modal.
#[cfg(target_os = "windows")]
pub fn start_deferred_drag_with_progress(
    backend: Arc<dyn ArchiveBackend>,
    archive_path: PathBuf,
    entries: Vec<ArchiveEntry>,
    password: Option<String>,
    progress: Option<DragProgressCallback>,
) -> Result<DragResult, DragError> {
    use windows_native::start_deferred_drag_with_progress as native_start;

    match native_start(backend, archive_path, entries, password, progress) {
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

#[cfg(not(target_os = "windows"))]
pub fn start_deferred_drag_with_progress(
    _backend: Arc<dyn ArchiveBackend>,
    _archive_path: PathBuf,
    _entries: Vec<ArchiveEntry>,
    _password: Option<String>,
    _progress: Option<DragProgressCallback>,
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
