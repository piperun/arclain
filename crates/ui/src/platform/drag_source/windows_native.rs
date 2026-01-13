//! Native Windows drag-out with deferred file extraction
//!
//! Uses IDataObject and IDropSource COM interfaces to implement
//! lazy file extraction - files are only extracted when the user drops.

use parking_lot::RwLock;
use std::path::PathBuf;
use windows::{
    core::{implement, Error, HRESULT},
    Win32::{
        Foundation::{BOOL, E_NOTIMPL, S_FALSE, S_OK},
        System::{
            Com::*,
            Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE},
            Ole::{OleInitialize, OleUninitialize, *},
            SystemServices::{MK_LBUTTON, MODIFIERKEYS_FLAGS},
        },
    },
};

// Make DROPEFFECT public so mod.rs can use it in return type signature
pub use windows::Win32::System::Ole::DROPEFFECT;

// Manually define constants that might be missing or in different paths
const CF_HDROP: u16 = 15;
#[allow(non_snake_case)]
const DRAGDROP_S_DROP: HRESULT = HRESULT(0x00040100);
#[allow(non_snake_case)]
const DRAGDROP_S_CANCEL: HRESULT = HRESULT(0x00040101);
#[allow(non_snake_case)]
const DRAGDROP_S_USEDEFAULTCURSORS: HRESULT = HRESULT(0x00040102);

/// Simple FORMATETC enumerator for CF_HDROP
#[implement(IEnumFORMATETC)]
pub struct FormatEnumerator {
    index: RwLock<usize>,
}

impl FormatEnumerator {
    fn new() -> Self {
        Self {
            index: RwLock::new(0),
        }
    }

    fn get_hdrop_format() -> FORMATETC {
        FORMATETC {
            cfFormat: CF_HDROP,
            ptd: std::ptr::null_mut(),
            dwAspect: DVASPECT_CONTENT.0 as u32,
            lindex: -1,
            tymed: TYMED_HGLOBAL.0 as u32,
        }
    }
}

#[allow(non_snake_case)]
impl IEnumFORMATETC_Impl for FormatEnumerator {
    fn Next(&self, celt: u32, rgelt: *mut FORMATETC, pceltfetched: *mut u32) -> HRESULT {
        let mut index = self.index.write();
        let mut fetched = 0u32;

        if *index == 0 && celt >= 1 {
            unsafe {
                *rgelt = Self::get_hdrop_format();
            }
            fetched = 1;
            *index = 1;
        }

        if !pceltfetched.is_null() {
            unsafe {
                *pceltfetched = fetched;
            }
        }

        if fetched == celt {
            S_OK
        } else {
            S_FALSE
        }
    }

    fn Skip(&self, celt: u32) -> windows::core::Result<()> {
        let mut index = self.index.write();
        *index = (*index + celt as usize).min(1);
        Ok(())
    }

    fn Reset(&self) -> windows::core::Result<()> {
        *self.index.write() = 0;
        Ok(())
    }

    fn Clone(&self) -> windows::core::Result<IEnumFORMATETC> {
        let new_enum = FormatEnumerator::new();
        *new_enum.index.write() = *self.index.read();
        Ok(new_enum.into())
    }
}

/// Callback for extracting files on drop
pub type ExtractCallback = Box<dyn Fn() -> std::result::Result<Vec<PathBuf>, String> + Send + Sync>;

/// Deferred data object that extracts files only when GetData is called
#[implement(IDataObject)]
pub struct LazyArchiveDataObject {
    /// Callback to extract files when data is requested
    extract_callback: ExtractCallback,
    /// Cached extracted paths (set after first GetData call)
    extracted_paths: RwLock<Option<Vec<PathBuf>>>,
}

impl LazyArchiveDataObject {
    pub fn new(extract_callback: ExtractCallback) -> Self {
        Self {
            extract_callback,
            extracted_paths: RwLock::new(None),
        }
    }

    /// Build HDROP structure from file paths
    fn build_hdrop(&self, paths: &[PathBuf]) -> std::result::Result<Vec<u8>, String> {
        let mut buffer = Vec::new();

        // DROPFILES structure: pFiles (4), pt.x (4), pt.y (4), fNC (4), fWide (4) = 20 bytes
        buffer.extend_from_slice(&20u32.to_le_bytes()); // pFiles offset
        buffer.extend_from_slice(&0i32.to_le_bytes()); // pt.x
        buffer.extend_from_slice(&0i32.to_le_bytes()); // pt.y
        buffer.extend_from_slice(&0u32.to_le_bytes()); // fNC
        buffer.extend_from_slice(&1u32.to_le_bytes()); // fWide = TRUE

        for path in paths {
            // Canonicalize to get absolute path with proper backslashes
            let canonical = path.canonicalize().unwrap_or_else(|_| path.clone());
            let mut path_str = canonical.to_string_lossy().to_string();

            // Strip \\?\ prefix - Explorer doesn't accept extended-length paths in HDROP
            if path_str.starts_with(r"\\?\") {
                path_str = path_str[4..].to_string();
            }

            // Debug: log exact path and check existence
            let exists = std::path::Path::new(&path_str).exists();
            tracing::debug!("HDROP path: '{}' (exists: {})", path_str, exists);

            for c in path_str.encode_utf16() {
                buffer.extend_from_slice(&c.to_le_bytes());
            }
            buffer.extend_from_slice(&0u16.to_le_bytes()); // null terminator
        }
        buffer.extend_from_slice(&0u16.to_le_bytes()); // double null terminator

        tracing::debug!("HDROP buffer size: {} bytes", buffer.len());
        Ok(buffer)
    }
}

#[allow(non_snake_case)]
impl IDataObject_Impl for LazyArchiveDataObject {
    fn GetData(&self, pformatetc: *const FORMATETC) -> windows::core::Result<STGMEDIUM> {
        let format = unsafe { &*pformatetc };

        // Debug: Log what Explorer is requesting
        tracing::debug!(
            "GetData called: cfFormat={}, tymed={}, dwAspect={}",
            format.cfFormat,
            format.tymed,
            format.dwAspect
        );

        // Fix: CF_HDROP is u16 constant, directly compare
        if format.cfFormat != CF_HDROP {
            tracing::debug!(
                "Rejecting cfFormat {} (want CF_HDROP={})",
                format.cfFormat,
                CF_HDROP
            );
            return Err(Error::from(E_NOTIMPL));
        }

        let mut cached = self.extracted_paths.write();
        if cached.is_none() {
            tracing::debug!("LazyArchiveDataObject: Extracting files on drop...");
            match (self.extract_callback)() {
                Ok(paths) => {
                    tracing::debug!("Extracted {} files", paths.len());
                    *cached = Some(paths);
                }
                Err(e) => {
                    tracing::error!("Extraction failed: {}", e);
                    return Err(Error::from(E_NOTIMPL));
                }
            }
        }

        let paths = cached.as_ref().unwrap();
        if paths.is_empty() {
            return Err(Error::from(E_NOTIMPL));
        }

        let hdrop_data = self
            .build_hdrop(paths)
            .map_err(|_| Error::from(E_NOTIMPL))?;

        // Debug: hex dump of first 40 bytes
        let hex_preview: String = hdrop_data
            .iter()
            .take(40)
            .map(|b| format!("{:02x}", b))
            .collect::<Vec<_>>()
            .join(" ");
        tracing::debug!("HDROP hex (first 40 bytes): {}", hex_preview);

        let hglobal = unsafe { GlobalAlloc(GMEM_MOVEABLE, hdrop_data.len()) }.map_err(|e| {
            tracing::error!("GlobalAlloc failed: {:?}", e);
            Error::from(E_NOTIMPL)
        })?;

        tracing::debug!("GlobalAlloc succeeded, hglobal={:?}", hglobal);

        unsafe {
            let ptr = GlobalLock(hglobal);
            tracing::debug!("GlobalLock returned: {:?}", ptr);
            if !ptr.is_null() {
                std::ptr::copy_nonoverlapping(
                    hdrop_data.as_ptr(),
                    ptr as *mut u8,
                    hdrop_data.len(),
                );
                let _ = GlobalUnlock(hglobal);
            } else {
                tracing::error!("GlobalLock returned null!");
            }
        }

        tracing::debug!(
            "Returning STGMEDIUM with tymed=TYMED_HGLOBAL({})",
            TYMED_HGLOBAL.0
        );
        Ok(STGMEDIUM {
            tymed: TYMED_HGLOBAL.0 as u32,
            u: STGMEDIUM_0 { hGlobal: hglobal },
            pUnkForRelease: std::mem::ManuallyDrop::new(None),
        })
    }

    fn GetDataHere(
        &self,
        _pformatetc: *const FORMATETC,
        _pmedium: *mut STGMEDIUM,
    ) -> windows::core::Result<()> {
        Err(Error::from(E_NOTIMPL))
    }

    fn QueryGetData(&self, pformatetc: *const FORMATETC) -> HRESULT {
        let format = unsafe { &*pformatetc };
        if format.cfFormat == CF_HDROP {
            S_OK
        } else {
            S_FALSE
        }
    }

    fn GetCanonicalFormatEtc(
        &self,
        _pformatectin: *const FORMATETC,
        _pformatetcout: *mut FORMATETC,
    ) -> HRESULT {
        E_NOTIMPL
    }

    fn SetData(
        &self,
        _pformatetc: *const FORMATETC,
        _pmedium: *const STGMEDIUM,
        _frelease: BOOL,
    ) -> windows::core::Result<()> {
        Err(Error::from(E_NOTIMPL))
    }

    fn EnumFormatEtc(&self, dwdirection: u32) -> windows::core::Result<IEnumFORMATETC> {
        // DATADIR_GET = 1, DATADIR_SET = 2
        if dwdirection == 1 {
            tracing::debug!("EnumFormatEtc: Returning CF_HDROP enumerator");
            Ok(FormatEnumerator::new().into())
        } else {
            Err(Error::from(E_NOTIMPL))
        }
    }

    fn DAdvise(
        &self,
        _pformatetc: *const FORMATETC,
        _advf: u32,
        _padvsink: Option<&IAdviseSink>,
    ) -> windows::core::Result<u32> {
        Err(Error::from(E_NOTIMPL))
    }

    fn DUnadvise(&self, _dwconnection: u32) -> windows::core::Result<()> {
        Err(Error::from(E_NOTIMPL))
    }

    fn EnumDAdvise(&self) -> windows::core::Result<IEnumSTATDATA> {
        Err(Error::from(E_NOTIMPL))
    }
}

/// Simple drop source that tracks drag state
#[implement(IDropSource)]
pub struct SimpleDropSource;

#[allow(non_snake_case)]
impl IDropSource_Impl for SimpleDropSource {
    fn QueryContinueDrag(&self, fescapepressed: BOOL, grfkeystate: MODIFIERKEYS_FLAGS) -> HRESULT {
        if fescapepressed.as_bool() {
            DRAGDROP_S_CANCEL
        } else if (grfkeystate.0 & MK_LBUTTON.0) == 0 {
            DRAGDROP_S_DROP
        } else {
            S_OK
        }
    }

    fn GiveFeedback(&self, _dweffect: DROPEFFECT) -> HRESULT {
        DRAGDROP_S_USEDEFAULTCURSORS
    }
}

/// Start a deferred drag operation
///
/// The callback will be invoked when the user drops.
pub fn start_deferred_drag(
    extract_callback: ExtractCallback,
) -> std::result::Result<DROPEFFECT, String> {
    unsafe {
        let _ = OleInitialize(None);
    }

    // Ensure OLE is uninitialized when function returns
    struct OleGuard;
    impl Drop for OleGuard {
        fn drop(&mut self) {
            unsafe { OleUninitialize() };
        }
    }
    let _ole_guard = OleGuard;

    let data_object: IDataObject = LazyArchiveDataObject::new(extract_callback).into();
    let drop_source: IDropSource = SimpleDropSource.into();

    let mut effect = DROPEFFECT_NONE;

    tracing::debug!("Starting deferred drag operation (native Windows)...");

    let result = unsafe {
        DoDragDrop(
            &data_object,
            &drop_source,
            DROPEFFECT_COPY | DROPEFFECT_MOVE,
            &mut effect,
        )
    };

    // Fix: DoDragDrop returns HRESULT, check against success codes
    if result == DRAGDROP_S_DROP {
        tracing::debug!("Drag completed with effect: {:?}", effect);
        Ok(effect)
    } else if result == DRAGDROP_S_CANCEL {
        tracing::debug!("Drag cancelled");
        Ok(effect) // Cancelled is valid result, effect should be NONE
    } else {
        tracing::warn!("Drag failed with HRESULT: {:?}", result);
        Err(format!("HRESULT: {:?}", result))
    }
}
