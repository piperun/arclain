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

fn directory_io_error(context: &str, path: &Path, error: std::io::Error) -> ApplicationError {
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

/// The store every [`MaterializationLease`] is tracked through for the
/// lifetime of its id's validity. See the module doc comment.
pub(crate) struct MaterializationStore {
    root: PathBuf,
    ttl: Duration,
    leases: RwLock<HashMap<MaterializationLeaseId, LeaseRecord>>,
    /// Per-store, not a process-wide `static` -- mirrors
    /// `ArchiveSessionStore::next_id`'s own field (see its doc comment for
    /// why a `static` counter shared across every store instance in one
    /// test binary is itself a latent smell, independent of any one test).
    next_id: AtomicU64,
}

impl MaterializationStore {
    pub(crate) fn new(root: PathBuf, ttl: Duration) -> Self {
        Self {
            root,
            ttl,
            leases: RwLock::new(HashMap::new()),
            next_id: AtomicU64::new(1),
        }
    }

    fn expires_at(&self, now_unix_ms: i64) -> i64 {
        now_unix_ms.saturating_add(self.ttl.as_millis().min(i64::MAX as u128) as i64)
    }

    /// Mints a fresh lease id and creates its (empty) owned directory,
    /// returning an RAII-guarded reservation the caller extracts into and
    /// then hands to [`Self::commit`]. Not yet visible to `get`/`renew`/
    /// `release`/`read_range` -- a caller that errors out before calling
    /// `commit` should just let the returned guard drop.
    pub(crate) fn reserve(&self) -> Result<ReservedLease, ApplicationError> {
        let id = MaterializationLeaseId::from_raw(self.next_id.fetch_add(1, Ordering::Relaxed));
        let dir = self.root.join(id.into_raw().to_string());
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
    pub(crate) fn sweep_expired(&self, now_unix_ms: i64) {
        let expired: Vec<(MaterializationLeaseId, PathBuf)> = {
            let leases = self.leases.read();
            leases
                .iter()
                .filter(|(_, record)| record.expires_at_unix_ms <= now_unix_ms)
                .map(|(id, record)| (*id, record.lease_dir.clone()))
                .collect()
        };
        if expired.is_empty() {
            return;
        }
        {
            let mut leases = self.leases.write();
            for (id, _) in &expired {
                leases.remove(id);
            }
        }
        for (_, dir) in expired {
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
