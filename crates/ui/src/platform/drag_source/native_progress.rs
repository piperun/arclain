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

/// Extract files with a native Windows progress dialog
///
/// Shows a native Windows progress dialog (IProgressDialog) during extraction.
/// Per-file progress is delivered by running the extraction on a worker thread
/// and streaming `ExtractionProgress` events back through a channel; the main
/// thread (which owns the COM-bound dialog) drains the channel and forwards
/// updates to the dialog. Cancellation is wired both directions: the dialog's
/// own cancel button flips an `Arc<AtomicBool>` that the worker honors via the
/// backend's `CancellationToken`.
///
/// If creating the dialog fails (e.g. headless / RDP weirdness) we fall back
/// to the plain `extract_files` call without progress UI.
pub fn extract_with_native_progress(
    backend: std::sync::Arc<dyn arclain_core::ArchiveBackend>,
    archive_path: &std::path::Path,
    dest_dir: &std::path::Path,
    file_paths: &[String],
    password: Option<&str>,
) -> Result<(), String> {
    let file_count = file_paths.len();

    // For very small file counts, skip the dialog entirely — the COM
    // round-trip + worker-thread setup costs more than the extraction itself.
    if file_count <= 2 {
        debug!(
            "[native_progress] Small file count ({}), extracting without dialog",
            file_count
        );
        return backend
            .extract_files(archive_path, dest_dir, file_paths, password)
            .map_err(|e| format!("Extraction failed: {}", e));
    }

    info!(
        "[native_progress] Starting extraction of {} files with native dialog",
        file_count
    );

    // Create the progress dialog (apartment-threaded COM, lives on this
    // thread). If creation fails we still want the extraction to run.
    let dialog = match NativeProgressDialog::new(
        &format!("Extracting {} files", file_count),
        file_count as u32,
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

    if let Some(ref d) = dialog {
        d.update(0, file_count as u32, "Starting extraction...");
    }

    // No dialog → no point spinning up a worker thread to feed nothing.
    // Just run synchronously.
    let Some(ref d) = dialog else {
        return backend
            .extract_files(archive_path, dest_dir, file_paths, password)
            .map_err(|e| format!("Extraction failed: {}", e));
    };

    let cancel_token: arclain_core::CancellationToken = d.cancel_token();

    // The ProgressCallback trait requires `Send + Sync`, but mpsc::Sender
    // is `!Sync`. Wrap it in a Mutex so the closure satisfies the bounds.
    // Only one thread (the worker) ever calls it, so contention is nil.
    let (tx, rx) = std::sync::mpsc::channel::<arclain_core::ExtractionProgress>();
    let tx_for_worker = std::sync::Mutex::new(tx);

    // Move owned copies of the borrowed args into the worker. `backend`
    // is already an Arc; clone it for the thread.
    let archive_path_owned = archive_path.to_path_buf();
    let dest_dir_owned = dest_dir.to_path_buf();
    let file_paths_owned: Vec<String> = file_paths.to_vec();
    let password_owned: Option<String> = password.map(|s| s.to_string());
    let backend_for_worker = std::sync::Arc::clone(&backend);
    let cancel_for_worker = std::sync::Arc::clone(&cancel_token);

    let handle = std::thread::spawn(move || {
        let cb = move |p: arclain_core::ExtractionProgress| {
            // Silent on send failure — main thread will have stopped
            // listening only if it bailed out (e.g. cancel), and the
            // worker already sees that via `cancel_for_worker`.
            let _ = tx_for_worker.lock().unwrap().send(p);
        };
        backend_for_worker.extract_files_with_progress(
            &archive_path_owned,
            &dest_dir_owned,
            &file_paths_owned,
            password_owned.as_deref(),
            Some(&cb),
            Some(&cancel_for_worker),
        )
    });

    // Main thread drains progress events and forwards them to the dialog.
    // 50ms timeout keeps cancellation responsive (HasUserCancelled is
    // touched every loop iteration via `d.is_cancelled()`).
    loop {
        match rx.recv_timeout(std::time::Duration::from_millis(50)) {
            Ok(p) => {
                d.update(p.current as u32, p.total as u32, &p.current_file);
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                // Touch the dialog to pick up cancel-button presses. The
                // call updates `cancel_token` which the worker checks
                // between files.
                if d.is_cancelled() {
                    debug!("[native_progress] User cancelled — signalling worker");
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                break;
            }
        }
    }

    let worker_result = match handle.join() {
        Ok(r) => r,
        Err(_) => return Err("Extraction worker thread panicked".to_string()),
    };
    let result = worker_result.map_err(|e| format!("Extraction failed: {}", e));

    if result.is_ok() {
        d.update(file_count as u32, file_count as u32, "Complete!");
    }

    if let Some(d) = dialog {
        d.close();
    }

    result
}
