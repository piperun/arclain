use super::types::{DragEntry, ExtractionCache};
use super::utils::{
    extract_with_progress_dialog, find_common_directory, get_clipboard_format,
    MAX_FILES_FOR_EXTRACT_FILES,
};
use crate::platform::drag_source::stream::ArchiveStream;
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
use windows::Win32::Foundation::{
    BOOL, DV_E_FORMATETC, DV_E_LINDEX, E_NOTIMPL, E_OUTOFMEMORY, E_UNEXPECTED, S_FALSE, S_OK,
};
use windows::Win32::Storage::FileSystem::FILE_ATTRIBUTE_NORMAL;
use windows::Win32::System::Com::{
    IAdviseSink, IEnumSTATDATA, DATADIR_GET, DVASPECT_CONTENT, FORMATETC, STGMEDIUM, TYMED_HGLOBAL,
    TYMED_ISTREAM,
};
use windows::Win32::System::Memory::{
    GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE, GMEM_ZEROINIT,
};
use windows::Win32::UI::Shell::{FD_ATTRIBUTES, FD_FILESIZE, FILEDESCRIPTORW};

/// Enumerator for supported formats (FileDescriptor + FileContents)
#[implement(windows::Win32::System::Com::IEnumFORMATETC)]
pub struct FormatEnumerator {
    index: RwLock<usize>,
}

impl FormatEnumerator {
    pub fn new() -> Self {
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
            // Content (ISTREAM)
            FORMATETC {
                cfFormat: fc_format,
                ptd: std::ptr::null_mut(),
                dwAspect: DVASPECT_CONTENT.0,
                lindex: -1,
                tymed: TYMED_ISTREAM.0 as u32,
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

    fn Skip(&self, celt: u32) -> windows::core::Result<()> {
        let mut idx = self.index.write();
        *idx += celt as usize;
        Ok(())
    }

    fn Reset(&self) -> windows::core::Result<()> {
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
    /// Channel for sending progress updates
    progress_tx: Option<Sender<ProgressUpdate>>,
}

impl LazyArchiveDataObject {
    pub fn new(
        backend: Arc<dyn ArchiveBackend>,
        archive_path: PathBuf,
        entries: Vec<ArchiveEntry>,
        password: Option<String>,
        progress_tx: Option<Sender<ProgressUpdate>>,
    ) -> Self {
        let common_prefix = Self::compute_common_prefix(&entries);
        info!("[drag] Computed common prefix: {}", common_prefix.as_deref().unwrap_or("<none>"));

        let drag_entries: Vec<DragEntry> = entries
            .iter()
            .filter(|e| !e.is_dir)
            .map(|e| {
                let display_path = if let Some(ref prefix) = common_prefix {
                    let stripped = e.path.strip_prefix(prefix).unwrap_or(&e.path);
                    let stripped = stripped.trim_start_matches('/').trim_start_matches('\\');
                    stripped.to_string()
                } else {
                    e.path.clone()
                };

                debug!(
                    "[drag] Entry: archive='{}' -> display='{}'",
                    e.path, display_path
                );

                DragEntry {
                    archive_path: e.path.clone(),
                    display_path,
                    size: e.size,
                }
            })
            .collect();

        info!(
            "[drag] Created {} drag entries (filtered from {} archive entries)",
            drag_entries.len(),
            entries.len()
        );

        Self {
            backend,
            archive_path,
            entries,
            drag_entries,
            password,
            cache: RwLock::new(None),
            progress_tx,
        }
    }

    fn compute_common_prefix(entries: &[ArchiveEntry]) -> Option<String> {
        if entries.is_empty() {
            return None;
        }

        let file_entries: Vec<_> = entries.iter().filter(|e| !e.is_dir).collect();
        if file_entries.is_empty() {
            return None;
        }

        if file_entries.len() == 1 {
            let first = &file_entries[0].path;
            let sep_pos = first.rfind(|c| c == '/' || c == '\\');
            return match sep_pos {
                Some(pos) => Some(first[..=pos].to_string()),
                None => None,
            };
        }

        let first = &file_entries[0].path;
        let first_sep = first.find(|c| c == '/' || c == '\\');
        let first_dir = match first_sep {
            Some(pos) => &first[..=pos],
            None => return None,
        };

        for entry in file_entries.iter().skip(1) {
            if !entry.path.starts_with(first_dir) {
                return None;
            }
        }

        None
    }

    fn ensure_extracted(&self) -> std::result::Result<(), String> {
        // Fast path: already extracted? Just a read lock.
        {
            let cache = self.cache.read();
            if cache.as_ref().map(|c| c.extracted).unwrap_or(false) {
                debug!("[drag] Already extracted, skipping");
                return Ok(());
            }
        }

        // Slow path: extract WITHOUT holding the lock. Audit finding R1 —
        // the previous version held `cache.write()` across `backend.extract_*`,
        // blocking every other COM caller (Explorer asking for additional
        // FileContents items, sibling GetData() calls) for the full
        // extraction window. Mirrors the pattern HDropDataObject already
        // uses: do work locally, write-lock briefly only to publish.
        //
        // Race: two concurrent callers can both pass the fast-path check,
        // both create temp dirs, and both extract in parallel. That's wasted
        // work but correct — the second writer drops its temp dir on cache
        // install (see "loser drops temp" below) and TempDir's Drop handles
        // filesystem cleanup. Acceptable given how rare concurrent drags are.
        let start = Instant::now();

        let temp_dir =
            tempfile::tempdir().map_err(|e| format!("Failed to create temp dir: {}", e))?;
        info!("[drag] Temp dir created at: {}", temp_dir.path().display());

        let file_paths: Vec<String> = self
            .entries
            .iter()
            .filter(|e| !e.is_dir)
            .map(|e| e.path.clone())
            .collect();
        let file_count = file_paths.len();
        info!(
            "[drag] Batch extracting {} files to temp dir: {:?}",
            file_count, file_paths
        );

        if let Some(tx) = &self.progress_tx {
            let _ = tx.send(ProgressUpdate {
                percent: 0,
                message: Some(format!("Starting extraction of {} files...", file_count)),
            });

            let use_extract_all = file_count > MAX_FILES_FOR_EXTRACT_FILES;
            if use_extract_all {
                let common_dir = find_common_directory(&file_paths);
                let _ = tx.send(ProgressUpdate {
                    percent: 0,
                    message: Some("Extracting batch (please wait)...".to_string()),
                });

                if let Some(dir_path) = common_dir {
                    debug!(
                        "[drag] Using extract_directory with pattern: {}/*",
                        dir_path
                    );
                    self.backend
                        .extract_directory(
                            &self.archive_path,
                            temp_dir.path(),
                            &dir_path,
                            self.password.as_deref(),
                        )
                        .map_err(|e| {
                            warn!("[drag] extract_directory error: {}", e);
                            format!("Extraction failed: {}", e)
                        })?;
                } else {
                    debug!("[drag] No common directory found, using extract_all");
                    self.backend
                        .extract_all(
                            &self.archive_path,
                            temp_dir.path(),
                            self.password.as_deref(),
                        )
                        .map_err(|e| {
                            warn!("[drag] extract_all error: {}", e);
                            format!("Extraction failed: {}", e)
                        })?;
                }
            } else {
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
                    .map_err(|e| format!("Extraction failed: {}", e))?;
            }

            let _ = tx.send(ProgressUpdate {
                percent: 100,
                message: Some("Extraction complete".to_string()),
            });
        } else {
            extract_with_progress_dialog(
                Arc::clone(&self.backend),
                &self.archive_path,
                temp_dir.path(),
                &file_paths,
                self.password.as_deref(),
            )?;
        }

        let elapsed = start.elapsed();
        info!(
            "[drag] Batch extraction complete: {} files in {:.2}s",
            file_count,
            elapsed.as_secs_f64()
        );

        // Publish. Brief write lock — only held for the swap, not extraction.
        let mut cache = self.cache.write();
        if cache.as_ref().map(|c| c.extracted).unwrap_or(false) {
            // Loser drops temp: another caller raced us to extraction. Our
            // temp_dir falls out of scope here and TempDir::drop cleans up
            // the filesystem. The cache stays pointing at the winner's dir.
            debug!("[drag] Race detected — other caller already published; discarding our temp dir");
            return Ok(());
        }
        *cache = Some(ExtractionCache {
            temp_dir,
            extracted: true,
        });
        Ok(())
    }

    fn get_extracted_path(&self, entry_path: &str) -> Option<PathBuf> {
        let cache = self.cache.read();
        cache.as_ref().map(|c| {
            let normalized = entry_path
                .replace('/', std::path::MAIN_SEPARATOR_STR)
                .replace('\\', std::path::MAIN_SEPARATOR_STR);
            c.temp_dir.path().join(&normalized)
        })
    }

    fn get_file_descriptor(&self) -> windows::core::Result<STGMEDIUM> {
        info!(
            "[drag] get_file_descriptor: {} drag entries",
            self.drag_entries.len()
        );
        for (i, entry) in self.drag_entries.iter().enumerate() {
            debug!(
                "[drag]   [{:3}] display='{}' archive='{}' size={}",
                i, entry.display_path, entry.archive_path, entry.size
            );
        }

        let count = self.drag_entries.len();
        let header_size = std::mem::size_of::<u32>();
        let item_size = std::mem::size_of::<FILEDESCRIPTORW>();
        let total_size = header_size + (count * item_size);

        let hglobal = unsafe { GlobalAlloc(GMEM_MOVEABLE | GMEM_ZEROINIT, total_size)? };
        let ptr = unsafe { GlobalLock(hglobal) };
        if ptr.is_null() {
            return Err(windows::core::Error::from(E_UNEXPECTED));
        }

        unsafe {
            *(ptr as *mut u32) = count as u32;
            let descriptors_ptr = ptr.add(header_size) as *mut FILEDESCRIPTORW;

            for (i, entry) in self.drag_entries.iter().enumerate() {
                let name = &entry.display_path;
                let name_wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
                let mut name_arr = [0u16; 260];
                let copy_len = std::cmp::min(name_wide.len(), 259);
                name_arr[..copy_len].copy_from_slice(&name_wide[..copy_len]);

                let descriptor = FILEDESCRIPTORW {
                    dwFlags: FD_ATTRIBUTES.0 as u32 | FD_FILESIZE.0 as u32,
                    dwFileAttributes: FILE_ATTRIBUTE_NORMAL.0,
                    nFileSizeLow: entry.size as u32,
                    nFileSizeHigh: (entry.size >> 32) as u32,
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
        if lindex < 0 || lindex as usize >= self.drag_entries.len() {
            return Err(windows::core::Error::from(DV_E_LINDEX));
        }

        let drag_entry = &self.drag_entries[lindex as usize];
        debug!(
            "[drag] get_file_contents: lindex={} display='{}'",
            lindex, drag_entry.display_path
        );

        debug!("[drag] Calling ensure_extracted...");
        if let Err(e) = self.ensure_extracted() {
            warn!("[drag] Batch extraction failed: {}", e);
            return Err(windows::core::Error::from(E_UNEXPECTED));
        }

        let extracted_path = self
            .get_extracted_path(&drag_entry.archive_path)
            .ok_or_else(|| windows::core::Error::from(E_UNEXPECTED))?;
        debug!("[drag] Reading extracted file from: {:?}", extracted_path);

        let buffer =
            std::fs::read(&extracted_path).map_err(|_| windows::core::Error::from(E_UNEXPECTED))?;
        let alloc_size = if buffer.is_empty() { 1 } else { buffer.len() };

        let hglobal = unsafe { GlobalAlloc(GMEM_MOVEABLE | GMEM_ZEROINIT, alloc_size)? };
        let ptr = unsafe { GlobalLock(hglobal) };
        if ptr.is_null() {
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

        Ok(STGMEDIUM {
            tymed: TYMED_HGLOBAL.0 as u32,
            u: windows::Win32::System::Com::STGMEDIUM_0 { hGlobal: hglobal },
            pUnkForRelease: std::mem::ManuallyDrop::new(None),
        })
    }

    /// Create an IStream from an already extracted file in temp directory
    #[allow(dead_code)]
    fn get_temp_file_stream(&self, lindex: i32) -> windows::core::Result<STGMEDIUM> {
        if lindex < 0 || lindex as usize >= self.drag_entries.len() {
            return Err(windows::core::Error::from(DV_E_LINDEX));
        }

        let drag_entry = &self.drag_entries[lindex as usize];
        let extracted_path = self
            .get_extracted_path(&drag_entry.archive_path)
            .ok_or_else(|| windows::core::Error::from(E_UNEXPECTED))?;

        debug!(
            "[drag] Creating IStream from temp file: {:?}",
            extracted_path
        );

        // Open file safely for reading
        // We use SHCreateStreamOnFileEx for easy IStream creation from file path if possible?
        // Or standard windows::Win32::System::Com::SHCreateStreamOnFileW

        use windows::Win32::System::Com::STGM_READ;
        use windows::Win32::UI::Shell::SHCreateStreamOnFileW;

        let path_wide: Vec<u16> = extracted_path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();

        let stream = unsafe {
            SHCreateStreamOnFileW(windows::core::PCWSTR(path_wide.as_ptr()), STGM_READ.0)?
        };

        // Stream is already IStream from Result
        // let stream = stream_opt.ok_or_else(|| windows::core::Error::from(E_UNEXPECTED))?;

        Ok(STGMEDIUM {
            tymed: TYMED_ISTREAM.0 as u32,
            u: windows::Win32::System::Com::STGMEDIUM_0 {
                pstm: std::mem::ManuallyDrop::new(Some(stream)),
            },
            pUnkForRelease: std::mem::ManuallyDrop::new(None),
        })
    }

    fn get_file_stream(&self, lindex: i32) -> windows::core::Result<STGMEDIUM> {
        if lindex < 0 || lindex as usize >= self.drag_entries.len() {
            return Err(windows::core::Error::from(DV_E_LINDEX));
        }
        let drag_entry = &self.drag_entries[lindex as usize];
        debug!("[drag] Creating IStream for '{}'", drag_entry.display_path);

        let stream = ArchiveStream::new(
            self.backend.clone(),
            self.archive_path.clone(),
            drag_entry.archive_path.clone(),
            self.password.clone(),
            drag_entry.size,
            self.progress_tx.clone(),
        );

        let istream: windows::Win32::System::Com::IStream = stream.into();

        Ok(STGMEDIUM {
            tymed: TYMED_ISTREAM.0 as u32,
            u: windows::Win32::System::Com::STGMEDIUM_0 {
                pstm: std::mem::ManuallyDrop::new(Some(istream)),
            },
            pUnkForRelease: std::mem::ManuallyDrop::new(None),
        })
    }
}

impl windows::Win32::System::Com::IDataObject_Impl for LazyArchiveDataObject {
    fn GetData(&self, pformatetc: *const FORMATETC) -> windows::core::Result<STGMEDIUM> {
        let format = unsafe { &*pformatetc };
        let fd_format = get_clipboard_format("FileGroupDescriptorW");
        let fc_format = get_clipboard_format("FileContents");

        // Use debug logs as requested
        if format.cfFormat == fd_format && (format.tymed & TYMED_HGLOBAL.0 as u32) != 0 {
            debug!("[drag] Returning FileGroupDescriptorW");
            self.get_file_descriptor()
        } else if format.cfFormat == fc_format {
            if (format.tymed & TYMED_ISTREAM.0 as u32) != 0 {
                debug!(
                    "[drag] Returning FileContents as ISTREAM for lindex={}",
                    format.lindex
                );
                self.get_file_stream(format.lindex)
            } else if (format.tymed & TYMED_HGLOBAL.0 as u32) != 0 {
                debug!(
                    "[drag] Returning FileContents as HGLOBAL for lindex={}",
                    format.lindex
                );
                self.get_file_contents(format.lindex)
            } else {
                debug!(
                    "[drag] Unsupported format: cfFormat={} tymed={}",
                    format.cfFormat, format.tymed
                );
                Err(windows::core::Error::from(DV_E_FORMATETC))
            }
        } else {
            debug!(
                "[drag] Unsupported format: cfFormat={} tymed={}",
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
        let fd_format = get_clipboard_format("FileGroupDescriptorW");
        let fc_format = get_clipboard_format("FileContents");

        if (format.cfFormat == fd_format || format.cfFormat == fc_format)
            && (format.tymed & (TYMED_HGLOBAL.0 as u32 | TYMED_ISTREAM.0 as u32)) != 0
        {
            debug!(
                "[drag] QueryGetData: Supported format cf={} tymed={}",
                format.cfFormat, format.tymed
            );
            S_OK
        } else {
            debug!(
                "[drag] QueryGetData: Unsupported format cf={} tymed={}",
                format.cfFormat, format.tymed
            );
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
            Ok(FormatEnumerator::new().into())
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
