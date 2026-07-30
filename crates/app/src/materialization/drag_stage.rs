//! Multi-entry staging of archive content onto local disk for an OS
//! drag-out -- the operation behind `ArclainApp::start_drag_stage` and
//! its synchronous companion `ArclainApp::stage_drag_payload_blocking`
//! (see `crate::runtime::drag_stage_ops`).
//!
//! # Why this exists next to, not inside, `run_materialize`
//!
//! A drag-out stages *the rows the user actually dragged*: any number of
//! files and directories at once. [`MaterializeRequest`] deliberately
//! rejects two or more entry ids -- a general materialization lease must
//! resolve to one coherent `local_path`, and there is no principled way
//! to point at "these three unrelated entries" (see that type's own doc
//! comment). A drag *does* have a principled answer: the lease's
//! `local_path` is the staging **root directory** itself, and the shell
//! is handed the selection's top-level names under it -- exactly the
//! shape a CF_HDROP transfer wants. So this worker accepts the
//! multi-entry selection, extracts it (directories expanded to their
//! subtrees through the same
//! [`crate::archive::ArchiveSession::resolve_extractable_paths`]
//! expansion extraction uses), and commits the staging root as the
//! lease's `local_path`. [`MaterializeRequest`]'s single-entry contract
//! is untouched.
//!
//! # Why a password failure fails fast instead of raising a `Challenge`
//!
//! Every other extraction-shaped operation raises
//! `Challenge::Password` and waits. This worker must not: at the moment
//! it runs, the OS shell is synchronously blocked inside our
//! `IDataObject::GetData` waiting for the staged bytes (that is the
//! entire reason `stage_drag_payload_blocking` exists). A challenge
//! would leave Explorer frozen until the user finds and answers a
//! password dialog -- and the session's own password (the one the
//! archive was opened and listed with) is already supplied here, so a
//! password-shaped failure means the archive's *content* needs a
//! different password than its listing did. The pre-facade drag path
//! failed the drop in that case too; this keeps that behavior and
//! reports `PasswordRequired` with `SuggestedAction::SupplyPassword`.
//!
//! # Command-line-length batching is load-bearing, not a relic
//!
//! `SevenZipCli::extract_files_with_progress` delegates to
//! `extract_files`, which **silently truncates** the file list once the
//! command line exceeds its length cap ("This may cause incomplete
//! extraction"). The pre-facade drag layer defended against that with a
//! 75-file threshold, switching to a common-directory (or whole-archive)
//! batch extraction above it. That defense moves here with the
//! extraction call itself -- dropping it would have made large drags
//! silently incomplete. (The same hazard exists independently in
//! `run_materialize`'s whole-archive path; surfaced in this task's
//! report rather than silently fixed, since that worker is another
//! task's surface.)

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use crate::archive::EntryKind;
use crate::error::{ApplicationError, ApplicationErrorKind, Recoverability, SuggestedAction};
use crate::event::{OperationResult, OperationState};
use crate::ids::{ArchiveSessionId, EntryId, OperationId};
use crate::materialization::MaterializationLease;
use crate::runtime::AppRuntime;

/// A request to stage archive entries on local disk for an OS drag-out,
/// the argument to `ArclainApp::start_drag_stage` and
/// `ArclainApp::stage_drag_payload_blocking`.
///
/// Unlike `MaterializeRequest`/`ExtractRequest`, `entry_ids` here must be
/// **non-empty**: a drag gesture always names the specific rows being
/// dragged, so an empty selection is a caller bug (`InvalidInput`), not a
/// "whole archive" convention. Any mix of files and directories is
/// accepted; directories expand to their whole subtrees.
#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub struct DragStageRequest {
    pub session_id: ArchiveSessionId,
    pub entry_ids: Vec<EntryId>,
}

/// What `ArclainApp::stage_drag_payload_blocking` reports through its
/// callback while it blocks.
///
/// `Started` delivers the `OperationId` as soon as the operation is
/// registered, *before* extraction begins -- it is the caller's handle
/// for cancelling the stage from another thread
/// (`ArclainApp::cancel_operation`) while this thread is still blocked.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DragStageEvent {
    Started {
        operation_id: OperationId,
    },
    Progress {
        percent: u8,
        message: Option<String>,
    },
}

/// An RAII handle to a successfully staged drag payload.
///
/// While this value lives, a background task on the application runtime
/// keeps the underlying [`MaterializationLease`] renewed, so the staged
/// files survive however long the OS shell takes to finish reading them
/// (a large Explorer copy can easily outlast the lease TTL). Dropping it
/// aborts the renewal task and releases the lease (removing the staged
/// directory) via a fire-and-forget task on the same runtime.
///
/// Holding this value holds an `ArclainApp` clone, which is exactly what
/// guarantees the runtime its release/renewal tasks run on cannot be torn
/// down underneath them: teardown only happens once every `ArclainApp`
/// reference is gone, and this one is not gone until `Drop` has already
/// spawned the release. If the application was explicitly `shutdown()`
/// first, the release call is refused by the shutdown gate and the
/// directory is reclaimed by shutdown's own `clear_all` (or, after a
/// crash, by the next bootstrap's leftover-clearing) instead.
#[derive(Debug)]
pub struct DragStagingLease {
    app: crate::runtime::ArclainApp,
    lease: MaterializationLease,
    handle: tokio::runtime::Handle,
    renew_task: Option<tokio::task::AbortHandle>,
}

impl DragStagingLease {
    /// Constructed only by `ArclainApp::stage_drag_payload_blocking` (via
    /// `crate::runtime::drag_stage_ops`), which is what has the
    /// application's own runtime handle to spawn the renewal task on.
    pub(crate) fn new(
        app: crate::runtime::ArclainApp,
        lease: MaterializationLease,
        handle: tokio::runtime::Handle,
    ) -> Self {
        let renew_task = {
            let app = app.clone();
            let lease_id = lease.id;
            // Renew at a third of the TTL, so a renewal has room to land
            // well before expiry even under scheduling delay. That is
            // slack, not retry tolerance: the loop below treats any
            // renewal error as terminal. Clamped, because test TTL
            // overrides are tens of milliseconds and production is
            // minutes.
            let ttl_ms = (lease.expires_at_unix_ms - super::current_unix_ms()).max(0) as u64;
            let interval = std::time::Duration::from_millis((ttl_ms / 3).clamp(25, 60_000));
            let task = handle.spawn(async move {
                loop {
                    tokio::time::sleep(interval).await;
                    // NotFound (released/expired underneath us) or the
                    // post-shutdown gate both mean there is nothing left
                    // to keep alive.
                    if app.renew_materialization(lease_id).await.is_err() {
                        return;
                    }
                }
            });
            Some(task.abort_handle())
        };
        Self {
            app,
            lease,
            handle,
            renew_task,
        }
    }

    /// The staging root directory every staged path lives under -- the
    /// lease's own `local_path`.
    pub fn local_root(&self) -> &std::path::Path {
        &self.lease.local_path
    }

    /// The underlying lease, for callers that want its id or expiry.
    pub fn lease(&self) -> &MaterializationLease {
        &self.lease
    }
}

impl Drop for DragStagingLease {
    fn drop(&mut self) {
        if let Some(renew) = self.renew_task.take() {
            renew.abort();
        }
        let app = self.app.clone();
        let lease_id = self.lease.id;
        // Fire-and-forget: `release_materialization` is idempotent, and if
        // the application has been shut down in the meantime the dispatch
        // gate refuses it -- shutdown's own `clear_all` (or the next
        // bootstrap's leftover clear) reclaims the directory instead. The
        // spawn itself is safe: `self.app` still holds the runtime alive
        // at this point (see the type's doc comment).
        self.handle.spawn(async move {
            let _ = app.release_materialization(lease_id).await;
        });
    }
}

/// Above this many resolved files, extraction switches from an explicit
/// per-file invocation to a common-directory (or whole-archive) batch --
/// the 7-Zip CLI backend silently truncates over-long command lines (see
/// the module doc comment). Same threshold the pre-facade drag layer
/// used.
pub(crate) const MAX_DIRECT_EXTRACT_FILES: usize = 75;

/// Finds the deepest directory (forward-slash form, no trailing slash)
/// containing every path in `file_paths`; `Some("")` when everything sits
/// at the archive root, `None` when there is no common directory. Ported
/// verbatim in behavior from the pre-facade drag layer's own helper so
/// batch extraction selects the same 7-Zip invocation it always did.
pub(crate) fn find_common_directory(file_paths: &[String]) -> Option<String> {
    if file_paths.is_empty() {
        return None;
    }
    let normalized: Vec<String> = file_paths.iter().map(|p| p.replace('\\', "/")).collect();
    let first = &normalized[0];
    let first_parts: Vec<&str> = first.split('/').collect();
    if first_parts.len() <= 1 {
        let all_in_root = normalized.iter().all(|p| !p.contains('/'));
        return if all_in_root {
            Some(String::new())
        } else {
            None
        };
    }
    let mut common_parts = &first_parts[..first_parts.len() - 1];
    for path in normalized.iter().skip(1) {
        let parts: Vec<&str> = path.split('/').collect();
        let dir_parts = &parts[..parts.len().saturating_sub(1)];
        let mut match_count = 0;
        for (i, part) in common_parts.iter().enumerate() {
            if i < dir_parts.len() && dir_parts[i] == *part {
                match_count += 1;
            } else {
                break;
            }
        }
        common_parts = &common_parts[..match_count];
        if common_parts.is_empty() {
            return None;
        }
    }
    if common_parts.is_empty() {
        None
    } else {
        Some(common_parts.join("/"))
    }
}

fn empty_selection_error() -> ApplicationError {
    ApplicationError::new(
        ApplicationErrorKind::InvalidInput,
        "a drag stage requires a non-empty selection",
    )
    .with_recoverability(Recoverability::Fatal)
    .with_field("entry_ids")
}

fn password_error(session_id: ArchiveSessionId, diagnostic: String) -> ApplicationError {
    ApplicationError::new(
        ApplicationErrorKind::PasswordRequired,
        "the archive's content needs a password this session does not hold",
    )
    .with_diagnostic(diagnostic)
    .with_recoverability(Recoverability::UserAction)
    .with_suggested_action(SuggestedAction::SupplyPassword)
    .with_archive_session_id(session_id)
}

async fn fail(inner: &Arc<AppRuntime>, operation_id: OperationId, error: ApplicationError) {
    let _ = inner
        .operations()
        .transition(operation_id, OperationState::Failed { error })
        .await;
}

/// The `start_drag_stage` background worker. Spawned via the
/// application's own runtime handle; runs until the operation reaches a
/// terminal state.
///
/// Cancellation follows `run_materialize`'s shape, with one documented
/// gap: the registry's
/// cancel flag is handed to the backend as its `CancellationToken`, the
/// blocking extraction is awaited to completion (never abandoned -- the
/// reservation's cleanup must not race a backend still writing into its
/// directory), and the cancelled check afterwards drops the reservation,
/// which removes the staging directory. The registry publishes
/// `Cancelled` immediately when `cancel_operation` is called, so a
/// blocked `stage_drag_payload_blocking` caller unblocks right away
/// while this worker finishes cooperatively in the background.
///
/// The gap: `run_materialize` passes the token on every arm, but the
/// batched (>75 file) arm here has no token to pass -- that backend
/// entry point takes none. A cancel during a batched stage still
/// unblocks the shell immediately and still cleans up, but the backend
/// churns to completion first. This matches what the pre-facade drag
/// did; closing it needs a cancellable batch entry point in the backend.
pub(crate) async fn run_drag_stage(
    inner: Arc<AppRuntime>,
    operation_id: OperationId,
    cancel: Arc<AtomicBool>,
    request: DragStageRequest,
) {
    if inner
        .operations()
        .transition(operation_id, OperationState::Started)
        .await
        .is_err()
    {
        return;
    }

    let session = match inner.archive_sessions().get(request.session_id).await {
        Ok(session) => session,
        Err(_) => {
            fail(
                &inner,
                operation_id,
                super::unknown_session_error(request.session_id),
            )
            .await;
            return;
        }
    };

    if request.entry_ids.is_empty() {
        fail(&inner, operation_id, empty_selection_error()).await;
        return;
    }

    // Selected directories' own paths: extraction only creates a
    // directory as a side effect of writing files beneath it, so a
    // selected *empty* directory must be created explicitly after
    // extraction for the shell to receive it.
    let mut selected_dirs: Vec<String> = Vec::new();
    for &entry_id in &request.entry_ids {
        match session.entry(entry_id) {
            Some(dto) => {
                if dto.kind == EntryKind::Directory {
                    selected_dirs.push(dto.path.as_str().to_string());
                }
            }
            None => {
                fail(
                    &inner,
                    operation_id,
                    super::unknown_entry_error(request.session_id, entry_id),
                )
                .await;
                return;
            }
        }
    }

    let files = match session.resolve_extractable_paths(&request.entry_ids) {
        Ok(files) => files,
        Err(bad_id) => {
            fail(
                &inner,
                operation_id,
                super::unknown_entry_error(request.session_id, bad_id),
            )
            .await;
            return;
        }
    };

    if inner.operations().is_cancelled(operation_id).await {
        return;
    }

    let reserved = match inner.materialization().reserve() {
        Ok(reserved) => reserved,
        Err(error) => {
            fail(&inner, operation_id, error).await;
            return;
        }
    };
    let dest_dir = reserved.dir().to_path_buf();

    let source_path = session.source_path().to_path_buf();
    let (backend, password) = {
        let archive = session.archive_arc();
        let guard = archive.lock();
        (
            guard.backend_arc(),
            guard.password_ref().map(str::to_string),
        )
    };

    let Some(handle) = inner.tokio_handle() else {
        return;
    };

    let _ = inner
        .operations()
        .transition(
            operation_id,
            OperationState::Progress {
                completed_units: 0,
                total_units: Some(100),
                message: Some(format!("Staging {} files for drag...", files.len())),
            },
        )
        .await;

    // Progress arrives on a blocking thread and must reach the async
    // registry: forward through an unbounded channel drained by a task on
    // the app runtime (the same shape `crate::operations::merge` uses).
    // The join below guarantees no `Progress` event is ever published
    // after the terminal transition.
    let (progress_tx, mut progress_rx) =
        tokio::sync::mpsc::unbounded_channel::<arclain_core::ExtractionProgress>();
    let forwarder = {
        let forward_inner = inner.clone();
        handle.spawn(async move {
            let mut last_percent: Option<u8> = None;
            while let Some(update) = progress_rx.recv().await {
                if last_percent == Some(update.percent) {
                    continue;
                }
                last_percent = Some(update.percent);
                let _ = forward_inner
                    .operations()
                    .transition(
                        operation_id,
                        OperationState::Progress {
                            completed_units: u64::from(update.percent),
                            total_units: Some(100),
                            message: Some(update.current_file),
                        },
                    )
                    .await;
            }
        })
    };

    let backend_for_spawn = backend.clone();
    let files_for_spawn = files.clone();
    let dest_for_spawn = dest_dir.clone();
    let source_for_spawn = source_path.clone();
    let password_for_spawn = password.clone();
    let cancel_for_spawn = cancel.clone();

    let spawn_result = handle
        .spawn_blocking(move || {
            if files_for_spawn.is_empty() {
                // A selection of only empty directories resolves to zero
                // files. The backend must NOT be invoked for it: the 7-Zip
                // CLI treats "no file arguments" as "no filter" and would
                // extract the entire archive. The selected directories
                // themselves are created explicitly below.
                Ok(())
            } else if files_for_spawn.len() > MAX_DIRECT_EXTRACT_FILES {
                // Batch strategy: never hand the CLI backend a file list
                // long enough to truncate (see the module doc comment).
                // Extraction of extra sibling files is invisible to the
                // shell -- the drop only ever names the selection's own
                // top-level paths under the staging root.
                match find_common_directory(&files_for_spawn) {
                    Some(dir_path) => backend_for_spawn.extract_directory(
                        &source_for_spawn,
                        &dest_for_spawn,
                        &dir_path,
                        password_for_spawn.as_deref(),
                    ),
                    None => backend_for_spawn.extract_all(
                        &source_for_spawn,
                        &dest_for_spawn,
                        password_for_spawn.as_deref(),
                    ),
                }
            } else {
                let progress_cb = move |p: arclain_core::ExtractionProgress| {
                    let _ = progress_tx.send(p);
                };
                backend_for_spawn.extract_files_with_progress(
                    &source_for_spawn,
                    &dest_for_spawn,
                    &files_for_spawn,
                    password_for_spawn.as_deref(),
                    Some(&progress_cb),
                    Some(&cancel_for_spawn),
                )
            }
        })
        .await;

    // All senders are gone once the blocking call returns (the >75 branch
    // dropped its clone without ever sending), so the forwarder drains and
    // exits on its own.
    let _ = forwarder.await;

    match spawn_result {
        Ok(Ok(())) => {}
        Ok(Err(error)) => {
            if inner.operations().is_cancelled(operation_id).await {
                return; // A cancelled backend's error is not a failure.
            }
            let diagnostic = format!("{error:#}");
            let error = if super::is_password_error(&diagnostic) {
                password_error(request.session_id, diagnostic)
            } else {
                ApplicationError::new(
                    ApplicationErrorKind::Backend,
                    "failed to stage the dragged entries",
                )
                .with_diagnostic(diagnostic)
                .with_recoverability(Recoverability::Retry)
                .with_retryable(true)
            };
            fail(&inner, operation_id, error).await;
            return;
        }
        Err(join_error) => {
            fail(
                &inner,
                operation_id,
                ApplicationError::new(ApplicationErrorKind::Internal, "drag stage worker failed")
                    .with_diagnostic(join_error.to_string()),
            )
            .await;
            return;
        }
    }

    if inner.operations().is_cancelled(operation_id).await {
        return; // `reserved` drops here, removing the staging directory.
    }

    let Some(handle) = inner.tokio_handle() else {
        return;
    };
    let size_dest = dest_dir.clone();
    let size_result = handle
        .spawn_blocking(move || {
            // Selected empty directories produced no files for extraction
            // to create them through; `create_dir_all` is idempotent for
            // the (common) non-empty case.
            for dir in &selected_dirs {
                let dir_path: PathBuf = size_dest.join(dir);
                std::fs::create_dir_all(&dir_path)
                    .map_err(|error| super::fs_error(&dir_path, error))?;
            }
            super::compute_total_size(&size_dest)
        })
        .await;
    let size = match size_result {
        Ok(Ok(size)) => size,
        Ok(Err(error)) => {
            fail(&inner, operation_id, error).await;
            return;
        }
        Err(join_error) => {
            fail(
                &inner,
                operation_id,
                ApplicationError::new(ApplicationErrorKind::Internal, "drag stage worker failed")
                    .with_diagnostic(join_error.to_string()),
            )
            .await;
            return;
        }
    };

    // The staging ROOT is the lease's local_path -- see the module doc
    // comment for why a multi-entry stage commits the root rather than
    // any one entry's own path.
    let lease =
        match inner
            .materialization()
            .commit(reserved, dest_dir, size, super::current_unix_ms())
        {
            Ok(lease) => lease,
            Err(error) => {
                fail(&inner, operation_id, error).await;
                return;
            }
        };

    let _ = inner
        .operations()
        .transition(
            operation_id,
            OperationState::Completed {
                result: OperationResult::Materialized { lease },
            },
        )
        .await;
}

#[cfg(test)]
mod tests {
    use super::find_common_directory;

    #[test]
    fn common_directory_of_root_level_files_is_the_empty_root() {
        assert_eq!(
            find_common_directory(&["a.txt".into(), "b.txt".into()]),
            Some(String::new())
        );
    }

    #[test]
    fn common_directory_of_one_subtree_is_that_directory() {
        assert_eq!(
            find_common_directory(&["game/a.txt".into(), "game/data/b.txt".into()]),
            Some("game".to_string())
        );
    }

    #[test]
    fn no_common_directory_when_a_root_file_mixes_with_a_nested_one() {
        assert_eq!(
            find_common_directory(&["game/a.txt".into(), "readme.txt".into()]),
            None
        );
    }

    #[test]
    fn backslash_paths_normalize_before_comparison() {
        assert_eq!(
            find_common_directory(&["game\\a.txt".into(), "game\\b.txt".into()]),
            Some("game".to_string())
        );
    }
}
