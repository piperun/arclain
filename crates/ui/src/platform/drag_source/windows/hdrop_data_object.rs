//! CF_HDROP-based drag data object with 7-Zip style deferred staging.
//!
//! This implementation uses the same mechanism as 7-Zip:
//! 1. During drag/hover: Returns HDROP with just a placeholder temp
//!    folder path -- **no staging, no facade calls, no archive I/O**
//! 2. On actual drop: Stages the dragged selection through the
//!    [`DragPayloadSource`] (which blocks this STA thread on the
//!    application facade's drag-stage operation), then returns HDROP
//!    with the real staged paths
//!
//! This ensures a smooth drag cursor and staging only on drop. The
//! shell frequently queries a drag target's data without ever dropping
//! (Explorer calls `GetData(CF_HDROP)` during hover to inspect paths);
//! the pre-HDROP is what answers those queries for free -- see this
//! file's own tests, which pin exactly that against a counting fake
//! source.
//!
//! All archive knowledge lives behind [`DragPayloadSource`]; this file
//! is pure COM/shell mechanics and holds no `arclain_core` (or even
//! `arclain_app`) types beyond what the payload seam re-exports.

use super::drop_source::DragState;
use crate::platform::drag_source::payload::{
    DragPayloadSource, DragProgressUpdate, StagedDragPayload,
};
use parking_lot::RwLock;
use std::os::windows::ffi::OsStrExt;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::mpsc::Sender;
use std::sync::Arc;
use std::time::Instant;
use tracing::{debug, info, warn};
use windows::core::{implement, HRESULT};
use windows::Win32::Foundation::{GlobalFree, HGLOBAL};
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

/// CF_HDROP-based data object with 7-Zip style deferred staging.
///
/// Uses dual-HDROP mechanism:
/// - `hdrop_pre`: Contains just a placeholder temp folder (returned
///   during hover, built eagerly at construction -- cheap, an empty dir)
/// - `hdrop_final`: Contains the actual staged paths (built once staging
///   has run at drop time)
#[implement(windows::Win32::System::Com::IDataObject)]
pub struct HDropDataObject {
    /// Stages the dragged selection on demand -- the one seam to the
    /// application. Never invoked during hover.
    source: Arc<dyn DragPayloadSource>,

    /// Archive-root-relative paths of the rows the user dragged, used
    /// to shape the final HDROP's top-level items under the staging
    /// root. Folder rows appear as themselves (their staged subtree
    /// lands beneath them), so the selection alone determines the
    /// HDROP's top level.
    selection_paths: Vec<String>,

    progress_tx: Option<Sender<DragProgressUpdate>>,

    /// Shared state with DropSourceWithState
    drag_state: Arc<DragState>,

    /// Pre-built HDROP with just the placeholder folder (for hover)
    hdrop_pre: RwLock<Option<HGLOBAL>>,

    /// Pre-built HDROP with final staged paths (for after staging)
    hdrop_final: RwLock<Option<HGLOBAL>>,

    /// Placeholder directory the pre-HDROP names. Nothing is ever
    /// written into it; it exists so the hover-time HDROP names a real
    /// path. Removed by `TempDir`'s own Drop when the shell releases
    /// this object.
    pre_placeholder: RwLock<Option<tempfile::TempDir>>,

    /// The staged payload, present once a drop has triggered staging.
    /// Owns the staged files (for the facade-backed source, a
    /// self-renewing materialization lease released when this object is
    /// released by the shell).
    staged: RwLock<Option<StagedDragPayload>>,
}

impl HDropDataObject {
    pub fn new(
        source: Arc<dyn DragPayloadSource>,
        selection_paths: Vec<String>,
        progress_tx: Option<Sender<DragProgressUpdate>>,
        drag_state: Arc<DragState>,
    ) -> Self {
        info!(
            "[hdrop] Creating HDropDataObject for {} selected paths (deferred staging)",
            selection_paths.len()
        );

        let obj = Self {
            source,
            selection_paths,
            progress_tx,
            drag_state,
            hdrop_pre: RwLock::new(None),
            hdrop_final: RwLock::new(None),
            pre_placeholder: RwLock::new(None),
            staged: RwLock::new(None),
        };

        // Build the pre-HDROP immediately (just a placeholder folder
        // path) so hover-time GetData calls have an answer that costs
        // no archive I/O.
        if let Err(e) = obj.build_pre_hdrop() {
            warn!("[hdrop] Failed to build pre-HDROP: {}", e);
        }

        obj
    }

    /// Build the "pre" HDROP containing just a placeholder folder path.
    /// This is returned during hover so Explorer doesn't see missing
    /// files.
    fn build_pre_hdrop(&self) -> Result<(), String> {
        let placeholder =
            tempfile::tempdir().map_err(|e| format!("Failed to create temp dir: {}", e))?;
        let placeholder_path = placeholder.path().to_path_buf();

        info!(
            "[hdrop] Created placeholder dir for pre-HDROP: {}",
            placeholder_path.display()
        );

        let hdrop = self.build_hdrop_for_paths(&[placeholder_path])?;

        *self.pre_placeholder.write() = Some(placeholder);
        *self.hdrop_pre.write() = Some(hdrop);
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

    /// Stage the dragged selection and build the "final" HDROP. Runs on
    /// the drag STA thread, at drop time only. Idempotent: a second call
    /// (the shell can ask for CF_HDROP more than once while processing
    /// the drop) reuses the already-staged payload.
    ///
    /// Reentrancy note: COM calls arrive on this object's STA thread
    /// one at a time, and staging does not pump messages while it
    /// blocks, so the check-then-stage below cannot interleave with
    /// itself.
    fn do_staging(&self) -> Result<(), String> {
        if self.staged.read().is_some() {
            return Ok(()); // Already staged
        }

        let start = Instant::now();
        info!(
            "[hdrop] Drop committed - staging {} selected paths",
            self.selection_paths.len()
        );

        let staged = match &self.progress_tx {
            Some(tx) => {
                // Progress flows to the egui drag dialog through the
                // same channel shape the pre-facade extraction used.
                let tx = tx.clone();
                let mut forward = move |update: DragProgressUpdate| {
                    let _ = tx.send(update);
                };
                self.source.stage_blocking(&mut forward)?
            }
            None => {
                // No frontend progress channel: drive a native Windows
                // progress dialog instead (the pre-facade fallback,
                // preserved).
                crate::platform::drag_source::native_progress::stage_with_native_progress(
                    Arc::clone(&self.source),
                    self.selection_paths.len(),
                )?
            }
        };

        if let Some(tx) = &self.progress_tx {
            let _ = tx.send(DragProgressUpdate {
                percent: 100,
                message: Some("Staging complete".to_string()),
            });
        }

        info!(
            "[hdrop] Staging complete in {:.2}s at {}",
            start.elapsed().as_secs_f64(),
            staged.root().display()
        );

        self.build_final_hdrop(&staged)?;
        *self.staged.write() = Some(staged);
        Ok(())
    }

    /// Build the "final" HDROP with the staged top-level paths.
    fn build_final_hdrop(&self, staged: &StagedDragPayload) -> Result<(), String> {
        let root = staged.root();

        let hdrop_paths: Vec<PathBuf> =
            if let Some(root_folder) = self.find_root_folder(&self.selection_paths) {
                let folder_path = root.join(&root_folder);
                if folder_path.exists() && folder_path.is_dir() {
                    info!("[hdrop] Final HDROP: root folder {}", folder_path.display());
                    vec![folder_path]
                } else {
                    self.collect_top_level_items(root, &self.selection_paths)
                }
            } else {
                self.collect_top_level_items(root, &self.selection_paths)
            };

        if hdrop_paths.is_empty() {
            return Err("staging produced no top-level items to hand to the shell".to_string());
        }

        info!(
            "[hdrop] Building final HDROP with {} items",
            hdrop_paths.len()
        );

        let hdrop = self.build_hdrop_for_paths(&hdrop_paths)?;
        let previous = self.hdrop_final.write().replace(hdrop);
        if let Some(previous) = previous {
            // Unreachable in practice (staging runs once), but never
            // leak a master allocation if it somehow ran twice.
            let _ = unsafe { GlobalFree(previous) };
        }
        Ok(())
    }

    /// Find common root folder if all paths start with the same directory.
    fn find_root_folder(&self, paths: &[String]) -> Option<String> {
        if paths.is_empty() {
            return None;
        }

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

        if root_candidates.len() == 1 {
            let root = root_candidates.into_iter().next().unwrap();
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

    /// Collect top-level items (files and folders at root of the staged
    /// payload).
    fn collect_top_level_items(
        &self,
        staged_root: &std::path::Path,
        paths: &[String],
    ) -> Vec<PathBuf> {
        let mut top_level: std::collections::HashSet<PathBuf> = std::collections::HashSet::new();

        for path in paths {
            let normalized = path
                .replace('/', std::path::MAIN_SEPARATOR_STR)
                .replace('\\', std::path::MAIN_SEPARATOR_STR);

            let first_component = normalized
                .split(std::path::MAIN_SEPARATOR)
                .next()
                .unwrap_or(&normalized);

            let full_path = staged_root.join(first_component);
            if full_path.exists() {
                top_level.insert(full_path);
            }
        }

        top_level.into_iter().collect()
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

        // Check if we need to stage (drop was triggered)
        if need_extract && !extract_done {
            info!("[hdrop] Drop detected - staging selection");
            if let Err(e) = self.do_staging() {
                warn!("[hdrop] Staging failed: {}", e);
                return Err(windows::core::Error::from(E_UNEXPECTED));
            }
            self.drag_state.extract_done.store(true, Ordering::SeqCst);
        }

        // Return appropriate HDROP
        let hglobal = if use_pre {
            debug!("[hdrop] Returning pre-HDROP (placeholder folder only)");
            self.hdrop_pre.read().ok_or_else(|| {
                warn!("[hdrop] Pre-HDROP not built");
                windows::core::Error::from(E_UNEXPECTED)
            })?
        } else {
            debug!("[hdrop] Returning final HDROP (staged files)");
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

impl Drop for HDropDataObject {
    fn drop(&mut self) {
        // The pre/final HDROP masters are ours (only their duplicates
        // are handed to callers) -- free them rather than leaking one
        // pair of GlobalAlloc blocks per drag, which the pre-facade
        // version silently did.
        for slot in [&self.hdrop_pre, &self.hdrop_final] {
            if let Some(hglobal) = slot.write().take() {
                let _ = unsafe { GlobalFree(hglobal) };
            }
        }
        // `pre_placeholder` and `staged` clean themselves up via their
        // own Drop impls (TempDir removal; lease release).
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

#[cfg(test)]
mod tests {
    //! COM-level tests against a counting fake [`DragPayloadSource`] --
    //! what lets "a target queried but never dropped must not stage" be
    //! pinned against the real `IDataObject` state machine without a
    //! live shell. Method calls go straight through the
    //! `IDataObject_Impl` trait on the struct; no OLE apartment or
    //! marshaling is involved, which is fine for vtable-free direct
    //! dispatch.

    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicUsize};
    use windows::Win32::System::Com::IDataObject_Impl;

    /// Counts staging calls; stages by writing the configured files
    /// under a fresh temp root. `fail` scripts an error instead.
    struct CountingSource {
        stage_calls: AtomicUsize,
        files: Vec<(String, Vec<u8>)>,
        fail: bool,
        guard_dropped: Arc<AtomicBool>,
    }

    impl CountingSource {
        fn new(files: &[(&str, &[u8])]) -> Arc<Self> {
            Arc::new(Self {
                stage_calls: AtomicUsize::new(0),
                files: files
                    .iter()
                    .map(|(p, b)| (p.to_string(), b.to_vec()))
                    .collect(),
                fail: false,
                guard_dropped: Arc::new(AtomicBool::new(false)),
            })
        }

        fn failing() -> Arc<Self> {
            Arc::new(Self {
                stage_calls: AtomicUsize::new(0),
                files: Vec::new(),
                fail: true,
                guard_dropped: Arc::new(AtomicBool::new(false)),
            })
        }

        fn calls(&self) -> usize {
            self.stage_calls.load(Ordering::SeqCst)
        }
    }

    /// Flags its owner's `guard_dropped` when the staged payload is
    /// dropped -- pins that releasing the COM object releases whatever
    /// owns the staged files.
    struct DropFlag(Arc<AtomicBool>);
    impl Drop for DropFlag {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    impl DragPayloadSource for CountingSource {
        fn stage_blocking(
            &self,
            on_progress: &mut dyn FnMut(DragProgressUpdate),
        ) -> Result<StagedDragPayload, String> {
            self.stage_calls.fetch_add(1, Ordering::SeqCst);
            if self.fail {
                return Err("scripted staging failure".to_string());
            }
            on_progress(DragProgressUpdate {
                percent: 50,
                message: Some("staging".to_string()),
            });
            let root = tempfile::tempdir().expect("create fake staging root");
            for (path, bytes) in &self.files {
                let target = root.path().join(path);
                if let Some(parent) = target.parent() {
                    std::fs::create_dir_all(parent).unwrap();
                }
                std::fs::write(target, bytes).unwrap();
            }
            let root_path = root.path().to_path_buf();
            Ok(StagedDragPayload::new(
                root_path,
                (root, DropFlag(self.guard_dropped.clone())),
            ))
        }

        fn request_cancel(&self) {}
    }

    fn hdrop_format() -> FORMATETC {
        FORMATETC {
            cfFormat: CF_HDROP,
            ptd: std::ptr::null_mut(),
            dwAspect: DVASPECT_CONTENT.0,
            lindex: -1,
            tymed: TYMED_HGLOBAL.0 as u32,
        }
    }

    /// Reads the wide paths out of a CF_HDROP STGMEDIUM and frees the
    /// duplicated HGLOBAL the object handed us ownership of.
    fn paths_of(medium: STGMEDIUM) -> Vec<PathBuf> {
        let mut out = Vec::new();
        unsafe {
            let hglobal = medium.u.hGlobal;
            let base = GlobalLock(hglobal) as *const u8;
            assert!(!base.is_null());
            let dropfiles = &*(base as *const DROPFILES);
            assert!(dropfiles.fWide.as_bool());
            let mut cursor = base.add(dropfiles.pFiles as usize) as *const u16;
            loop {
                let mut wide = Vec::new();
                while *cursor != 0 {
                    wide.push(*cursor);
                    cursor = cursor.add(1);
                }
                cursor = cursor.add(1); // skip the string's own null
                if wide.is_empty() {
                    break; // double null: list end
                }
                out.push(PathBuf::from(String::from_utf16(&wide).unwrap()));
            }
            let _ = GlobalUnlock(hglobal);
            let _ = GlobalFree(hglobal);
        }
        out
    }

    fn dropped_state(state: &Arc<DragState>) {
        state.use_pre_global.store(false, Ordering::SeqCst);
        state.need_extract.store(true, Ordering::SeqCst);
    }

    #[test]
    fn hover_queries_are_served_from_the_placeholder_and_never_stage() {
        // THE pinned optimization: the shell frequently queries a drag
        // target's CF_HDROP without ever dropping. In the default
        // (hover) drag state, GetData/QueryGetData/EnumFormatEtc must
        // all answer without a single staging call.
        let source = CountingSource::new(&[("RJ123456/scene_a.dat", b"bytes")]);
        let state = DragState::new();
        let (tx, _rx) = std::sync::mpsc::channel();
        let obj = HDropDataObject::new(
            source.clone(),
            vec!["RJ123456/scene_a.dat".to_string()],
            Some(tx),
            Arc::clone(&state),
        );

        let fmt = hdrop_format();
        assert_eq!(obj.QueryGetData(&fmt), S_OK);
        let _ = obj.EnumFormatEtc(DATADIR_GET.0 as u32).unwrap();
        for _ in 0..5 {
            let medium = obj
                .GetData(&fmt)
                .expect("hover-time GetData must succeed from the placeholder");
            let paths = paths_of(medium);
            assert_eq!(paths.len(), 1, "hover HDROP names only the placeholder");
        }

        assert_eq!(
            source.calls(),
            0,
            "a target queried but never dropped must not stage/extract anything"
        );
    }

    #[test]
    fn a_committed_drop_stages_exactly_once_and_serves_the_staged_top_level() {
        let source = CountingSource::new(&[
            ("RJ123456/scene_a.dat", b"aaaa".as_slice()),
            ("RJ123456/img/cover.png", b"bb".as_slice()),
        ]);
        let state = DragState::new();
        let (tx, rx) = std::sync::mpsc::channel();
        let obj = HDropDataObject::new(
            source.clone(),
            vec![
                "RJ123456/scene_a.dat".to_string(),
                "RJ123456/img/cover.png".to_string(),
            ],
            Some(tx),
            Arc::clone(&state),
        );

        dropped_state(&state);
        let fmt = hdrop_format();

        let first = paths_of(obj.GetData(&fmt).expect("drop-time GetData must stage"));
        // Both selected paths share the "RJ123456" first component, so
        // the final HDROP is that single root folder under the staged
        // root -- and its content is what staging wrote.
        assert_eq!(first.len(), 1);
        assert!(first[0].ends_with("RJ123456"), "got {first:?}");
        assert_eq!(
            std::fs::read(first[0].join("scene_a.dat")).unwrap(),
            b"aaaa"
        );
        assert_eq!(
            std::fs::read(first[0].join("img/cover.png")).unwrap(),
            b"bb"
        );

        // The shell may ask again while processing the drop: served from
        // the same staged payload, no second stage.
        let second = paths_of(obj.GetData(&fmt).unwrap());
        assert_eq!(second, first);
        assert_eq!(source.calls(), 1, "staging must run exactly once per drag");

        // Progress reached the frontend channel.
        let updates: Vec<DragProgressUpdate> = rx.try_iter().collect();
        assert!(
            updates.iter().any(|u| u.percent == 50),
            "the staging progress tick must reach the drag progress channel"
        );
        assert!(
            updates.iter().any(|u| u.percent == 100),
            "a final 100% update must reach the drag progress channel"
        );
    }

    #[test]
    fn a_failed_stage_surfaces_as_a_com_error_without_panicking() {
        let source = CountingSource::failing();
        let state = DragState::new();
        let (tx, _rx) = std::sync::mpsc::channel();
        let obj = HDropDataObject::new(
            source.clone(),
            vec!["RJ123456/scene_a.dat".to_string()],
            Some(tx),
            Arc::clone(&state),
        );

        dropped_state(&state);
        let error = match obj.GetData(&hdrop_format()) {
            Err(error) => error,
            Ok(_) => panic!("a failed stage must fail the GetData call"),
        };
        assert_eq!(error.code(), E_UNEXPECTED);
        assert_eq!(source.calls(), 1);
    }

    #[test]
    fn dropping_the_data_object_drops_the_staged_payload_guard() {
        let source = CountingSource::new(&[("RJ123456/scene_a.dat", b"x")]);
        let state = DragState::new();
        let (tx, _rx) = std::sync::mpsc::channel();
        let obj = HDropDataObject::new(
            source.clone(),
            vec!["RJ123456/scene_a.dat".to_string()],
            Some(tx),
            Arc::clone(&state),
        );

        dropped_state(&state);
        let medium = obj.GetData(&hdrop_format()).unwrap();
        let staged_paths = paths_of(medium);
        assert!(!source.guard_dropped.load(Ordering::SeqCst));

        drop(obj);

        assert!(
            source.guard_dropped.load(Ordering::SeqCst),
            "releasing the data object must drop the staged payload's keep-alive guard \
             (for the facade source: release the materialization lease)"
        );
        // And the fake's TempDir-backed staging root is gone with it.
        assert!(!staged_paths[0].exists());
    }
}
