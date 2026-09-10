use super::rar_headers::{self, Header};
use crate::{protocol::*, ImportError};
use std::{
    io::Write,
    panic::{catch_unwind, AssertUnwindSafe},
    path::{Path, PathBuf},
};
use unboxer::{Container, EntryName, Inventory, Limits, ObjectKind, PackedBlock, PhysicalEntry};
use unrar_sys as ffi;

pub(super) struct Archive {
    path: PathBuf,
    headers: Vec<Header>,
    pub inventory: Inventory,
}
impl Archive {
    pub fn open(source: &mut super::source::Source, limits: &Limits) -> Result<Self, ImportError> {
        let headers = rar_headers::attest(&mut source.file, limits)?;
        let path = source.native_path()?;
        let mut context = Context {
            out: None,
            error: None,
            written: 0,
            declared: 0,
        };
        let native = Native::open(&path, ffi::RAR_OM_LIST_INCSPLIT, &mut context)?;
        let mut inventory = Inventory {
            container: Container::Rar,
            entries: vec![],
            blocks: vec![],
        };
        while let Some(header) = native.header()? {
            let index = inventory.entries.len();
            let original = headers.get(index).ok_or(ImportError::Corrupt)?;
            attest_native(&header, original)?;
            if header.flags & (ffi::RHDF_SPLITBEFORE | ffi::RHDF_SPLITAFTER | ffi::RHDF_ENCRYPTED)
                != 0
            {
                return Err(ImportError::Unsupported);
            }
            let mode = (header.host_os == 3).then_some(header.file_attr);
            let kind = if header.redir_type != 0
                || header.host_os == 2 && header.file_attr & 0x400 != 0
                || mode.is_some_and(|m| m & 0o170000 == 0o120000)
            {
                ObjectKind::Link
            } else if mode.is_some_and(|m| !matches!(m & 0o170000, 0 | 0o040000 | 0o100000)) {
                ObjectKind::Special
            } else if header.flags & ffi::RHDF_DIRECTORY != 0 {
                ObjectKind::Directory
            } else {
                ObjectKind::File
            };
            let block = if kind == ObjectKind::Directory {
                None
            } else {
                let block = if header.flags & ffi::RHDF_SOLID != 0 {
                    inventory
                        .blocks
                        .len()
                        .checked_sub(1)
                        .ok_or(ImportError::Corrupt)?
                } else {
                    inventory.blocks.push(PackedBlock {
                        packed_bytes: 0,
                        unpacked_bytes: 0,
                    });
                    inventory.blocks.len() - 1
                };
                inventory.blocks[block].packed_bytes = inventory.blocks[block]
                    .packed_bytes
                    .checked_add(original.packed)
                    .ok_or(ImportError::Corrupt)?;
                inventory.blocks[block].unpacked_bytes = inventory.blocks[block]
                    .unpacked_bytes
                    .checked_add(original.size)
                    .ok_or(ImportError::Corrupt)?;
                Some(block)
            };
            inventory.entries.push(PhysicalEntry {
                index,
                name: EntryName::Unicode(original.name.clone()),
                kind,
                size: original.size,
                block,
                encrypted: false,
                method: format!("RAR:{}:{}", header.unp_ver, header.method),
                method_supported: header.unp_ver <= 70 && (0x30..=0x35).contains(&header.method),
                unix_mode: mode,
            });
            native.process(ffi::RAR_SKIP)?;
        }
        if inventory.entries.len() != headers.len() {
            return Err(ImportError::Corrupt);
        }
        drop(native);
        source.verify()?;
        Ok(Self {
            path,
            headers,
            inventory,
        })
    }
    pub fn decode(&mut self, out: &mut dyn Write) -> Result<(), ImportError> {
        let mut context = Context {
            out: Some(out),
            error: None,
            written: 0,
            declared: 0,
        };
        let native = Native::open(&self.path, ffi::RAR_OM_EXTRACT, &mut context)?;
        let mut count = 0;
        while let Some(header) = native.header()? {
            let original = self.headers.get(count).ok_or(ImportError::Corrupt)?;
            attest_native(&header, original)?;
            context.written = 0;
            context.declared = original.size;
            frame(
                context.out.as_deref_mut().unwrap(),
                BEGIN,
                &(count as u64).to_le_bytes(),
            )?;
            let result = native.process(ffi::RAR_TEST);
            if let Some(error) = context.error {
                return Err(error);
            }
            result?;
            if context.written != context.declared {
                return Err(ImportError::Corrupt);
            }
            frame(
                context.out.as_deref_mut().unwrap(),
                END,
                &(count as u64).to_le_bytes(),
            )?;
            count += 1;
        }
        if count != self.headers.len() {
            return Err(ImportError::Corrupt);
        }
        Ok(())
    }
}

struct Context<'a> {
    out: Option<&'a mut dyn Write>,
    error: Option<ImportError>,
    written: u64,
    declared: u64,
}
extern "C" fn callback(
    message: ffi::UINT,
    user: ffi::LPARAM,
    first: ffi::LPARAM,
    second: ffi::LPARAM,
) -> i32 {
    // SAFETY: user points at the stack context supplied to RAROpenArchiveEx;
    // the native handle is closed before that context leaves its scope, and
    // UnRAR invokes this synchronously. No panic crosses the native ABI.
    if user == 0 {
        return -1;
    }
    let context = unsafe { &mut *(user as *mut Context<'_>) };
    let result = catch_unwind(AssertUnwindSafe(|| -> Result<(), ImportError> {
        if message != ffi::UCM_PROCESSDATA {
            return Err(
                if message == ffi::UCM_NEEDPASSWORD || message == ffi::UCM_NEEDPASSWORDW {
                    ImportError::Encrypted
                } else {
                    ImportError::Unsupported
                },
            );
        }
        if first == 0 || second < 0 {
            return Err(ImportError::Corrupt);
        }
        let count = usize::try_from(second).map_err(|_| ImportError::Limit)?;
        context.written = context
            .written
            .checked_add(count as u64)
            .ok_or(ImportError::Limit)?;
        if context.written > context.declared {
            return Err(ImportError::Corrupt);
        }
        // SAFETY: PROCESSDATA supplies count readable bytes valid for this
        // callback. They are sliced and forwarded immediately, never retained.
        let bytes = unsafe { std::slice::from_raw_parts(first as *const u8, count) };
        let output = context.out.as_deref_mut().ok_or(ImportError::Corrupt)?;
        for chunk in bytes.chunks(MAX_FRAME) {
            frame(output, DATA, chunk)?;
        }
        Ok(())
    }));
    match result {
        Ok(Ok(())) => 1,
        Ok(Err(error)) => {
            context.error = Some(error);
            -1
        }
        Err(_) => {
            context.error = Some(ImportError::Corrupt);
            -1
        }
    }
}

struct Native(*const ffi::Handle);
impl Drop for Native {
    fn drop(&mut self) {
        unsafe {
            ffi::RARCloseArchive(self.0);
        }
    }
}
impl Native {
    fn open(path: &Path, mode: u32, context: &mut Context<'_>) -> Result<Self, ImportError> {
        // SAFETY: the FFI contract requires all reserved fields initialized to
        // zero. Filename buffers and callback context outlive this call; the
        // native engine copies the archive path during open.
        let mut data: ffi::OpenArchiveDataEx = unsafe { std::mem::zeroed() };
        data.open_mode = mode;
        data.callback = Some(callback);
        data.user_data = context as *mut _ as ffi::LPARAM;
        #[cfg(windows)]
        let encoded = {
            use std::os::windows::ffi::OsStrExt;
            let mut p = path.as_os_str().encode_wide().collect::<Vec<_>>();
            if p.contains(&0) {
                return Err(ImportError::Unsupported);
            }
            p.push(0);
            data.archive_name_w = p.as_ptr();
            p
        };
        #[cfg(unix)]
        let encoded = {
            use std::os::unix::ffi::OsStrExt;
            let p = std::ffi::CString::new(path.as_os_str().as_bytes())
                .map_err(|_| ImportError::Unsupported)?;
            data.archive_name = p.as_ptr();
            p
        };
        // The upstream binding declares const pointers, but the native DLL
        // writes these structures. Pass mutable raw pointers, never shared refs.
        let handle = unsafe { ffi::RAROpenArchiveEx(std::ptr::addr_of_mut!(data)) };
        drop(encoded);
        if handle.is_null() {
            return Err(map_error(data.open_result as i32));
        }
        let native = Self(handle);
        if data.open_result != 0 {
            return Err(map_error(data.open_result as i32));
        }
        if data.flags & (ffi::ROADF_VOLUME | ffi::ROADF_ENCHEADERS) != 0 {
            return Err(ImportError::Unsupported);
        }
        Ok(native)
    }
    fn header(&self) -> Result<Option<ffi::HeaderDataEx>, ImportError> {
        let mut header: ffi::HeaderDataEx = unsafe { std::mem::zeroed() };
        match unsafe { ffi::RARReadHeaderEx(self.0, std::ptr::addr_of_mut!(header)) } {
            ffi::ERAR_SUCCESS => Ok(Some(header)),
            ffi::ERAR_END_ARCHIVE => Ok(None),
            code => Err(map_error(code)),
        }
    }
    fn process(&self, operation: i32) -> Result<(), ImportError> {
        let result =
            unsafe { ffi::RARProcessFile(self.0, operation, std::ptr::null(), std::ptr::null()) };
        if result == 0 {
            Ok(())
        } else {
            Err(map_error(result))
        }
    }
}
fn map_error(code: i32) -> ImportError {
    match code {
        ffi::ERAR_MISSING_PASSWORD | ffi::ERAR_BAD_PASSWORD => ImportError::Encrypted,
        ffi::ERAR_NO_MEMORY => ImportError::Limit,
        _ => ImportError::Corrupt,
    }
}
fn attest_native(header: &ffi::HeaderDataEx, original: &Header) -> Result<(), ImportError> {
    let length = header
        .filename_w
        .iter()
        .position(|c| *c == 0)
        .ok_or(ImportError::Unsupported)?;
    if length >= 1023 {
        return Err(ImportError::Unsupported);
    }
    #[cfg(windows)]
    let name =
        String::from_utf16(&header.filename_w[..length]).map_err(|_| ImportError::Unsupported)?;
    #[cfg(unix)]
    let name = header.filename_w[..length]
        .iter()
        .map(|c| char::from_u32(*c as u32).ok_or(ImportError::Unsupported))
        .collect::<Result<String, _>>()?;
    let name = if cfg!(windows) {
        name.replace('\\', "/")
    } else {
        name
    };
    let size = (header.unp_size_high as u64) << 32 | header.unp_size as u64;
    let packed = (header.pack_size_high as u64) << 32 | header.pack_size as u64;
    if name != original.name
        || size != original.size
        || packed != original.packed
        || header.host_os != original.host
        || header.file_attr != original.attributes
        || (header.flags & ffi::RHDF_DIRECTORY != 0) != original.directory
    {
        return Err(ImportError::Unsupported);
    }
    Ok(())
}
