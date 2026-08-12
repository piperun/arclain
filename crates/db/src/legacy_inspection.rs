//! Read-only inspection of the two legacy network-settings stores.
//!
//! The redb source is never handed to `redb::Database`: its public database
//! opener is write-capable and may repair on open or persist allocator state on
//! drop. Instead, this module holds redb's own whole-file lock on a read-only
//! source handle, copies a bounded snapshot into zeroizing memory, and lets all
//! mandatory redb writes land only in that memory backend.

use std::fmt;
use std::fs::{self, File, Metadata, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::RwLock;
use std::thread;
use std::time::Duration;

use redb::backends::FileBackend;
use redb::{StorageBackend, TableDefinition};
use rusqlite::ffi::ErrorCode;
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use sha2::{Digest, Sha256};
use zeroize::{Zeroize, Zeroizing};

const SOCKS5_PASSWORD_KEY: &str = "proxy:socks5";
const METADATA_TABLE: TableDefinition<&str, &[u8]> = TableDefinition::new("metadata");
const SOURCE_LIMIT: u64 = 64 * 1024 * 1024;
const MEMORY_LIMIT: usize = 128 * 1024 * 1024;
const COPY_CHUNK: usize = 1024 * 1024;
const LOCK_ATTEMPTS: usize = 3;
const LOCK_RETRY_DELAY: Duration = Duration::from_millis(10);

/// Stable semantic classes a caller maps into its own error envelope.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LegacyInspectionErrorKind {
    Busy,
    PermissionDenied,
    Backend,
}

/// Bounded and path-free: legacy profile paths and backend internals do not
/// belong in a frontend diagnostic.
pub struct LegacyInspectionError {
    kind: LegacyInspectionErrorKind,
    message: &'static str,
}

impl LegacyInspectionError {
    fn new(kind: LegacyInspectionErrorKind, message: &'static str) -> Self {
        Self { kind, message }
    }

    pub fn kind(&self) -> LegacyInspectionErrorKind {
        self.kind
    }
}

impl fmt::Debug for LegacyInspectionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("LegacyInspectionError")
            .field("kind", &self.kind)
            .field("message", &self.message)
            .finish()
    }
}

impl fmt::Display for LegacyInspectionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.message)
    }
}

impl std::error::Error for LegacyInspectionError {}

fn io_error(error: io::Error, message: &'static str) -> LegacyInspectionError {
    let kind = if error.kind() == io::ErrorKind::PermissionDenied {
        LegacyInspectionErrorKind::PermissionDenied
    } else {
        LegacyInspectionErrorKind::Backend
    };
    LegacyInspectionError::new(kind, message)
}

fn backend_error(message: &'static str) -> LegacyInspectionError {
    LegacyInspectionError::new(LegacyInspectionErrorKind::Backend, message)
}

fn busy_error(message: &'static str) -> LegacyInspectionError {
    LegacyInspectionError::new(LegacyInspectionErrorKind::Busy, message)
}

fn permission_error(message: &'static str) -> LegacyInspectionError {
    LegacyInspectionError::new(LegacyInspectionErrorKind::PermissionDenied, message)
}

fn sqlite_error(error: rusqlite::Error, message: &'static str) -> LegacyInspectionError {
    match error.sqlite_error_code() {
        Some(ErrorCode::DatabaseBusy | ErrorCode::DatabaseLocked | ErrorCode::CannotOpen) => {
            busy_error("legacy configuration source is currently in use or changed")
        }
        Some(ErrorCode::PermissionDenied | ErrorCode::ReadOnly) => permission_error(message),
        _ => backend_error(message),
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
struct FileIdentity {
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
    #[cfg(windows)]
    volume_serial: u32,
    #[cfg(windows)]
    file_index: u64,
}

#[cfg(unix)]
fn file_identity(file: &File) -> Result<FileIdentity, LegacyInspectionError> {
    use std::os::unix::fs::MetadataExt;

    let metadata = file
        .metadata()
        .map_err(|error| io_error(error, "legacy storage identity could not be read"))?;
    Ok(FileIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
    })
}

#[cfg(windows)]
fn file_identity(file: &File) -> Result<FileIdentity, LegacyInspectionError> {
    use std::ffi::c_void;
    use std::os::windows::io::AsRawHandle;

    #[repr(C)]
    struct FileTime {
        low: u32,
        high: u32,
    }

    #[repr(C)]
    struct ByHandleFileInformation {
        file_attributes: u32,
        creation_time: FileTime,
        last_access_time: FileTime,
        last_write_time: FileTime,
        volume_serial_number: u32,
        file_size_high: u32,
        file_size_low: u32,
        number_of_links: u32,
        file_index_high: u32,
        file_index_low: u32,
    }

    unsafe extern "system" {
        fn GetFileInformationByHandle(
            file: *mut c_void,
            information: *mut ByHandleFileInformation,
        ) -> i32;
    }

    let mut information = std::mem::MaybeUninit::<ByHandleFileInformation>::uninit();
    let succeeded = unsafe {
        GetFileInformationByHandle(file.as_raw_handle().cast(), information.as_mut_ptr())
    };
    if succeeded == 0 {
        return Err(io_error(
            io::Error::last_os_error(),
            "legacy storage identity could not be read",
        ));
    }
    let information = unsafe { information.assume_init() };
    Ok(FileIdentity {
        volume_serial: information.volume_serial_number,
        file_index: ((information.file_index_high as u64) << 32)
            | information.file_index_low as u64,
    })
}

#[cfg(not(any(unix, windows)))]
fn file_identity(_file: &File) -> Result<FileIdentity, LegacyInspectionError> {
    Err(backend_error(
        "legacy storage inspection is unsupported on this platform",
    ))
}

fn is_reparse_point(metadata: &Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
    }
    #[cfg(not(windows))]
    {
        let _ = metadata;
        false
    }
}

fn validate_regular_path(path: &Path) -> Result<Option<Metadata>, LegacyInspectionError> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(io_error(error, "legacy storage metadata could not be read"));
        }
    };
    if metadata.file_type().is_symlink() || is_reparse_point(&metadata) {
        return Err(permission_error(
            "legacy storage links and reparse points are not allowed",
        ));
    }
    if !metadata.is_file() {
        return Err(permission_error(
            "legacy storage source is not a regular file",
        ));
    }
    Ok(Some(metadata))
}

fn open_regular_read_only(path: &Path) -> Result<Option<File>, LegacyInspectionError> {
    if validate_regular_path(path)?.is_none() {
        return Ok(None);
    }
    let file = match OpenOptions::new().read(true).open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(io_error(error, "legacy storage could not be opened")),
    };
    let metadata = file
        .metadata()
        .map_err(|error| io_error(error, "legacy storage metadata could not be read"))?;
    if !metadata.is_file() || validate_regular_path(path)?.is_none() {
        return Err(permission_error(
            "legacy storage source is not a regular file",
        ));
    }
    // Bind the opened handle back to the currently named path. This closes the
    // validate/open race without ever following a link as an accepted source:
    // a replacement between either operation produces a different identity.
    let confirmed = OpenOptions::new()
        .read(true)
        .open(path)
        .map_err(|error| io_error(error, "legacy storage identity could not be confirmed"))?;
    if file_identity(&file)? != file_identity(&confirmed)? {
        return Err(busy_error(
            "legacy storage source changed during inspection",
        ));
    }
    Ok(Some(file))
}

struct HashedFileRead {
    size: u64,
    hash: [u8; 32],
    captured: Option<Zeroizing<Vec<u8>>>,
}

fn zeroizing_extend(bytes: &mut Zeroizing<Vec<u8>>, data: &[u8]) {
    let new_len = bytes
        .len()
        .checked_add(data.len())
        .expect("the bounded source length fits usize");
    if new_len <= bytes.capacity() {
        bytes.extend_from_slice(data);
        return;
    }

    let new_capacity = next_zeroizing_capacity(bytes.capacity(), new_len, SOURCE_LIMIT as usize);
    let mut replacement = Zeroizing::new(Vec::with_capacity(new_capacity));
    replacement.extend_from_slice(bytes);
    replacement.extend_from_slice(data);
    bytes.zeroize();
    std::mem::swap(bytes, &mut replacement);
}

fn next_zeroizing_capacity(current: usize, required: usize, limit: usize) -> usize {
    current
        .max(COPY_CHUNK)
        .saturating_mul(2)
        .max(required)
        .min(limit)
}

fn read_and_hash(file: &File, capture: bool) -> Result<HashedFileRead, LegacyInspectionError> {
    let mut reader = file
        .try_clone()
        .map_err(|error| io_error(error, "legacy storage could not be read"))?;
    reader
        .seek(SeekFrom::Start(0))
        .map_err(|error| io_error(error, "legacy storage could not be read"))?;
    let mut hasher = Sha256::new();
    let mut total = 0_u64;
    let mut captured = if capture {
        let source_len = file
            .metadata()
            .map_err(|error| io_error(error, "legacy storage metadata could not be read"))?
            .len();
        if source_len > SOURCE_LIMIT {
            return Err(backend_error(
                "legacy storage source exceeds the 64 MiB size limit",
            ));
        }
        Some(Zeroizing::new(Vec::with_capacity(source_len as usize)))
    } else {
        None
    };
    let mut chunk = Zeroizing::new(vec![0_u8; COPY_CHUNK]);
    loop {
        let read = reader
            .read(&mut chunk)
            .map_err(|error| io_error(error, "legacy storage could not be read"))?;
        if read == 0 {
            break;
        }
        total = total
            .checked_add(read as u64)
            .ok_or_else(|| backend_error("legacy storage source exceeds the size limit"))?;
        if total > SOURCE_LIMIT {
            return Err(backend_error(
                "legacy storage source exceeds the 64 MiB size limit",
            ));
        }
        hasher.update(&chunk[..read]);
        if let Some(bytes) = captured.as_mut() {
            zeroizing_extend(bytes, &chunk[..read]);
        }
    }
    Ok(HashedFileRead {
        size: total,
        hash: hasher.finalize().into(),
        captured,
    })
}

struct CappedZeroizingMemoryBackend {
    bytes: RwLock<Zeroizing<Vec<u8>>>,
}

impl CappedZeroizingMemoryBackend {
    fn from_bytes(bytes: Zeroizing<Vec<u8>>) -> Self {
        Self {
            bytes: RwLock::new(bytes),
        }
    }

    fn bounds(offset: u64, len: usize) -> io::Result<(usize, usize)> {
        let start = usize::try_from(offset)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "offset out of range"))?;
        let end = start
            .checked_add(len)
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "range out of bounds"))?;
        if end > MEMORY_LIMIT {
            return Err(io::Error::new(
                io::ErrorKind::OutOfMemory,
                "legacy inspection memory limit exceeded",
            ));
        }
        Ok((start, end))
    }
}

impl fmt::Debug for CappedZeroizingMemoryBackend {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("CappedZeroizingMemoryBackend([redacted])")
    }
}

impl StorageBackend for CappedZeroizingMemoryBackend {
    fn len(&self) -> io::Result<u64> {
        let bytes = self
            .bytes
            .read()
            .map_err(|_| io::Error::other("legacy inspection memory lock poisoned"))?;
        Ok(bytes.len() as u64)
    }

    fn read(&self, offset: u64, len: usize) -> io::Result<Vec<u8>> {
        let (start, end) = Self::bounds(offset, len)?;
        let bytes = self
            .bytes
            .read()
            .map_err(|_| io::Error::other("legacy inspection memory lock poisoned"))?;
        if end > bytes.len() {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "legacy inspection read exceeded memory snapshot",
            ));
        }
        Ok(bytes[start..end].to_vec())
    }

    fn set_len(&self, len: u64) -> io::Result<()> {
        let new_len = usize::try_from(len)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "length out of range"))?;
        if new_len > MEMORY_LIMIT {
            return Err(io::Error::new(
                io::ErrorKind::OutOfMemory,
                "legacy inspection memory limit exceeded",
            ));
        }
        let mut bytes = self
            .bytes
            .write()
            .map_err(|_| io::Error::other("legacy inspection memory lock poisoned"))?;
        if new_len < bytes.len() {
            bytes[new_len..].zeroize();
            bytes.truncate(new_len);
        } else if new_len > bytes.len() {
            let mut replacement = Zeroizing::new(vec![0; new_len]);
            replacement[..bytes.len()].copy_from_slice(&bytes);
            bytes.zeroize();
            std::mem::swap(&mut *bytes, &mut replacement);
        }
        Ok(())
    }

    fn sync_data(&self, _eventual: bool) -> io::Result<()> {
        Ok(())
    }

    fn write(&self, offset: u64, data: &[u8]) -> io::Result<()> {
        let (start, end) = Self::bounds(offset, data.len())?;
        let mut bytes = self
            .bytes
            .write()
            .map_err(|_| io::Error::other("legacy inspection memory lock poisoned"))?;
        if end > bytes.len() {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "legacy inspection write exceeded memory snapshot",
            ));
        }
        bytes[start..end].copy_from_slice(data);
        Ok(())
    }
}

/// Holds the source's whole-file lock across the caller's SQLite inspection.
///
/// Deliberately opaque and without `Debug`: it contains the fixed-key presence
/// result plus live storage handles, neither of which belongs in diagnostics.
pub struct LegacySecretsInspectionLease {
    source_path: PathBuf,
    source_file: File,
    _source_lock: FileBackend,
    source_identity: FileIdentity,
    source_size: u64,
    source_hash: [u8; 32],
    socks5_password_configured: bool,
}

impl LegacySecretsInspectionLease {
    pub fn socks5_password_configured(&self) -> bool {
        self.socks5_password_configured
    }

    /// Verifies the locked source handle and its path still identify the exact
    /// bytes inspected before releasing the lock.
    pub fn finish(self) -> Result<(), LegacyInspectionError> {
        let metadata = self
            .source_file
            .metadata()
            .map_err(|error| io_error(error, "legacy secrets source could not be verified"))?;
        let identity = file_identity(&self.source_file)?;
        let verified = read_and_hash(&self.source_file, false)?;
        if identity != self.source_identity
            || metadata.len() != self.source_size
            || verified.size != self.source_size
            || verified.hash != self.source_hash
        {
            return Err(busy_error(
                "legacy secrets source changed during inspection",
            ));
        }

        let path_file = open_regular_read_only(&self.source_path)?
            .ok_or_else(|| busy_error("legacy secrets source changed during inspection"))?;
        if file_identity(&path_file)? != self.source_identity {
            return Err(busy_error(
                "legacy secrets source changed during inspection",
            ));
        }
        Ok(())
    }
}

fn acquire_source_lock(path: &Path) -> Result<Option<(File, FileBackend)>, LegacyInspectionError> {
    #[cfg(not(any(windows, all(unix, not(target_os = "wasi")))))]
    {
        let _ = path;
        return Err(backend_error(
            "legacy storage inspection is unsupported on this platform",
        ));
    }

    #[cfg(any(windows, all(unix, not(target_os = "wasi"))))]
    for attempt in 0..LOCK_ATTEMPTS {
        let Some(file) = open_regular_read_only(path)? else {
            return Ok(None);
        };
        let lock_file = file
            .try_clone()
            .map_err(|error| io_error(error, "legacy secrets source could not be locked"))?;
        match FileBackend::new(lock_file) {
            Ok(lock) => return Ok(Some((file, lock))),
            Err(redb::DatabaseError::DatabaseAlreadyOpen) if attempt + 1 < LOCK_ATTEMPTS => {
                thread::sleep(LOCK_RETRY_DELAY);
            }
            Err(redb::DatabaseError::DatabaseAlreadyOpen) => {
                return Err(busy_error("legacy secrets source is currently in use"));
            }
            Err(redb::DatabaseError::Storage(redb::StorageError::Io(error)))
                if error.kind() == io::ErrorKind::PermissionDenied =>
            {
                return Err(permission_error(
                    "legacy secrets source could not be locked",
                ));
            }
            Err(_) => {
                return Err(backend_error("legacy secrets source could not be locked"));
            }
        }
    }
    unreachable!("bounded lock loop always returns")
}

/// Locks and inspects only the fixed legacy SOCKS5-password key.
pub fn lock_and_inspect_legacy_socks5_password(
    path: &Path,
) -> Result<Option<LegacySecretsInspectionLease>, LegacyInspectionError> {
    let Some((source_file, source_lock)) = acquire_source_lock(path)? else {
        return Ok(None);
    };
    let source_identity = file_identity(&source_file)?;
    let metadata = source_file
        .metadata()
        .map_err(|error| io_error(error, "legacy secrets metadata could not be read"))?;
    if metadata.len() > SOURCE_LIMIT {
        return Err(backend_error(
            "legacy secrets source exceeds the 64 MiB size limit",
        ));
    }
    let snapshot = read_and_hash(&source_file, true)?;
    if snapshot.size != metadata.len() {
        return Err(busy_error(
            "legacy secrets source changed during inspection",
        ));
    }
    let backend = CappedZeroizingMemoryBackend::from_bytes(
        snapshot
            .captured
            .expect("capture was requested for the source snapshot"),
    );
    let mut builder = redb::Database::builder();
    builder
        .set_cache_size(8 * 1024 * 1024)
        .set_repair_callback(|session| session.abort());
    let database = builder
        .create_with_backend(backend)
        .map_err(|error| match error {
            redb::DatabaseError::RepairAborted => {
                backend_error("legacy secrets source requires repair")
            }
            _ => backend_error("legacy secrets source is invalid"),
        })?;
    let read = database
        .begin_read()
        .map_err(|_| backend_error("legacy secrets source could not be read"))?;
    let socks5_password_configured = match read.open_table(METADATA_TABLE) {
        Ok(table) => table
            .get(SOCKS5_PASSWORD_KEY)
            .map_err(|_| backend_error("legacy secrets source could not be read"))?
            .is_some(),
        Err(redb::TableError::TableDoesNotExist(_)) => false,
        Err(_) => return Err(backend_error("legacy secrets source is invalid")),
    };
    drop(read);
    drop(database);

    Ok(Some(LegacySecretsInspectionLease {
        source_path: path.to_path_buf(),
        source_file,
        _source_lock: source_lock,
        source_identity,
        source_size: snapshot.size,
        source_hash: snapshot.hash,
        socks5_password_configured,
    }))
}

/// The exact legacy `user_config` columns the app-level migration surface
/// consumes. The plugin-proxy JSON stays raw so the app can parse it strictly.
pub struct LegacyNetworkRow {
    pub socks5_enabled: bool,
    pub socks5_address: Option<String>,
    pub socks5_username: Option<String>,
    pub plugin_proxy_settings: Option<String>,
}

struct FileSnapshot {
    path: PathBuf,
    file: File,
    identity: FileIdentity,
    size: u64,
    hash: [u8; 32],
}

impl FileSnapshot {
    fn capture(path: &Path) -> Result<Option<Self>, LegacyInspectionError> {
        let Some(file) = open_regular_read_only(path)? else {
            return Ok(None);
        };
        let identity = file_identity(&file)?;
        let snapshot = read_and_hash(&file, false)?;
        Ok(Some(Self {
            path: path.to_path_buf(),
            file,
            identity,
            size: snapshot.size,
            hash: snapshot.hash,
        }))
    }

    fn unchanged(&self) -> Result<bool, LegacyInspectionError> {
        let retained = read_and_hash(&self.file, false)?;
        if file_identity(&self.file)? != self.identity
            || retained.size != self.size
            || retained.hash != self.hash
        {
            return Ok(false);
        }
        let Some(file) = open_regular_read_only(&self.path)? else {
            return Ok(false);
        };
        let snapshot = read_and_hash(&file, false)?;
        Ok(file_identity(&file)? == self.identity
            && snapshot.size == self.size
            && snapshot.hash == self.hash)
    }
}

#[cfg(test)]
thread_local! {
    static BEFORE_SQLITE_OPEN_HOOK: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        std::cell::RefCell::new(None);
    static AFTER_SQLITE_QUERY_HOOK: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        std::cell::RefCell::new(None);
}

#[cfg(test)]
fn run_before_sqlite_open_hook() {
    BEFORE_SQLITE_OPEN_HOOK.with(|hook| {
        if let Some(hook) = hook.borrow_mut().take() {
            hook();
        }
    });
}

#[cfg(test)]
fn run_after_sqlite_query_hook() {
    AFTER_SQLITE_QUERY_HOOK.with(|hook| {
        if let Some(hook) = hook.borrow_mut().take() {
            hook();
        }
    });
}

#[cfg(test)]
struct SqliteInspectionHookCleanup;

#[cfg(test)]
impl Drop for SqliteInspectionHookCleanup {
    fn drop(&mut self) {
        BEFORE_SQLITE_OPEN_HOOK.with(|hook| {
            hook.borrow_mut().take();
        });
        AFTER_SQLITE_QUERY_HOOK.with(|hook| {
            hook.borrow_mut().take();
        });
    }
}

fn sqlite_sidecar(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(suffix);
    path.with_file_name(name)
}

fn verify_sqlite_sources(
    config_snapshot: &FileSnapshot,
    wal_path: &Path,
    wal_snapshot: &Option<FileSnapshot>,
    shm_path: &Path,
    shm_snapshot: &Option<FileSnapshot>,
) -> Result<(), LegacyInspectionError> {
    if !config_snapshot.unchanged()? {
        return Err(busy_error(
            "legacy configuration database changed during inspection",
        ));
    }
    let wal_unchanged = match wal_snapshot {
        Some(snapshot) => snapshot.unchanged()?,
        None => FileSnapshot::capture(wal_path)?.is_none(),
    };
    if !wal_unchanged {
        return Err(busy_error(
            "legacy configuration WAL changed during inspection",
        ));
    }
    let shm_unchanged = match shm_snapshot {
        Some(snapshot) => snapshot.unchanged()?,
        None => FileSnapshot::capture(shm_path)?.is_none(),
    };
    if !shm_unchanged {
        return Err(busy_error(
            "legacy configuration shared-memory sidecar changed during inspection",
        ));
    }
    Ok(())
}

fn immutable_sqlite_uri(path: &Path) -> Result<String, LegacyInspectionError> {
    let normalized = path
        .to_str()
        .ok_or_else(|| backend_error("legacy configuration path is not valid Unicode"))?
        .replace('\\', "/");
    let uri_path = if cfg!(windows) && normalized.as_bytes().get(1) == Some(&b':') {
        format!("/{normalized}")
    } else {
        normalized
    };
    let mut uri = String::with_capacity(uri_path.len() + 24);
    uri.push_str("file:");
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    for byte in uri_path.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~' | b'/' | b':') {
            uri.push(char::from(byte));
        } else {
            uri.push('%');
            uri.push(char::from(HEX[(byte >> 4) as usize]));
            uri.push(char::from(HEX[(byte & 0x0f) as usize]));
        }
    }
    uri.push_str("?immutable=1");
    Ok(uri)
}

#[cfg(all(unix, not(target_os = "wasi")))]
fn retained_file_path(snapshot: &FileSnapshot) -> Result<PathBuf, LegacyInspectionError> {
    use std::os::fd::AsRawFd;

    #[cfg(any(target_os = "linux", target_os = "android"))]
    let path = PathBuf::from(format!("/proc/self/fd/{}", snapshot.file.as_raw_fd()));
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    let path = PathBuf::from(format!("/dev/fd/{}", snapshot.file.as_raw_fd()));

    let rebound = File::open(&path)
        .map_err(|_| backend_error("descriptor-bound legacy SQLite inspection is unsupported"))?;
    if file_identity(&rebound)? != snapshot.identity {
        return Err(busy_error(
            "legacy configuration source changed during inspection",
        ));
    }
    Ok(path)
}

#[cfg(windows)]
struct SqliteSourceWriteGuard {
    _files: Vec<File>,
}

#[cfg(windows)]
impl SqliteSourceWriteGuard {
    fn acquire(snapshots: &[&FileSnapshot]) -> Result<Self, LegacyInspectionError> {
        use std::os::windows::fs::OpenOptionsExt;

        const FILE_SHARE_READ: u32 = 0x0000_0001;
        let mut files = Vec::with_capacity(snapshots.len());
        for snapshot in snapshots {
            let mut opened = None;
            for attempt in 0..LOCK_ATTEMPTS {
                match OpenOptions::new()
                    .read(true)
                    .share_mode(FILE_SHARE_READ)
                    .open(&snapshot.path)
                {
                    Ok(file) => {
                        if file_identity(&file)? != snapshot.identity {
                            return Err(busy_error(
                                "legacy configuration source changed during inspection",
                            ));
                        }
                        opened = Some(file);
                        break;
                    }
                    Err(error)
                        if error.raw_os_error() == Some(32) && attempt + 1 < LOCK_ATTEMPTS =>
                    {
                        thread::sleep(LOCK_RETRY_DELAY);
                    }
                    Err(error) if error.raw_os_error() == Some(32) => {
                        return Err(busy_error(
                            "legacy configuration source is currently in use",
                        ));
                    }
                    Err(error) => {
                        return Err(io_error(
                            error,
                            "legacy configuration source could not be protected",
                        ));
                    }
                }
            }
            files.push(opened.expect("bounded source guard loop always opens or returns"));
        }
        Ok(Self { _files: files })
    }
}

/// Reads the legacy network columns through SQLite's no-create, read-only
/// opener and verifies the database plus WAL/SHM sidecars were unchanged.
pub fn inspect_legacy_network_row(
    path: &Path,
) -> Result<Option<LegacyNetworkRow>, LegacyInspectionError> {
    let Some(config_snapshot) = FileSnapshot::capture(path)? else {
        return Ok(None);
    };
    let wal_path = sqlite_sidecar(path, "-wal");
    let shm_path = sqlite_sidecar(path, "-shm");
    let wal_snapshot = FileSnapshot::capture(&wal_path)?;
    let shm_snapshot = FileSnapshot::capture(&shm_path)?;
    #[cfg(test)]
    run_before_sqlite_open_hook();
    if wal_snapshot.is_some() && shm_snapshot.is_none() {
        return Err(backend_error(
            "legacy configuration WAL has no existing shared-memory sidecar",
        ));
    }

    #[cfg(windows)]
    let source_guard = {
        let mut snapshots = vec![&config_snapshot];
        if let Some(snapshot) = wal_snapshot.as_ref() {
            snapshots.push(snapshot);
        }
        if let Some(snapshot) = shm_snapshot.as_ref() {
            snapshots.push(snapshot);
        }
        SqliteSourceWriteGuard::acquire(&snapshots)?
    };

    // A cleanly closed WAL-mode database may have no sidecars, and SQLite's
    // normal read-only opener can recreate them. Immutable mode is sufficient
    // when there is no WAL to replay. On Windows, a WAL set is guarded against
    // writes first; SQLite then opens its SHM read-only and builds a private
    // heap WAL index rather than persisting reader marks. Unix opens the
    // already-captured file descriptor rather than the mutable pathname.
    // WAL-backed inspection fails closed off Windows: pinned
    // SQLite's read-only `unix-excl` path does not obtain its special exclusive
    // process lock, so its heap WAL index cannot be proven isolated from a
    // concurrent writer.
    let connection = if wal_snapshot.is_none() {
        #[cfg(windows)]
        let immutable_path = path.to_path_buf();
        #[cfg(all(unix, not(target_os = "wasi")))]
        let immutable_path = retained_file_path(&config_snapshot)?;
        #[cfg(not(any(windows, all(unix, not(target_os = "wasi")))))]
        let immutable_path = path.to_path_buf();
        Connection::open_with_flags(
            immutable_sqlite_uri(&immutable_path)?,
            OpenFlags::SQLITE_OPEN_READ_ONLY
                | OpenFlags::SQLITE_OPEN_NO_MUTEX
                | OpenFlags::SQLITE_OPEN_URI,
        )
    } else {
        #[cfg(windows)]
        {
            Connection::open_with_flags(
                path,
                OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
            )
        }
        #[cfg(not(windows))]
        {
            return Err(backend_error(
                "read-only legacy WAL inspection is unsupported on this platform",
            ));
        }
    }
    .map_err(|error| sqlite_error(error, "legacy configuration database is invalid"))?;
    connection
        .busy_timeout(Duration::from_millis(50))
        .map_err(|error| sqlite_error(error, "legacy configuration database could not be read"))?;
    let has_table: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='user_config')",
            [],
            |row| row.get(0),
        )
        .map_err(|error| sqlite_error(error, "legacy configuration database is invalid"))?;
    let result = if has_table {
        connection
            .query_row(
                "SELECT socks5_enabled, socks5_address, socks5_username, plugin_proxy_settings \
                 FROM user_config WHERE id = 1",
                [],
                |row| {
                    Ok(LegacyNetworkRow {
                        socks5_enabled: row.get(0)?,
                        socks5_address: row.get(1)?,
                        socks5_username: row.get(2)?,
                        plugin_proxy_settings: row.get(3)?,
                    })
                },
            )
            .optional()
            .map_err(|error| sqlite_error(error, "legacy configuration database is invalid"))?
    } else {
        None
    };
    drop(connection);
    #[cfg(test)]
    run_after_sqlite_query_hook();
    // Verify once after SQLite closes while the platform protection is still
    // live, then again after releasing it so close/release behavior is covered.
    verify_sqlite_sources(
        &config_snapshot,
        &wal_path,
        &wal_snapshot,
        &shm_path,
        &shm_snapshot,
    )?;
    #[cfg(windows)]
    drop(source_guard);
    verify_sqlite_sources(
        &config_snapshot,
        &wal_path,
        &wal_snapshot,
        &shm_path,
        &shm_snapshot,
    )?;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seed_network_row(path: &Path, address: &str) {
        let connection = Connection::open(path).unwrap();
        connection
            .execute_batch(&format!(
                "CREATE TABLE user_config (\
                   id INTEGER PRIMARY KEY,\
                   socks5_enabled INTEGER NOT NULL,\
                   socks5_address TEXT,\
                   socks5_username TEXT,\
                   plugin_proxy_settings TEXT\
                 );\
                 INSERT INTO user_config VALUES \
                   (1, 1, '{address}', 'user', '{{}}');"
            ))
            .unwrap();
    }

    #[test]
    fn capped_memory_backend_rejects_growth_beyond_128_mib() {
        let backend = CappedZeroizingMemoryBackend::from_bytes(Zeroizing::new(vec![0; 16]));
        let error = backend.set_len(MEMORY_LIMIT as u64 + 1).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::OutOfMemory);
        assert_eq!(backend.len().unwrap(), 16);
    }

    #[test]
    fn memory_backend_debug_is_redacted() {
        let backend = CappedZeroizingMemoryBackend::from_bytes(Zeroizing::new(
            b"never-print-these-bytes".to_vec(),
        ));
        let rendered = format!("{backend:?}");
        assert_eq!(rendered, "CappedZeroizingMemoryBackend([redacted])");
        assert!(!rendered.contains("never-print-these-bytes"));
    }

    #[test]
    fn memory_backend_growth_uses_controlled_zeroizing_replacement() {
        let backend =
            CappedZeroizingMemoryBackend::from_bytes(Zeroizing::new(b"sensitive-page".to_vec()));
        backend.set_len(4096).unwrap();
        assert_eq!(backend.read(0, 14).unwrap(), b"sensitive-page");
        assert_eq!(backend.read(14, 4082).unwrap(), vec![0; 4082]);
    }

    #[test]
    fn snapshot_growth_is_geometric_and_bounded() {
        assert_eq!(
            next_zeroizing_capacity(0, 1, SOURCE_LIMIT as usize),
            2 * COPY_CHUNK
        );
        assert_eq!(
            next_zeroizing_capacity(2 * COPY_CHUNK, 2 * COPY_CHUNK + 1, SOURCE_LIMIT as usize),
            4 * COPY_CHUNK
        );
        assert_eq!(
            next_zeroizing_capacity(
                SOURCE_LIMIT as usize / 2,
                SOURCE_LIMIT as usize,
                SOURCE_LIMIT as usize
            ),
            SOURCE_LIMIT as usize
        );
    }

    #[cfg(any(windows, all(unix, not(target_os = "wasi"))))]
    #[test]
    fn pathname_replacement_after_capture_is_rejected() {
        let _hook_cleanup = SqliteInspectionHookCleanup;
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.sqlite");
        let replacement = temp.path().join("replacement.sqlite");
        let parked = temp.path().join("captured.sqlite");
        seed_network_row(&path, "captured:1080");
        seed_network_row(&replacement, "replacement:1080");

        let hook_path = path.clone();
        BEFORE_SQLITE_OPEN_HOOK.with(|hook| {
            let replacement_for_open = replacement.clone();
            let path_for_open = hook_path.clone();
            let parked_for_open = parked.clone();
            *hook.borrow_mut() = Some(Box::new(move || {
                fs::rename(&path_for_open, &parked_for_open).unwrap();
                fs::rename(&replacement_for_open, &path_for_open).unwrap();
            }));
        });
        AFTER_SQLITE_QUERY_HOOK.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                fs::rename(&hook_path, &replacement).unwrap();
                fs::rename(&parked, &hook_path).unwrap();
            }));
        });

        let inspected = inspect_legacy_network_row(&path);
        // An identity guard may fail closed before SQLite opens. Consume the
        // pending restore hook so no thread-local state reaches another test.
        run_after_sqlite_query_hook();
        match inspected {
            Err(error) => assert_eq!(error.kind(), LegacyInspectionErrorKind::Busy),
            Ok(Some(row)) => assert_eq!(row.socks5_address.as_deref(), Some("captured:1080")),
            Ok(None) => panic!("the captured configuration row must not disappear"),
        }
    }
}
