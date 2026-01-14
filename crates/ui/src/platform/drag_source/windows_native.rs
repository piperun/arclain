//! Native Windows drag-out with deferred file extraction
//!
//! Uses IDataObject and IDropSource COM interfaces to implement
//! lazy file extraction - files are only extracted when the user drops.

// mod stream; // declared in mod.rs now

use super::stream::ArchiveStream;
// use crate::core::state::ExtractCallback; // Removed (unused)
use arclain_core::{ArchiveBackend, ArchiveEntry};
use parking_lot::RwLock;

use std::path::PathBuf;
use std::sync::Arc;
use windows::core::{implement, Result, HRESULT};

use windows::Win32::Foundation::{
    BOOL, DRAGDROP_S_CANCEL, DRAGDROP_S_DROP, DRAGDROP_S_USEDEFAULTCURSORS, DV_E_FORMATETC,
    DV_E_LINDEX, E_NOTIMPL, E_UNEXPECTED, S_FALSE, S_OK,
};
use windows::Win32::Storage::FileSystem::FILE_ATTRIBUTE_NORMAL;
use windows::Win32::System::Com::{
    IAdviseSink, IDataObject, IEnumSTATDATA, DATADIR_GET, DVASPECT_CONTENT, FORMATETC, STGMEDIUM,
    TYMED_HGLOBAL, TYMED_ISTREAM,
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
            // Content (IStream)
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

#[implement(windows::Win32::System::Com::IDataObject)]
pub struct LazyArchiveDataObject {
    backend: Arc<dyn ArchiveBackend>,
    archive_path: PathBuf,
    entries: Vec<ArchiveEntry>,
    password: Option<String>,
}

impl LazyArchiveDataObject {
    pub fn new(
        backend: Arc<dyn ArchiveBackend>,
        archive_path: PathBuf,
        entries: Vec<ArchiveEntry>,
        password: Option<String>,
    ) -> Self {
        Self {
            backend,
            archive_path,
            entries,
            password,
        }
    }

    fn get_file_descriptor(&self) -> windows::core::Result<STGMEDIUM> {
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

                // Construct descriptor locally
                let descriptor = FILEDESCRIPTORW {
                    dwFlags: (FD_ATTRIBUTES.0 | FD_FILESIZE.0) as u32,
                    dwFileAttributes: FILE_ATTRIBUTE_NORMAL.0,
                    nFileSizeLow: entry.size as u32,
                    nFileSizeHigh: (entry.size >> 32) as u32,
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

        // Create stream
        let stream = ArchiveStream::new(
            self.backend.clone(),
            self.archive_path.clone(),
            entry.path.clone(),
            self.password.clone(),
            entry.size,
        );
        let stream_interface: windows::Win32::System::Com::IStream = stream.into();

        Ok(STGMEDIUM {
            tymed: TYMED_ISTREAM.0 as u32,
            u: windows::Win32::System::Com::STGMEDIUM_0 {
                pstm: std::mem::ManuallyDrop::new(Some(stream_interface)),
            },
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

        if format.cfFormat == fd_format && (format.tymed & TYMED_HGLOBAL.0 as u32) != 0 {
            self.get_file_descriptor()
        } else if format.cfFormat == fc_format && (format.tymed & TYMED_ISTREAM.0 as u32) != 0 {
            self.get_file_contents(format.lindex)
        } else {
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

/// Start a deferred drag operation using IStream
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
