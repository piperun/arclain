//! CF_HDROP-based drag data object with 7-Zip style deferred extraction.
//!
//! This implementation uses the same mechanism as 7-Zip:
//! 1. During drag/hover: Returns HDROP with just the temp folder path
//! 2. On actual drop: Extracts files, then returns HDROP with real paths
//!
//! This ensures smooth drag cursor and extraction only on drop.

use super::drop_source::DragState;
use super::types::ExtractionCache;
use arclain_core::backends::sevenz_cli::ProgressUpdate;
use arclain_core::{ArchiveBackend, ArchiveEntry};
use parking_lot::RwLock;
use std::os::windows::ffi::OsStrExt;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::time::Instant;
use tracing::{debug, info, warn};
use windows::core::{implement, HRESULT};
use windows::Win32::Foundation::HGLOBAL;
use windows::Win32::Foundation::{BOOL, DV_E_FORMATETC, E_NOTIMPL, E_UNEXPECTED, S_FALSE, S_OK};
use windows::Win32::System::Com::{
    IAdviseSink, IEnumSTATDATA, DATADIR_GET, DVASPECT_CONTENT, FORMATETC, STGMEDIUM, TYMED_HGLOBAL,
};
use windows::Win32::System::Memory::{
    GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE, GMEM_ZEROINIT,
};
use windows::Win32::UI::Shell::DROPFILES;

const CF_HDROP: u16 = 15;

/// Enumerator for supported formats (CF_HDROP only)
#[implement(windows::Win32::System::Com::IEnumFORMATETC)]
pub struct HDropFormatEnumerator {
    index: RwLock<usize>,
}

impl HDropFormatEnumerator {
    pub fn new() -> Self {
        Self {
            index: RwLock::new(0),
        }
    }

    fn get_formats() -> Vec<FORMATETC> {
        vec![FORMATETC {
            cfFormat: CF_HDROP,
            ptd: std::ptr::null_mut(),
            dwAspect: DVASPECT_CONTENT.0,
            lindex: -1,
            tymed: TYMED_HGLOBAL.0 as u32,
        }]
    }
}

impl windows::Win32::System::Com::IEnumFORMATETC_Impl for HDropFormatEnumerator {
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

    fn Skip(&self, celt: u32) -> windows::core::Result<()> {
        *self.index.write() += celt as usize;
        Ok(())
    }

    fn Reset(&self) -> windows::core::Result<()> {
        *self.index.write() = 0;
        Ok(())
    }

    fn Clone(&self) -> windows::core::Result<windows::Win32::System::Com::IEnumFORMATETC> {
        let new_enum = HDropFormatEnumerator::new();
        *new_enum.index.write() = *self.index.read();
        Ok(new_enum.into())
    }
}

/// CF_HDROP-based data object with 7-Zip style deferred extraction.
///
/// Uses dual-HDROP mechanism:
/// - `hdrop_pre`: Contains just the temp folder (returned during hover)
/// - `hdrop_final`: Contains actual file paths (returned after extraction)
#[implement(windows::Win32::System::Com::IDataObject)]
pub struct HDropDataObject {
    backend: Arc<dyn ArchiveBackend>,
    archive_path: PathBuf,
    entries: Vec<ArchiveEntry>,
    password: Option<String>,
    cache: RwLock<Option<ExtractionCache>>,
    progress_tx: Option<Sender<ProgressUpdate>>,

    /// Shared state with DropSourceWithState
    drag_state: Arc<DragState>,

    /// Pre-built HDROP with just temp folder (for hover)
    hdrop_pre: RwLock<Option<HGLOBAL>>,

    /// Pre-built HDROP with final file paths (for after extraction)
    hdrop_final: RwLock<Option<HGLOBAL>>,

    /// Temp directory path (created immediately, not extracted yet)
    temp_dir_path: RwLock<Option<PathBuf>>,
}

impl HDropDataObject {
    pub fn new(
        backend: Arc<dyn ArchiveBackend>,
        archive_path: PathBuf,
        entries: Vec<ArchiveEntry>,
        password: Option<String>,
        progress_tx: Option<Sender<ProgressUpdate>>,
        drag_state: Arc<DragState>,
    ) -> Self {
        info!(
            "[hdrop] Creating HDropDataObject for {} entries (deferred extraction)",
            entries.len()
        );

        let mut obj = Self {
            backend,
            archive_path,
            entries,
            password,
            cache: RwLock::new(None),
            progress_tx,
            drag_state,
            hdrop_pre: RwLock::new(None),
            hdrop_final: RwLock::new(None),
            temp_dir_path: RwLock::new(None),
        };

        // Build the pre-HDROP immediately (just temp folder path)
        if let Err(e) = obj.build_pre_hdrop() {
            warn!("[hdrop] Failed to build pre-HDROP: {}", e);
        }

        obj
    }

    /// Build the "pre" HDROP containing just the temp folder path.
    /// This is returned during hover so Explorer doesn't see missing files.
    fn build_pre_hdrop(&mut self) -> Result<(), String> {
        // Create temp directory
        let temp_dir =
            tempfile::tempdir().map_err(|e| format!("Failed to create temp dir: {}", e))?;
        let temp_path = temp_dir.path().to_path_buf();

        info!("[hdrop] Created temp dir for pre-HDROP: {:?}", temp_path);

        // Build HDROP with just the temp folder
        let hdrop = self.build_hdrop_for_paths(&[temp_path.clone()])?;

        // Store temp dir (keeps it alive) and HDROP
        *self.temp_dir_path.write() = Some(temp_path);
        *self.hdrop_pre.write() = Some(hdrop);

        // Store the TempDir in cache to keep it alive
        // Leaking the directory (guard=None) is required to fix race condition with Explorer copy
        let path = temp_dir.into_path();
        *self.cache.write() = Some(ExtractionCache {
            temp_dir_path: path,
            _guard: None,
            extracted: false,
        });

        Ok(())
    }

    /// Build HDROP structure from a list of paths.
    fn build_hdrop_for_paths(&self, paths: &[PathBuf]) -> Result<HGLOBAL, String> {
        let header_size = std::mem::size_of::<DROPFILES>();

        // Calculate total size for wide strings
        let mut strings_size = 0usize;
        for path in paths {
            let wide: Vec<u16> = path
                .as_os_str()
                .encode_wide()
                .chain(std::iter::once(0))
                .collect();
            strings_size += wide.len() * 2;
        }
        strings_size += 2; // Double null terminator

        let total_size = header_size + strings_size;

        let hglobal = unsafe {
            GlobalAlloc(GMEM_MOVEABLE | GMEM_ZEROINIT, total_size)
                .map_err(|e| format!("GlobalAlloc failed: {:?}", e))?
        };

        let ptr = unsafe { GlobalLock(hglobal) };
        if ptr.is_null() {
            // Note: Memory leak on error, but this is rare and acceptable
            return Err("GlobalLock failed".to_string());
        }

        unsafe {
            let dropfiles = ptr as *mut DROPFILES;
            (*dropfiles).pFiles = header_size as u32;
            (*dropfiles).fWide = BOOL::from(true);

            let mut string_ptr = (ptr as *mut u8).add(header_size) as *mut u16;
            for path in paths {
                let wide: Vec<u16> = path
                    .as_os_str()
                    .encode_wide()
                    .chain(std::iter::once(0))
                    .collect();
                std::ptr::copy_nonoverlapping(wide.as_ptr(), string_ptr, wide.len());
                string_ptr = string_ptr.add(wide.len());
            }

            let _ = GlobalUnlock(hglobal);
        }

        Ok(hglobal)
    }

    /// Extract files and build the "final" HDROP.
    fn do_extraction(&self) -> Result<(), String> {
        // Check if already extracted
        {
            let cache_guard = self.cache.read();
            if cache_guard.as_ref().map(|c| c.extracted).unwrap_or(false) {
                return Ok(()); // Already extracted
            }
            if cache_guard.is_none() {
                return Err("No temp dir".to_string());
            }
        }

        let start = Instant::now();
        let temp_dir = self
            .cache
            .read()
            .as_ref()
            .map(|c| c.temp_dir_path.clone())
            .unwrap();
        info!("[hdrop] Starting extraction to temp: {:?}", temp_dir);

        let file_paths: Vec<String> = self
            .entries
            .iter()
            .filter(|e| !e.is_dir)
            .map(|e| e.path.clone())
            .collect();

        let file_count = file_paths.len();

        if let Some(tx) = &self.progress_tx {
            let _ = tx.send(ProgressUpdate {
                percent: 0,
                message: Some(format!("Extracting {} entries...", file_count)),
            });
        }

        // Build EntryRefs from our entries
        let entry_refs: Vec<arclain_core::EntryRef<'_>> = self
            .entries
            .iter()
            .map(|e| arclain_core::EntryRef::from(e))
            .collect();

        // Use unified extract() - handles both "all" and "selected" with proper progress
        if let Some(tx) = &self.progress_tx {
            let tx_clone = tx.clone();
            self.backend
                .extract(
                    &self.archive_path,
                    &temp_dir,
                    Some(&entry_refs),
                    self.password.as_deref(),
                    Some(&move |p| {
                        let _ = tx_clone.send(ProgressUpdate {
                            percent: p.percent,
                            message: Some(format!(
                                "Extracting ({}/{}): {}",
                                p.current, p.total, p.current_file
                            )),
                        });
                    }),
                    None, // No cancellation for now
                )
                .map_err(|e| format!("extract failed: {}", e))?;
        } else {
            // No progress channel - still use unified extract
            self.backend
                .extract(
                    &self.archive_path,
                    &temp_dir,
                    Some(&entry_refs),
                    self.password.as_deref(),
                    None,
                    None,
                )
                .map_err(|e| format!("extract failed: {}", e))?;
        }

        if let Some(tx) = &self.progress_tx {
            let _ = tx.send(ProgressUpdate {
                percent: 100,
                message: Some("Extraction complete".to_string()),
            });
        }

        info!(
            "[hdrop] Extraction complete in {:.2}s",
            start.elapsed().as_secs_f64()
        );

        // Mark as extracted
        if let Some(cache) = self.cache.write().as_mut() {
            cache.extracted = true;
        }

        // Now build the final HDROP with actual file paths
        self.build_final_hdrop()?;

        Ok(())
    }

    /// Build the "final" HDROP with actual extracted file paths.
    fn build_final_hdrop(&self) -> Result<(), String> {
        let cache_guard = self.cache.read();
        let temp_dir = cache_guard
            .as_ref()
            .map(|c| c.temp_dir_path.as_path())
            .ok_or("No temp dir")?;

        // Build HDROP paths from actual selected entries
        // Each selected file/folder gets its own entry in HDROP
        let mut hdrop_paths: Vec<PathBuf> = Vec::new();

        for entry in &self.entries {
            // Skip directories - their contents are already included as files
            // Unless the directory itself was explicitly selected
            if entry.is_dir {
                // For directories, only include if it doesn't have any child entries selected
                // (i.e., user selected the folder, not individual files in it)
                let has_child_selected = self
                    .entries
                    .iter()
                    .any(|e| !e.is_dir && e.path.starts_with(&format!("{}/", entry.path)));
                if has_child_selected {
                    continue; // Skip folder, its children are already selected
                }
            }

            let file_path = temp_dir.join(&entry.path);
            if file_path.exists() {
                hdrop_paths.push(file_path);
            }
        }

        // Deduplicate in case of overlapping paths
        hdrop_paths.sort();
        hdrop_paths.dedup();

        info!(
            "[hdrop] Building final HDROP with {} items",
            hdrop_paths.len()
        );

        let hdrop = self.build_hdrop_for_paths(&hdrop_paths)?;
        *self.hdrop_final.write() = Some(hdrop);

        Ok(())
    }

    /// Get the appropriate HDROP based on current drag state.
    fn get_hdrop(&self) -> windows::core::Result<STGMEDIUM> {
        let use_pre = self.drag_state.use_pre_global.load(Ordering::SeqCst);
        let need_extract = self.drag_state.need_extract.load(Ordering::SeqCst);
        let extract_done = self.drag_state.extract_done.load(Ordering::SeqCst);

        debug!(
            "[hdrop] GetData: use_pre={}, need_extract={}, extract_done={}",
            use_pre, need_extract, extract_done
        );

        // Check if we need to extract (drop was triggered)
        if need_extract && !extract_done {
            info!("[hdrop] Drop detected - starting extraction");
            if let Err(e) = self.do_extraction() {
                warn!("[hdrop] Extraction failed: {}", e);
                return Err(windows::core::Error::from(E_UNEXPECTED));
            }
            self.drag_state.extract_done.store(true, Ordering::SeqCst);
        }

        // Return appropriate HDROP
        let hglobal = if use_pre {
            debug!("[hdrop] Returning pre-HDROP (temp folder only)");
            self.hdrop_pre.read().ok_or_else(|| {
                warn!("[hdrop] Pre-HDROP not built");
                windows::core::Error::from(E_UNEXPECTED)
            })?
        } else {
            debug!("[hdrop] Returning final HDROP (extracted files)");
            self.hdrop_final.read().ok_or_else(|| {
                warn!("[hdrop] Final HDROP not built");
                windows::core::Error::from(E_UNEXPECTED)
            })?
        };

        // Duplicate the HGLOBAL for the caller
        let dup_hglobal = self.duplicate_hglobal(hglobal)?;

        Ok(STGMEDIUM {
            tymed: TYMED_HGLOBAL.0 as u32,
            u: windows::Win32::System::Com::STGMEDIUM_0 {
                hGlobal: dup_hglobal,
            },
            pUnkForRelease: std::mem::ManuallyDrop::new(None),
        })
    }

    /// Duplicate an HGLOBAL (required because caller takes ownership).
    fn duplicate_hglobal(&self, src: HGLOBAL) -> windows::core::Result<HGLOBAL> {
        unsafe {
            let size = windows::Win32::System::Memory::GlobalSize(src);
            if size == 0 {
                return Err(windows::core::Error::from(E_UNEXPECTED));
            }

            let src_ptr = GlobalLock(src);
            if src_ptr.is_null() {
                return Err(windows::core::Error::from(E_UNEXPECTED));
            }

            let dest = GlobalAlloc(GMEM_MOVEABLE | GMEM_ZEROINIT, size)?;
            let dest_ptr = GlobalLock(dest);
            if dest_ptr.is_null() {
                // Note: Memory leak on error, but this is rare and acceptable
                let _ = GlobalUnlock(src);
                return Err(windows::core::Error::from(E_UNEXPECTED));
            }

            std::ptr::copy_nonoverlapping(src_ptr as *const u8, dest_ptr as *mut u8, size);

            let _ = GlobalUnlock(dest);
            let _ = GlobalUnlock(src);

            Ok(dest)
        }
    }
}

impl windows::Win32::System::Com::IDataObject_Impl for HDropDataObject {
    fn GetData(&self, pformatetc: *const FORMATETC) -> windows::core::Result<STGMEDIUM> {
        let format = unsafe { &*pformatetc };

        if format.cfFormat == CF_HDROP && (format.tymed & TYMED_HGLOBAL.0 as u32) != 0 {
            self.get_hdrop()
        } else {
            debug!(
                "[hdrop] GetData: Unsupported format cf={} tymed={}",
                format.cfFormat, format.tymed
            );
            Err(windows::core::Error::from(DV_E_FORMATETC))
        }
    }

    fn GetDataHere(&self, _: *const FORMATETC, _: *mut STGMEDIUM) -> windows::core::Result<()> {
        Err(windows::core::Error::from(E_NOTIMPL))
    }

    fn QueryGetData(&self, pformatetc: *const FORMATETC) -> HRESULT {
        let format = unsafe { &*pformatetc };
        if format.cfFormat == CF_HDROP && (format.tymed & TYMED_HGLOBAL.0 as u32) != 0 {
            S_OK
        } else {
            DV_E_FORMATETC.into()
        }
    }

    fn GetCanonicalFormatEtc(&self, _: *const FORMATETC, _: *mut FORMATETC) -> HRESULT {
        windows::core::Error::from(E_NOTIMPL).into()
    }

    fn SetData(
        &self,
        _: *const FORMATETC,
        _: *const STGMEDIUM,
        _: BOOL,
    ) -> windows::core::Result<()> {
        Err(windows::core::Error::from(E_NOTIMPL))
    }

    fn EnumFormatEtc(
        &self,
        dwdirection: u32,
    ) -> windows::core::Result<windows::Win32::System::Com::IEnumFORMATETC> {
        if dwdirection == DATADIR_GET.0 as u32 {
            Ok(HDropFormatEnumerator::new().into())
        } else {
            Err(windows::core::Error::from(E_NOTIMPL))
        }
    }

    fn DAdvise(
        &self,
        _: *const FORMATETC,
        _: u32,
        _: Option<&IAdviseSink>,
    ) -> windows::core::Result<u32> {
        Err(windows::core::Error::from(E_NOTIMPL))
    }

    fn DUnadvise(&self, _: u32) -> windows::core::Result<()> {
        Err(windows::core::Error::from(E_NOTIMPL))
    }

    fn EnumDAdvise(&self) -> windows::core::Result<IEnumSTATDATA> {
        Err(windows::core::Error::from(E_NOTIMPL))
    }
}
