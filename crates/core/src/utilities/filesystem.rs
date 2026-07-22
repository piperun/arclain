//! Cross-platform filesystem operations with explicit safety semantics.

use std::path::Path;

/// Atomically renames `from` to `to` without replacing an existing path.
///
/// Both paths must be on the same filesystem. If `to` already exists, the
/// operation fails and leaves both paths unchanged.
#[cfg(any(
    target_os = "linux",
    target_os = "android",
    target_vendor = "apple",
    target_os = "redox"
))]
pub fn rename_no_replace(from: &Path, to: &Path) -> std::io::Result<()> {
    rustix::fs::renameat_with(
        rustix::fs::CWD,
        from,
        rustix::fs::CWD,
        to,
        rustix::fs::RenameFlags::NOREPLACE,
    )
    .map_err(std::io::Error::from)
}

/// Atomically renames `from` to `to` without replacing an existing path.
///
/// Both paths must be on the same filesystem. If `to` already exists, the
/// operation fails and leaves both paths unchanged.
#[cfg(windows)]
pub fn rename_no_replace(from: &Path, to: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;

    let from = from
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let to = to
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();

    // MoveFileExW only replaces a destination when MOVEFILE_REPLACE_EXISTING
    // is set. The zero flags used here provide atomic no-replace semantics.
    // SAFETY: both vectors are NUL-terminated UTF-16 paths, remain alive for
    // the call, and MoveFileExW only reads through their pointers.
    let moved = unsafe {
        windows_sys::Win32::Storage::FileSystem::MoveFileExW(from.as_ptr(), to.as_ptr(), 0)
    };
    if moved == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// Reports unsupported atomic no-replace semantics on other targets.
#[cfg(not(any(
    windows,
    target_os = "linux",
    target_os = "android",
    target_vendor = "apple",
    target_os = "redox"
)))]
pub fn rename_no_replace(_from: &Path, _to: &Path) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "atomic no-replace rename is unavailable on this target",
    ))
}
