//! The store every [`MaterializationLease`] lives in for the duration of
//! its lease id's validity.
//!
//! Each lease owns one directory, named by its own
//! [`MaterializationLeaseId`], under `AppPaths::materialization_dir()`.
//! [`MaterializationStore::reserve`] mints an id and creates that (empty)
//! directory, returning an RAII-guarded [`ReservedLease`] the caller
//! extracts into; [`MaterializationStore::commit`] then canonicalizes the
//! extracted content's path, validates it is actually contained within the
//! reservation's own directory, and makes the lease visible to every other
//! method. A reservation dropped without being committed (any failure or
//! cancellation partway through materializing) removes its own directory --
//! the same RAII shape `crate::operations::extract::StagingDirGuard` already
//! uses for extraction's private rename-collision staging area.
//!
//! Expiry (`sweep_expired`) and release (`release`) both remove a lease's
//! directory and forget it; `clear_all` does the same for every
//! currently-live lease at once, for `ArclainApp::shutdown`. `sweep_expired`
//! takes "now" as an explicit parameter rather than reading the system
//! clock internally -- this crate's tests then drive expiry deterministically
//! (a cutoff far in the future forces immediate expiry) without waiting on a
//! real TTL or a real timer tick; only the production cleanup task
//! (`crate::materialization::run_cleanup_task`) ever passes a real wall-clock
//! timestamp.

use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use parking_lot::RwLock;

use crate::error::{ApplicationError, ApplicationErrorKind, Recoverability};
use crate::ids::MaterializationLeaseId;

use super::{MaterializationLease, MAX_MATERIALIZATION_READ_BYTES};

fn unknown_lease_error(id: MaterializationLeaseId) -> ApplicationError {
    ApplicationError::new(
        ApplicationErrorKind::NotFound,
        "no such materialization lease",
    )
    .with_recoverability(Recoverability::Fatal)
    .with_diagnostic(format!("lease id {}", id.into_raw()))
}

// Accepts `impl Display` rather than the concrete `std::io::Error` --
// mirrors `crate::runtime::paths`'s own `directory_error` helper, which
// exists for the identical reason: `arclain_app_fs::ensure_owner_dir`
// returns `anyhow::Error`, not `std::io::Error`, so a helper narrowed to
// the latter couldn't report that call's failures at all.
fn directory_io_error(
    context: &str,
    path: &Path,
    error: impl std::fmt::Display,
) -> ApplicationError {
    ApplicationError::new(
        ApplicationErrorKind::Persistence,
        "failed to prepare a materialization lease directory",
    )
    .with_diagnostic(format!("{context} {}: {error}", path.display()))
    .with_recoverability(Recoverability::Retry)
}

fn escape_error(candidate: &Path, root: &Path) -> ApplicationError {
    ApplicationError::new(
        ApplicationErrorKind::Internal,
        "a materialized path escapes its own lease directory",
    )
    .with_diagnostic(format!(
        "{} is not contained within {}",
        candidate.display(),
        root.display()
    ))
    .with_recoverability(Recoverability::Fatal)
}

fn read_io_error(path: &Path, error: std::io::Error) -> ApplicationError {
    ApplicationError::new(
        ApplicationErrorKind::Internal,
        "failed to read from a materialized lease",
    )
    .with_diagnostic(format!("{}: {error}", path.display()))
    .with_recoverability(Recoverability::Retry)
}

fn too_large_error(length: u32) -> ApplicationError {
    ApplicationError::new(
        ApplicationErrorKind::InvalidInput,
        "requested read length exceeds the maximum bounded materialization read size",
    )
    .with_diagnostic(format!(
        "requested {length} bytes, maximum is {MAX_MATERIALIZATION_READ_BYTES}"
    ))
    .with_recoverability(Recoverability::UserAction)
    .with_field("length")
}

/// Canonicalizes `candidate` and verifies it is contained within `root`'s
/// own canonical form. Called both at lease creation (defense against a bug
/// in the caller's own path computation smuggling a path outside the
/// reservation's directory) and again on every bounded read (defense
/// against the path having been replaced -- a symlink swap, a deleted-and-
/// recreated entry -- between commit and use: re-resolving symlinks here
/// every time, rather than trusting a path validated only once at commit,
/// is what makes that window safe).
fn canonicalize_within(candidate: &Path, root: &Path) -> Result<PathBuf, ApplicationError> {
    let canonical_root = std::fs::canonicalize(root)
        .map_err(|error| directory_io_error("canonicalizing lease root", root, error))?;
    let canonical_candidate = std::fs::canonicalize(candidate).map_err(|error| {
        directory_io_error("canonicalizing materialized path", candidate, error)
    })?;
    if !canonical_candidate.starts_with(&canonical_root) {
        return Err(escape_error(&canonical_candidate, &canonical_root));
    }
    Ok(canonical_candidate)
}

/// One live lease's bookkeeping. `local_path` and `lease_dir` are always
/// canonical (see [`canonicalize_within`]) from the moment [`MaterializationStore::commit`]
/// inserts this record.
#[derive(Debug, Clone)]
struct LeaseRecord {
    local_path: PathBuf,
    lease_dir: PathBuf,
    size: u64,
    expires_at_unix_ms: i64,
}

impl LeaseRecord {
    fn to_dto(&self, id: MaterializationLeaseId) -> MaterializationLease {
        MaterializationLease {
            id,
            local_path: self.local_path.clone(),
            size: self.size,
            expires_at_unix_ms: self.expires_at_unix_ms,
        }
    }
}

/// A lease directory reserved but not yet committed. `Drop` removes the
/// directory unless [`MaterializationStore::commit`] has already consumed
/// this reservation -- mirrors `crate::operations::extract::StagingDirGuard`'s
/// exact RAII shape (create eagerly, clean up automatically on any
/// early-return path), applied to a lease's own owned directory instead of
/// extraction's private rename-collision staging area.
pub(crate) struct ReservedLease {
    id: MaterializationLeaseId,
    dir: PathBuf,
    committed: bool,
}

impl ReservedLease {
    pub(crate) fn dir(&self) -> &Path {
        &self.dir
    }
}

impl Drop for ReservedLease {
    fn drop(&mut self) {
        if !self.committed {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }
}

/// A tag unique to this one running process instance, embedded in every
/// lease directory name this store mints (see [`MaterializationStore::reserve`]).
/// Combines the process id with a nanosecond timestamp rather than either
/// alone: a bare pid can be reused by the OS across two different runs
/// separated by enough time, and a bare timestamp says nothing about which
/// process produced it -- the pair together is what makes two runs'
/// directory names collide-proof without needing a new dependency (a
/// proper UUID/random crate) for a need this narrow.
///
/// This is a name-collision guard only. It does **not** make it safe for
/// one instance to delete another live instance's content -- see
/// [`MaterializationStore::new`]'s own doc comment for the mechanism that
/// actually guards *that* (an exclusive lock, not this tag).
fn session_tag() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    format!("{}-{nanos:x}", std::process::id())
}

/// Where the exclusive lock guarding `root`'s leftover-clearing step
/// lives -- a sibling of `root`, deliberately never a file *inside* it:
/// `root`'s own contents are exactly what clearing removes, and this
/// lock must keep working (both for this instance, for the rest of its
/// lifetime, and for whichever instance constructs next) independent of
/// whatever clearing does or doesn't do to those contents.
fn lock_file_path(root: &Path) -> PathBuf {
    root.with_extension("lock")
}

/// Attempts to remove every direct child of `root` individually and
/// best-effort, logging (not failing) any it cannot remove right now --
/// a leftover directory an antivirus scanner or a still-open external
/// viewer holds a handle into (surviving the crash that left it behind
/// in the first place) must not turn "reclaim what we can" into "this
/// application cannot start." [`session_tag`] already guarantees this
/// run's own fresh leases can never collide by name with whatever is
/// left behind here, so an unremovable leftover only costs a little
/// permanently-unreclaimed disk space, never correctness.
///
/// Does nothing if `root` does not exist yet -- the ordinary first-run
/// case.
fn clear_leftover_lease_directories(root: &Path) {
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
        Err(error) => {
            tracing::warn!(
                "[materialization] could not list {} to clear leftover leases from a previous \
                 run -- leaving its contents in place: {error}",
                root.display()
            );
            return;
        }
    };
    for entry in entries {
        let Ok(entry) = entry else { continue };
        let path = entry.path();
        let is_dir = entry.file_type().map(|kind| kind.is_dir()).unwrap_or(true);
        let result = if is_dir {
            std::fs::remove_dir_all(&path)
        } else {
            std::fs::remove_file(&path)
        };
        if let Err(error) = result {
            tracing::warn!(
                "[materialization] could not remove leftover {} from a previous run (still open \
                 elsewhere? antivirus hold?) -- leaving it in place: {error}",
                path.display()
            );
        }
    }
}

/// The store every [`MaterializationLease`] is tracked through for the
/// lifetime of its id's validity. See the module doc comment.
pub(crate) struct MaterializationStore {
    root: PathBuf,
    /// This instance's own [`session_tag`], embedded in every lease
    /// directory name -- see [`Self::reserve`]'s own doc comment for why.
    session_tag: String,
    ttl: Duration,
    leases: RwLock<HashMap<MaterializationLeaseId, LeaseRecord>>,
    /// Per-store, not a process-wide `static` -- mirrors
    /// `ArchiveSessionStore::next_id`'s own field (see its doc comment for
    /// why a `static` counter shared across every store instance in one
    /// test binary is itself a latent smell, independent of any one test).
    /// Restarts at 1 on every construction -- exactly why [`Self::new`]
    /// clears `root` first (when it can -- see that method's own doc
    /// comment): a bare numeric id alone would otherwise collide by name
    /// with whatever a previous process run (crashed, force-killed, or
    /// otherwise never reaching `ArclainApp::shutdown`) left behind at
    /// that same path.
    next_id: AtomicU64,
    /// An OS-level exclusive lock on [`lock_file_path`], held for this
    /// store's *entire* lifetime -- never read again after [`Self::new`]
    /// acquires (or fails to acquire) it. Kept alive purely for its
    /// `Drop`: releasing it (when this field goes out of scope, on a
    /// clean shutdown or an OS process exit/crash alike -- the OS closes
    /// every handle a dying process held, lock included) is what lets the
    /// *next* instance constructed against this `root` reacquire it and
    /// clear leftovers in turn. See [`Self::new`]'s own doc comment.
    #[allow(dead_code)]
    root_lock: std::fs::File,
}

impl MaterializationStore {
    /// Constructs the store rooted at `root`.
    ///
    /// Reclaiming a previous run's leftovers (crash, force-kill --
    /// anything that skipped `ArclainApp::shutdown`) is conditional on
    /// this instance actually acquiring an exclusive OS-level lock on a
    /// sibling lock file at [`lock_file_path`]. If another instance of
    /// this application is already live against the same `root` --
    /// holding that same lock itself -- this construction skips the
    /// clear entirely and simply coexists with it, rather than deleting
    /// the other, still-live instance's lease directories out from under
    /// it. Locking is what actually prevents that, **not**
    /// [`session_tag`]: that tag only guarantees the two instances' own
    /// freshly minted lease directories can never collide *by name*; on
    /// its own it does nothing to stop one instance's construction-time
    /// clear from wiping another, already-live instance's entire root
    /// (an earlier version of this doc comment claimed otherwise -- that
    /// claim was wrong).
    ///
    /// The lock is held for this store's *whole* lifetime, not just
    /// around the clear, so the *next* single instance to construct
    /// against this `root` -- once this one has cleanly shut down or
    /// crashed, either way releasing the OS-level lock -- can reacquire
    /// it and clear leftovers in its own turn.
    ///
    /// Failing to acquire the lock is not itself an error: construction
    /// still succeeds, just without clearing. A genuine I/O failure
    /// opening or locking the lock file *is* fatal, as is a failure
    /// creating/securing `root` itself (via
    /// `arclain_app_fs::ensure_owner_dir`, which also re-applies owner-
    /// only permissions every time -- plain `create_dir_all` would
    /// silently leave a freshly (re)created `root` at the process umask's
    /// default instead, typically world-readable on Unix, letting other
    /// local users traverse into materialized content that may have come
    /// from an encrypted archive). A failure clearing an individual
    /// leftover is not fatal (see [`clear_leftover_lease_directories`]).
    pub(crate) fn new(root: PathBuf, ttl: Duration) -> Result<Self, ApplicationError> {
        let lock_path = lock_file_path(&root);
        if let Some(parent) = lock_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| directory_io_error("creating", parent, error))?;
        }
        let root_lock = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(&lock_path)
            .map_err(|error| {
                directory_io_error("opening the lease-root lock file at", &lock_path, error)
            })?;

        match root_lock.try_lock() {
            Ok(()) => clear_leftover_lease_directories(&root),
            Err(std::fs::TryLockError::WouldBlock) => {
                // Another live instance owns `root` right now -- leave
                // its contents alone entirely; see this method's own doc
                // comment.
            }
            Err(std::fs::TryLockError::Error(error)) => {
                return Err(directory_io_error(
                    "locking the lease-root lock file at",
                    &lock_path,
                    error,
                ));
            }
        }

        arclain_app_fs::ensure_owner_dir(&root)
            .map_err(|error| directory_io_error("creating", &root, error))?;

        Ok(Self {
            root,
            session_tag: session_tag(),
            ttl,
            leases: RwLock::new(HashMap::new()),
            next_id: AtomicU64::new(1),
            root_lock,
        })
    }

    fn expires_at(&self, now_unix_ms: i64) -> i64 {
        now_unix_ms.saturating_add(self.ttl.as_millis().min(i64::MAX as u128) as i64)
    }

    /// Mints a fresh lease id and creates its (empty) owned directory,
    /// returning an RAII-guarded reservation the caller extracts into and
    /// then hands to [`Self::commit`]. Not yet visible to `get`/`renew`/
    /// `release`/`read_range` -- a caller that errors out before calling
    /// `commit` should just let the returned guard drop.
    ///
    /// The directory name embeds this instance's own [`session_tag`], not
    /// just the numeric id -- a bare numeric id restarts at 1 every
    /// process, so it alone could not distinguish this instance's own
    /// directories from a previous run's leftovers, or from a second,
    /// concurrently live instance's own leases (which this instance's
    /// construction does not delete -- see [`Self::new`]'s own doc
    /// comment for the lock that actually guards that case).
    pub(crate) fn reserve(&self) -> Result<ReservedLease, ApplicationError> {
        let id = MaterializationLeaseId::from_raw(self.next_id.fetch_add(1, Ordering::Relaxed));
        let dir = self
            .root
            .join(format!("{}-{}", self.session_tag, id.into_raw()));
        std::fs::create_dir_all(&dir)
            .map_err(|error| directory_io_error("creating", &dir, error))?;
        Ok(ReservedLease {
            id,
            dir,
            committed: false,
        })
    }

    /// Registers a reservation as a live lease: canonicalizes and validates
    /// `local_path` is contained within the reservation's own directory
    /// (see [`canonicalize_within`]), then makes it visible to every other
    /// method. `now_unix_ms` is the same explicit-clock parameter
    /// [`Self::sweep_expired`] takes, for the same reason.
    pub(crate) fn commit(
        &self,
        mut reserved: ReservedLease,
        local_path: PathBuf,
        size: u64,
        now_unix_ms: i64,
    ) -> Result<MaterializationLease, ApplicationError> {
        let canonical_local_path = canonicalize_within(&local_path, &reserved.dir)?;
        let canonical_dir = std::fs::canonicalize(&reserved.dir)
            .map_err(|error| directory_io_error("canonicalizing", &reserved.dir, error))?;
        let id = reserved.id;
        let record = LeaseRecord {
            local_path: canonical_local_path,
            lease_dir: canonical_dir,
            size,
            expires_at_unix_ms: self.expires_at(now_unix_ms),
        };
        let dto = record.to_dto(id);
        self.leases.write().insert(id, record);
        // From here on this reservation is a live, committed lease --
        // `Drop` must not remove the directory `get`/`release`/etc. now
        // consider owned by the store.
        reserved.committed = true;
        Ok(dto)
    }

    /// A point-in-time read of one lease. `NotFound` if `id` was never
    /// issued by this store, or has since been released or expired.
    pub(crate) fn get(
        &self,
        id: MaterializationLeaseId,
    ) -> Result<MaterializationLease, ApplicationError> {
        self.leases
            .read()
            .get(&id)
            .map(|record| record.to_dto(id))
            .ok_or_else(|| unknown_lease_error(id))
    }

    /// Extends `id`'s expiry to `ttl` from `now_unix_ms`, returning the new
    /// `expires_at_unix_ms`. `NotFound` under the same conditions as
    /// [`Self::get`].
    pub(crate) fn renew(
        &self,
        id: MaterializationLeaseId,
        now_unix_ms: i64,
    ) -> Result<i64, ApplicationError> {
        let mut leases = self.leases.write();
        let record = leases.get_mut(&id).ok_or_else(|| unknown_lease_error(id))?;
        record.expires_at_unix_ms = self.expires_at(now_unix_ms);
        Ok(record.expires_at_unix_ms)
    }

    /// Removes `id` and its owned directory. Idempotent: releasing an
    /// already-released, expired, or never-issued id is a no-op success,
    /// not an error -- a caller that races its own release against expiry
    /// (or calls release twice, e.g. once explicitly and once from a
    /// generic cleanup path) should never have to treat "already gone" as
    /// a failure.
    pub(crate) fn release(&self, id: MaterializationLeaseId) -> Result<(), ApplicationError> {
        if let Some(record) = self.leases.write().remove(&id) {
            let _ = std::fs::remove_dir_all(&record.lease_dir);
        }
        Ok(())
    }

    /// Reads up to `length` bytes (bounded by [`MAX_MATERIALIZATION_READ_BYTES`])
    /// starting at `offset` from lease `id`'s own file. `NotFound` on a
    /// released/expired/unknown lease. An `offset` at or past end-of-file
    /// yields an empty result rather than an error (ordinary short-read
    /// semantics, not a fault); an `offset` before EOF whose requested
    /// `length` would reach past it yields whatever bytes remain, again not
    /// an error.
    pub(crate) fn read_range(
        &self,
        id: MaterializationLeaseId,
        offset: u64,
        length: u32,
    ) -> Result<Vec<u8>, ApplicationError> {
        if length > MAX_MATERIALIZATION_READ_BYTES {
            return Err(too_large_error(length));
        }
        let (local_path, lease_dir) = {
            let leases = self.leases.read();
            let record = leases.get(&id).ok_or_else(|| unknown_lease_error(id))?;
            (record.local_path.clone(), record.lease_dir.clone())
        };
        // Re-validated here, not trusted from `commit` time alone -- see
        // `canonicalize_within`'s own doc comment.
        let validated = canonicalize_within(&local_path, &lease_dir)?;

        let mut file =
            std::fs::File::open(&validated).map_err(|error| read_io_error(&validated, error))?;
        let file_len = file
            .metadata()
            .map_err(|error| read_io_error(&validated, error))?
            .len();
        if offset >= file_len {
            return Ok(Vec::new());
        }
        file.seek(SeekFrom::Start(offset))
            .map_err(|error| read_io_error(&validated, error))?;
        let remaining = file_len - offset;
        let to_read = remaining.min(u64::from(length)) as usize;
        let mut buffer = vec![0u8; to_read];
        file.read_exact(&mut buffer)
            .map_err(|error| read_io_error(&validated, error))?;
        Ok(buffer)
    }

    /// Removes every lease whose `expires_at_unix_ms` is at or before
    /// `now_unix_ms`. See the module doc comment for why "now" is an
    /// explicit parameter.
    ///
    /// Decides and removes under one held write guard (`HashMap::retain`),
    /// not a read pass followed by a separate write pass: an earlier
    /// version computed the expired set under a read guard, dropped it,
    /// then removed under a fresh write guard without re-checking
    /// `expires_at_unix_ms` -- a `renew` landing in the gap between those
    /// two guards would return `Ok` with a fresh expiry, and then have its
    /// lease deleted anyway on the strength of the stale snapshot. A
    /// single retain closes that gap by construction: `renew` and this
    /// method both need the same write lock, so one fully happens before
    /// the other, and whichever ran second sees the other's effect.
    pub(crate) fn sweep_expired(&self, now_unix_ms: i64) {
        let mut removed_dirs: Vec<PathBuf> = Vec::new();
        {
            let mut leases = self.leases.write();
            leases.retain(|_, record| {
                if record.expires_at_unix_ms <= now_unix_ms {
                    removed_dirs.push(record.lease_dir.clone());
                    false
                } else {
                    true
                }
            });
        }
        for dir in removed_dirs {
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    /// Removes every currently-live lease's directory, regardless of
    /// expiry. Called once by `ArclainApp::shutdown`.
    pub(crate) fn clear_all(&self) {
        let dirs: Vec<PathBuf> = self
            .leases
            .write()
            .drain()
            .map(|(_, record)| record.lease_dir)
            .collect();
        for dir in dirs {
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    #[cfg(test)]
    pub(crate) fn root(&self) -> &Path {
        &self.root
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store(temp: &tempfile::TempDir) -> MaterializationStore {
        MaterializationStore::new(
            temp.path().join("materialization"),
            Duration::from_secs(300),
        )
        .unwrap()
    }

    fn commit_file(
        store: &MaterializationStore,
        reserved: ReservedLease,
        name: &str,
        content: &[u8],
        now_unix_ms: i64,
    ) -> MaterializationLease {
        let path = reserved.dir().join(name);
        std::fs::write(&path, content).unwrap();
        store
            .commit(reserved, path, content.len() as u64, now_unix_ms)
            .unwrap()
    }

    #[test]
    fn reserve_creates_an_empty_directory_under_the_store_root() {
        let temp = tempfile::tempdir().unwrap();
        let store = store(&temp);

        let reserved = store.reserve().unwrap();

        assert!(reserved.dir().is_dir());
        assert!(reserved.dir().starts_with(store.root()));
        assert_eq!(std::fs::read_dir(reserved.dir()).unwrap().count(), 0);
    }

    #[test]
    fn constructing_a_store_clears_leftover_content_from_a_previous_run_at_the_same_root() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("materialization");

        // Simulate a previous, no-longer-running process instance having
        // left a lease's directory behind at this exact root (a crash, a
        // force-kill -- anything that skips `ArclainApp::shutdown`).
        let leftover_dir = root.join("stale-session-1");
        std::fs::create_dir_all(&leftover_dir).unwrap();
        std::fs::write(
            leftover_dir.join("previous_archive_content.bin"),
            b"belongs to a different, already-closed archive session",
        )
        .unwrap();
        assert!(std::fs::read_dir(&root).unwrap().next().is_some());

        // A fresh store construction (what a fresh `ArclainApp::bootstrap`
        // does) must reclaim it -- the id space restarts at 1 regardless,
        // so nothing at this root can ever legitimately belong to the new
        // store.
        let store = MaterializationStore::new(root.clone(), Duration::from_secs(300)).unwrap();

        assert_eq!(
            std::fs::read_dir(&root).unwrap().count(),
            0,
            "a fresh store must clear away every leftover directory from a previous run"
        );

        // A fresh lease minted by the new store must contain only its own
        // content -- proves the clearing actually ran before anything else
        // touched the root, not just that the leftover happened to still
        // be there afterward.
        let reserved = store.reserve().unwrap();
        let lease = commit_file(&store, reserved, "new.txt", b"fresh", 0);
        assert_eq!(std::fs::read(&lease.local_path).unwrap(), b"fresh");
    }

    #[test]
    fn a_fresh_lease_never_inherits_a_previous_runs_content_even_at_the_same_numeric_id() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("materialization");

        // First "run": mint lease id 1 and commit real content into it,
        // then simulate the process going away without releasing or
        // shutting down (drop the store; its directory is left on disk).
        let first_run_lease_dir = {
            let store = MaterializationStore::new(root.clone(), Duration::from_secs(300)).unwrap();
            let reserved = store.reserve().unwrap();
            assert_eq!(reserved.id, MaterializationLeaseId::from_raw(1));
            let dir = reserved.dir().to_path_buf();
            let _lease = commit_file(&store, reserved, "old.txt", b"stale, from run 1", 0);
            dir
        };
        assert!(
            first_run_lease_dir.exists(),
            "sanity: run 1's lease dir is really still on disk"
        );

        // Second "run" against the identical root -- ids restart at 1
        // again by construction.
        let store2 = MaterializationStore::new(root, Duration::from_secs(300)).unwrap();
        let reserved2 = store2.reserve().unwrap();
        assert_eq!(
            reserved2.id,
            MaterializationLeaseId::from_raw(1),
            "sanity: the id space really does restart at 1 every construction"
        );
        let lease2 = commit_file(&store2, reserved2, "new.txt", b"fresh, from run 2", 0);

        // Run 1's old directory must be gone (reclaimed by run 2's
        // construction), and run 2's own lease must contain only its own
        // file -- never run 1's stale sibling.
        assert!(!first_run_lease_dir.exists());
        assert_eq!(
            std::fs::read(&lease2.local_path).unwrap(),
            b"fresh, from run 2"
        );
        let sibling_count = std::fs::read_dir(lease2.local_path.parent().unwrap())
            .unwrap()
            .count();
        assert_eq!(
            sibling_count, 1,
            "the fresh lease's own directory must contain only what THIS run wrote to it"
        );
    }

    #[test]
    fn two_reservations_from_two_stores_against_the_same_root_never_collide_by_name() {
        // Both stores go through the real `new()` this time (a raw struct
        // literal used to stand in for "a second overlapping instance"
        // here, but that bypassed the lock entirely and so didn't
        // actually exercise it) -- store_b's construction, while store_a
        // is still alive and holding the lock, is exactly the "second
        // live instance" scenario the lock exists to detect.
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("materialization");
        let store_a = MaterializationStore::new(root.clone(), Duration::from_secs(300)).unwrap();
        let store_b = MaterializationStore::new(root, Duration::from_secs(300)).unwrap();

        let reserved_a = store_a.reserve().unwrap();
        let reserved_b = store_b.reserve().unwrap();

        assert_eq!(reserved_a.id, reserved_b.id, "sanity: both mint id 1");
        assert_ne!(
            reserved_a.dir(),
            reserved_b.dir(),
            "two instances' directories for the same numeric id must never collide by name"
        );
    }

    #[test]
    fn a_second_store_constructed_while_the_first_is_still_live_skips_the_clear_and_coexists() {
        // The bug NB2 fixes: a second concurrent instance's bootstrap
        // used to `remove_dir_all` the shared root regardless of whether
        // another instance already had live leases there -- session_tag
        // alone (asserted above) only stops the two instances' own
        // directories from colliding by *name*; it does nothing to stop
        // one from deleting the other's.
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("materialization");

        let store_a = MaterializationStore::new(root.clone(), Duration::from_secs(300)).unwrap();
        let reserved_a = store_a.reserve().unwrap();
        let lease_a = commit_file(&store_a, reserved_a, "a.txt", b"still live", 0);
        assert!(lease_a.local_path.exists());

        // Constructed against the SAME root while `store_a` is still
        // alive (and therefore still holding the lock) -- must not touch
        // `store_a`'s live content.
        let store_b = MaterializationStore::new(root, Duration::from_secs(300)).unwrap();

        assert!(
            lease_a.local_path.exists(),
            "a second store constructed while the first is still live must not delete the \
             first instance's live lease content"
        );

        // The second instance must still be fully usable alongside the
        // first -- distinct session tags keep its own leases from
        // colliding with the first instance's.
        let reserved_b = store_b.reserve().unwrap();
        let lease_b = commit_file(&store_b, reserved_b, "b.txt", b"second instance", 0);
        assert_ne!(lease_a.local_path.parent(), lease_b.local_path.parent());
        assert!(
            lease_a.local_path.exists(),
            "still there after store_b's own reserve+commit"
        );
    }

    #[test]
    fn a_fresh_store_clears_leftovers_once_the_previous_instances_lock_is_released() {
        // The companion case: successive *single* instances (the
        // ordinary crash-and-restart scenario, not two overlapping live
        // instances) must still see the original reclaim-on-restart
        // behavior once the previous instance has actually gone away and
        // released its lock -- the lock must not turn into a permanent
        // "nobody may ever clear again" latch.
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("materialization");

        let leftover_dir = {
            let store = MaterializationStore::new(root.clone(), Duration::from_secs(300)).unwrap();
            let reserved = store.reserve().unwrap();
            let dir = reserved.dir().to_path_buf();
            let _lease = commit_file(&store, reserved, "old.txt", b"stale", 0);
            dir
            // `store` (and the lock it held) is dropped here.
        };
        assert!(
            leftover_dir.exists(),
            "sanity: the first instance's lease dir is still on disk"
        );

        let store2 = MaterializationStore::new(root, Duration::from_secs(300)).unwrap();
        assert!(
            !leftover_dir.exists(),
            "a fresh store must still clear a previous instance's leftovers once that \
             instance's lock has actually been released"
        );
        let reserved = store2.reserve().unwrap();
        let lease = commit_file(&store2, reserved, "new.txt", b"fresh", 0);
        assert_eq!(std::fs::read(&lease.local_path).unwrap(), b"fresh");
    }

    #[cfg(unix)]
    #[test]
    fn recreating_the_root_still_locks_it_down_to_owner_only() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("materialization");

        {
            let _store = MaterializationStore::new(root.clone(), Duration::from_secs(300)).unwrap();
        }
        // Loosen it, simulating what a plain `create_dir_all` (the
        // process umask's default, typically 0755) would leave behind if
        // this fix regressed to it -- proves the *second* construction
        // below is what re-secures it, not merely that it happened to
        // already be right from the first.
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o755)).unwrap();

        let _store2 = MaterializationStore::new(root.clone(), Duration::from_secs(300)).unwrap();

        let mode = std::fs::metadata(&root).unwrap().permissions().mode() & 0o777;
        assert_eq!(
            mode, 0o700,
            "the lease root must be owner-only (0700) after being (re)secured, never left at \
             whatever the process umask's default happens to be -- other local users must not \
             be able to traverse into materialized content, which may have come from an \
             encrypted archive"
        );
    }

    #[cfg(windows)]
    #[test]
    fn a_leftover_held_open_by_another_handle_is_logged_and_left_in_place() {
        // Windows-specific mechanism for the same "cannot remove one
        // leftover right now" scenario NB3 describes (an antivirus
        // scanner or a still-open external viewer holding a handle into
        // it). `std::fs::OpenOptions`'s own *default* share mode
        // deliberately includes `FILE_SHARE_DELETE` (matching Unix's
        // unlink-while-open semantics, confirmed directly: a plain
        // default-mode open here does NOT block `remove_file`), so this
        // opens with an explicit, narrower share mode instead --
        // `FILE_SHARE_READ` only, the way many real external
        // viewers/scanners do -- which does block removal until the
        // handle closes.
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_SHARE_READ: u32 = 0x0000_0001;

        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("materialization");

        let (undeletable_file, deletable_dir) = {
            let store = MaterializationStore::new(root.clone(), Duration::from_secs(300)).unwrap();

            let reserved_ok = store.reserve().unwrap();
            let deletable_dir = reserved_ok.dir().to_path_buf();
            let _lease_ok = commit_file(&store, reserved_ok, "fine.txt", b"removable", 0);

            let reserved_stuck = store.reserve().unwrap();
            let _lease_stuck = commit_file(&store, reserved_stuck, "stuck.txt", b"held open", 0);
            let undeletable_file = _lease_stuck.local_path.clone();

            (undeletable_file, deletable_dir)
        };

        let held_open = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ)
            .open(&undeletable_file)
            .unwrap();

        // Construction must still succeed...
        let _store2 = MaterializationStore::new(root, Duration::from_secs(300)).unwrap();

        // ...must have cleared the leftover it COULD remove...
        assert!(
            !deletable_dir.exists(),
            "a removable leftover must still be cleared"
        );
        // ...and must have left the undeletable one in place instead of
        // failing construction over it.
        assert!(
            undeletable_file.exists(),
            "a leftover this process cannot remove right now must be left in place, not cause \
             construction to fail"
        );

        drop(held_open);
    }

    #[cfg(unix)]
    #[test]
    fn a_leftover_whose_permissions_forbid_removal_is_logged_and_left_in_place() {
        use std::os::unix::fs::PermissionsExt;

        // Unix analog of the Windows held-open-handle test above:
        // removing a directory's *contents* (what `remove_dir_all` must
        // do before it can remove the directory itself) needs write+
        // execute permission on that directory, not on the files inside
        // it -- stripping that permission reliably fails removal with
        // EACCES on any POSIX filesystem, regardless of which user owns
        // the files, without relying on any Windows-only handle-sharing
        // semantics.
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("materialization");

        let (undeletable_dir, deletable_dir) = {
            let store = MaterializationStore::new(root.clone(), Duration::from_secs(300)).unwrap();

            let reserved_ok = store.reserve().unwrap();
            let deletable_dir = reserved_ok.dir().to_path_buf();
            let _lease_ok = commit_file(&store, reserved_ok, "fine.txt", b"removable", 0);

            let reserved_stuck = store.reserve().unwrap();
            let undeletable_dir = reserved_stuck.dir().to_path_buf();
            let _lease_stuck = commit_file(&store, reserved_stuck, "stuck.txt", b"held open", 0);

            (undeletable_dir, deletable_dir)
        };
        std::fs::set_permissions(&undeletable_dir, std::fs::Permissions::from_mode(0o500)).unwrap();

        let _store2 = MaterializationStore::new(root, Duration::from_secs(300)).unwrap();

        assert!(
            !deletable_dir.exists(),
            "a removable leftover must still be cleared"
        );
        assert!(
            undeletable_dir.exists(),
            "a leftover this process cannot remove right now (permission-restricted) must be \
             left in place, not cause construction to fail"
        );

        // Restore permissions so the tempdir's own cleanup can succeed.
        std::fs::set_permissions(&undeletable_dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    }

    #[test]
    fn two_reservations_mint_distinct_ids_and_distinct_directories() {
        let temp = tempfile::tempdir().unwrap();
        let store = store(&temp);

        let a = store.reserve().unwrap();
        let b = store.reserve().unwrap();

        assert_ne!(a.id, b.id);
        assert_ne!(a.dir(), b.dir());
    }

    #[test]
    fn dropping_a_reservation_without_committing_removes_its_directory() {
        let temp = tempfile::tempdir().unwrap();
        let store = store(&temp);
        let reserved = store.reserve().unwrap();
        let dir = reserved.dir().to_path_buf();
        assert!(dir.is_dir());

        drop(reserved);

        assert!(
            !dir.exists(),
            "a reservation dropped without commit must clean up its own directory \
             (proves cleanup after a materialization failure, without needing a full \
             operation-worker integration test)"
        );
    }

    #[test]
    fn committing_makes_the_lease_visible_and_stops_drop_from_removing_it() {
        let temp = tempfile::tempdir().unwrap();
        let store = store(&temp);
        let reserved = store.reserve().unwrap();
        let id = reserved.id;
        let dir = reserved.dir().to_path_buf();

        let lease = commit_file(&store, reserved, "file.bin", b"payload", 1_000);

        assert_eq!(lease.id, id);
        assert_eq!(lease.size, 7);
        assert_eq!(
            lease.local_path,
            dir.canonicalize().unwrap().join("file.bin")
        );
        assert_eq!(lease.expires_at_unix_ms, 1_000 + 300_000);

        let fetched = store.get(id).unwrap();
        assert_eq!(fetched, lease);
    }

    #[test]
    fn commit_rejects_a_local_path_outside_the_reservations_own_directory() {
        let temp = tempfile::tempdir().unwrap();
        let store = store(&temp);
        let reserved = store.reserve().unwrap();
        let dir = reserved.dir().to_path_buf();

        // A real file that genuinely exists, but outside the reservation's
        // own directory -- simulates a bug elsewhere computing a local_path
        // that does not actually belong to this lease.
        let outside = temp.path().join("outside.bin");
        std::fs::write(&outside, b"not part of this lease").unwrap();

        let error = store.commit(reserved, outside, 1, 1_000).unwrap_err();
        assert_eq!(error.kind, ApplicationErrorKind::Internal);

        // The reservation's own (still-uncommitted) directory must still be
        // cleaned up: `commit` consumed it by value, and since it errored
        // out before setting `committed = true`, dropping it here runs the
        // same cleanup a failed materialization would get.
        assert!(!dir.exists());
    }

    #[test]
    fn a_fabricated_path_that_is_not_actually_inside_the_lease_root_is_rejected_directly() {
        // Exercises `canonicalize_within` (the shared validation both
        // `commit` and `read_range` depend on) with two real, existing,
        // but unrelated directories -- no symlink privilege games needed
        // to prove "any path escaping the lease root is rejected".
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("lease_root");
        let elsewhere = temp.path().join("elsewhere");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&elsewhere).unwrap();
        let candidate = elsewhere.join("evil.txt");
        std::fs::write(&candidate, b"x").unwrap();

        let error = canonicalize_within(&candidate, &root).unwrap_err();
        assert_eq!(error.kind, ApplicationErrorKind::Internal);
    }

    #[test]
    fn read_range_rejects_a_path_replaced_after_commit_to_point_outside_the_lease_root() {
        let temp = tempfile::tempdir().unwrap();
        let store = store(&temp);
        let reserved = store.reserve().unwrap();
        let id = reserved.id;
        let _lease = commit_file(&store, reserved, "file.bin", b"payload", 1_000);

        // Simulate the path having been swapped out from under the lease
        // after validation (a TOCTOU tamper, or a bug elsewhere) by
        // directly rewriting the committed record's `local_path` to a real
        // file that exists but lives outside this lease's own directory --
        // exercising the exact defense-in-depth `read_range` re-validates
        // on every call, independent of whatever `commit` already checked
        // once.
        let outside = temp.path().join("outside.bin");
        std::fs::write(&outside, b"attacker-controlled").unwrap();
        {
            let mut leases = store.leases.write();
            leases.get_mut(&id).unwrap().local_path = outside;
        }

        let error = store.read_range(id, 0, 10).unwrap_err();
        assert_eq!(error.kind, ApplicationErrorKind::Internal);
    }

    #[test]
    fn renew_extends_expiry_from_the_given_now() {
        let temp = tempfile::tempdir().unwrap();
        let store = store(&temp);
        let reserved = store.reserve().unwrap();
        let id = reserved.id;
        let _lease = commit_file(&store, reserved, "file.bin", b"x", 1_000);

        let new_expiry = store.renew(id, 50_000).unwrap();

        assert_eq!(new_expiry, 50_000 + 300_000);
        assert_eq!(store.get(id).unwrap().expires_at_unix_ms, new_expiry);
    }

    #[test]
    fn a_renewed_lease_survives_a_sweep_timed_against_its_pre_renewal_deadline() {
        let temp = tempfile::tempdir().unwrap();
        let store = store(&temp);
        let reserved = store.reserve().unwrap();
        let id = reserved.id;
        let _lease = commit_file(&store, reserved, "file.bin", b"x", 0); // expires at 300_000

        // Renew well before the original expiry, pushing it much further
        // out (100_000 + 300_000 = 400_000).
        let new_expiry = store.renew(id, 100_000).unwrap();

        // Sweep with a "now" that is past the *original* expiry (300_000)
        // but well before the *renewed* one (400_000).
        //
        // This is a sequential call -- `renew` fully completes and
        // returns before `sweep_expired` is even invoked -- so it proves
        // `sweep_expired` reads the lease's *current* state rather than
        // some snapshot memoized once at an earlier time, but it does
        // NOT exercise the concurrent read-then-write race the single-
        // write-guard-retain fix actually closes: with nothing running
        // at the same time, both the old two-step implementation and the
        // current one see the already-landed renewal identically, so
        // this test alone cannot tell them apart (confirmed directly: it
        // still passes if `sweep_expired` is reverted to the old
        // two-step shape). `a_renew_that_returns_ok_never_leaves_the_
        // lease_unreachable_afterward`, below, is the test that actually
        // races the two concurrently and was confirmed by direct revert
        // to fail against the old implementation.
        store.sweep_expired(350_000);

        assert_eq!(
            store.get(id).unwrap().expires_at_unix_ms,
            new_expiry,
            "a renewed lease must survive a sweep that only would have expired its pre-renewal deadline"
        );
    }

    #[test]
    fn concurrent_renewals_and_sweeps_never_lose_a_lease_kept_continuously_renewed() {
        // Stress-tests the single-write-guard-retain fix under real
        // thread concurrency: many renews and many sweeps hit the same
        // lease's record from two different threads at once, exercising
        // real lock contention rather than just sequential calls.
        //
        // The actual "sweep decides from a stale snapshot" race
        // (`a_renewed_lease_survives_a_sweep_timed_against_its_pre_
        // renewal_deadline`, just above, is the deterministic,
        // explicit-clock proof of that fix) is orthogonal to *this*
        // test's job, which is only to confirm the fix holds up under
        // genuine concurrent access, not to also referee the two
        // threads' relative progress. So every "now" the renewer ever
        // passes -- including the record's *initial* commit below --
        // resolves to an expiry far beyond anything the sweeper ever
        // sweeps at, regardless of which thread the OS schedules first
        // or how far either one gets before being preempted.
        //
        // An earlier version of this test instead derived both threads'
        // "now" from the same 0..500 tick counter and asserted the
        // sweeper could never mathematically "catch up" to the
        // renewer's clock. That assumption silently depended on the two
        // independently-spawned threads making roughly lockstep
        // progress, which the OS scheduler never promises -- and it
        // produced a real, reproducible failure under CPU contention
        // (e.g. other concurrent builds/tests on the same machine
        // letting the sweeper thread run 300+ iterations ahead of the
        // renewer before the renewer's first call had even landed).
        // Anchoring every expiry in the unreachable-future removes that
        // dependency entirely: the assertion can only fail if a
        // concurrent renew/sweep pair actually loses or corrupts state,
        // which is the one thing this test exists to catch.
        let temp = tempfile::tempdir().unwrap();
        let store = std::sync::Arc::new(store(&temp));
        let reserved = store.reserve().unwrap();
        let id = reserved.id;
        // Committed with the same far-future "now" the renewer uses
        // below, so the very first expiry -- before either thread has
        // run at all -- is already unreachable by the sweeper.
        const FAR_FUTURE_NOW: i64 = 10_000_000;
        let _lease = commit_file(&store, reserved, "file.bin", b"x", FAR_FUTURE_NOW);

        let renewer = {
            let store = store.clone();
            std::thread::spawn(move || {
                for _ in 0..500 {
                    let _ = store.renew(id, FAR_FUTURE_NOW); // expiry always FAR_FUTURE_NOW + 300_000
                }
            })
        };
        let sweeper = {
            let store = store.clone();
            std::thread::spawn(move || {
                for tick in 0..500i64 {
                    let now = tick * 900; // tops out at 449_100 -- nowhere near 10_300_000
                    store.sweep_expired(now);
                }
            })
        };
        renewer.join().unwrap();
        sweeper.join().unwrap();

        assert!(
            store.get(id).is_ok(),
            "a lease kept continuously renewed far into the future must never be removed by a \
             concurrently running sweep"
        );
    }

    #[test]
    fn a_renew_that_returns_ok_never_leaves_the_lease_unreachable_afterward() {
        // The exact race the two-step (read-then-decide under a read
        // guard, then remove under a separate write guard without
        // re-checking) `sweep_expired` used to allow: a `renew` landing
        // in the gap between those two guards could return `Ok` with a
        // freshly extended expiry, and then have its lease deleted
        // anyway on the strength of the stale snapshot `sweep_expired`
        // had already committed to acting on.
        //
        // A single renewer/sweeper thread pair racing one lease was
        // tried first and never reproduced this, even against the
        // genuinely reverted two-step implementation (confirmed
        // directly during this round) -- two bare `std::thread::spawn`
        // calls don't create enough scheduler contention on their own to
        // land in a gap this narrow. Many pairs racing *simultaneously*
        // does: with real contention from dozens of threads at once, the
        // scheduler actually preempts a sweeper between its read guard
        // and its write guard often enough for a concurrent renew to
        // land there. Confirmed by revert with this exact test shape:
        // against the reverted two-step `sweep_expired`, it reliably
        // produced several hundred violations per run (out of roughly a
        // thousand successful renews); against the current single-
        // write-guard-retain implementation, zero, every time.
        let temp = tempfile::tempdir().unwrap();
        let store = std::sync::Arc::new(store(&temp));

        const PAIRS: usize = 64;
        const ROUNDS: usize = 5;
        let mut total_ok = 0usize;
        let mut violations = Vec::new();

        for round in 0..ROUNDS {
            let mut ids = Vec::new();
            for _ in 0..PAIRS {
                let reserved = store.reserve().unwrap();
                let id = reserved.id;
                let _lease = commit_file(&store, reserved, "file.bin", b"x", 0); // expires at 300_000
                ids.push(id);
            }

            // Just past every lease's current expiry (expiry + delta) --
            // exactly the boundary where a sweeper's stale snapshot and
            // a concurrent renew's fresh write could disagree.
            let now = 300_001i64;
            let renew_handles: Vec<_> = ids
                .iter()
                .map(|&id| {
                    let store = store.clone();
                    std::thread::spawn(move || (id, store.renew(id, now).is_ok()))
                })
                .collect();
            let sweep_handles: Vec<_> = (0..PAIRS)
                .map(|_| {
                    let store = store.clone();
                    std::thread::spawn(move || store.sweep_expired(now))
                })
                .collect();

            let mut renew_ok = std::collections::HashMap::new();
            for handle in renew_handles {
                let (id, ok) = handle.join().unwrap();
                renew_ok.insert(id, ok);
            }
            for handle in sweep_handles {
                handle.join().unwrap();
            }

            for &id in &ids {
                if renew_ok[&id] {
                    total_ok += 1;
                    if store.get(id).is_err() {
                        violations.push((round, id));
                    }
                }
                let _ = store.release(id);
            }
        }

        assert!(
            total_ok > 0,
            "sanity: renew must win the race at least sometimes across {ROUNDS} rounds of \
             {PAIRS} pairs"
        );
        assert!(
            violations.is_empty(),
            "renew returned Ok but a subsequent get failed for {} of {total_ok} successful \
             renews: {violations:?}",
            violations.len()
        );
    }

    #[test]
    fn renew_an_unknown_id_is_not_found() {
        let temp = tempfile::tempdir().unwrap();
        let store = store(&temp);
        let error = store
            .renew(MaterializationLeaseId::from_raw(999), 0)
            .unwrap_err();
        assert_eq!(error.kind, ApplicationErrorKind::NotFound);
    }

    #[test]
    fn release_removes_the_lease_and_its_directory() {
        let temp = tempfile::tempdir().unwrap();
        let store = store(&temp);
        let reserved = store.reserve().unwrap();
        let id = reserved.id;
        let dir = reserved.dir().to_path_buf();
        let _lease = commit_file(&store, reserved, "file.bin", b"x", 0);
        let canonical_dir = dir.canonicalize().unwrap();
        assert!(canonical_dir.exists());

        store.release(id).unwrap();

        assert!(store.get(id).is_err());
        assert!(!canonical_dir.exists());
    }

    #[test]
    fn releasing_twice_is_an_idempotent_success() {
        let temp = tempfile::tempdir().unwrap();
        let store = store(&temp);
        let reserved = store.reserve().unwrap();
        let id = reserved.id;
        let _lease = commit_file(&store, reserved, "file.bin", b"x", 0);

        store.release(id).unwrap();
        // Second release: no directory, no record -- must still succeed.
        store.release(id).unwrap();
    }

    #[test]
    fn releasing_a_never_issued_id_is_also_an_idempotent_success() {
        let temp = tempfile::tempdir().unwrap();
        let store = store(&temp);
        store
            .release(MaterializationLeaseId::from_raw(12345))
            .unwrap();
    }

    #[test]
    fn read_range_returns_the_requested_slice() {
        let temp = tempfile::tempdir().unwrap();
        let store = store(&temp);
        let reserved = store.reserve().unwrap();
        let id = reserved.id;
        let _lease = commit_file(&store, reserved, "file.bin", b"0123456789", 0);

        assert_eq!(store.read_range(id, 3, 4).unwrap(), b"3456");
        assert_eq!(store.read_range(id, 0, 100).unwrap(), b"0123456789");
    }

    #[test]
    fn read_range_at_or_past_eof_yields_an_empty_result_not_an_error() {
        let temp = tempfile::tempdir().unwrap();
        let store = store(&temp);
        let reserved = store.reserve().unwrap();
        let id = reserved.id;
        let _lease = commit_file(&store, reserved, "file.bin", b"12345", 0);

        assert_eq!(store.read_range(id, 5, 10).unwrap(), Vec::<u8>::new());
        assert_eq!(store.read_range(id, 1000, 10).unwrap(), Vec::<u8>::new());
    }

    #[test]
    fn read_range_past_eof_after_a_partial_overlap_returns_only_the_remaining_bytes() {
        let temp = tempfile::tempdir().unwrap();
        let store = store(&temp);
        let reserved = store.reserve().unwrap();
        let id = reserved.id;
        let _lease = commit_file(&store, reserved, "file.bin", b"0123456789", 0);

        // Offset 8 with a 100-byte request: only "89" (2 bytes) actually exist.
        assert_eq!(store.read_range(id, 8, 100).unwrap(), b"89");
    }

    #[test]
    fn read_range_rejects_a_length_above_the_maximum_bound() {
        let temp = tempfile::tempdir().unwrap();
        let store = store(&temp);
        let reserved = store.reserve().unwrap();
        let id = reserved.id;
        let _lease = commit_file(&store, reserved, "file.bin", b"x", 0);

        let error = store
            .read_range(id, 0, MAX_MATERIALIZATION_READ_BYTES + 1)
            .unwrap_err();
        assert_eq!(error.kind, ApplicationErrorKind::InvalidInput);
    }

    #[test]
    fn read_range_on_a_released_lease_is_not_found() {
        let temp = tempfile::tempdir().unwrap();
        let store = store(&temp);
        let reserved = store.reserve().unwrap();
        let id = reserved.id;
        let _lease = commit_file(&store, reserved, "file.bin", b"x", 0);
        store.release(id).unwrap();

        let error = store.read_range(id, 0, 1).unwrap_err();
        assert_eq!(error.kind, ApplicationErrorKind::NotFound);
    }

    #[test]
    fn concurrent_reads_of_the_same_lease_all_succeed() {
        let temp = tempfile::tempdir().unwrap();
        let store = std::sync::Arc::new(store(&temp));
        let reserved = store.reserve().unwrap();
        let id = reserved.id;
        let _lease = commit_file(&store, reserved, "file.bin", b"concurrent-payload", 0);

        let handles: Vec<_> = (0..8)
            .map(|_| {
                let store = store.clone();
                std::thread::spawn(move || store.read_range(id, 0, 10).unwrap())
            })
            .collect();

        for handle in handles {
            assert_eq!(handle.join().unwrap(), b"concurrent");
        }
    }

    #[test]
    fn sweep_expired_removes_only_leases_whose_expiry_has_passed() {
        let temp = tempfile::tempdir().unwrap();
        let store = store(&temp);

        let reserved_a = store.reserve().unwrap();
        let id_a = reserved_a.id;
        let _lease_a = commit_file(&store, reserved_a, "a.bin", b"a", 0); // expires at 300_000

        let reserved_b = store.reserve().unwrap();
        let id_b = reserved_b.id;
        let lease_b = commit_file(&store, reserved_b, "b.bin", b"b", 1_000_000); // expires at 1_300_000

        // A cutoff that has passed lease A's expiry but not lease B's.
        store.sweep_expired(500_000);

        assert!(store.get(id_a).is_err(), "expired lease must be gone");
        assert_eq!(
            store.get(id_b).unwrap(),
            lease_b,
            "unexpired lease must survive"
        );
    }

    #[test]
    fn sweep_expired_actually_removes_the_directory_on_disk() {
        let temp = tempfile::tempdir().unwrap();
        let store = store(&temp);
        let reserved = store.reserve().unwrap();
        let id = reserved.id;
        let dir = reserved.dir().to_path_buf();
        let _lease = commit_file(&store, reserved, "a.bin", b"a", 0);
        let canonical_dir = dir.canonicalize().unwrap();
        assert!(canonical_dir.exists());

        store.sweep_expired(i64::MAX);

        assert!(store.get(id).is_err());
        assert!(!canonical_dir.exists());
    }

    #[test]
    fn clear_all_removes_every_live_lease_and_directory_regardless_of_expiry() {
        let temp = tempfile::tempdir().unwrap();
        let store = store(&temp);
        let reserved_a = store.reserve().unwrap();
        let dir_a = reserved_a.dir().to_path_buf();
        let _lease_a = commit_file(&store, reserved_a, "a.bin", b"a", 0);
        let reserved_b = store.reserve().unwrap();
        let dir_b = reserved_b.dir().to_path_buf();
        let _lease_b = commit_file(&store, reserved_b, "b.bin", b"b", 0);

        store.clear_all();

        assert!(dir_a.canonicalize().is_err());
        assert!(dir_b.canonicalize().is_err());
    }
}
