//! CF_HDROP-based drag data object for fast multi-file transfers.
//!
//! This implementation pre-extracts files to a temp directory and provides
//! CF_HDROP format, allowing Explorer to perform direct filesystem copies.
//! This is MUCH faster than IStream-based transfer for multiple files.

use super::types::ExtractionCache;
use super::utils::{
    extract_with_progress_dialog, find_common_directory, MAX_FILES_FOR_EXTRACT_FILES,
};
use arclain_core::backends::sevenz_cli::ProgressUpdate;
use arclain_core::{ArchiveBackend, ArchiveEntry};
use parking_lot::RwLock;

use std::os::windows::ffi::OsStrExt;
use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::time::Instant;
use tracing::{debug, info, warn};
use windows::core::{implement, HRESULT};
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

/// CF_HDROP-based data object for fast drag operations.
///
/// Pre-extracts all files to temp and returns CF_HDROP with file paths.
/// Explorer then performs direct filesystem copy (blazing fast).
#[implement(windows::Win32::System::Com::IDataObject)]
pub struct HDropDataObject {
    backend: Arc<dyn ArchiveBackend>,
    archive_path: PathBuf,
    entries: Vec<ArchiveEntry>,
    password: Option<String>,
    cache: RwLock<Option<ExtractionCache>>,
    progress_tx: Option<Sender<ProgressUpdate>>,
}

impl HDropDataObject {
    pub fn new(
        backend: Arc<dyn ArchiveBackend>,
        archive_path: PathBuf,
        entries: Vec<ArchiveEntry>,
        password: Option<String>,
        progress_tx: Option<Sender<ProgressUpdate>>,
    ) -> Self {
        info!(
            "[hdrop] Creating HDropDataObject for {} entries",
            entries.len()
        );
        Self {
            backend,
            archive_path,
            entries,
            password,
            cache: RwLock::new(None),
            progress_tx,
        }
    }

    /// Ensure all files are extracted to temp directory.
    fn ensure_extracted(&self) -> Result<(), String> {
        {
            let cache = self.cache.read();
            if cache.as_ref().map(|c| c.extracted).unwrap_or(false) {
                return Ok(());
            }
        }

        let mut cache = self.cache.write();
        if cache.as_ref().map(|c| c.extracted).unwrap_or(false) {
            return Ok(());
        }

        let start = Instant::now();
        let temp_dir =
            tempfile::tempdir().map_err(|e| format!("Failed to create temp dir: {}", e))?;

        info!("[hdrop] Extracting to temp: {:?}", temp_dir.path());

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
                message: Some(format!("Extracting {} files...", file_count)),
            });
        }

        // Use batch extraction
        let use_extract_all = file_count > MAX_FILES_FOR_EXTRACT_FILES;
        if use_extract_all {
            let common_dir = find_common_directory(&file_paths);
            if let Some(dir_path) = common_dir {
                self.backend
                    .extract_directory(
                        &self.archive_path,
                        temp_dir.path(),
                        &dir_path,
                        self.password.as_deref(),
                    )
                    .map_err(|e| format!("extract_directory failed: {}", e))?;
            } else {
                self.backend
                    .extract_all(
                        &self.archive_path,
                        temp_dir.path(),
                        self.password.as_deref(),
                    )
                    .map_err(|e| format!("extract_all failed: {}", e))?;
            }
        } else if let Some(tx) = &self.progress_tx {
            let tx_clone = tx.clone();
            self.backend
                .extract_files_with_progress(
                    &self.archive_path,
                    temp_dir.path(),
                    &file_paths,
                    self.password.as_deref(),
                    Some(&move |p| {
                        let _ = tx_clone.send(ProgressUpdate {
                            percent: p.percent,
                            message: Some(format!("Extracting: {}", p.current_file)),
                        });
                    }),
                    None,
                )
                .map_err(|e| format!("extract_files_with_progress failed: {}", e))?;
        } else {
            extract_with_progress_dialog(
                Arc::clone(&self.backend),
                &self.archive_path,
                temp_dir.path(),
                &file_paths,
                self.password.as_deref(),
            )?;
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

        *cache = Some(ExtractionCache {
            temp_dir,
            extracted: true,
        });
        Ok(())
    }

    /// Build CF_HDROP structure with paths to extracted files/folders.
    ///
    /// If all entries share a common root folder, returns just that folder.
    /// This ensures Explorer copies the entire directory structure correctly.
    fn get_hdrop(&self) -> windows::core::Result<STGMEDIUM> {
        // Ensure extraction is done
        if let Err(e) = self.ensure_extracted() {
            warn!("[hdrop] Extraction failed: {}", e);
            return Err(windows::core::Error::from(E_UNEXPECTED));
        }

        let cache = self.cache.read();
        let temp_dir = cache
            .as_ref()
            .map(|c| c.temp_dir.path())
            .ok_or_else(|| windows::core::Error::from(E_UNEXPECTED))?;

        // Find the common root folder for all entries
        let all_paths: Vec<String> = self.entries.iter().map(|e| e.path.clone()).collect();

        // Determine what to include in CF_HDROP
        let hdrop_paths: Vec<PathBuf> = if let Some(root_folder) = self.find_root_folder(&all_paths)
        {
            // All entries are under a common folder - return just that folder
            let folder_path = temp_dir.join(&root_folder);
            if folder_path.exists() && folder_path.is_dir() {
                info!("[hdrop] Returning root folder: {:?}", folder_path);
                vec![folder_path]
            } else {
                // Fallback to listing top-level items
                self.collect_top_level_items(temp_dir, &all_paths)
            }
        } else {
            // No common folder - return top-level items
            self.collect_top_level_items(temp_dir, &all_paths)
        };

        info!(
            "[hdrop] Preparing CF_HDROP with {} items",
            hdrop_paths.len()
        );
        for p in &hdrop_paths {
            debug!("[hdrop]   {:?}", p);
        }

        // Build DROPFILES structure
        let header_size = std::mem::size_of::<DROPFILES>();

        // Calculate total size for wide strings
        let mut strings_size = 0usize;
        for path in &hdrop_paths {
            let wide: Vec<u16> = path
                .as_os_str()
                .encode_wide()
                .chain(std::iter::once(0))
                .collect();
            strings_size += wide.len() * 2; // UTF-16 = 2 bytes per char
        }
        strings_size += 2; // Double null terminator

        let total_size = header_size + strings_size;

        let hglobal = unsafe { GlobalAlloc(GMEM_MOVEABLE | GMEM_ZEROINIT, total_size)? };
        let ptr = unsafe { GlobalLock(hglobal) };
        if ptr.is_null() {
            return Err(windows::core::Error::from(E_UNEXPECTED));
        }

        unsafe {
            // Write DROPFILES header
            let dropfiles = ptr as *mut DROPFILES;
            (*dropfiles).pFiles = header_size as u32;
            (*dropfiles).fWide = BOOL::from(true); // Unicode paths

            // Write paths
            let mut string_ptr = (ptr as *mut u8).add(header_size) as *mut u16;
            for path in &hdrop_paths {
                let wide: Vec<u16> = path
                    .as_os_str()
                    .encode_wide()
                    .chain(std::iter::once(0))
                    .collect();
                std::ptr::copy_nonoverlapping(wide.as_ptr(), string_ptr, wide.len());
                string_ptr = string_ptr.add(wide.len());
            }
            // Double null terminator (already zeroed by GMEM_ZEROINIT)

            let _ = GlobalUnlock(hglobal);
        }

        Ok(STGMEDIUM {
            tymed: TYMED_HGLOBAL.0 as u32,
            u: windows::Win32::System::Com::STGMEDIUM_0 { hGlobal: hglobal },
            pUnkForRelease: std::mem::ManuallyDrop::new(None),
        })
    }

    /// Find common root folder if all paths start with the same directory.
    fn find_root_folder(&self, paths: &[String]) -> Option<String> {
        if paths.is_empty() {
            return None;
        }

        // Normalize and find the first component of each path
        let mut root_candidates: std::collections::HashSet<String> =
            std::collections::HashSet::new();

        for path in paths {
            let normalized = path.replace('\\', "/");
            if let Some(first_component) = normalized.split('/').next() {
                if !first_component.is_empty() {
                    root_candidates.insert(first_component.to_string());
                }
            }
        }

        // If all paths share exactly one root folder, return it
        if root_candidates.len() == 1 {
            let root = root_candidates.into_iter().next().unwrap();
            // Verify it's actually a folder (not just a file at root)
            let has_nested = paths.iter().any(|p| {
                let normalized = p.replace('\\', "/");
                normalized.starts_with(&format!("{}/", root))
            });
            if has_nested {
                return Some(root);
            }
        }

        None
    }

    /// Collect top-level items (files and folders at root of extraction).
    fn collect_top_level_items(
        &self,
        temp_dir: &std::path::Path,
        paths: &[String],
    ) -> Vec<PathBuf> {
        let mut top_level: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();

        for path in paths {
            let normalized = path
                .replace('/', std::path::MAIN_SEPARATOR_STR)
                .replace('\\', std::path::MAIN_SEPARATOR_STR);

            // Get first component (file or folder at root)
            let first_component = normalized
                .split(std::path::MAIN_SEPARATOR)
                .next()
                .unwrap_or(&normalized);

            let full_path = temp_dir.join(first_component);
            if full_path.exists() {
                top_level.insert(full_path);
            }
        }

        top_level.into_iter().collect()
    }
}

impl windows::Win32::System::Com::IDataObject_Impl for HDropDataObject {
    fn GetData(&self, pformatetc: *const FORMATETC) -> windows::core::Result<STGMEDIUM> {
        let format = unsafe { &*pformatetc };

        if format.cfFormat == CF_HDROP && (format.tymed & TYMED_HGLOBAL.0 as u32) != 0 {
            debug!("[hdrop] GetData: Returning CF_HDROP");
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
