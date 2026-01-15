//! Native Windows IProgressDialog wrapper for extraction progress
//!
//! Uses the Windows Shell Progress Dialog (IProgressDialog) which manages
//! its own UI thread internally, allowing it to update while the main thread
//! does blocking work (like extraction).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tracing::{debug, info, warn};

use windows::core::{GUID, PCWSTR};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize,
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED,
};
use windows::Win32::UI::Shell::{
    IProgressDialog,
    PROGDLG_AUTOTIME, PROGDLG_NOMINIMIZE,
};
use windows::Win32::Foundation::HWND;

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
            let dialog: IProgressDialog = CoCreateInstance(
                &CLSID_PROGRESSDIALOG,
                None,
                CLSCTX_INPROC_SERVER,
            ).map_err(|e| format!("Failed to create IProgressDialog: {:?}", e))?;
            
            // Set the title
            let title_wide: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
            dialog.SetTitle(PCWSTR(title_wide.as_ptr()))
                .map_err(|e| format!("Failed to set title: {:?}", e))?;
            
            // Set the line texts
            let line1 = format!("Extracting {} files...", total_files);
            let line1_wide: Vec<u16> = line1.encode_utf16().chain(std::iter::once(0)).collect();
            dialog.SetLine(1, PCWSTR(line1_wide.as_ptr()), false, None)
                .map_err(|e| format!("Failed to set line 1: {:?}", e))?;
            
            // Start the dialog - this creates the window on the current thread's message pump
            // PROGDLG_AUTOTIME: automatically updates time remaining
            let flags = PROGDLG_AUTOTIME | PROGDLG_NOMINIMIZE;
            dialog.StartProgressDialog(HWND::default(), None, flags, None)
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
            let line2_wide: Vec<u16> = current_file.encode_utf16().chain(std::iter::once(0)).collect();
            let _ = self.dialog.SetLine(2, PCWSTR(line2_wide.as_ptr()), true, None);
            
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
/// This shows a native Windows progress dialog (IProgressDialog) during extraction.
/// The dialog is managed by Windows and updates properly even during blocking operations.
/// 
/// Note: Since IProgressDialog is not Send/Sync (COM objects are thread-bound),
/// we don't use the `extract_files_with_progress` method. Instead, we fall back to
/// the simple extraction for now. The dialog at least shows "Extracting X files..."
/// 
/// TODO: To get per-file progress updates, we would need to:
/// 1. Extract files one-by-one in a loop with dialog.update() calls, OR
/// 2. Use a different threading model (spawn extraction in a thread, poll from main)
pub fn extract_with_native_progress(
    backend: std::sync::Arc<dyn arclain_core::ArchiveBackend>,
    archive_path: &std::path::Path,
    dest_dir: &std::path::Path,
    file_paths: &[String],
    password: Option<&str>,
) -> Result<(), String> {
    let file_count = file_paths.len();
    
    // For very small file counts, skip the dialog
    if file_count <= 2 {
        debug!("[native_progress] Small file count ({}), extracting without dialog", file_count);
        return backend
            .extract_files(archive_path, dest_dir, file_paths, password)
            .map_err(|e| format!("Extraction failed: {}", e));
    }
    
    info!("[native_progress] Starting extraction of {} files with native dialog", file_count);
    
    // Create and show the progress dialog
    let dialog = match NativeProgressDialog::new(
        &format!("Extracting {} files", file_count),
        file_count as u32,
    ) {
        Ok(d) => Some(d),
        Err(e) => {
            warn!("[native_progress] Failed to create dialog: {}, continuing without", e);
            None
        }
    };
    
    // Set initial progress
    if let Some(ref d) = dialog {
        d.update(0, file_count as u32, "Starting extraction...");
    }
    
    // Do the extraction (simple method - no per-file progress callback)
    // The dialog will at least show that extraction is in progress
    let result = backend.extract_files(archive_path, dest_dir, file_paths, password)
        .map_err(|e| format!("Extraction failed: {}", e));
    
    // Update dialog to show completion
    if let Some(ref d) = dialog {
        d.update(file_count as u32, file_count as u32, "Complete!");
    }
    
    // Close the dialog
    if let Some(d) = dialog {
        d.close();
    }
    
    result
}
