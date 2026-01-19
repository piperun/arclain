pub mod data_object;
pub mod drop_source;
pub mod types;
pub mod utils;

pub use data_object::*;
pub use drop_source::*;
pub use types::*;
pub use utils::*;

use arclain_core::backends::sevenz_cli::ProgressUpdate;
use arclain_core::{ArchiveBackend, ArchiveEntry};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tracing::{info, warn};

use windows::Win32::Foundation::{DRAGDROP_S_CANCEL, DRAGDROP_S_DROP};
use windows::Win32::System::Com::IDataObject;
use windows::Win32::System::Ole::IDropSource;
use windows::Win32::System::Ole::{
    DoDragDrop, OleInitialize, OleUninitialize, DROPEFFECT_COPY, DROPEFFECT_MOVE, DROPEFFECT_NONE,
};

/// Global flag indicating an outgoing drag operation is in progress.
/// Used to detect and reject our own drops (P3: show "NO" cursor effect).
pub static OUTGOING_DRAG_ACTIVE: AtomicBool = AtomicBool::new(false);

/// Check if an outgoing drag operation is currently active.
pub fn is_outgoing_drag_active() -> bool {
    OUTGOING_DRAG_ACTIVE.load(Ordering::SeqCst)
}

/// Strategy for drag-and-drop data transfer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DragStrategy {
    /// IStream-based transfer (FileContents format).
    /// Slower for multi-file, but works with virtual drop targets.
    IStream,

    /// CF_HDROP-based transfer (file paths).
    /// Fast for multi-file (Explorer does filesystem copy).
    /// This is what WinRAR/7-Zip use.
    #[default]
    HDrop,
}

/// Start a drag operation with the specified strategy.
///
/// - `HDrop` (default): Pre-extracts to temp, returns CF_HDROP. Fast.
/// - `IStream`: Returns FileContents via IStream. Slower but more compatible.
pub fn start_drag(
    backend: Arc<dyn ArchiveBackend>,
    archive_path: PathBuf,
    entries: Vec<ArchiveEntry>,
    password: Option<String>,
    strategy: DragStrategy,
) -> std::result::Result<std::sync::mpsc::Receiver<ProgressUpdate>, String> {
    match strategy {
        DragStrategy::HDrop => start_hdrop_drag(backend, archive_path, entries, password),
        DragStrategy::IStream => start_deferred_drag(backend, archive_path, entries, password),
    }
}

/// Start a deferred drag operation with batch pre-extraction.
///
/// Files are extracted to a temp directory using a single batch operation,
/// with a native Windows IProgressDialog shown during extraction.
/// This is MUCH faster than extracting files one-by-one.
pub fn start_deferred_drag(
    backend: Arc<dyn ArchiveBackend>,
    archive_path: PathBuf,
    entries: Vec<ArchiveEntry>,
    password: Option<String>,
) -> std::result::Result<std::sync::mpsc::Receiver<ProgressUpdate>, String> {
    let (tx, rx) = std::sync::mpsc::channel();

    // Capture main thread ID to attach input later
    let main_thread_id = unsafe { windows::Win32::System::Threading::GetCurrentThreadId() };

    // Spawn background thread for drag operation (must be STA)
    std::thread::spawn(move || {
        info!("[drag] Background thread started");

        // Force creation of message queue
        unsafe {
            use windows::Win32::Foundation::HWND;
            use windows::Win32::UI::WindowsAndMessaging::{PeekMessageW, MSG, PM_NOREMOVE};
            let mut msg = MSG::default();
            let _ = PeekMessageW(&mut msg, HWND::default(), 0, 0, PM_NOREMOVE);
        }

        unsafe {
            // OleInitialize returns HRESULT, not Result
            // It expects Option<*const c_void>. None is null.
            let reserved: Option<*const std::ffi::c_void> = None;
            if OleInitialize(reserved).is_err() {
                warn!("[drag] OleInitialize failed");
            }
        }

        struct OleGuard;
        impl Drop for OleGuard {
            fn drop(&mut self) {
                unsafe { OleUninitialize() };
            }
        }
        let _ole_guard = OleGuard;

        // Attach input to main thread so DoDragDrop can receive mouse events
        let bg_thread_id = unsafe { windows::Win32::System::Threading::GetCurrentThreadId() };
        let attached = unsafe {
            use windows::Win32::System::Threading::AttachThreadInput;
            AttachThreadInput(bg_thread_id, main_thread_id, true).as_bool()
        };

        if attached {
            info!(
                "[drag] Attached thread input to main thread ({} -> {})",
                bg_thread_id, main_thread_id
            );
        } else {
            warn!("[drag] Failed to attach thread input");
        }

        // Create data object
        let data_object: IDataObject =
            LazyArchiveDataObject::new(backend, archive_path, entries, password, Some(tx.clone()))
                .into();
        let drop_source: IDropSource = DropSource.into();

        let mut effect = DROPEFFECT_NONE;

        info!("[DRAG] Calling DoDragDrop (blocking on background thread)...");

        let result = unsafe {
            DoDragDrop(
                &data_object,
                &drop_source,
                DROPEFFECT_COPY | DROPEFFECT_MOVE,
                &mut effect,
            )
        };

        info!("[DRAG] DoDragDrop returned with result: {:?}", result);

        // Detach input
        if attached {
            unsafe {
                use windows::Win32::System::Threading::AttachThreadInput;
                let _ = AttachThreadInput(bg_thread_id, main_thread_id, false);
            }
            info!("[DRAG] Detached thread input");
        }

        if result == DRAGDROP_S_DROP {
            tracing::debug!("[DRAG] Drag completed with effect: {:?}", effect);
            let _ = tx.send(ProgressUpdate {
                percent: 100,
                message: Some("Drop complete".to_string()),
            });
        } else if result == DRAGDROP_S_CANCEL {
            tracing::debug!("[DRAG] Drag cancelled");
            let _ = tx.send(ProgressUpdate {
                percent: 100,
                message: Some("Cancelled".to_string()),
            });
        } else {
            tracing::warn!("[DRAG] Drag failed with HRESULT: {:?}", result);
            let _ = tx.send(ProgressUpdate {
                percent: 100,
                message: Some(format!("Failed: {:?}", result)),
            });
        }
    });

    Ok(rx)
}

pub mod hdrop_data_object;
pub use drop_source::{DragState, DropSourceWithState};
pub use hdrop_data_object::HDropDataObject;

/// Start a CF_HDROP-based drag operation with 7-Zip style deferred extraction.
///
/// Uses dual-HDROP mechanism:
/// - During hover: Returns HDROP with just temp folder (no extraction)
/// - On drop: Extracts files, returns HDROP with real paths
pub fn start_hdrop_drag(
    backend: Arc<dyn ArchiveBackend>,
    archive_path: PathBuf,
    entries: Vec<ArchiveEntry>,
    password: Option<String>,
) -> std::result::Result<std::sync::mpsc::Receiver<ProgressUpdate>, String> {
    let (tx, rx) = std::sync::mpsc::channel();

    let main_thread_id = unsafe { windows::Win32::System::Threading::GetCurrentThreadId() };

    std::thread::spawn(move || {
        info!("[hdrop] Background thread started (deferred extraction mode)");

        unsafe {
            use windows::Win32::Foundation::HWND;
            use windows::Win32::UI::WindowsAndMessaging::{PeekMessageW, MSG, PM_NOREMOVE};
            let mut msg = MSG::default();
            let _ = PeekMessageW(&mut msg, HWND::default(), 0, 0, PM_NOREMOVE);
        }

        unsafe {
            let reserved: Option<*const std::ffi::c_void> = None;
            if OleInitialize(reserved).is_err() {
                warn!("[hdrop] OleInitialize failed");
            }
        }

        struct OleGuard;
        impl Drop for OleGuard {
            fn drop(&mut self) {
                unsafe { OleUninitialize() };
            }
        }
        let _ole_guard = OleGuard;

        let bg_thread_id = unsafe { windows::Win32::System::Threading::GetCurrentThreadId() };
        let attached = unsafe {
            use windows::Win32::System::Threading::AttachThreadInput;
            AttachThreadInput(bg_thread_id, main_thread_id, true).as_bool()
        };

        if attached {
            info!(
                "[hdrop] Attached thread input ({} -> {})",
                bg_thread_id, main_thread_id
            );
        } else {
            warn!("[hdrop] Failed to attach thread input");
        }

        // Create shared drag state
        let drag_state = DragState::new();

        // Create HDropDataObject with drag state
        let data_object: IDataObject = HDropDataObject::new(
            backend,
            archive_path,
            entries,
            password,
            Some(tx.clone()),
            Arc::clone(&drag_state),
        )
        .into();

        // Create DropSourceWithState that shares the drag state
        let drop_source: IDropSource = DropSourceWithState::new(Arc::clone(&drag_state)).into();

        let mut effect = DROPEFFECT_NONE;

        // TODO: To show "NO" cursor when hovering over arclain during outgoing drag,
        // we would need to either:
        // 1. Implement our own IDropTarget that checks for our data format and rejects it
        // 2. Use RevokeDragDrop (but this permanently breaks file drops from Explorer)
        // 3. Modify eframe/winit to expose custom drop target registration
        // For now, egui's built-in drop target accepts our CF_HDROP, showing "copy" cursor.

        info!("[HDROP] Calling DoDragDrop (deferred extraction)...");

        // Set flag to indicate outgoing drag is active (for P3: reject own drops)
        OUTGOING_DRAG_ACTIVE.store(true, Ordering::SeqCst);

        let result = unsafe {
            DoDragDrop(
                &data_object,
                &drop_source,
                DROPEFFECT_COPY | DROPEFFECT_MOVE,
                &mut effect,
            )
        };

        // Clear the flag now that drag is complete
        OUTGOING_DRAG_ACTIVE.store(false, Ordering::SeqCst);

        info!("[HDROP] DoDragDrop returned: {:?}", result);

        if attached {
            unsafe {
                use windows::Win32::System::Threading::AttachThreadInput;
                let _ = AttachThreadInput(bg_thread_id, main_thread_id, false);
            }
        }

        if result == DRAGDROP_S_DROP {
            let _ = tx.send(ProgressUpdate {
                percent: 100,
                message: Some("Drop complete".to_string()),
            });
        } else if result == DRAGDROP_S_CANCEL {
            let _ = tx.send(ProgressUpdate {
                percent: 100,
                message: Some("Cancelled".to_string()),
            });
        } else {
            let _ = tx.send(ProgressUpdate {
                percent: 100,
                message: Some(format!("Failed: {:?}", result)),
            });
        }

        // Explicitly drop to ensure channel disconnects
        drop(data_object);
        drop(drop_source);
        drop(tx);
    });

    Ok(rx)
}
