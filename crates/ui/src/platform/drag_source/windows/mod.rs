pub mod drop_source;
pub mod hdrop_data_object;

pub use drop_source::{DragState, DropSourceWithState};
pub use hdrop_data_object::HDropDataObject;

use std::sync::mpsc::Sender;
use std::sync::Arc;
use tracing::{info, warn};

use crate::platform::drag_source::payload::{DragPayloadSource, DragProgressUpdate};

use windows::Win32::Foundation::{DRAGDROP_S_CANCEL, DRAGDROP_S_DROP};
use windows::Win32::System::Com::IDataObject;
use windows::Win32::System::Ole::IDropSource;
use windows::Win32::System::Ole::{
    DoDragDrop, OleInitialize, OleUninitialize, DROPEFFECT_COPY, DROPEFFECT_MOVE, DROPEFFECT_NONE,
};

/// Start a CF_HDROP-based drag operation with 7-Zip style deferred
/// staging.
///
/// Dual-HDROP mechanism, unchanged by the facade cutover:
/// - During hover: returns an HDROP naming just a placeholder temp
///   folder (no staging, no facade calls)
/// - On drop: stages the selection through `source` (blocking this
///   drag thread, which is exactly the thread the shell is waiting
///   on), then returns an HDROP with the real staged paths
///
/// The spawned thread is a plain OS thread running an STA/OLE modal
/// loop -- foreign to the application's Tokio runtime by construction,
/// which is what makes the drop-time blocking facade call legal (see
/// `arclain_app`'s `stage_drag_payload_blocking` threading contract).
pub fn start_hdrop_drag(
    source: Arc<dyn DragPayloadSource>,
    selection_paths: Vec<String>,
    progress_tx: Sender<DragProgressUpdate>,
) -> std::result::Result<(), String> {
    let main_thread_id = unsafe { windows::Win32::System::Threading::GetCurrentThreadId() };

    std::thread::spawn(move || {
        info!("[hdrop] Background thread started (deferred staging mode)");

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
            source,
            selection_paths,
            Some(progress_tx.clone()),
            Arc::clone(&drag_state),
        )
        .into();

        // Create DropSourceWithState that shares the drag state
        let drop_source: IDropSource = DropSourceWithState::new(Arc::clone(&drag_state)).into();

        let mut effect = DROPEFFECT_NONE;

        info!("[HDROP] Calling DoDragDrop (deferred staging)...");

        let result = unsafe {
            DoDragDrop(
                &data_object,
                &drop_source,
                DROPEFFECT_COPY | DROPEFFECT_MOVE,
                &mut effect,
            )
        };

        info!("[HDROP] DoDragDrop returned: {:?}", result);

        if attached {
            unsafe {
                use windows::Win32::System::Threading::AttachThreadInput;
                let _ = AttachThreadInput(bg_thread_id, main_thread_id, false);
            }
        }

        let message = if result == DRAGDROP_S_DROP {
            "Drop complete".to_string()
        } else if result == DRAGDROP_S_CANCEL {
            "Cancelled".to_string()
        } else {
            format!("Failed: {:?}", result)
        };
        let _ = progress_tx.send(DragProgressUpdate {
            percent: 100,
            message: Some(message),
        });
        // `progress_tx` (and the data object's own clone, released with
        // it by the shell) dropping is what tells the per-frame updater
        // the drag is over.
    });

    Ok(())
}
