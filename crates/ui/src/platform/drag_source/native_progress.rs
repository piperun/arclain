//! Native Windows IProgressDialog wrapper for extraction progress
//!
//! Uses the Windows Shell Progress Dialog (IProgressDialog) which manages
//! its own UI thread internally, allowing it to update while the main thread
//! does blocking work (like extraction).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tracing::{debug, info, warn};

use windows::core::{GUID, PCWSTR};
use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER,
    COINIT_APARTMENTTHREADED,
};
use windows::Win32::UI::Shell::{IProgressDialog, PROGDLG_AUTOTIME, PROGDLG_NOMINIMIZE};

// CLSID for ProgressDialog - {F8383852-FCD3-11d1-A6B9-006097DF5BD4}
const CLSID_PROGRESSDIALOG: GUID = GUID::from_u128(0xF8383852_FCD3_11d1_A6B9_006097DF5BD4);

/// Wrapper around native Windows IProgressDialog
pub struct NativeProgressDialog {
    dialog: IProgressDialog,
    cancelled: Arc<AtomicBool>,
}

impl NativeProgressDialog {
    /// Create and show a new progress dialog
    pub fn new(title: &str, total_files: u32) -> Result<Self, String> {
        unsafe {
            // Initialize COM for this thread (if not already)
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);

            // Create the progress dialog COM object using the CLSID
            let dialog: IProgressDialog =
                CoCreateInstance(&CLSID_PROGRESSDIALOG, None, CLSCTX_INPROC_SERVER)
                    .map_err(|e| format!("Failed to create IProgressDialog: {:?}", e))?;

            // Set the title
            let title_wide: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
            dialog
                .SetTitle(PCWSTR(title_wide.as_ptr()))
                .map_err(|e| format!("Failed to set title: {:?}", e))?;

            // Set the line texts
            let line1 = format!("Extracting {} files...", total_files);
            let line1_wide: Vec<u16> = line1.encode_utf16().chain(std::iter::once(0)).collect();
            dialog
                .SetLine(1, PCWSTR(line1_wide.as_ptr()), false, None)
                .map_err(|e| format!("Failed to set line 1: {:?}", e))?;

            // Start the dialog - this creates the window on the current thread's message pump
            // PROGDLG_AUTOTIME: automatically updates time remaining
            let flags = PROGDLG_AUTOTIME | PROGDLG_NOMINIMIZE;
            dialog
                .StartProgressDialog(HWND::default(), None, flags, None)
                .map_err(|e| format!("Failed to start dialog: {:?}", e))?;

            let cancelled = Arc::new(AtomicBool::new(false));

            info!("[native_progress] Dialog started");

            Ok(Self { dialog, cancelled })
        }
    }

    /// Update progress with current file info
    pub fn update(&self, current: u32, total: u32, current_file: &str) {
        unsafe {
            // Set progress (0-100 scale, or we can use actual counts)
            // IProgressDialog::SetProgress takes current and max as u32
            let _ = self.dialog.SetProgress(current, total);

            // Update line 2 with current file
            let line2_wide: Vec<u16> = current_file
                .encode_utf16()
                .chain(std::iter::once(0))
                .collect();
            let _ = self
                .dialog
                .SetLine(2, PCWSTR(line2_wide.as_ptr()), true, None);

            // Check if user clicked cancel
            if self.dialog.HasUserCancelled().as_bool() {
                self.cancelled.store(true, Ordering::SeqCst);
            }
        }
    }

    /// Check if the user has clicked Cancel
    pub fn is_cancelled(&self) -> bool {
        unsafe {
            if self.dialog.HasUserCancelled().as_bool() {
                self.cancelled.store(true, Ordering::SeqCst);
                true
            } else {
                self.cancelled.load(Ordering::SeqCst)
            }
        }
    }

    /// Get the cancellation token for use with extraction
    pub fn cancel_token(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.cancelled)
    }

    /// Close the dialog
    pub fn close(self) {
        unsafe {
            let _ = self.dialog.StopProgressDialog();
            info!("[native_progress] Dialog closed");
        }
    }
}

impl Drop for NativeProgressDialog {
    fn drop(&mut self) {
        unsafe {
            let _ = self.dialog.StopProgressDialog();
            CoUninitialize();
        }
    }
}

/// Stage a drag payload with a native Windows progress dialog.
///
/// The fallback progress UI for drags started without a frontend
/// progress channel: shows a native Windows progress dialog
/// (IProgressDialog) while the payload source stages the dragged
/// selection. Staging runs on a spawned worker thread (a plain OS
/// thread -- satisfying `DragPayloadSource::stage_blocking`'s
/// non-runtime-thread contract) and streams `DragProgressUpdate`s back
/// through a channel; the calling thread (which owns the COM-bound
/// dialog) drains the channel and forwards updates to the dialog.
/// Cancellation is wired through the source: the dialog's cancel button
/// triggers `DragPayloadSource::request_cancel`, which cancels the
/// facade's staging operation and unblocks the worker with an error.
///
/// If creating the dialog fails (e.g. headless / RDP weirdness) we fall
/// back to staging without progress UI.
pub fn stage_with_native_progress(
    source: std::sync::Arc<dyn crate::platform::drag_source::DragPayloadSource>,
    item_count: usize,
) -> Result<crate::platform::drag_source::StagedDragPayload, String> {
    // For very small selections, skip the dialog entirely — the COM
    // round-trip + worker-thread setup costs more than it is worth, and
    // the pre-facade code took the same shortcut.
    if item_count <= 2 {
        debug!(
            "[native_progress] Small selection ({}), staging without dialog",
            item_count
        );
        return source.stage_blocking(&mut |_| {});
    }

    info!(
        "[native_progress] Staging {} dragged items with native dialog",
        item_count
    );

    // Create the progress dialog (apartment-threaded COM, lives on this
    // thread). If creation fails we still want the staging to run.
    let dialog = match NativeProgressDialog::new(
        &format!("Extracting {} items", item_count),
        item_count as u32,
    ) {
        Ok(d) => Some(d),
        Err(e) => {
            warn!(
                "[native_progress] Failed to create dialog: {}, continuing without",
                e
            );
            None
        }
    };

    // No dialog → no point spinning up a worker thread to feed nothing.
    // Just stage synchronously on this thread.
    let Some(ref d) = dialog else {
        return source.stage_blocking(&mut |_| {});
    };

    d.update(0, 100, "Starting extraction...");

    let (tx, rx) = std::sync::mpsc::channel::<crate::platform::drag_source::DragProgressUpdate>();

    let source_for_worker = std::sync::Arc::clone(&source);
    let handle = std::thread::spawn(move || {
        let mut cb = move |p: crate::platform::drag_source::DragProgressUpdate| {
            // Silent on send failure — the pump only stops listening once
            // staging has already finished.
            let _ = tx.send(p);
        };
        source_for_worker.stage_blocking(&mut cb)
    });

    // This thread drains progress events and forwards them to the
    // dialog. 50ms timeout keeps cancellation responsive
    // (HasUserCancelled is touched every loop iteration via
    // `d.is_cancelled()`).
    let mut cancel_requested = false;
    loop {
        match rx.recv_timeout(std::time::Duration::from_millis(50)) {
            Ok(p) => {
                d.update(
                    u32::from(p.percent),
                    100,
                    p.message.as_deref().unwrap_or("Extracting..."),
                );
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                if d.is_cancelled() && !cancel_requested {
                    debug!("[native_progress] User cancelled — cancelling the staging operation");
                    cancel_requested = true;
                    source.request_cancel();
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                break;
            }
        }
    }

    let worker_result = match handle.join() {
        Ok(r) => r,
        Err(_) => return Err("Staging worker thread panicked".to_string()),
    };

    if worker_result.is_ok() {
        d.update(100, 100, "Complete!");
    }

    if let Some(d) = dialog {
        d.close();
    }

    worker_result
}
