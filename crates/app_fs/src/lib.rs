//! Filesystem helpers for arclain's on-disk app data.
//!
//! Two operations, both about *restricting access* to files and
//! directories arclain creates in the user's home/data directory:
//!
//! - [`ensure_owner_dir`] — create a directory (and any missing
//!   parents) and chmod it to `0o700` on Unix. Used for the parent
//!   directories of secrets/config databases so a curious local
//!   user can't `ls` the path and see what files exist.
//!
//! - [`restrict_owner_file`] — chmod an existing file to `0o600` on
//!   Unix. Called after a third-party crate (sqlite, redb, …) has
//!   created the file using its own `open(2)` call, which respects
//!   the process umask — default `0o022` on most Linux distros
//!   means the file ends up world-readable as `0o644` unless we
//!   tighten it explicitly.
//!
//! Both functions are no-ops on Windows, with a caveat worth
//! reading carefully: protection comes from NTFS ACL *inheritance*
//! from the parent directory, NOT from an explicit DACL set by this
//! crate. That covers the realistic threat when arclain's data
//! lives under `%APPDATA%` / `%LOCALAPPDATA%` — the user profile's
//! ACL already excludes other non-admin users. But it does *not*
//! protect:
//!
//!  * **Custom data directories outside the user profile** (e.g.
//!    user picks `D:\arclain\` in a config setting). The drive
//!    root's default ACL on most Windows installs grants
//!    `Authenticated Users: Read/Execute`, so other users on the
//!    same box can read the files.
//!  * **FAT / exFAT volumes** — no ACL system at all. Anyone with
//!    physical or remote access reads everything.
//!  * **Network shares** — depends entirely on the server-side
//!    share permissions, which arclain has no visibility into.
//!
//! Local Administrators can read regardless of any ACL — they can
//! elevate to SYSTEM and bypass everything — so locking them out
//! is theatrical, not a real protection layer.
//!
//! If owner-only-against-other-users protection is needed in a
//! non-default location on Windows, the fix would be an explicit
//! `SetNamedSecurityInfo` (Win32 ACL API) call building a DACL with
//! only the current user's SID granted access. That's ~40 LOC of
//! `windows-sys` boilerplate per operation plus a non-trivial test
//! harness, so it's deferred until a concrete use case shows up —
//! file an issue rather than assuming this crate handles it.
//!
//! # Rationale
//!
//! arclain stores plugin proxy settings (config DB) and
//! AES-256-GCM-encrypted credentials (secrets DB) on disk. Even
//! though the secrets ciphertext is encrypted, leaking the file
//! *contents* to other local users gives them an offline guess
//! target if the key file is ever exposed — and leaking the
//! *directory listing* leaks side-channel metadata (file existence,
//! sizes, mtimes). Locking everything down to owner-only is the
//! standard pattern for SQLite/keyring-style on-disk state.
//!
//! Until this crate existed the three call sites in `arclain_db`
//! each open-coded their own `#[cfg(unix)] { … }` block. Extracted
//! here so future code (cache layers, plugin sandboxes, etc.) has a
//! single secure default to reach for, rather than reinventing the
//! pattern (and possibly getting the umask logic wrong).

use anyhow::Result;
use std::path::Path;

/// Create `path` as a directory (and any missing ancestors), then
/// chmod it to `0o700` on Unix — owner read/write/execute, no group
/// or other access.
///
/// On Windows this is exactly `std::fs::create_dir_all` (no explicit
/// ACL set). Protection relies on inheritance from the parent
/// directory's ACL — sufficient under `%APPDATA%` /
/// `%LOCALAPPDATA%`, NOT sufficient for custom drive roots, FAT
/// volumes, or network shares. See crate-level docs for the full
/// caveat list.
///
/// The directory's *contents* are not touched — if `path` already
/// exists with looser permissions, this tightens them. If it exists
/// with stricter permissions (e.g. `0o500`), this opens it back up
/// to `0o700`. The intent is "the user is allowed in, nobody else
/// is", not "preserve the user's tightening".
pub fn ensure_owner_dir(path: &Path) -> Result<()> {
    use anyhow::Context;

    std::fs::create_dir_all(path)
        .with_context(|| format!("creating directory {}", path.display()))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
            .with_context(|| format!("chmod 0700 {}", path.display()))?;
    }

    Ok(())
}

/// Chmod `path` to `0o600` on Unix — owner read/write, no group or
/// other access. Errors if `path` doesn't exist.
///
/// Call after a third-party crate (SQLite via diesel, ReDb, etc.)
/// has created the file: those crates use bare `open(2)` calls that
/// respect the process umask, so on most Linux systems the file
/// lands at `0o644` until we tighten it.
///
/// On Windows this is a no-op. The file inherits its parent's NTFS
/// ACL, which restricts cross-user access *only if* the parent is
/// itself protected — true under `%APPDATA%` / `%LOCALAPPDATA%`,
/// false on custom drive roots / FAT volumes / network shares. See
/// crate-level docs for the full caveat list.
pub fn restrict_owner_file(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use anyhow::Context;
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .with_context(|| format!("chmod 0600 {}", path.display()))?;
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn ensure_owner_dir_creates_missing_parents() {
        let temp = TempDir::new().unwrap();
        let deep = temp.path().join("a").join("b").join("c");
        ensure_owner_dir(&deep).expect("create deep dir");
        assert!(deep.is_dir());
    }

    #[test]
    fn ensure_owner_dir_is_idempotent() {
        let temp = TempDir::new().unwrap();
        let p = temp.path().join("x");
        ensure_owner_dir(&p).expect("first");
        ensure_owner_dir(&p).expect("second — must not error if already exists");
        assert!(p.is_dir());
    }

    #[cfg(unix)]
    #[test]
    fn ensure_owner_dir_sets_0o700() {
        use std::os::unix::fs::PermissionsExt;
        let temp = TempDir::new().unwrap();
        let p = temp.path().join("locked");
        ensure_owner_dir(&p).expect("create");
        let mode = std::fs::metadata(&p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700, "dir must be 0o700, got {:o}", mode);
    }

    #[cfg(unix)]
    #[test]
    fn restrict_owner_file_sets_0o600() {
        use std::os::unix::fs::PermissionsExt;
        let temp = TempDir::new().unwrap();
        let f = temp.path().join("data");
        std::fs::write(&f, b"hello").unwrap();
        // Pre-condition: file is whatever the umask produced (probably 0o644).
        restrict_owner_file(&f).expect("chmod");
        let mode = std::fs::metadata(&f).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "file must be 0o600, got {:o}", mode);
    }

    #[cfg(unix)]
    #[test]
    fn restrict_owner_file_errors_when_missing() {
        let temp = TempDir::new().unwrap();
        let missing = temp.path().join("does-not-exist");
        assert!(restrict_owner_file(&missing).is_err());
    }
}
