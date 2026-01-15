//! Native Windows drag-out with batch pre-extraction
//!
//! Uses IDataObject and IDropSource COM interfaces to implement
//! extraction to memory (HGLOBAL) - like 7-Zip File Manager does.
//!
//! OPTIMIZATION: When a drag starts, we pre-extract ALL requested files
//! to a temp directory in a single batch operation (using extract_files).
//! Then when Explorer requests each file via GetData, we simply read from disk.
//! This is MUCH faster than extracting files one-by-one per GetData call.
//!
//! PROGRESS DIALOG: Uses Windows Shell IProgressDialog to show extraction progress
//! during the drag operation. This works because IProgressDialog has its own
//! message pump, similar to how 7-Zip File Manager does it.

use arclain_core::{ArchiveBackend, ArchiveEntry};
use parking_lot::RwLock;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use tracing::{debug, info, warn};
use std::time::Instant;
use windows::core::{implement, Result, HRESULT, PCWSTR};

use windows::Win32::Foundation::{
    BOOL, DRAGDROP_S_CANCEL, DRAGDROP_S_DROP, DRAGDROP_S_USEDEFAULTCURSORS, DV_E_FORMATETC,
    DV_E_LINDEX, E_NOTIMPL, E_OUTOFMEMORY, E_UNEXPECTED, HWND, S_FALSE, S_OK,
};
use windows::Win32::Storage::FileSystem::FILE_ATTRIBUTE_NORMAL;
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, IAdviseSink, IDataObject, IEnumSTATDATA,
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, DATADIR_GET, DVASPECT_CONTENT, FORMATETC,
    STGMEDIUM, TYMED_HGLOBAL,
};
use windows::Win32::System::Memory::{
    GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE, GMEM_ZEROINIT,
};
use windows::Win32::System::Ole::{
    DoDragDrop, IDropSource, OleInitialize, OleUninitialize, DROPEFFECT_COPY, DROPEFFECT_MOVE,
    DROPEFFECT_NONE,
};
use windows::Win32::UI::Shell::{
    FD_ATTRIBUTES, FD_FILESIZE, FILEDESCRIPTORW, IProgressDialog,
    PROGDLG_AUTOTIME, PROGDLG_NOCANCEL, PROGDLG_NOMINIMIZE,
};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetForegroundWindow, PeekMessageW, TranslateMessage, MSG, PM_REMOVE,
};
use windows::core::GUID;

// CLSID for ProgressDialog
// {F8383852-FCD3-11d1-A6B9-006097DF5BD4}
const CLSID_PROGRESS_DIALOG: GUID = GUID::from_u128(0xF8383852_FCD3_11d1_A6B9_006097DF5BD4);

// Make DROPEFFECT public so mod.rs can use it in return type signature
pub use windows::Win32::System::Ole::DROPEFFECT;

// CF_HDROP not used in Stream mode, but we keep it for reference if needed
// const CF_HDROP: u16 = 15;

/// Register string format to get u16 ID
fn get_clipboard_format(name: &str) -> u16 {
    use windows::core::PCWSTR;
    let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        windows::Win32::System::DataExchange::RegisterClipboardFormatW(PCWSTR(wide.as_ptr())) as u16
    }
}

/// Enumerator for supported formats (FileDescriptor + FileContents)
#[implement(windows::Win32::System::Com::IEnumFORMATETC)]
pub struct FormatEnumerator {
    index: RwLock<usize>,
}

impl FormatEnumerator {
    fn new() -> Self {
        Self {
            index: RwLock::new(0),
        }
    }

    fn get_formats() -> Vec<FORMATETC> {
        let fd_format = get_clipboard_format("FileGroupDescriptorW");
        let fc_format = get_clipboard_format("FileContents");

        vec![
            // Metadata (HGLOBAL)
            FORMATETC {
                cfFormat: fd_format,
                ptd: std::ptr::null_mut(),
                dwAspect: DVASPECT_CONTENT.0,
                lindex: -1,
                tymed: TYMED_HGLOBAL.0 as u32,
            },
            // Content (HGLOBAL) - like 7-Zip, extract to memory
            FORMATETC {
                cfFormat: fc_format,
                ptd: std::ptr::null_mut(),
                dwAspect: DVASPECT_CONTENT.0,
                lindex: -1,
                tymed: TYMED_HGLOBAL.0 as u32,
            },
        ]
    }
}

impl windows::Win32::System::Com::IEnumFORMATETC_Impl for FormatEnumerator {
    fn Next(&self, celt: u32, rgelt: *mut FORMATETC, pceltfetched: *mut u32) -> HRESULT {
        let mut idx = self.index.write();
        let formats = Self::get_formats();
        let mut fetched = 0;

        if rgelt.is_null() {
            return E_UNEXPECTED;
        }

        unsafe {
            let rgelt = std::slice::from_raw_parts_mut(rgelt, celt as usize);

            for i in 0..celt as usize {
                if *idx < formats.len() {
                    rgelt[i] = formats[*idx];
                    *idx += 1;
                    fetched += 1;
                } else {
                    break;
                }
            }

            if !pceltfetched.is_null() {
                *pceltfetched = fetched as u32;
            }
        }

        if fetched == celt as usize {
            S_OK
        } else {
            S_FALSE
        }
    }

    fn Skip(&self, celt: u32) -> Result<()> {
        let mut idx = self.index.write();
        *idx += celt as usize;
        Ok(())
    }

    fn Reset(&self) -> Result<()> {
        let mut idx = self.index.write();
        *idx = 0;
        Ok(())
    }

    fn Clone(&self) -> windows::core::Result<windows::Win32::System::Com::IEnumFORMATETC> {
        let new_enum = FormatEnumerator::new();
        *new_enum.index.write() = *self.index.read();
        Ok(new_enum.into())
    }
}

/// State for batch extraction - extracted files are cached in a temp directory
struct ExtractionCache {
    /// Temp directory where files are extracted (auto-cleaned on drop via tempfile)
    temp_dir: tempfile::TempDir,
    /// Set to true once batch extraction is complete
    extracted: bool,
}

/// Helper to convert a Rust string to a null-terminated wide string
fn to_wide_string(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Shared state for the progress dialog running on a separate thread.
/// This allows the extraction thread to communicate progress to the dialog thread.
struct DialogState {
    /// Current file being extracted
    current_file: parking_lot::Mutex<String>,
    /// Number of files extracted so far
    current: AtomicUsize,
    /// Total number of files to extract
    total: AtomicUsize,
    /// Whether extraction is complete
    done: AtomicBool,
    /// Whether the dialog should close
    should_close: AtomicBool,
}

/// Extract files with a Windows Shell progress dialog.
///
/// This runs the progress dialog on a **separate thread** with its own COM apartment
/// and message pump. This is critical because:
/// 1. DoDragDrop blocks the main thread with a modal loop
/// 2. IProgressDialog needs a message pump to render and update
/// 3. By running on a separate thread, the dialog can update independently
///
/// This approach is similar to how 7-Zip handles progress dialogs during drag operations.
fn extract_with_progress_dialog(
    backend: Arc<dyn ArchiveBackend>,
    archive_path: &std::path::Path,
    dest_dir: &std::path::Path,
    file_paths: &[String],
    password: Option<&str>,
) -> std::result::Result<(), String> {
    let file_count = file_paths.len();
    
    // For small file counts, just extract directly without dialog
    if file_count <= 5 {
        info!("[drag] Small file count ({}), extracting without progress dialog", file_count);
        return backend
            .extract_files(archive_path, dest_dir, file_paths, password)
            .map_err(|e| format!("Extraction failed: {}", e));
    }
    
    info!("[drag] Starting extraction with IProgressDialog on separate thread for {} files", file_count);
    
    // Create shared state for communication between threads
    let dialog_state = Arc::new(DialogState {
        current_file: parking_lot::Mutex::new("Preparing...".to_string()),
        current: AtomicUsize::new(0),
        total: AtomicUsize::new(file_count),
        done: AtomicBool::new(false),
        should_close: AtomicBool::new(false),
    });
    
    // Clone for the dialog thread
    let dialog_state_clone = Arc::clone(&dialog_state);
    
    // Spawn a separate thread for the progress dialog with its own COM apartment
    let dialog_handle = std::thread::spawn(move || {
        // Initialize COM for this thread (STA required for shell dialogs)
        let com_init = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
        if com_init.is_err() {
            warn!("[drag] Failed to initialize COM on dialog thread: {:?}", com_init);
            return;
        }
        
        // Create the progress dialog on this thread
        let progress_dialog: IProgressDialog = match unsafe {
            CoCreateInstance(&CLSID_PROGRESS_DIALOG, None, CLSCTX_INPROC_SERVER)
        } {
            Ok(dialog) => dialog,
            Err(e) => {
                warn!("[drag] Failed to create progress dialog: {}", e);
                unsafe { CoUninitialize() };
                return;
            }
        };
        
        // Configure the dialog
        let title = to_wide_string("Extracting files for drag && drop");
        let cancel_msg = to_wide_string("Please wait...");
        
        unsafe {
            let _ = progress_dialog.SetTitle(PCWSTR(title.as_ptr()));
            let _ = progress_dialog.SetCancelMsg(PCWSTR(cancel_msg.as_ptr()), None);
            
            // Start the dialog WITHOUT modal flag (we have no parent window)
            // PROGDLG_NOCANCEL because batch extraction can't be cancelled mid-way
            // PROGDLG_NOMINIMIZE to keep it visible
            let flags = PROGDLG_AUTOTIME | PROGDLG_NOCANCEL | PROGDLG_NOMINIMIZE;
            
            // Use foreground window as parent to help with visibility
            let foreground = GetForegroundWindow();
            let parent = if foreground.0 != 0 { foreground } else { HWND::default() };
            
            if let Err(e) = progress_dialog.StartProgressDialog(parent, None, flags, None) {
                warn!("[drag] Failed to start progress dialog: {:?}", e);
                CoUninitialize();
                return;
            }
            
            // Try to bring dialog to foreground
            // The dialog window is created by the shell, we need to find it
            // Give it a moment to create
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        
        let line1 = to_wide_string("Extracting files...");
        let start_time = Instant::now();
        
        // Message pump loop
        loop {
            // Check if we should close
            if dialog_state_clone.should_close.load(Ordering::SeqCst) {
                break;
            }
            
            // Pump Windows messages
            unsafe {
                let mut msg = MSG::default();
                while PeekMessageW(&mut msg, HWND::default(), 0, 0, PM_REMOVE).as_bool() {
                    let _ = TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
            
            // Update progress
            let current = dialog_state_clone.current.load(Ordering::SeqCst);
            let total = dialog_state_clone.total.load(Ordering::SeqCst);
            let current_file = dialog_state_clone.current_file.lock().clone();
            let line2 = to_wide_string(&current_file);
            
            unsafe {
                let _ = progress_dialog.SetLine(1, PCWSTR(line1.as_ptr()), false, None);
                let _ = progress_dialog.SetLine(2, PCWSTR(line2.as_ptr()), false, None);
                let _ = progress_dialog.SetProgress(current as u32, total as u32);
            }
            
            // Brief sleep
            std::thread::sleep(std::time::Duration::from_millis(30));
            
            // Safety timeout
            if start_time.elapsed().as_secs() > 300 {
                warn!("[drag] Dialog thread timeout");
                break;
            }
        }
        
        // Stop the dialog
        unsafe {
            let _ = progress_dialog.StopProgressDialog();
            CoUninitialize();
        }
        
        debug!("[drag] Progress dialog thread finished");
    });
    
    // Give the dialog thread time to start and show the dialog
    std::thread::sleep(std::time::Duration::from_millis(100));
    
    // Do the extraction on the current thread
    *dialog_state.current_file.lock() = "Extracting...".to_string();
    
    let result = backend
        .extract_files(archive_path, dest_dir, file_paths, password)
        .map_err(|e| format!("Extraction failed: {}", e));
    
    // Signal the dialog to close
    dialog_state.current.store(file_count, Ordering::SeqCst);
    *dialog_state.current_file.lock() = "Complete".to_string();
    dialog_state.done.store(true, Ordering::SeqCst);
    dialog_state.should_close.store(true, Ordering::SeqCst);
    
    // Wait for dialog thread to finish
    let _ = dialog_handle.join();
    
    info!("[drag] Extraction with progress dialog completed");
    result
}

/// Entry for drag operation with both archive path and display path
#[derive(Debug, Clone)]
struct DragEntry {
    /// Full path in the archive (for extraction)
    archive_path: String,
    /// Display path for the file descriptor (relative to what user dragged)
    display_path: String,
    /// File size
    size: u64,
}

#[implement(windows::Win32::System::Com::IDataObject)]
pub struct LazyArchiveDataObject {
    backend: Arc<dyn ArchiveBackend>,
    archive_path: PathBuf,
    /// Original entries (for extraction)
    entries: Vec<ArchiveEntry>,
    /// Entries with display paths (for file descriptors) - excludes directories
    drag_entries: Vec<DragEntry>,
    password: Option<String>,
    /// Cache for batch-extracted files (lazily initialized on first GetData for FileContents)
    cache: RwLock<Option<ExtractionCache>>,
}

impl LazyArchiveDataObject {
    pub fn new(
        backend: Arc<dyn ArchiveBackend>,
        archive_path: PathBuf,
        entries: Vec<ArchiveEntry>,
        password: Option<String>,
    ) -> Self {
        // Compute the common prefix to strip from display paths
        // This makes dragging "folder/file.txt" display as "file.txt" if user is inside "folder"
        // Or if dragging a folder, show its contents relative to that folder
        let common_prefix = Self::compute_common_prefix(&entries);
        info!("[drag] Computed common prefix: {:?}", common_prefix);
        
        // Create drag entries, filtering out directories and stripping prefix
        let drag_entries: Vec<DragEntry> = entries
            .iter()
            .filter(|e| !e.is_dir)  // Skip directories - they cause merge prompts
            .map(|e| {
                let display_path = if let Some(ref prefix) = common_prefix {
                    // Strip the prefix and any leading slashes
                    let stripped = e.path.strip_prefix(prefix).unwrap_or(&e.path);
                    let stripped = stripped.trim_start_matches('/').trim_start_matches('\\');
                    stripped.to_string()
                } else {
                    e.path.clone()
                };
                
                debug!("[drag] Entry: archive='{}' -> display='{}'", e.path, display_path);
                
                DragEntry {
                    archive_path: e.path.clone(),
                    display_path,
                    size: e.size,
                }
            })
            .collect();
        
        info!("[drag] Created {} drag entries (filtered from {} archive entries)",
            drag_entries.len(), entries.len());
        
        Self {
            backend,
            archive_path,
            entries,
            drag_entries,
            password,
            cache: RwLock::new(None),
        }
    }
    
    /// Compute the common prefix to strip from display paths.
    ///
    /// Rules:
    /// - If dragging a single file: strip its parent directory (so "folder/file.txt" becomes "file.txt")
    /// - If dragging a folder (multiple files): keep the folder name as root (so files stay inside the folder)
    /// - If dragging multiple separate files: no prefix stripping
    fn compute_common_prefix(entries: &[ArchiveEntry]) -> Option<String> {
        if entries.is_empty() {
            return None;
        }
        
        // Find all file entries (not directories)
        let file_entries: Vec<_> = entries.iter().filter(|e| !e.is_dir).collect();
        if file_entries.is_empty() {
            return None;
        }
        
        // Check if we have a single file or multiple files
        if file_entries.len() == 1 {
            // Single file: strip the entire parent path
            let first = &file_entries[0].path;
            let sep_pos = first.rfind(|c| c == '/' || c == '\\');
            return match sep_pos {
                Some(pos) => Some(first[..=pos].to_string()),  // Include the separator
                None => None,  // No directory part to strip
            };
        }
        
        // Multiple files: Check if they're all in the same folder
        // Find the deepest common directory
        let first = &file_entries[0].path;
        
        // Get the first directory component (e.g., "folder/" from "folder/subfolder/file.txt")
        let first_sep = first.find(|c| c == '/' || c == '\\');
        let first_dir = match first_sep {
            Some(pos) => &first[..=pos],  // Include the separator
            None => return None,  // No directory part
        };
        
        // Check if all files share this first directory
        for entry in file_entries.iter().skip(1) {
            if !entry.path.starts_with(first_dir) {
                // Files are in different root directories, no common prefix
                return None;
            }
        }
        
        // All files share the same root folder - DON'T strip it
        // This keeps the folder structure when dragging a folder
        None
    }

    /// Ensure all files are extracted to the temp directory.
    /// This is called once on the first FileContents request.
    /// Uses IProgressDialog to show a native Windows progress dialog during extraction.
    fn ensure_extracted(&self) -> std::result::Result<(), String> {
        // Check if already extracted
        {
            let cache = self.cache.read();
            if cache.as_ref().map(|c| c.extracted).unwrap_or(false) {
                debug!("[drag] Already extracted, skipping");
                return Ok(());
            }
        }

        // Need to extract - take write lock
        let mut cache = self.cache.write();
        
        // Double-check after acquiring write lock
        if cache.as_ref().map(|c| c.extracted).unwrap_or(false) {
            debug!("[drag] Already extracted (after lock), skipping");
            return Ok(());
        }

        let start = Instant::now();
        
        // Create temp directory
        let temp_dir = tempfile::tempdir()
            .map_err(|e| format!("Failed to create temp dir: {}", e))?;
        
        info!("[drag] Temp dir created at: {:?}", temp_dir.path());
        
        // Collect file paths to extract (skip directories)
        let file_paths: Vec<String> = self.entries
            .iter()
            .filter(|e| !e.is_dir)
            .map(|e| e.path.clone())
            .collect();
        
        let file_count = file_paths.len();
        info!(
            "[drag] Batch extracting {} files to temp dir: {:?}",
            file_count, file_paths
        );

        // Use IProgressDialog for extraction with progress
        // This shows a native Windows progress dialog that works even during DoDragDrop
        extract_with_progress_dialog(
            Arc::clone(&self.backend),
            &self.archive_path,
            temp_dir.path(),
            &file_paths,
            self.password.as_deref(),
        )?;

        let elapsed = start.elapsed();
        info!(
            "[drag] Batch extraction complete: {} files in {:.2}s ({:.1} files/sec)",
            file_count,
            elapsed.as_secs_f64(),
            file_count as f64 / elapsed.as_secs_f64()
        );
        
        // Verify files were extracted
        for path in &file_paths {
            let full_path = temp_dir.path().join(path);
            if full_path.exists() {
                if let Ok(meta) = std::fs::metadata(&full_path) {
                    debug!("[drag] Extracted file verified: {:?} ({} bytes)", full_path, meta.len());
                }
            } else {
                warn!("[drag] Extracted file NOT FOUND: {:?}", full_path);
            }
        }

        *cache = Some(ExtractionCache {
            temp_dir,
            extracted: true,
        });

        Ok(())
    }

    /// Get the path to an extracted file in the temp directory
    fn get_extracted_path(&self, entry_path: &str) -> Option<PathBuf> {
        let cache = self.cache.read();
        cache.as_ref().map(|c| c.temp_dir.path().join(entry_path))
    }

    fn get_file_descriptor(&self) -> windows::core::Result<STGMEDIUM> {
        // Use drag_entries which have display paths and exclude directories
        info!("[drag] get_file_descriptor: {} drag entries", self.drag_entries.len());
        for (i, entry) in self.drag_entries.iter().enumerate() {
            debug!(
                "[drag]   [{:3}] display='{}' archive='{}' size={}",
                i, entry.display_path, entry.archive_path, entry.size
            );
        }

        // Create FILEGROUPDESCRIPTORW
        let count = self.drag_entries.len();
        let header_size = std::mem::size_of::<u32>();
        let item_size = std::mem::size_of::<FILEDESCRIPTORW>();
        let total_size = header_size + (count * item_size);

        let hglobal = unsafe { GlobalAlloc(GMEM_MOVEABLE | GMEM_ZEROINIT, total_size)? };
        let ptr = unsafe { GlobalLock(hglobal) };
        if ptr.is_null() {
            return Err(windows::core::Error::from(E_UNEXPECTED)); // Out of memory
        }

        unsafe {
            // Write count
            *(ptr as *mut u32) = count as u32;

            let descriptors_ptr = ptr.add(header_size) as *mut FILEDESCRIPTORW;

            for (i, entry) in self.drag_entries.iter().enumerate() {
                // Use display_path for the file name (stripped of common prefix)
                let name = &entry.display_path;
                let name_wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
                let mut name_arr = [0u16; 260];
                let copy_len = std::cmp::min(name_wide.len(), 259);
                name_arr[..copy_len].copy_from_slice(&name_wide[..copy_len]);

                // All drag entries are files (directories are filtered out)
                let dw_flags = FD_ATTRIBUTES.0 as u32 | FD_FILESIZE.0 as u32;
                let attributes = FILE_ATTRIBUTE_NORMAL.0;
                let size_low = entry.size as u32;
                let size_high = (entry.size >> 32) as u32;
                
                debug!(
                    "[drag] descriptor: display='{}' size={} flags=ATTR|SIZE",
                    name, entry.size
                );

                // Construct descriptor locally
                let descriptor = FILEDESCRIPTORW {
                    dwFlags: dw_flags,
                    dwFileAttributes: attributes,
                    nFileSizeLow: size_low,
                    nFileSizeHigh: size_high,
                    cFileName: name_arr,
                    ..Default::default()
                };

                std::ptr::write_unaligned(descriptors_ptr.add(i), descriptor);
            }

            let _ = GlobalUnlock(hglobal);
        }

        Ok(STGMEDIUM {
            tymed: TYMED_HGLOBAL.0 as u32,
            u: windows::Win32::System::Com::STGMEDIUM_0 { hGlobal: hglobal },
            pUnkForRelease: std::mem::ManuallyDrop::new(None),
        })
    }

    fn get_file_contents(&self, lindex: i32) -> windows::core::Result<STGMEDIUM> {
        // Use drag_entries for lindex lookup (matches file descriptor indices)
        if lindex < 0 || lindex as usize >= self.drag_entries.len() {
            warn!("[drag] get_file_contents: invalid lindex={} (drag_entries.len={})", lindex, self.drag_entries.len());
            return Err(windows::core::Error::from(DV_E_LINDEX));
        }

        let drag_entry = &self.drag_entries[lindex as usize];
        info!(
            "[drag] get_file_contents: lindex={} display='{}' archive='{}' size={}",
            lindex,
            drag_entry.display_path,
            drag_entry.archive_path,
            drag_entry.size
        );

        // All drag entries are files (directories are filtered out in constructor)

        // Ensure batch extraction is done (this is a no-op after the first call)
        debug!("[drag] Calling ensure_extracted...");
        if let Err(e) = self.ensure_extracted() {
            warn!("[drag] Batch extraction failed: {}", e);
            return Err(windows::core::Error::from(E_UNEXPECTED));
        }
        debug!("[drag] ensure_extracted completed successfully");

        // Read file from temp directory using archive_path (the path used during extraction)
        let extracted_path = self.get_extracted_path(&drag_entry.archive_path)
            .ok_or_else(|| {
                warn!("[drag] get_extracted_path returned None for '{}'", drag_entry.archive_path);
                windows::core::Error::from(E_UNEXPECTED)
            })?;
        
        debug!("[drag] Reading extracted file from: {:?}", extracted_path);
        
        let buffer = match std::fs::read(&extracted_path) {
            Ok(data) => {
                info!("[drag] Read {} bytes from {:?}", data.len(), extracted_path);
                data
            }
            Err(e) => {
                warn!(
                    "[drag] Failed to read extracted file '{}': {}",
                    extracted_path.display(), e
                );
                return Err(windows::core::Error::from(E_UNEXPECTED));
            }
        };

        // Handle empty files - allocate at least 1 byte to avoid issues
        let alloc_size = if buffer.is_empty() { 1 } else { buffer.len() };
        
        debug!(
            "[drag] Allocating HGLOBAL: {} bytes for '{}' (buffer.len={})",
            alloc_size, drag_entry.display_path, buffer.len()
        );

        // Allocate HGLOBAL and copy data
        let hglobal = unsafe {
            GlobalAlloc(GMEM_MOVEABLE | GMEM_ZEROINIT, alloc_size)
                .map_err(|e| {
                    warn!("[drag] GlobalAlloc failed: {:?}", e);
                    windows::core::Error::from(E_OUTOFMEMORY)
                })?
        };
        
        let ptr = unsafe { GlobalLock(hglobal) };
        if ptr.is_null() {
            warn!("[drag] GlobalLock failed for {} bytes", alloc_size);
            return Err(windows::core::Error::from(E_OUTOFMEMORY));
        }

        if !buffer.is_empty() {
            unsafe {
                std::ptr::copy_nonoverlapping(buffer.as_ptr(), ptr as *mut u8, buffer.len());
            }
        }
        
        unsafe {
            let _ = GlobalUnlock(hglobal);
        }

        info!("[drag] Prepared HGLOBAL for '{}' ({} bytes)", drag_entry.display_path, buffer.len());

        Ok(STGMEDIUM {
            tymed: TYMED_HGLOBAL.0 as u32,
            u: windows::Win32::System::Com::STGMEDIUM_0 { hGlobal: hglobal },
            pUnkForRelease: std::mem::ManuallyDrop::new(None),
        })
    }
}

#[allow(non_snake_case)]
impl windows::Win32::System::Com::IDataObject_Impl for LazyArchiveDataObject {
    fn GetData(&self, pformatetc: *const FORMATETC) -> windows::core::Result<STGMEDIUM> {
        let format = unsafe { &*pformatetc };

        let fd_format = get_clipboard_format("FileGroupDescriptorW");
        let fc_format = get_clipboard_format("FileContents");

        debug!(
            "[drag] GetData: cfFormat={} tymed={} lindex={}",
            format.cfFormat, format.tymed, format.lindex
        );

        if format.cfFormat == fd_format && (format.tymed & TYMED_HGLOBAL.0 as u32) != 0 {
            debug!("[drag] Returning FileGroupDescriptorW");
            self.get_file_descriptor()
        } else if format.cfFormat == fc_format && (format.tymed & TYMED_HGLOBAL.0 as u32) != 0 {
            debug!("[drag] Returning FileContents as HGLOBAL for lindex={}", format.lindex);
            self.get_file_contents(format.lindex)
        } else {
            debug!("[drag] Unsupported format: cfFormat={} tymed={}", format.cfFormat, format.tymed);
            Err(windows::core::Error::from(DV_E_FORMATETC))
        }
    }

    fn GetDataHere(
        &self,
        _pformatetc: *const FORMATETC,
        _pmedium: *mut STGMEDIUM,
    ) -> windows::core::Result<()> {
        Err(windows::core::Error::from(E_NOTIMPL))
    }

    fn QueryGetData(&self, pformatetc: *const FORMATETC) -> HRESULT {
        let format = unsafe { &*pformatetc };

        let fd_format = get_clipboard_format("FileGroupDescriptorW");
        let fc_format = get_clipboard_format("FileContents");

        if format.cfFormat == fd_format || format.cfFormat == fc_format {
            S_OK
        } else {
            // S_FALSE? Or DV_E_FORMATETC?
            // QueryGetData returns S_OK on success, or error code.
            // S_FALSE is sometimes used for "format not supported"?
            // Docs: "S_OK if the request is supported... DV_E_FORMATETC if not"
            windows::core::Error::from(DV_E_FORMATETC).into()
        }
    }

    fn GetCanonicalFormatEtc(
        &self,
        _pformatectin: *const FORMATETC,
        _pformatetcout: *mut FORMATETC,
    ) -> HRESULT {
        windows::core::Error::from(E_NOTIMPL).into()
    }

    fn SetData(
        &self,
        _pformatetc: *const FORMATETC,
        _pmedium: *const STGMEDIUM,
        _frelease: BOOL,
    ) -> windows::core::Result<()> {
        Err(windows::core::Error::from(E_NOTIMPL))
    }

    fn EnumFormatEtc(
        &self,
        dwdirection: u32,
    ) -> windows::core::Result<windows::Win32::System::Com::IEnumFORMATETC> {
        if dwdirection == DATADIR_GET.0 as u32 {
            Ok(FormatEnumerator::new().into())
        } else {
            Err(windows::core::Error::from(E_NOTIMPL))
        }
    }

    fn DAdvise(
        &self,
        _pformatetc: *const FORMATETC,
        _advf: u32,
        _padvsink: Option<&IAdviseSink>,
    ) -> windows::core::Result<u32> {
        Err(windows::core::Error::from(E_NOTIMPL))
    }

    fn DUnadvise(&self, _dwconnection: u32) -> windows::core::Result<()> {
        Err(windows::core::Error::from(E_NOTIMPL))
    }

    fn EnumDAdvise(&self) -> windows::core::Result<IEnumSTATDATA> {
        Err(windows::core::Error::from(E_NOTIMPL))
    }
}

/// Simple drop source that tracks drag state
#[implement(windows::Win32::System::Ole::IDropSource)]
pub struct SimpleDropSource;

impl windows::Win32::System::Ole::IDropSource_Impl for SimpleDropSource {
    fn QueryContinueDrag(
        &self,
        fescapepressed: BOOL,
        grfkeystate: windows::Win32::System::SystemServices::MODIFIERKEYS_FLAGS,
    ) -> HRESULT {
        if fescapepressed.as_bool() {
            DRAGDROP_S_CANCEL
        } else if (grfkeystate.0 & windows::Win32::System::SystemServices::MK_LBUTTON.0) == 0 {
            DRAGDROP_S_DROP
        } else {
            S_OK
        }
    }

    fn GiveFeedback(&self, _dweffect: DROPEFFECT) -> HRESULT {
        DRAGDROP_S_USEDEFAULTCURSORS
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
) -> std::result::Result<DROPEFFECT, String> {
    unsafe {
        let _ = OleInitialize(None);
    }

    struct OleGuard;
    impl Drop for OleGuard {
        fn drop(&mut self) {
            unsafe { OleUninitialize() };
        }
    }
    let _ole_guard = OleGuard;

    let data_object: IDataObject =
        LazyArchiveDataObject::new(backend, archive_path, entries, password).into();
    let drop_source: IDropSource = SimpleDropSource.into();

    let mut effect = DROPEFFECT_NONE;

    tracing::debug!("Starting stream-based drag operation...");

    let result = unsafe {
        DoDragDrop(
            &data_object,
            &drop_source,
            DROPEFFECT_COPY | DROPEFFECT_MOVE,
            &mut effect,
        )
    };

    if result == DRAGDROP_S_DROP {
        tracing::debug!("Drag completed with effect: {:?}", effect);
        Ok(effect)
    } else if result == DRAGDROP_S_CANCEL {
        tracing::debug!("Drag cancelled");
        Ok(effect)
    } else {
        tracing::warn!("Drag failed with HRESULT: {:?}", result);
        Err(format!("HRESULT: {:?}", result))
    }
}
