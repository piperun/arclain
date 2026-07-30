//! Platform-specific drag source abstraction
//!
//! Enables dragging files from Arclain to external applications
//! (Explorer, etc.) using native Windows COM APIs with 7-Zip style
//! deferred extraction: the shell is handed a CF_HDROP data object whose
//! hover answer is a pre-built placeholder, and the dragged selection is
//! only staged onto disk -- through the application facade's drag-stage
//! surface, see [`payload`] -- at the moment the shell commits to a
//! drop. No archive I/O happens for a drag that hovers targets and never
//! drops.
//!
//! The former `FileContents`/`IStream` strategy (`LazyArchiveDataObject`
//! plus a chunk-pipe `IStream` that streamed bytes during extraction)
//! was removed in the facade cutover: it had been unreachable for as
//! long as `start_deferred_drag` has hardcoded the HDROP strategy, and
//! porting it would have meant building a facade streaming surface for
//! code nothing can select. If virtual drop targets (mail clients
//! accepting `FileContents`) are ever wanted, the shape to build is
//! stage-then-stream: stage through the same facade surface used here,
//! then serve `IStream` reads from the staged lease (whose bounded
//! `read_materialization_range` API already exists).

pub mod payload;

#[cfg(target_os = "windows")]
pub mod native_progress;
#[cfg(target_os = "windows")]
pub mod windows;

pub use payload::{
    DragPayloadSource, DragProgressUpdate, FacadeDragPayloadSource, StagedDragPayload,
};

use std::sync::mpsc::Sender;
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
/// `source` stages the dragged content on demand (drop time, never
/// hover time); `selection_paths` are the archive-root-relative paths
/// of the rows the user dragged, used only to shape the final HDROP's
/// top-level items. Progress lands on `progress_tx` (drained by the
/// per-frame drag-dialog updater); the channel disconnecting is the
/// "drag finished" signal, exactly as before the facade cutover.
#[cfg(target_os = "windows")]
pub fn start_deferred_drag(
    source: Arc<dyn DragPayloadSource>,
    selection_paths: Vec<String>,
    progress_tx: Sender<DragProgressUpdate>,
) -> Result<(), DragError> {
    if selection_paths.is_empty() {
        return Err(DragError::NoFiles);
    }
    windows::start_hdrop_drag(source, selection_paths, progress_tx)
        .map_err(DragError::PlatformError)
}

#[cfg(not(target_os = "windows"))]
pub fn start_deferred_drag(
    _source: Arc<dyn DragPayloadSource>,
    _selection_paths: Vec<String>,
    _progress_tx: Sender<DragProgressUpdate>,
) -> Result<(), DragError> {
    Err(DragError::PlatformError(
        "Deferred drag not supported on this platform. Please extract first.".into(),
    ))
}
