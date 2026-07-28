//! Archive mutation (add/delete/replace-text) as a cancellable,
//! event-broadcasting application operation, plus the file-editing save
//! flow that rides the same `ReplaceText` variant.
//!
//! # Characterization: what this replaces
//!
//! Pre-facade, these lived directly on `crates/ui`'s shared `AppState`/
//! `FileOpsService`, each independently resolving a backend and calling
//! it with no shared capability gating, no revision/id concept, and no
//! background-task boundary:
//!
//! - **Add** (`crates/ui/src/core/operations/file.rs::add_files`) picked
//!   files via `rfd::FileDialog`, then called `AppState::
//!   add_files_to_archive` -- `backend_selector.select(archive)?.
//!   add_files(archive, &files)` -- synchronously, on the render thread,
//!   while holding the shared `AppState` mutex.
//! - **Delete** (`FileOpsService::delete_files`/`delete_files_with_io`)
//!   ran `backend.delete_files` + a re-`list()` inside `spawn_blocking`,
//!   serialized per tab via a `parking_lot::Mutex` (`archive_edit_lock`)
//!   so a delete and a save-edit on the same tab never raced each
//!   other's backend call -- but nothing serialized two *different*
//!   tabs that happened to share the same archive file, and the
//!   toolbar's own selection was always excluded of folder rows before
//!   ever reaching this call (see `crates/ui/src/core/operations/
//!   file.rs::selected_file_paths_for_search`'s doc comment) -- directory
//!   deletion was never attempted, not merely unsupported by the
//!   backend.
//! - **Save** (`crates/ui/src/core/arclain_app/dialog_handler.rs`'s
//!   `FileEditResult::Save` handler) called `AppState::
//!   add_or_update_file_from_str` directly and synchronously, on the
//!   render thread, then `operations::archive::refresh_entries_after_edit`
//!   (another synchronous backend `list()` call) -- both while holding
//!   locks a slow archive (a large 7z's extract-modify-recompress cycle)
//!   could block the entire UI on for seconds.
//!
//! None of the three checked `BackendCapabilities` before calling the
//! backend (a read-only backend's own `Err("... is read-only")` was the
//! only signal), none had any optimistic-concurrency concept (nothing
//! stopped two racing edits on the same archive file), and none could be
//! cancelled once started.
//!
//! # What this operation changes
//!
//! - **Backend capability gating.** `BackendCapabilities::can_add_files`/
//!   `can_delete_files`/`can_modify_files` is checked before ever
//!   invoking the corresponding backend method, producing a structured
//!   `ApplicationErrorKind::Unsupported` instead of a read-only backend's
//!   raw string error.
//! - **Optimistic concurrency.** Every request carries `expected_revision`;
//!   a mismatch against `ArchiveSession::revision()` is rejected as
//!   `Conflict` before any backend call. [`ArchiveSession::mutation_lock`]
//!   (a new, session-scoped async mutex -- see its own doc comment) makes
//!   that check-then-act sequence atomic against a second concurrent
//!   mutation operation on the same session, closing the narrow race an
//!   `expected_revision` comparison alone cannot: two operations reading
//!   the same starting revision before either has bumped it.
//! - **Directory deletion.** `DeleteEntries` reuses [`ArchiveSession::
//!   resolve_extractable_paths`] -- the exact expansion extraction
//!   already relies on -- to expand a `Directory` id to every descendant
//!   file, rather than silently excluding folder rows the way the
//!   pre-facade toolbar selection did. This is a deliberate capability
//!   this facade's path-stable `EntryId`s make well-defined for the
//!   first time, not a preserved limitation.
//! - **Index truth.** [`ArchiveSession::reindex`] only ever runs (and
//!   `revision` only ever advances) after the backend's own mutating
//!   call has already returned success -- a failed mutation leaves the
//!   session's index exactly as it was, still describing only what the
//!   backend actually committed.
//! - **Honest desync, not a silently-stale index.** If the mutating call
//!   succeeds but the follow-up re-list needed to safely reindex fails,
//!   the session cannot describe its own real contents anymore --
//!   [`ArchiveSession::mark_desynced`] records that, and every subsequent
//!   mutation attempt on this session is rejected outright (regardless of
//!   its own `expected_revision`) until the archive is closed and
//!   reopened fresh. Without this, a stale-but-still-"current"-looking
//!   index could pass a later `expected_revision` check and, for example,
//!   let a `ReplaceText` recreate a file an earlier, already-succeeded
//!   `DeleteEntries` had removed.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use crate::archive::{ArchivePath, ArchiveSession, EntryKind};
use crate::error::{ApplicationError, ApplicationErrorKind, Recoverability};
use crate::event::{OperationResult, OperationState};
use crate::ids::{ArchiveSessionId, EntryId, OperationId};
use crate::runtime::AppRuntime;

/// A request to mutate an open archive session, the argument to
/// `ArclainApp::start_archive_mutation`. Every variant carries
/// `session_id` + `expected_revision`: the mutation is rejected as a
/// structured `Conflict` if the session's current revision does not
/// match, before any backend work runs.
#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ArchiveMutationRequest {
    /// Adds `source_paths` (real filesystem paths, e.g. from a file
    /// picker or a native drag-and-drop) into the archive. `destination`
    /// is carried per the contract, but every backend in this workspace
    /// today (`ArchiveBackend::add_files`) always adds new members at
    /// the archive root, keyed by each source file's own basename --
    /// there is no way to honor a real subfolder destination yet. Only
    /// `ArchivePath::root()` is accepted; anything else is rejected as
    /// `Unsupported` before any backend call, rather than silently
    /// adding at the root while the caller believes it chose a folder.
    AddFiles {
        session_id: ArchiveSessionId,
        expected_revision: u64,
        destination: ArchivePath,
        source_paths: Vec<PathBuf>,
    },
    /// Deletes every entry named by `entry_ids`. A `Directory` id
    /// expands to every descendant file at any depth (see
    /// [`ArchiveSession::resolve_extractable_paths`]); an id that
    /// resolves to zero concrete files (an empty selection, or a
    /// directory with no descendants) completes as a harmless no-op
    /// rather than ever calling the backend with an empty file list --
    /// mirroring `crate::operations::extract`'s identical concern: an
    /// empty explicit file-list argument reads to some backends as "no
    /// filter at all", which would invert the operation into deleting
    /// (or, for `add`, silently overwriting) everything.
    DeleteEntries {
        session_id: ArchiveSessionId,
        expected_revision: u64,
        entry_ids: Vec<EntryId>,
    },
    /// Overwrites `entry_id`'s content with `content`. Rejected as
    /// `InvalidInput` if `entry_id` names a `Directory` rather than a
    /// file.
    ReplaceText {
        session_id: ArchiveSessionId,
        expected_revision: u64,
        entry_id: EntryId,
        content: String,
    },
}

impl ArchiveMutationRequest {
    pub(crate) fn session_id(&self) -> ArchiveSessionId {
        match self {
            Self::AddFiles { session_id, .. }
            | Self::DeleteEntries { session_id, .. }
            | Self::ReplaceText { session_id, .. } => *session_id,
        }
    }

    pub(crate) fn expected_revision(&self) -> u64 {
        match self {
            Self::AddFiles {
                expected_revision, ..
            }
            | Self::DeleteEntries {
                expected_revision, ..
            }
            | Self::ReplaceText {
                expected_revision, ..
            } => *expected_revision,
        }
    }

    /// True for a request that is a structural no-op regardless of the
    /// session's own state -- `AddFiles` with nothing to add, or
    /// `DeleteEntries` with nothing selected. Checked before the session
    /// is even looked up: there is no session-dependent way for either
    /// to become non-empty. `ReplaceText` always names exactly one
    /// entry, so it is never structurally empty.
    fn is_structurally_empty(&self) -> bool {
        match self {
            Self::AddFiles { source_paths, .. } => source_paths.is_empty(),
            Self::DeleteEntries { entry_ids, .. } => entry_ids.is_empty(),
            Self::ReplaceText { .. } => false,
        }
    }

    fn action_label(&self) -> &'static str {
        match self {
            Self::AddFiles { .. } => "add files to the archive",
            Self::DeleteEntries { .. } => "delete entries from the archive",
            Self::ReplaceText { .. } => "replace the file's content in the archive",
        }
    }
}

/// What [`resolve_mutation`] settled on: the concrete instructions
/// [`perform_mutation`] hands to the backend. A private, request-shaped
/// mirror of [`ArchiveMutationRequest`] with every id/path already
/// resolved against the session's current index -- `perform_mutation`
/// itself never touches the session again.
enum ResolvedMutation {
    AddFiles {
        source_paths: Vec<PathBuf>,
    },
    Delete {
        paths: Vec<String>,
    },
    ReplaceText {
        path_in_archive: String,
        content: String,
    },
}

fn unknown_entry_error(session_id: ArchiveSessionId, entry_id: EntryId) -> ApplicationError {
    ApplicationError::new(
        ApplicationErrorKind::NotFound,
        "one of the requested entries does not exist in this archive session",
    )
    .with_recoverability(Recoverability::Fatal)
    .with_archive_session_id(session_id)
    .with_entry_id(entry_id)
}

fn cannot_replace_directory_error(
    session_id: ArchiveSessionId,
    entry_id: EntryId,
) -> ApplicationError {
    ApplicationError::new(
        ApplicationErrorKind::InvalidInput,
        "cannot replace the text content of a directory entry",
    )
    .with_recoverability(Recoverability::Fatal)
    .with_archive_session_id(session_id)
    .with_entry_id(entry_id)
    .with_field("entry_id")
}

fn non_root_destination_error() -> ApplicationError {
    ApplicationError::new(
        ApplicationErrorKind::Unsupported,
        "adding files into a specific archive folder is not supported yet -- only the archive root is",
    )
    .with_recoverability(Recoverability::Fatal)
    .with_field("destination")
}

fn unsupported_mutation_error(action: &str) -> ApplicationError {
    ApplicationError::new(
        ApplicationErrorKind::Unsupported,
        format!("this archive's backend cannot {action}"),
    )
    .with_recoverability(Recoverability::Fatal)
}

fn revision_conflict_error(
    session_id: ArchiveSessionId,
    expected: u64,
    actual: u64,
) -> ApplicationError {
    ApplicationError::new(
        ApplicationErrorKind::Conflict,
        "the archive changed since this mutation was prepared",
    )
    .with_diagnostic(format!(
        "expected revision {expected}, current revision {actual}"
    ))
    .with_recoverability(Recoverability::Retry)
    .with_retryable(true)
    .with_archive_session_id(session_id)
}

/// Rejected unconditionally -- regardless of what `expected_revision` a
/// caller submits -- once [`ArchiveSession::is_desynced`] is true. See
/// that method's own doc comment for why: a prior mutation's post-success
/// re-list failed, so nothing in this session can prove what the archive's
/// real contents are anymore, and no `expected_revision` a caller could
/// supply (even one read after the desync, since `mark_desynced` also
/// bumps `revision`) is trustworthy against a possibly-stale index.
fn desynced_session_error(session_id: ArchiveSessionId) -> ApplicationError {
    ApplicationError::new(
        ApplicationErrorKind::Conflict,
        "this archive session's index could not be kept in sync with a previous change -- \
         close and reopen the archive to continue",
    )
    .with_recoverability(Recoverability::Fatal)
    .with_archive_session_id(session_id)
}

fn mutation_backend_error(action: &str, error: anyhow::Error) -> ApplicationError {
    ApplicationError::new(ApplicationErrorKind::Backend, format!("failed to {action}"))
        .with_diagnostic(format!("{error:#}"))
        .with_recoverability(Recoverability::Retry)
        .with_retryable(true)
}

/// Distinct from [`mutation_backend_error`]: the mutation itself already
/// succeeded here -- only the follow-up `list()` this operation needs in
/// order to safely reindex failed. The archive's real, on-disk content
/// has already changed; this session's own cached index simply could not
/// be refreshed to match it this time. Unlike an ordinary backend
/// failure, retrying will not help: the caller `session.mark_desynced()`
/// this triggers (see its own doc comment) rejects every subsequent
/// mutation on this session outright, so `Recoverability::Fatal` here is
/// accurate, not conservative -- and the summary itself carries the only
/// real recovery path (close and reopen), since the frozen
/// `ApplicationErrorKind`/`SuggestedAction` contract has no dedicated
/// variant for "this session needs a fresh open" to hang that guidance
/// off of instead.
fn relist_after_mutation_failed_error(
    session_id: ArchiveSessionId,
    error: anyhow::Error,
) -> ApplicationError {
    ApplicationError::new(
        ApplicationErrorKind::Backend,
        "the archive was modified but its updated contents could not be confirmed -- close and \
         reopen this archive to continue safely",
    )
    .with_diagnostic(format!("{error:#}"))
    .with_recoverability(Recoverability::Fatal)
    .with_archive_session_id(session_id)
}

fn internal_join_error(join_error: tokio::task::JoinError) -> ApplicationError {
    ApplicationError::new(
        ApplicationErrorKind::Internal,
        "archive mutation worker failed",
    )
    .with_diagnostic(join_error.to_string())
}

/// Checks `request`'s corresponding [`arclain_core::archive::
/// BackendCapabilities`] flag, before any backend call is attempted.
fn check_capability(
    request: &ArchiveMutationRequest,
    capabilities: &arclain_core::archive::BackendCapabilities,
) -> Result<(), ApplicationError> {
    let supported = match request {
        ArchiveMutationRequest::AddFiles { .. } => capabilities.can_add_files,
        ArchiveMutationRequest::DeleteEntries { .. } => capabilities.can_delete_files,
        ArchiveMutationRequest::ReplaceText { .. } => capabilities.can_modify_files,
    };
    if supported {
        Ok(())
    } else {
        Err(unsupported_mutation_error(request.action_label()))
    }
}

/// Resolves `request` against `session`'s *current* index into concrete
/// backend instructions. `Ok(None)` means the request is a genuine,
/// harmless no-op once resolved (a `DeleteEntries` selection that
/// expands to zero concrete files) -- the caller completes immediately
/// without ever calling the backend, mirroring
/// `crate::operations::extract`'s identical "nothing survived the
/// filter" handling.
fn resolve_mutation(
    session: &ArchiveSession,
    request: &ArchiveMutationRequest,
) -> Result<Option<ResolvedMutation>, ApplicationError> {
    match request {
        ArchiveMutationRequest::AddFiles {
            destination,
            source_paths,
            ..
        } => {
            if destination != &ArchivePath::root() {
                return Err(non_root_destination_error());
            }
            Ok(Some(ResolvedMutation::AddFiles {
                source_paths: source_paths.clone(),
            }))
        }
        ArchiveMutationRequest::DeleteEntries {
            session_id,
            entry_ids,
            ..
        } => {
            let paths = session
                .resolve_extractable_paths(entry_ids)
                .map_err(|bad_id| unknown_entry_error(*session_id, bad_id))?;
            if paths.is_empty() {
                Ok(None)
            } else {
                Ok(Some(ResolvedMutation::Delete { paths }))
            }
        }
        ArchiveMutationRequest::ReplaceText {
            session_id,
            entry_id,
            content,
            ..
        } => {
            let dto = session
                .entry(*entry_id)
                .ok_or_else(|| unknown_entry_error(*session_id, *entry_id))?;
            if dto.kind == EntryKind::Directory {
                return Err(cannot_replace_directory_error(*session_id, *entry_id));
            }
            Ok(Some(ResolvedMutation::ReplaceText {
                path_in_archive: dto.path.as_str().to_string(),
                content: content.clone(),
            }))
        }
    }
}

/// The single blocking backend call every resolved mutation kind
/// dispatches to. Runs inside `spawn_blocking`, holding
/// `ArchiveSession::mutation_lock` for its whole duration (see that
/// method's own doc comment) -- callers must invoke this only from a
/// blocking-safe context.
fn perform_mutation(
    backend: &Arc<dyn arclain_core::ArchiveBackend>,
    archive_path: &Path,
    resolved: ResolvedMutation,
) -> anyhow::Result<()> {
    match resolved {
        ResolvedMutation::AddFiles { source_paths } => {
            backend.add_files(archive_path, &source_paths)
        }
        ResolvedMutation::Delete { paths } => backend.delete_files(archive_path, &paths),
        ResolvedMutation::ReplaceText {
            path_in_archive,
            content,
        } => backend.add_or_update_file_from_str(archive_path, &path_in_archive, &content),
    }
}

async fn fail(inner: &Arc<AppRuntime>, operation_id: OperationId, error: ApplicationError) {
    let _ = inner
        .operations()
        .transition(operation_id, OperationState::Failed { error })
        .await;
}

/// The `start_archive_mutation` background worker. Spawned via the
/// application's own runtime handle; runs until the operation reaches a
/// terminal state (`Completed`, `Cancelled`, or `Failed`).
///
/// `_cancel` is unused directly -- every cancellation check goes through
/// `inner.operations().is_cancelled`/`wait_until_cancelled`, mirroring
/// every other operation kind in this crate (see `crate::operations::
/// extract::run_extract`'s identical parameter and its own doc comment
/// on why it is still threaded through rather than dropped).
///
/// Cancellation is checkpoint-based, not a race against the mutating
/// backend call itself: once `perform_mutation`'s `spawn_blocking` call
/// is dispatched, it always runs to completion (mirroring
/// `crate::runtime::processing_ops`'s identical, documented limitation
/// for its own per-file blocking calls) -- there is no lower-level
/// cancellation hook `ArchiveBackend::add_files`/`delete_files`/
/// `add_or_update_file_from_str` expose the way `ExtractRunner::kill`
/// does for extraction's CLI subprocess. Cancellation checks (and,
/// around acquiring `ArchiveSession::mutation_lock`, an actual race) sit
/// at every checkpoint *before* that dispatch instead, so a mutation
/// that has not yet started its backend call -- including one still
/// queued behind another mutation on the same session -- stops
/// cleanly and promptly.
pub(crate) async fn run_archive_mutation(
    inner: Arc<AppRuntime>,
    operation_id: OperationId,
    _cancel: Arc<AtomicBool>,
    request: ArchiveMutationRequest,
) {
    if inner
        .operations()
        .transition(operation_id, OperationState::Started)
        .await
        .is_err()
    {
        return;
    }

    // Session validation BEFORE anything else, including the structural
    // no-op short-circuit just below: the contract requires every facade
    // method to validate a reconstructed id against its owning store, and
    // a caller submitting a bogus/closed `session_id` alongside an empty
    // `entry_ids`/`source_paths` must still see `NotFound`, not a
    // false-positive `Completed`.
    let session_id = request.session_id();
    let session = match inner.archive_sessions().get(session_id).await {
        Ok(session) => session,
        Err(error) => {
            fail(&inner, operation_id, error).await;
            return;
        }
    };

    if inner.operations().is_cancelled(operation_id).await {
        return;
    }

    if request.is_structurally_empty() {
        let _ = inner
            .operations()
            .transition(
                operation_id,
                OperationState::Completed {
                    result: OperationResult::None,
                },
            )
            .await;
        return;
    }

    // Racing the lock acquisition itself against cancellation means a
    // mutation queued behind another one on the same session can still
    // be cancelled promptly, without waiting for the one ahead of it to
    // finish.
    let guard = tokio::select! {
        guard = session.mutation_lock().lock() => guard,
        () = inner.operations().wait_until_cancelled(operation_id) => {
            return;
        }
    };

    // Unconditional -- checked before, and independent of,
    // `expected_revision` -- see `ArchiveSession::is_desynced`'s own doc
    // comment for why a stale index must never again pass a revision
    // check once this is set, however "current" a caller's own revision
    // claim looks.
    if session.is_desynced() {
        fail(&inner, operation_id, desynced_session_error(session_id)).await;
        return;
    }

    // Revision check BEFORE any backend work -- and, holding `guard`,
    // atomic against a second concurrent mutation on this same session
    // rather than merely sequential.
    let current_revision = session.revision();
    if request.expected_revision() != current_revision {
        fail(
            &inner,
            operation_id,
            revision_conflict_error(session_id, request.expected_revision(), current_revision),
        )
        .await;
        return;
    }

    if inner.operations().is_cancelled(operation_id).await {
        return;
    }

    let backend = {
        let archive = session.archive_arc();
        let archive_guard = archive.lock();
        archive_guard.backend_arc()
    };
    if let Err(error) = check_capability(&request, &backend.capabilities()) {
        fail(&inner, operation_id, error).await;
        return;
    }

    let resolved = match resolve_mutation(&session, &request) {
        Ok(Some(resolved)) => resolved,
        Ok(None) => {
            let _ = inner
                .operations()
                .transition(
                    operation_id,
                    OperationState::Completed {
                        result: OperationResult::None,
                    },
                )
                .await;
            return;
        }
        Err(error) => {
            fail(&inner, operation_id, error).await;
            return;
        }
    };

    if inner.operations().is_cancelled(operation_id).await {
        return;
    }

    let Some(handle) = inner.tokio_handle() else {
        return;
    };

    let action = request.action_label();
    let source_path = session.source_path().to_path_buf();
    let backend_for_mutation = backend.clone();
    let mutate_result = handle
        .spawn_blocking(move || perform_mutation(&backend_for_mutation, &source_path, resolved))
        .await;

    match mutate_result {
        Ok(Ok(())) => {}
        Ok(Err(error)) => {
            fail(&inner, operation_id, mutation_backend_error(action, error)).await;
            return;
        }
        Err(join_error) => {
            fail(&inner, operation_id, internal_join_error(join_error)).await;
            return;
        }
    }

    // The mutation genuinely landed -- re-list and reindex, still under
    // `guard`, so a second mutation queued behind this one only ever
    // observes the fully-updated revision, never a half-applied one.
    let password = {
        let archive = session.archive_arc();
        let archive_guard = archive.lock();
        archive_guard.password_ref().map(str::to_string)
    };
    let source_path_for_list = session.source_path().to_path_buf();
    let backend_for_list = backend.clone();
    let session_for_reindex = session.clone();
    let relist_result = handle
        .spawn_blocking(move || -> anyhow::Result<u64> {
            let info = backend_for_list.list(&source_path_for_list, password.as_deref())?;
            Ok(session_for_reindex.reindex(&info.entries))
        })
        .await;

    drop(guard);

    match relist_result {
        Ok(Ok(revision)) => {
            let _ = inner
                .operations()
                .transition(
                    operation_id,
                    OperationState::SnapshotChanged {
                        session_id,
                        revision,
                    },
                )
                .await;
            let _ = inner
                .operations()
                .transition(
                    operation_id,
                    OperationState::Completed {
                        result: OperationResult::None,
                    },
                )
                .await;
        }
        Ok(Err(error)) => {
            // The mutation itself already landed on the backend, but we
            // can no longer prove what the archive's contents actually
            // are -- see `ArchiveSession::mark_desynced`'s own doc
            // comment. Marking this BEFORE `fail` is not load-bearing
            // for correctness (the operation is about to go terminal
            // either way), but keeps "the session is desynced" true from
            // the earliest possible instant for any other reader.
            session.mark_desynced();
            fail(
                &inner,
                operation_id,
                relist_after_mutation_failed_error(session_id, error),
            )
            .await;
        }
        Err(join_error) => {
            fail(&inner, operation_id, internal_join_error(join_error)).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn add_files_request(
        destination: ArchivePath,
        source_paths: Vec<PathBuf>,
    ) -> ArchiveMutationRequest {
        ArchiveMutationRequest::AddFiles {
            session_id: ArchiveSessionId::from_raw(1),
            expected_revision: 1,
            destination,
            source_paths,
        }
    }

    #[test]
    fn archive_mutation_request_serializes_snake_case_and_round_trips() {
        let request = add_files_request(ArchivePath::root(), vec![PathBuf::from("/tmp/a.txt")]);
        let value = serde_json::to_value(&request).unwrap();
        assert_eq!(
            value,
            serde_json::json!({
                "kind": "add_files",
                "session_id": 1,
                "expected_revision": 1,
                "destination": "",
                "source_paths": ["/tmp/a.txt"],
            })
        );
        let round_tripped: ArchiveMutationRequest = serde_json::from_value(value).unwrap();
        assert_eq!(round_tripped.session_id(), request.session_id());

        let delete = ArchiveMutationRequest::DeleteEntries {
            session_id: ArchiveSessionId::from_raw(2),
            expected_revision: 5,
            entry_ids: vec![EntryId::from_raw(9)],
        };
        assert_eq!(
            serde_json::to_value(&delete).unwrap(),
            serde_json::json!({
                "kind": "delete_entries",
                "session_id": 2,
                "expected_revision": 5,
                "entry_ids": [9],
            })
        );

        let replace = ArchiveMutationRequest::ReplaceText {
            session_id: ArchiveSessionId::from_raw(3),
            expected_revision: 7,
            entry_id: EntryId::from_raw(4),
            content: "hello".to_string(),
        };
        assert_eq!(
            serde_json::to_value(&replace).unwrap(),
            serde_json::json!({
                "kind": "replace_text",
                "session_id": 3,
                "expected_revision": 7,
                "entry_id": 4,
                "content": "hello",
            })
        );
    }

    #[test]
    fn session_id_and_expected_revision_are_exposed_for_every_variant() {
        let add = add_files_request(ArchivePath::root(), vec![]);
        assert_eq!(add.session_id(), ArchiveSessionId::from_raw(1));
        assert_eq!(add.expected_revision(), 1);

        let delete = ArchiveMutationRequest::DeleteEntries {
            session_id: ArchiveSessionId::from_raw(9),
            expected_revision: 3,
            entry_ids: vec![],
        };
        assert_eq!(delete.session_id(), ArchiveSessionId::from_raw(9));
        assert_eq!(delete.expected_revision(), 3);

        let replace = ArchiveMutationRequest::ReplaceText {
            session_id: ArchiveSessionId::from_raw(2),
            expected_revision: 8,
            entry_id: EntryId::from_raw(1),
            content: String::new(),
        };
        assert_eq!(replace.session_id(), ArchiveSessionId::from_raw(2));
        assert_eq!(replace.expected_revision(), 8);
    }

    #[test]
    fn add_files_with_no_source_paths_is_structurally_empty() {
        assert!(add_files_request(ArchivePath::root(), vec![]).is_structurally_empty());
        assert!(
            !add_files_request(ArchivePath::root(), vec![PathBuf::from("a")])
                .is_structurally_empty()
        );
    }

    #[test]
    fn delete_entries_with_no_ids_is_structurally_empty() {
        let empty = ArchiveMutationRequest::DeleteEntries {
            session_id: ArchiveSessionId::from_raw(1),
            expected_revision: 1,
            entry_ids: vec![],
        };
        assert!(empty.is_structurally_empty());

        let non_empty = ArchiveMutationRequest::DeleteEntries {
            session_id: ArchiveSessionId::from_raw(1),
            expected_revision: 1,
            entry_ids: vec![EntryId::from_raw(1)],
        };
        assert!(!non_empty.is_structurally_empty());
    }

    #[test]
    fn replace_text_is_never_structurally_empty() {
        let replace = ArchiveMutationRequest::ReplaceText {
            session_id: ArchiveSessionId::from_raw(1),
            expected_revision: 1,
            entry_id: EntryId::from_raw(1),
            content: String::new(),
        };
        assert!(!replace.is_structurally_empty());
    }

    #[test]
    fn check_capability_matches_each_variant_to_its_own_flag() {
        let read_only = arclain_core::archive::BackendCapabilities::read_only();
        let full = arclain_core::archive::BackendCapabilities::full_featured();

        let add = add_files_request(ArchivePath::root(), vec![PathBuf::from("a")]);
        assert!(check_capability(&add, &read_only).is_err());
        assert!(check_capability(&add, &full).is_ok());

        let delete = ArchiveMutationRequest::DeleteEntries {
            session_id: ArchiveSessionId::from_raw(1),
            expected_revision: 1,
            entry_ids: vec![EntryId::from_raw(1)],
        };
        assert!(check_capability(&delete, &read_only).is_err());
        assert!(check_capability(&delete, &full).is_ok());

        let replace = ArchiveMutationRequest::ReplaceText {
            session_id: ArchiveSessionId::from_raw(1),
            expected_revision: 1,
            entry_id: EntryId::from_raw(1),
            content: String::new(),
        };
        assert!(check_capability(&replace, &read_only).is_err());
        assert!(check_capability(&replace, &full).is_ok());
    }

    #[test]
    fn check_capability_failure_is_unsupported() {
        let read_only = arclain_core::archive::BackendCapabilities::read_only();
        let add = add_files_request(ArchivePath::root(), vec![PathBuf::from("a")]);
        let error = check_capability(&add, &read_only).unwrap_err();
        assert_eq!(error.kind, ApplicationErrorKind::Unsupported);
    }

    #[test]
    fn a_non_root_add_files_destination_is_rejected_before_resolution() {
        let destination = ArchivePath::parse("some/folder".to_string()).unwrap();
        // `resolve_mutation` needs a real session for the other two
        // variants, but `AddFiles`'s destination check runs first and
        // never touches the session -- exercised directly via
        // `non_root_destination_error`'s own call site is covered by the
        // integration test suite (`crates/app/tests/archive_mutation.rs`),
        // which can build a real session; this unit test only pins the
        // error's classification.
        let error = non_root_destination_error();
        assert_eq!(error.kind, ApplicationErrorKind::Unsupported);
        assert_ne!(destination, ArchivePath::root());
    }

    #[test]
    fn revision_conflict_error_is_retryable_and_carries_both_revisions() {
        let error = revision_conflict_error(ArchiveSessionId::from_raw(1), 3, 5);
        assert_eq!(error.kind, ApplicationErrorKind::Conflict);
        assert!(error.retryable);
        let diagnostic = error.diagnostic.expect("diagnostic must be set");
        assert!(diagnostic.contains('3'));
        assert!(diagnostic.contains('5'));
    }
}
