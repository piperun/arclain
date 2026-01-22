//! Platform-specific drag source abstraction
//!
//! Enables dragging files from Arclain to external applications (Explorer, etc.)
//! Uses native Windows COM APIs for deferred extraction with IProgressDialog,
//! or `drag` crate fallback for non-Windows platforms.

#[cfg(target_os = "windows")]
pub mod native_progress;
#[cfg(target_os = "windows")]
pub mod stream;
#[cfg(target_os = "windows")]
pub mod windows;
// pub mod windows_native; // Deprecated/Removed

use arclain_core::backends::sevenz_cli::ProgressUpdate;
use arclain_core::{ArchiveBackend, ArchiveEntry};
use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::sync::Arc;

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

/// Start a deferred drag operation using native Windows APIs.
///
/// Architecture (like 7-Zip):
/// 1. Show progress dialog while extracting in background
/// 2. After extraction completes, call DoDragDrop with temp files
///
/// This is simpler than trying to show progress during DoDragDrop.
#[cfg(target_os = "windows")]
pub fn start_deferred_drag(
    backend: Arc<dyn ArchiveBackend>,
    archive_path: PathBuf,
    entries: Vec<ArchiveEntry>,
    password: Option<String>,
) -> Result<Receiver<ProgressUpdate>, DragError> {
    // Use HDrop strategy (fast CF_HDROP-based transfer, like WinRAR/7-Zip)
    use windows::{start_drag, DragStrategy};

    match start_drag(
        backend,
        archive_path,
        entries,
        password,
        DragStrategy::HDrop,
    ) {
        Ok(rx) => Ok(rx),
        Err(e) => Err(DragError::PlatformError(e)),
    }
}

#[cfg(not(target_os = "windows"))]
pub fn start_deferred_drag(
    _backend: Arc<dyn ArchiveBackend>,
    _archive_path: PathBuf,
    _entries: Vec<ArchiveEntry>,
    _password: Option<String>,
) -> Result<Receiver<ProgressUpdate>, DragError> {
    Err(DragError::PlatformError(
        "Deferred drag not supported on this platform. Please extract first.".into(),
    ))
}
