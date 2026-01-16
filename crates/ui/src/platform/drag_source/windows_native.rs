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
//! PROGRESS DIALOG: Uses Windows native IProgressDialog COM interface which
//! manages its own UI thread internally. This works even during the blocking
//! DoDragDrop modal loop - similar to how 7-Zip handles progress dialogs.

use arclain_core::backends::sevenz_cli::ProgressUpdate;
use arclain_core::{ArchiveBackend, ArchiveEntry};
use parking_lot::RwLock;
use std::sync::mpsc::Sender;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;
use tracing::{debug, info, warn};
use windows::core::{implement, HRESULT};

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

/// State for batch extraction - extracted files are cached in a temp directory
struct ExtractionCache {
    /// Temp directory where files are extracted (auto-cleaned on drop via tempfile)
    temp_dir: tempfile::TempDir,
    /// Set to true once batch extraction is complete
    extracted: bool,
}

/// Find the common directory that contains all the given file paths.
/// Returns None if files are in different root directories or if the list is empty.
///
/// For example:
/// - ["folder/a.txt", "folder/b.txt", "folder/sub/c.txt"] -> Some("folder")
/// - ["a.txt", "b.txt"] -> Some("") (root)
/// - ["folder1/a.txt", "folder2/b.txt"] -> None (different folders)
fn find_common_directory(file_paths: &[String]) -> Option<String> {
    if file_paths.is_empty() {
        return None;
    }

    // Normalize paths to use forward slashes
    let normalized: Vec<String> = file_paths.iter().map(|p| p.replace('\\', "/")).collect();

    // Get the first path's directory components
    let first = &normalized[0];
    let first_parts: Vec<&str> = first.split('/').collect();

    // If first path has no directory part, check if all paths are in root
    if first_parts.len() <= 1 {
        // All files must be in root (no directory part)
        let all_in_root = normalized.iter().all(|p| !p.contains('/'));
        return if all_in_root {
            Some(String::new())
        } else {
            None
        };
    }

    // Find the longest common directory prefix
    // Start with the parent directory of the first file
    let mut common_parts = &first_parts[..first_parts.len() - 1]; // Exclude filename

    for path in normalized.iter().skip(1) {
        let parts: Vec<&str> = path.split('/').collect();
        let dir_parts = &parts[..parts.len().saturating_sub(1)]; // Exclude filename

        // Find how many parts match
        let mut match_count = 0;
        for (i, part) in common_parts.iter().enumerate() {
            if i < dir_parts.len() && dir_parts[i] == *part {
                match_count += 1;
            } else {
                break;
            }
        }

        // Shrink common_parts to the matching portion
        common_parts = &common_parts[..match_count];

        // If no common directory at all, return None
        if common_parts.is_empty() {
            return None;
        }
    }

    if common_parts.is_empty() {
        None
    } else {
        Some(common_parts.join("/"))
    }
}

/// Extract files with a native Windows progress dialog.
///
/// Uses the native Windows IProgressDialog COM interface which manages its own
/// UI thread internally, allowing it to update while the main thread does
/// blocking work (extraction).
///
/// This is simpler than the child process IPC approach and works like 7-Zip.
///
/// Threshold for when to use extract_all vs extract_files
/// Command line length limit on Windows is ~8KB, so with average path length of 50 chars,
/// we can safely handle ~100 files. Use 75 as a safe threshold.
const MAX_FILES_FOR_EXTRACT_FILES: usize = 75;

fn extract_with_progress_dialog(
    backend: Arc<dyn ArchiveBackend>,
    archive_path: &std::path::Path,
    dest_dir: &std::path::Path,
    file_paths: &[String],
    password: Option<&str>,
) -> std::result::Result<(), String> {
    let file_count = file_paths.len();

    // For very small file counts (1-2 files), just extract directly without dialog
    if file_count <= 2 {
        debug!(
            "[drag] Small file count ({}), extracting without progress dialog",
            file_count
        );
        return backend
            .extract_files(archive_path, dest_dir, file_paths, password)
            .map_err(|e| format!("Extraction failed: {}", e));
    }

    // For large file counts, use extract_all to avoid command line length limits
    let use_extract_all = file_count > MAX_FILES_FOR_EXTRACT_FILES;
    if use_extract_all {
        debug!(
            "[drag] Large file count ({}), will use extract_all to avoid command line limits",
            file_count
        );

        // Find the common directory prefix for all files
        let common_dir = find_common_directory(file_paths);

        if let Some(dir_path) = common_dir {
            debug!(
                "[drag] Using extract_directory with pattern: {}/*",
                dir_path
            );
            return backend
                .extract_directory(archive_path, dest_dir, &dir_path, password)
                .map_err(|e| {
                    warn!("[drag] extract_directory error: {}", e);
                    format!("Extraction failed: {}", e)
                });
        } else {
            debug!("[drag] No common directory found, using extract_all");
            return backend
                .extract_all(archive_path, dest_dir, password)
                .map_err(|e| {
                    warn!("[drag] extract_all error: {}", e);
                    format!("Extraction failed: {}", e)
                });
        }
    }

    // Use native Windows IProgressDialog for extraction with progress
    debug!(
        "[drag] Starting extraction with native Windows progress dialog for {} files",
        file_count
    );
    super::native_progress::extract_with_native_progress(
        backend,
        archive_path,
        dest_dir,
        file_paths,
        password,
    )
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
        // Compute the common prefix to strip from display paths
        // This makes dragging "folder/file.txt" display as "file.txt" if user is inside "folder"
        // Or if dragging a folder, show its contents relative to that folder
        let common_prefix = Self::compute_common_prefix(&entries);
        info!("[drag] Computed common prefix: {:?}", common_prefix);

        // Create drag entries, filtering out directories and stripping prefix
        let drag_entries: Vec<DragEntry> = entries
            .iter()
            .filter(|e| !e.is_dir) // Skip directories - they cause merge prompts
            .map(|e| {
                let display_path = if let Some(ref prefix) = common_prefix {
                    // Strip the prefix and any leading slashes
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
                Some(pos) => Some(first[..=pos].to_string()), // Include the separator
                None => None,                                 // No directory part to strip
            };
        }

        // Multiple files: Check if they're all in the same folder
        // Find the deepest common directory
        let first = &file_entries[0].path;

        // Get the first directory component (e.g., "folder/" from "folder/subfolder/file.txt")
        let first_sep = first.find(|c| c == '/' || c == '\\');
        let first_dir = match first_sep {
            Some(pos) => &first[..=pos], // Include the separator
            None => return None,         // No directory part
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
        let temp_dir =
            tempfile::tempdir().map_err(|e| format!("Failed to create temp dir: {}", e))?;

        info!("[drag] Temp dir created at: {:?}", temp_dir.path());

        // Collect file paths to extract (skip directories)
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

        // Use IProgressDialog or channel for extraction progress
        if let Some(tx) = &self.progress_tx {
            let _ = tx.send(ProgressUpdate {
                percent: 0,
                message: Some(format!("Starting extraction of {} files...", file_count)),
            });

            // Re-use logic for large file counts (extract_all / extract_directory)
            let use_extract_all = file_count > MAX_FILES_FOR_EXTRACT_FILES;
            if use_extract_all {
                // Large batch: use directory extraction or extract all (no granular progress yet)
                // Find the common directory prefix for all files
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
                // Standard file extraction - use extract_files_with_progress to get real updates
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
                        None, // No cancellation token for now (drag cancellation is harder to wire up)
                    )
                    .map_err(|e| format!("Extraction failed: {}", e))?;
            }

            let _ = tx.send(ProgressUpdate {
                percent: 100,
                message: Some("Extraction complete".to_string()),
            });
        } else {
            // Fallback to native dialog
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
            "[drag] Batch extraction complete: {} files in {:.2}s ({:.1} files/sec)",
            file_count,
            elapsed.as_secs_f64(),
            file_count as f64 / elapsed.as_secs_f64()
        );

        // List what was actually extracted to the temp directory
        info!("[drag] Listing temp dir contents:");
        fn list_dir_recursive(dir: &std::path::Path, prefix: &str, depth: usize) {
            if depth > 3 {
                return;
            } // Limit depth to avoid spam
            if let Ok(entries) = std::fs::read_dir(dir) {
                for entry in entries.take(10) {
                    if let Ok(e) = entry {
                        let path = e.path();
                        let name = path.file_name().unwrap_or_default().to_string_lossy();
                        if path.is_dir() {
                            info!("[drag]   {}{}/ (dir)", prefix, name);
                            list_dir_recursive(&path, &format!("{}  ", prefix), depth + 1);
                        } else {
                            if let Ok(meta) = std::fs::metadata(&path) {
                                info!("[drag]   {}{} ({} bytes)", prefix, name, meta.len());
                            }
                        }
                    }
                }
            }
        }
        list_dir_recursive(temp_dir.path(), "", 0);

        // Verify files were extracted - normalize path separators
        let mut verified_count = 0;
        let mut missing_count = 0;
        for path in file_paths.iter().take(5) {
            // Only check first 5 to avoid spam
            // Normalize path separators for Windows
            let normalized = path
                .replace('/', std::path::MAIN_SEPARATOR_STR)
                .replace('\\', std::path::MAIN_SEPARATOR_STR);
            let full_path = temp_dir.path().join(&normalized);
            if full_path.exists() {
                if let Ok(meta) = std::fs::metadata(&full_path) {
                    debug!(
                        "[drag] Extracted file verified: {:?} ({} bytes)",
                        full_path,
                        meta.len()
                    );
                    verified_count += 1;
                }
            } else {
                warn!("[drag] Extracted file NOT FOUND: {:?}", full_path);
                missing_count += 1;
            }
        }
        info!(
            "[drag] Verification: {} verified, {} missing (checked first 5)",
            verified_count, missing_count
        );

        *cache = Some(ExtractionCache {
            temp_dir,
            extracted: true,
        });

        Ok(())
    }

    /// Get the path to an extracted file in the temp directory
    /// Normalizes path separators to handle both forward and back slashes
    fn get_extracted_path(&self, entry_path: &str) -> Option<PathBuf> {
        let cache = self.cache.read();
        cache.as_ref().map(|c| {
            // Normalize the path to use forward slashes first, then let PathBuf handle it
            // This handles cases where the archive uses backslashes but the file system uses forward slashes
            let normalized = entry_path
                .replace('/', std::path::MAIN_SEPARATOR_STR)
                .replace('\\', std::path::MAIN_SEPARATOR_STR);
            c.temp_dir.path().join(&normalized)
        })
    }

    fn get_file_descriptor(&self) -> windows::core::Result<STGMEDIUM> {
        // Use drag_entries which have display paths and exclude directories
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
            warn!(
                "[drag] get_file_contents: invalid lindex={} (drag_entries.len={})",
                lindex,
                self.drag_entries.len()
            );
            return Err(windows::core::Error::from(DV_E_LINDEX));
        }

        let drag_entry = &self.drag_entries[lindex as usize];
        info!(
            "[drag] get_file_contents: lindex={} display='{}' archive='{}' size={}",
            lindex, drag_entry.display_path, drag_entry.archive_path, drag_entry.size
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
        let extracted_path = self
            .get_extracted_path(&drag_entry.archive_path)
            .ok_or_else(|| {
                warn!(
                    "[drag] get_extracted_path returned None for '{}'",
                    drag_entry.archive_path
                );
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
                    extracted_path.display(),
                    e
                );
                return Err(windows::core::Error::from(E_UNEXPECTED));
            }
        };

        // Handle empty files - allocate at least 1 byte to avoid issues
        let alloc_size = if buffer.is_empty() { 1 } else { buffer.len() };

        debug!(
            "[drag] Allocating HGLOBAL: {} bytes for '{}' (buffer.len={})",
            alloc_size,
            drag_entry.display_path,
            buffer.len()
        );

        // Allocate HGLOBAL and copy data
        let hglobal = unsafe {
            GlobalAlloc(GMEM_MOVEABLE | GMEM_ZEROINIT, alloc_size).map_err(|e| {
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

        info!(
            "[drag] Prepared HGLOBAL for '{}' ({} bytes)",
            drag_entry.display_path,
            buffer.len()
        );

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

        info!(
            "[drag] GetData: cfFormat={} tymed={} lindex={}",
            format.cfFormat, format.tymed, format.lindex
        );

        if format.cfFormat == fd_format && (format.tymed & TYMED_HGLOBAL.0 as u32) != 0 {
            info!("[drag] Returning FileGroupDescriptorW");
            self.get_file_descriptor()
        } else if format.cfFormat == fc_format && (format.tymed & TYMED_HGLOBAL.0 as u32) != 0 {
            info!(
                "[drag] Returning FileContents as HGLOBAL for lindex={}",
                format.lindex
            );
            self.get_file_contents(format.lindex)
        } else {
            info!(
                "[drag] Unsupported format: cfFormat={} tymed={}",
                format.cfFormat, format.tymed
            );
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

        // Check if format is supported AND if we support the requested medium (HGLOBAL)
        if (format.cfFormat == fd_format || format.cfFormat == fc_format)
            && (format.tymed & TYMED_HGLOBAL.0 as u32) != 0
        {
            // Log success at DEBUG to avoid spamming excessively on mouse move,
            // but maybe INFO is needed if we suspect failures
            // Let's use INFO for now to debug
            info!(
                "[drag] QueryGetData: Supported format cf={} tymed={}",
                format.cfFormat, format.tymed
            );
            S_OK
        } else {
            info!(
                "[drag] QueryGetData: Unsupported format cf={} tymed={}",
                format.cfFormat, format.tymed
            );
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
        // internal throttling to prove it's alive without spamming
        // info!("[drag] QueryContinueDrag: keys={:?}", grfkeystate);

        if fescapepressed.as_bool() {
            info!("[drag] QueryContinueDrag: Escape pressed, cancelling");
            DRAGDROP_S_CANCEL
        } else if (grfkeystate.0 & windows::Win32::System::SystemServices::MK_LBUTTON.0) == 0 {
            info!("[drag] QueryContinueDrag: LButton released, dropping");
            DRAGDROP_S_DROP
        } else {
            S_OK
        }
    }

    fn GiveFeedback(&self, _dweffect: DROPEFFECT) -> HRESULT {
        // info!("[drag] GiveFeedback: effect={:?}", dweffect);
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
            // It might fail if threads are already attached? Or some other reason.
            // But we proceed anyway.
            warn!("[drag] Failed to attach thread input");
        }

        // Create data object
        let data_object: IDataObject =
            LazyArchiveDataObject::new(backend, archive_path, entries, password, Some(tx.clone()))
                .into();
        let drop_source: IDropSource = SimpleDropSource.into();

        let mut effect = DROPEFFECT_NONE;

        info!("[drag] Calling DoDragDrop (blocking on background thread)...");

        // Notify that we are ready to drag
        // We do NOT send a message here anymore to avoid showing the modal prematurely.
        // The modal will be triggered when ensure_extracted() is called on drop.

        let result = unsafe {
            DoDragDrop(
                &data_object,
                &drop_source,
                DROPEFFECT_COPY | DROPEFFECT_MOVE,
                &mut effect,
            )
        };

        info!("[drag] DoDragDrop returned with result: {:?}", result);

        // Detach input
        if attached {
            unsafe {
                use windows::Win32::System::Threading::AttachThreadInput;
                let _ = AttachThreadInput(bg_thread_id, main_thread_id, false);
            }
            info!("[drag] Detached thread input");
        }

        if result == DRAGDROP_S_DROP {
            tracing::debug!("[drag] Drag completed with effect: {:?}", effect);
            let _ = tx.send(ProgressUpdate {
                percent: 100,
                message: Some("Drop complete".to_string()),
            });
        } else if result == DRAGDROP_S_CANCEL {
            tracing::debug!("[drag] Drag cancelled");
            // If cancelled, we might want to close the dialog
            let _ = tx.send(ProgressUpdate {
                percent: 100,
                message: Some("Cancelled".to_string()),
            });
        } else {
            tracing::warn!("[drag] Drag failed with HRESULT: {:?}", result);
            let _ = tx.send(ProgressUpdate {
                percent: 100,
                message: Some(format!("Failed: {:?}", result)),
            });
        }
    });

    Ok(rx)
}
