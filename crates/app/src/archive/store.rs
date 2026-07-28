//! The store every open [`ArchiveSession`] lives in for the duration of
//! its session id's validity.
//!
//! Mints [`ArchiveSessionId`]s itself, from a counter owned by each store
//! instance (not a process-wide `static`, unlike `OperationRegistry`'s
//! `next_operation_id` -- see [`ArchiveSessionStore::next_id`]'s own doc
//! comment for why this store's id namespace does not follow that
//! pattern). Is the one place a reconstructed id (round-tripped through
//! `ArchiveSessionId::from_raw`, e.g. from a bridge payload or persisted
//! UI state) gets validated: [`ArchiveSessionStore::get`] and
//! [`ArchiveSessionStore::close`] both reject an id this store never
//! minted, or one it already closed, with `NotFound` rather than trusting
//! the caller's reconstructed value.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use tokio::runtime::Handle;
use tokio::sync::RwLock;

use crate::archive::ArchiveSession;
use crate::error::{ApplicationError, ApplicationErrorKind, Recoverability};
use crate::ids::ArchiveSessionId;

fn unknown_session_error(session_id: ArchiveSessionId) -> ApplicationError {
    ApplicationError::new(ApplicationErrorKind::NotFound, "no such archive session")
        .with_recoverability(Recoverability::Fatal)
        .with_archive_session_id(session_id)
}

/// The store every open [`ArchiveSession`] is tracked through for the
/// lifetime of its id's validity.
pub(crate) struct ArchiveSessionStore {
    sessions: RwLock<HashMap<ArchiveSessionId, Arc<ArchiveSession>>>,
    /// Per-store, not `crate::operations::registry::next_operation_id`'s
    /// process-wide `static` pattern: a `static` counter is shared by
    /// *every* `ArchiveSessionStore` instance that ever exists in the
    /// process, including one from an unrelated bootstrap in the same
    /// test binary (`cargo test` runs many tests, each with its own
    /// store, concurrently in one process). That cross-instance sharing
    /// is itself a latent smell independent of any one test -- two
    /// unrelated stores should not draw from the same id sequence -- and
    /// it also made an id-probing test's premise ("id 1 is the first
    /// this store would ever mint") false in practice, since another
    /// test's store could easily have already consumed id 1 before this
    /// one ran. A field, seeded fresh at `new()`, makes "the first id
    /// this store instance mints is 1" a true, deterministic statement
    /// again.
    next_id: AtomicU64,
}

impl ArchiveSessionStore {
    pub(crate) fn new() -> Self {
        Self {
            sessions: RwLock::new(HashMap::new()),
            next_id: AtomicU64::new(1),
        }
    }

    /// Mints a fresh [`ArchiveSessionId`], builds the session (indexing
    /// `entries` -- see [`ArchiveSession::new`]), inserts it, and returns
    /// it. The caller is expected to have already performed the actual
    /// backend `list()` call (and any password retry loop) before calling
    /// this -- building the entry index is pure in-memory work, but this
    /// method itself briefly takes the store's write lock, so it must
    /// never be called while holding a lock across blocking archive I/O.
    ///
    /// `ArchiveSession::new` runs inside `handle.spawn_blocking` rather
    /// than directly on this call's own async task: indexing walks every
    /// entry, synthesizes ancestor directories, and aggregates each
    /// folder's totals, which for a large archive is real, potentially
    /// slow CPU work that has no business running on an async executor's
    /// worker thread (it would otherwise starve every other task sharing
    /// that thread for the duration). `handle` is the caller's own
    /// application-owned runtime handle, threaded through explicitly
    /// rather than reached for via the ambient `tokio::task::
    /// spawn_blocking` -- this crate's runtime rules are "never the
    /// caller's ambient runtime", full stop, not "unless it happens to
    /// already be the right one": `archive_ops::run_open_archive` (this
    /// method's one production call site) already holds exactly this
    /// handle in scope for its own `spawn_blocking` call just above, so
    /// threading it one call further costs nothing and removes the
    /// implicit assumption entirely.
    pub(crate) async fn open(
        &self,
        source_path: PathBuf,
        archive_type: String,
        archive: arclain_core::Archive,
        entries: Arc<Vec<arclain_core::ArchiveEntry>>,
        handle: &Handle,
    ) -> Result<Arc<ArchiveSession>, ApplicationError> {
        let id = ArchiveSessionId::from_raw(self.next_id.fetch_add(1, Ordering::Relaxed));
        let session = handle
            .spawn_blocking(move || {
                Arc::new(ArchiveSession::new(
                    id,
                    source_path,
                    archive_type,
                    archive,
                    entries.as_slice(),
                ))
            })
            .await
            .map_err(|join_error| {
                ApplicationError::new(ApplicationErrorKind::Internal, "failed to index archive")
                    .with_diagnostic(join_error.to_string())
                    .with_recoverability(Recoverability::Fatal)
                    .with_archive_session_id(id)
            })?;
        self.sessions.write().await.insert(id, session.clone());
        Ok(session)
    }

    /// Validates `session_id` against this store and returns a clone of
    /// the session `Arc`. Callers must release any guard this returns
    /// before invoking a synchronous backend call through the session's
    /// own `archive_arc()` -- this method's own read guard is already
    /// released by the time it returns, so it never needs that care
    /// itself.
    pub(crate) async fn get(
        &self,
        session_id: ArchiveSessionId,
    ) -> Result<Arc<ArchiveSession>, ApplicationError> {
        self.sessions
            .read()
            .await
            .get(&session_id)
            .cloned()
            .ok_or_else(|| unknown_session_error(session_id))
    }

    /// Removes `session_id` from the store. Rejects an unknown or
    /// already-closed id with `NotFound` rather than silently succeeding,
    /// so a caller cannot mistake "closed twice" for "closed".
    pub(crate) async fn close(&self, session_id: ArchiveSessionId) -> Result<(), ApplicationError> {
        self.sessions
            .write()
            .await
            .remove(&session_id)
            .map(|_| ())
            .ok_or_else(|| unknown_session_error(session_id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    struct NoopBackend;
    impl arclain_core::ArchiveBackend for NoopBackend {
        fn name(&self) -> &str {
            "noop"
        }
        fn capabilities(&self) -> arclain_core::archive::BackendCapabilities {
            arclain_core::archive::BackendCapabilities::read_only()
        }
        fn identify(&self, _path: &Path) -> anyhow::Result<arclain_core::archive::ArchiveKind> {
            Ok(arclain_core::archive::ArchiveKind::Zip)
        }
        fn list(
            &self,
            _path: &Path,
            _password: Option<&str>,
        ) -> anyhow::Result<arclain_core::ArchiveInfo> {
            unimplemented!()
        }
        fn extract_all(
            &self,
            _path: &Path,
            _dest: &Path,
            _password: Option<&str>,
        ) -> anyhow::Result<()> {
            unimplemented!()
        }
        fn extract_files(
            &self,
            _path: &Path,
            _dest: &Path,
            _files: &[String],
            _password: Option<&str>,
        ) -> anyhow::Result<()> {
            unimplemented!()
        }
        fn extract_directory(
            &self,
            _path: &Path,
            _dest: &Path,
            _dir_path: &str,
            _password: Option<&str>,
        ) -> anyhow::Result<()> {
            unimplemented!()
        }
        fn recompress_7z(&self, _source: &Path, _dest_7z: &Path) -> anyhow::Result<()> {
            unimplemented!()
        }
        fn add_files(&self, _archive: &Path, _files: &[std::path::PathBuf]) -> anyhow::Result<()> {
            unimplemented!()
        }
        fn create_archive(
            &self,
            _dest: &Path,
            _files: &[std::path::PathBuf],
            _format: &str,
        ) -> anyhow::Result<()> {
            unimplemented!()
        }
        fn read_text_file(
            &self,
            _archive: &Path,
            _path_in_archive: &str,
            _password: Option<&str>,
        ) -> anyhow::Result<String> {
            unimplemented!()
        }
        fn delete_files(&self, _archive: &Path, _files: &[String]) -> anyhow::Result<()> {
            unimplemented!()
        }
        fn add_or_update_file_from_str(
            &self,
            _archive: &Path,
            _path_in_archive: &str,
            _content: &str,
        ) -> anyhow::Result<()> {
            unimplemented!()
        }
        fn convert_to_7z(
            &self,
            _source: &arclain_core::Archive,
            _dest: &Path,
            _temp_dir: &Path,
        ) -> anyhow::Result<()> {
            unimplemented!()
        }
        fn crc32_of_entry(
            &self,
            _archive: &Path,
            _path_in_archive: &str,
            _password: Option<&str>,
        ) -> anyhow::Result<String> {
            unimplemented!()
        }
    }

    fn dummy_archive() -> arclain_core::Archive {
        arclain_core::Archive::new(Arc::new(NoopBackend), PathBuf::from("dummy.zip"))
    }

    #[tokio::test]
    async fn open_mints_a_fresh_id_and_the_session_is_retrievable() {
        let store = ArchiveSessionStore::new();
        let session = store
            .open(
                PathBuf::from("a.zip"),
                "zip".to_string(),
                dummy_archive(),
                Arc::new(Vec::new()),
                &Handle::current(),
            )
            .await
            .unwrap();

        let fetched = store.get(session.id()).await.unwrap();
        assert_eq!(fetched.id(), session.id());
    }

    #[tokio::test]
    async fn two_opens_mint_distinct_ids() {
        let store = ArchiveSessionStore::new();
        let a = store
            .open(
                PathBuf::from("a.zip"),
                "zip".to_string(),
                dummy_archive(),
                Arc::new(Vec::new()),
                &Handle::current(),
            )
            .await
            .unwrap();
        let b = store
            .open(
                PathBuf::from("b.zip"),
                "zip".to_string(),
                dummy_archive(),
                Arc::new(Vec::new()),
                &Handle::current(),
            )
            .await
            .unwrap();
        assert_ne!(a.id(), b.id());
    }

    #[tokio::test]
    async fn a_fresh_store_always_mints_id_1_first_regardless_of_other_stores_in_the_process() {
        // Regression guard for the bug a per-store counter fixes: with a
        // process-wide `static` counter, this would be flaky/false
        // depending on how many *other* stores' `open()` calls happened
        // to run first in the same test binary. A fresh field-backed
        // counter makes it a deterministic fact about this store alone.
        let store = ArchiveSessionStore::new();
        let session = store
            .open(
                PathBuf::from("a.zip"),
                "zip".to_string(),
                dummy_archive(),
                Arc::new(Vec::new()),
                &Handle::current(),
            )
            .await
            .unwrap();
        assert_eq!(session.id(), ArchiveSessionId::from_raw(1));
    }

    #[tokio::test]
    async fn unknown_reconstructed_id_is_rejected_with_not_found() {
        let store = ArchiveSessionStore::new();
        // Never opened by this store -- a purely reconstructed id, as if
        // round-tripped through `ArchiveSessionId::from_raw` from a stale
        // bridge payload.
        let reconstructed = ArchiveSessionId::from_raw(999_999);

        let error = store.get(reconstructed).await.unwrap_err();
        assert_eq!(error.kind, ApplicationErrorKind::NotFound);
        assert_eq!(error.archive_session_id, Some(reconstructed));

        let close_error = store.close(reconstructed).await.unwrap_err();
        assert_eq!(close_error.kind, ApplicationErrorKind::NotFound);
    }

    #[tokio::test]
    async fn closing_a_session_makes_it_unreachable_and_closing_twice_is_not_found() {
        let store = ArchiveSessionStore::new();
        let session = store
            .open(
                PathBuf::from("a.zip"),
                "zip".to_string(),
                dummy_archive(),
                Arc::new(Vec::new()),
                &Handle::current(),
            )
            .await
            .unwrap();

        store.close(session.id()).await.unwrap();

        let error = store.get(session.id()).await.unwrap_err();
        assert_eq!(error.kind, ApplicationErrorKind::NotFound);

        let second_close = store.close(session.id()).await.unwrap_err();
        assert_eq!(second_close.kind, ApplicationErrorKind::NotFound);
    }
}
