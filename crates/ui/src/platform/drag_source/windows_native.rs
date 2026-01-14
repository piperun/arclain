//! Native Windows drag-out with batch pre-extraction
//!
//! Uses IDataObject and IDropSource COM interfaces to implement
//! extraction to memory (HGLOBAL) - like 7-Zip File Manager does.
//!
//! OPTIMIZATION: When a drag starts, we pre-extract ALL requested files
//! to a temp directory in a single batch operation (using extract_files_with_progress).
//! Then when Explorer requests each file via GetData, we simply read from disk.
//! This is MUCH faster than extracting files one-by-one per GetData call.
//!
//! Progress callback support allows the UI to show an extraction modal during drag.

use super::DragProgressCallback;
use arclain_core::{ArchiveBackend, ArchiveEntry, ExtractionProgress};
use parking_lot::RwLock;

use std::path::PathBuf;
use std::sync::Arc;
use tracing::{debug, info, warn};
use std::time::Instant;
use windows::core::{implement, Result, HRESULT};

use windows::Win32::Foundation::{
    BOOL, DRAGDROP_S_CANCEL, DRAGDROP_S_DROP, DRAGDROP_S_USEDEFAULTCURSORS, DV_E_FORMATETC,
    DV_E_LINDEX, E_NOTIMPL, E_OUTOFMEMORY, E_UNEXPECTED, S_FALSE, S_OK,
};
use windows::Win32::Storage::FileSystem::FILE_ATTRIBUTE_NORMAL;
use windows::Win32::System::Com::{
    IAdviseSink, IDataObject, IEnumSTATDATA, DATADIR_GET, DVASPECT_CONTENT, FORMATETC, STGMEDIUM,
    TYMED_HGLOBAL,
};
use windows::Win32::System::Memory::{
    GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE, GMEM_ZEROINIT,
};
use windows::Win32::System::Ole::{
    DoDragDrop, IDropSource, OleInitialize, OleUninitialize, DROPEFFECT_COPY, DROPEFFECT_MOVE,
    DROPEFFECT_NONE,
};
use windows::Win32::UI::Shell::{FD_ATTRIBUTES, FD_FILESIZE, FILEDESCRIPTORW};

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

#[implement(windows::Win32::System::Com::IDataObject)]
pub struct LazyArchiveDataObject {
    backend: Arc<dyn ArchiveBackend>,
    archive_path: PathBuf,
    entries: Vec<ArchiveEntry>,
    password: Option<String>,
    /// Cache for batch-extracted files (lazily initialized on first GetData for FileContents)
    cache: RwLock<Option<ExtractionCache>>,
    /// Optional progress callback for extraction updates
    progress_callback: Option<DragProgressCallback>,
}

impl LazyArchiveDataObject {
    pub fn new(
        backend: Arc<dyn ArchiveBackend>,
        archive_path: PathBuf,
        entries: Vec<ArchiveEntry>,
        password: Option<String>,
    ) -> Self {
        Self::with_progress(backend, archive_path, entries, password, None)
    }

    pub fn with_progress(
        backend: Arc<dyn ArchiveBackend>,
        archive_path: PathBuf,
        entries: Vec<ArchiveEntry>,
        password: Option<String>,
        progress_callback: Option<DragProgressCallback>,
    ) -> Self {
        Self {
            backend,
            archive_path,
            entries,
            password,
            cache: RwLock::new(None),
            progress_callback,
        }
    }

    /// Ensure all files are extracted to the temp directory.
    /// This is called once on the first FileContents request.
    fn ensure_extracted(&self) -> std::result::Result<(), String> {
        // Check if already extracted
        {
            let cache = self.cache.read();
            if cache.as_ref().map(|c| c.extracted).unwrap_or(false) {
                return Ok(());
            }
        }

        // Need to extract - take write lock
        let mut cache = self.cache.write();
        
        // Double-check after acquiring write lock
        if cache.as_ref().map(|c| c.extracted).unwrap_or(false) {
            return Ok(());
        }

        let start = Instant::now();
        
        // Create temp directory
        let temp_dir = tempfile::tempdir()
            .map_err(|e| format!("Failed to create temp dir: {}", e))?;
        
        // Collect file paths to extract (skip directories)
        let file_paths: Vec<String> = self.entries
            .iter()
            .filter(|e| !e.is_dir)
            .map(|e| e.path.clone())
            .collect();
        
        let file_count = file_paths.len();
        info!(
            "[drag] Batch extracting {} files to temp dir...",
            file_count
        );

        // Send initial progress update if callback is set
        if let Some(ref callback) = self.progress_callback {
            callback(ExtractionProgress {
                current: 0,
                total: file_count,
                current_file: "Starting extraction...".to_string(),
                percent: 0,
            });
        }

        // Use extract_files_with_progress if we have a callback, otherwise use simple extract_files
        if let Some(callback) = self.progress_callback.clone() {
            // Create a progress callback wrapper that bridges to our DragProgressCallback
            // Clone the Arc so the closure owns it and has 'static lifetime
            let progress_cb = move |progress: ExtractionProgress| {
                callback(progress);
            };
            
            self.backend
                .extract_files_with_progress(
                    &self.archive_path,
                    temp_dir.path(),
                    &file_paths,
                    self.password.as_deref(),
                    Some(&progress_cb),
                    None, // No cancellation support for now
                )
                .map_err(|e| format!("Batch extraction failed: {}", e))?;
        } else {
            // No progress callback - use simple extraction
            self.backend
                .extract_files(
                    &self.archive_path,
                    temp_dir.path(),
                    &file_paths,
                    self.password.as_deref(),
                )
                .map_err(|e| format!("Batch extraction failed: {}", e))?;
        }

        let elapsed = start.elapsed();
        info!(
            "[drag] Batch extraction complete: {} files in {:.2}s ({:.1} files/sec)",
            file_count,
            elapsed.as_secs_f64(),
            file_count as f64 / elapsed.as_secs_f64()
        );

        // Send completion progress update if callback is set
        if let Some(ref callback) = self.progress_callback {
            callback(ExtractionProgress {
                current: file_count,
                total: file_count,
                current_file: "Extraction complete".to_string(),
                percent: 100,
            });
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
        // Log all entries being advertised
        info!("[drag] get_file_descriptor: {} entries", self.entries.len());
        for (i, entry) in self.entries.iter().enumerate() {
            debug!(
                "[drag]   [{:3}] path='{}' is_dir={} size={}",
                i, entry.path, entry.is_dir, entry.size
            );
        }

        // Create FILEGROUPDESCRIPTORW
        let count = self.entries.len();
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

            for (i, entry) in self.entries.iter().enumerate() {
                // Filename buffer
                let name = &entry.path;
                let name_wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
                let mut name_arr = [0u16; 260];
                let copy_len = std::cmp::min(name_wide.len(), 259);
                name_arr[..copy_len].copy_from_slice(&name_wide[..copy_len]);

                let mut dw_flags = FD_ATTRIBUTES.0 as u32;
                let mut attributes = FILE_ATTRIBUTE_NORMAL.0;
                let mut size_low = 0u32;
                let mut size_high = 0u32;

                if entry.is_dir {
                    attributes = windows::Win32::Storage::FileSystem::FILE_ATTRIBUTE_DIRECTORY.0;
                    debug!(
                        "[drag] descriptor (dir): name='{}' flags=ATTRIBUTES attr=DIR",
                        name
                    );
                } else {
                    dw_flags |= FD_FILESIZE.0 as u32;
                    size_low = entry.size as u32;
                    size_high = (entry.size >> 32) as u32;
                    debug!(
                        "[drag] descriptor (file): name='{}' size={} flags=ATTR|SIZE",
                        name, entry.size
                    );
                }

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

            let _ = GlobalUnlock(hglobal); // Unlock for safety, though dragging might need it?
                                           // STGMEDIUM usually takes ownership of HGLOBAL, caller handles lock/unlock?
                                           // Actually ReleaseStgMedium frees it.
        }

        Ok(STGMEDIUM {
            tymed: TYMED_HGLOBAL.0 as u32,
            u: windows::Win32::System::Com::STGMEDIUM_0 { hGlobal: hglobal },
            pUnkForRelease: std::mem::ManuallyDrop::new(None),
        })
    }

    fn get_file_contents(&self, lindex: i32) -> windows::core::Result<STGMEDIUM> {
        if lindex < 0 || lindex as usize >= self.entries.len() {
            return Err(windows::core::Error::from(DV_E_LINDEX));
        }

        let entry = &self.entries[lindex as usize];
        debug!(
            "[drag] request FileContents lindex={} name='{}' dir={} size={}",
            lindex,
            entry.path,
            entry.is_dir,
            entry.size
        );

        // Directories have no content; Explorer should not request content when FD_FILESIZE
        // is absent, but if it does, signal unsupported format for this item.
        if entry.is_dir {
            debug!("[drag] Skipping directory entry");
            return Err(windows::core::Error::from(DV_E_FORMATETC));
        }

        // Ensure batch extraction is done (this is a no-op after the first call)
        if let Err(e) = self.ensure_extracted() {
            warn!("[drag] Batch extraction failed: {}", e);
            return Err(windows::core::Error::from(E_UNEXPECTED));
        }

        // Read file from temp directory (fast disk read)
        let extracted_path = self.get_extracted_path(&entry.path)
            .ok_or_else(|| windows::core::Error::from(E_UNEXPECTED))?;
        
        let buffer = match std::fs::read(&extracted_path) {
            Ok(data) => data,
            Err(e) => {
                warn!(
                    "[drag] Failed to read extracted file '{}': {}",
                    extracted_path.display(), e
                );
                return Err(windows::core::Error::from(E_UNEXPECTED));
            }
        };

        debug!(
            "[drag] Read {} bytes from temp file for '{}'",
            buffer.len(),
            entry.path
        );

        // Allocate HGLOBAL and copy data
        let size = buffer.len();
        let hglobal = unsafe {
            GlobalAlloc(GMEM_MOVEABLE | GMEM_ZEROINIT, size)
                .map_err(|_| windows::core::Error::from(E_OUTOFMEMORY))?
        };
        
        let ptr = unsafe { GlobalLock(hglobal) };
        if ptr.is_null() {
            warn!("[drag] GlobalLock failed for {} bytes", size);
            return Err(windows::core::Error::from(E_OUTOFMEMORY));
        }

        unsafe {
            std::ptr::copy_nonoverlapping(buffer.as_ptr(), ptr as *mut u8, size);
            let _ = GlobalUnlock(hglobal);
        }

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

/// Start a deferred drag operation using IStream (without progress callback)
pub fn start_deferred_drag(
    backend: Arc<dyn ArchiveBackend>,
    archive_path: PathBuf,
    entries: Vec<ArchiveEntry>,
    password: Option<String>,
) -> std::result::Result<DROPEFFECT, String> {
    start_deferred_drag_with_progress(backend, archive_path, entries, password, None)
}

/// Start a deferred drag operation using IStream with optional progress callback
///
/// The progress callback is invoked during batch extraction to report progress.
/// Note: Due to DoDragDrop blocking the UI thread, progress updates may not render
/// in the egui modal during extraction. The callback is still useful for logging
/// or future async implementations.
pub fn start_deferred_drag_with_progress(
    backend: Arc<dyn ArchiveBackend>,
    archive_path: PathBuf,
    entries: Vec<ArchiveEntry>,
    password: Option<String>,
    progress: Option<DragProgressCallback>,
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
        LazyArchiveDataObject::with_progress(backend, archive_path, entries, password, progress).into();
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
