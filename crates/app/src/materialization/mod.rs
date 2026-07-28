//! Explicit, leased materialization of one archive entry onto a real local
//! disk path.
//!
//! Every UI surface that needs archive content to exist as a real file on
//! disk -- launching it in an external application, dragging it out to the
//! OS shell, previewing or editing it -- used to extract into an ad hoc
//! temporary directory and either leak it (`std::mem::forget`, so the
//! files stay put for as long as the process runs, forever) or risk it
//! disappearing out from under whatever still needed it (the directory's
//! owner dropping and cleaning up on `Drop` while an external viewer, a
//! spawned OS process, or an in-flight OS drag operation was still reading
//! from it). [`MaterializationLease`] replaces both failure modes with an
//! explicit, application-owned resource with an explicit lifetime: created
//! by [`crate::ArclainApp::start_materialization`], kept alive by
//! [`crate::ArclainApp::renew_materialization`] for as long as a caller
//! still needs it, released explicitly
//! ([`crate::ArclainApp::release_materialization`]) or left to expire on
//! its own -- either way, [`store::MaterializationStore`] is the one place
//! that ever removes the directory, and it always does.
//!
//! [`run_materialize`] is the background worker `start_materialization`
//! spawns: it resolves the requested entry (a single file materializes to
//! that one extracted path; a directory materializes to the whole
//! extracted subtree, reusing [`crate::archive::ArchiveSession::resolve_extractable_paths`]'s
//! existing recursive expansion -- the same traversal-safety guarantee
//! extraction already established, since an `EntryId` can only ever name a
//! path that already passed `ArchivePath::parse`), reserves a lease
//! directory, extracts into it (retrying through a password challenge the
//! same way `crate::operations::extract` does, via the same
//! `Challenge`/`ChallengeWaiters` machinery), and completes with
//! `OperationResult::Materialized { lease }`.

pub(crate) mod store;

pub(crate) use store::MaterializationStore;

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;

use crate::archive::{ArchiveSession, EntryKind};
use crate::challenge::{next_challenge_id, Challenge, ChallengeResponse};
use crate::error::{ApplicationError, ApplicationErrorKind, Recoverability};
use crate::event::{OperationResult, OperationState};
use crate::ids::{ArchiveSessionId, EntryId, OperationId};
use crate::runtime::AppRuntime;

/// A request to materialize one archive entry onto a real local disk path,
/// the argument to `ArclainApp::start_materialization`.
#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub struct MaterializeRequest {
    pub session_id: ArchiveSessionId,
    pub entry_id: EntryId,
    pub purpose: MaterializationPurpose,
}

/// Why a caller wants an entry materialized. Purely informational today --
/// every purpose is served identically by the same lease machinery, with
/// lifetime managed entirely through explicit renew/release calls rather
/// than a purpose-specific policy -- but is part of the request so a
/// caller's own UI logic (how aggressively to renew, what to log) can
/// branch on intent without this crate needing to guess it from context.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MaterializationPurpose {
    DragOut,
    Edit,
    ExternalOpen,
    Preview,
}

/// One live materialization lease: a real path on local disk, owned by the
/// application for as long as `expires_at_unix_ms` (extended by
/// `renew_materialization`) has not passed and `release_materialization`
/// has not been called.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct MaterializationLease {
    pub id: crate::ids::MaterializationLeaseId,
    pub local_path: PathBuf,
    pub size: u64,
    pub expires_at_unix_ms: i64,
}

/// The largest `length` `ArclainApp::read_materialization_range` accepts in
/// one call -- a caller that needs a whole large file reads it in bounded
/// chunks rather than in one unbounded call that could otherwise force this
/// process to buffer an arbitrarily large `Vec<u8>` in memory at once.
pub const MAX_MATERIALIZATION_READ_BYTES: u32 = 2 * 1024 * 1024;

/// Production default lease lifetime: how long a freshly-created or
/// renewed lease stays valid before the cleanup task removes it.
/// `BootstrapConfig::materialization_lease_ttl_override` overrides this in
/// tests that need a much shorter TTL to observe real expiry without a
/// long wait.
pub(crate) const DEFAULT_LEASE_TTL: Duration = Duration::from_secs(300);

/// Production default interval between expiry sweeps (see
/// [`run_cleanup_task`]). `BootstrapConfig::materialization_cleanup_interval_override`
/// overrides this in tests for the same reason as [`DEFAULT_LEASE_TTL`].
pub(crate) const DEFAULT_CLEANUP_INTERVAL: Duration = Duration::from_secs(30);

/// The current wall-clock time, in Unix milliseconds. The one place
/// production code reads the system clock for lease expiry -- every store
/// method that needs "now" takes it as an explicit parameter instead (see
/// `store`'s own module doc comment), so a test never has to mock this
/// function itself, only pass a different value into the store directly.
pub(crate) fn current_unix_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(i64::MAX as u128) as i64)
        .unwrap_or(0)
}

/// The background task `ArclainApp::bootstrap` spawns once, for the life of
/// the application runtime, to remove expired materialization leases. Never
/// stopped explicitly -- like every other task spawned onto this app's own
/// runtime, it is simply abandoned when the runtime shuts down.
pub(crate) async fn run_cleanup_task(inner: Arc<AppRuntime>, interval: Duration) {
    let mut ticker = tokio::time::interval(interval);
    loop {
        ticker.tick().await;
        inner.materialization().sweep_expired(current_unix_ms());
    }
}

fn unknown_session_error(session_id: ArchiveSessionId) -> ApplicationError {
    ApplicationError::new(ApplicationErrorKind::NotFound, "no such archive session")
        .with_recoverability(Recoverability::Fatal)
        .with_archive_session_id(session_id)
}

fn unknown_entry_error(session_id: ArchiveSessionId, entry_id: EntryId) -> ApplicationError {
    ApplicationError::new(
        ApplicationErrorKind::NotFound,
        "the requested entry does not exist in this archive session",
    )
    .with_recoverability(Recoverability::Fatal)
    .with_archive_session_id(session_id)
    .with_entry_id(entry_id)
}

fn fs_error(path: &Path, error: std::io::Error) -> ApplicationError {
    ApplicationError::new(
        ApplicationErrorKind::Internal,
        "failed to inspect materialized content",
    )
    .with_diagnostic(format!("{}: {error}", path.display()))
}

/// Detects whether a backend error message indicates a password failure.
/// Duplicated from `crate::operations::extract::is_password_error` rather
/// than shared -- that module's own doc comment gives the reasoning
/// (different callers reading different error shapes; small enough that a
/// shared generic version would cost more indirection than it saves).
fn is_password_error(diagnostic: &str) -> bool {
    diagnostic.contains("Wrong password")
        || diagnostic.contains("Incorrect password")
        || diagnostic.contains("Password for encrypted archive not specified")
        || diagnostic.contains("Cannot open encrypted")
        || diagnostic.contains("Can not open encrypted")
        || diagnostic.contains("Enter password")
        || diagnostic.contains("code Some(2)")
        || diagnostic.contains("code Some(11)")
        || diagnostic.contains("code Some(255)")
}

async fn fail(inner: &Arc<AppRuntime>, operation_id: OperationId, error: ApplicationError) {
    let _ = inner
        .operations()
        .transition(operation_id, OperationState::Failed { error })
        .await;
}

/// Raises a `Challenge::Password` on `operation_id`, awaits the caller's
/// response, and returns the freshly supplied password. `None` means the
/// operation was cancelled (or the challenge channel closed) while
/// waiting. Mirrors `crate::operations::extract::await_password_retry`
/// exactly, so materialization behaves identically to extraction and
/// archive-open from a frontend's perspective.
async fn await_password_retry(
    inner: &Arc<AppRuntime>,
    operation_id: OperationId,
    archive_name: &str,
    attempt: &mut u32,
) -> Option<String> {
    let challenge_id = next_challenge_id();
    let receiver = inner.challenges().register(operation_id);
    if inner
        .operations()
        .transition(
            operation_id,
            OperationState::Challenge {
                challenge: Challenge::Password {
                    id: challenge_id,
                    archive_name: archive_name.to_string(),
                    attempt: *attempt,
                },
            },
        )
        .await
        .is_err()
    {
        inner.challenges().cancel(operation_id);
        return None;
    }

    let response = tokio::select! {
        response = receiver => response,
        () = inner.operations().wait_until_cancelled(operation_id) => {
            inner.challenges().cancel(operation_id);
            return None;
        }
    };

    match response {
        Ok(ChallengeResponse::Password { value, .. }) => {
            *attempt += 1;
            Some(value.expose_secret().to_string())
        }
        _ => None,
    }
}

/// What one materialization request resolves to: a single file (the
/// requested entry's own path), or a directory (its own path, plus every
/// descendant file's path, recursively -- the same expansion extraction's
/// own directory selection uses). `own_path` is kept for the `Directory`
/// case (not just the descendant file list) because extraction preserves
/// each descendant's full archive-relative path, prefix included --
/// requesting directory `"game"` extracts to `dest_dir/game/...`, not
/// `dest_dir/...` -- so `local_path` must join `dest_dir` with the
/// directory's own path to point at the folder that actually resulted,
/// exactly the same reasoning already applied to the `File` case.
enum MaterializeSelection {
    File(String),
    Directory {
        own_path: String,
        files: Vec<String>,
    },
}

impl MaterializeSelection {
    fn files(&self) -> Vec<String> {
        match self {
            MaterializeSelection::File(path) => vec![path.clone()],
            MaterializeSelection::Directory { files, .. } => files.clone(),
        }
    }

    /// Where the lease's `local_path` points once extraction into
    /// `dest_dir` succeeds: the one extracted file itself for a `File`
    /// selection, or the extracted directory (preserving its own
    /// archive-relative path under `dest_dir`) for a `Directory` selection.
    fn local_path(&self, dest_dir: &Path) -> PathBuf {
        match self {
            MaterializeSelection::File(path) => dest_dir.join(path),
            MaterializeSelection::Directory { own_path, .. } => dest_dir.join(own_path),
        }
    }
}

fn resolve_selection(
    session: &ArchiveSession,
    session_id: ArchiveSessionId,
    entry_id: EntryId,
) -> Result<MaterializeSelection, ApplicationError> {
    let entry = session
        .entry(entry_id)
        .ok_or_else(|| unknown_entry_error(session_id, entry_id))?;
    match entry.kind {
        EntryKind::Directory => {
            let files = session
                .resolve_extractable_paths(&[entry_id])
                .map_err(|bad_id| unknown_entry_error(session_id, bad_id))?;
            Ok(MaterializeSelection::Directory {
                own_path: entry.path.as_str().to_string(),
                files,
            })
        }
        EntryKind::File | EntryKind::Symlink => {
            Ok(MaterializeSelection::File(entry.path.as_str().to_string()))
        }
    }
}

/// Sums the byte size of `path`: its own length if it is a file, or the
/// recursive total of every file beneath it if it is a directory (an empty
/// directory -- a `Directory` selection with zero descendant files -- sums
/// to zero, not an error).
fn compute_total_size(path: &Path) -> Result<u64, ApplicationError> {
    let metadata = std::fs::symlink_metadata(path).map_err(|error| fs_error(path, error))?;
    if !metadata.is_dir() {
        return Ok(metadata.len());
    }
    let mut total: u64 = 0;
    let mut stack = vec![path.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).map_err(|error| fs_error(&dir, error))? {
            let entry = entry.map_err(|error| fs_error(&dir, error))?;
            let entry_path = entry.path();
            let entry_metadata = entry
                .metadata()
                .map_err(|error| fs_error(&entry_path, error))?;
            if entry_metadata.is_dir() {
                stack.push(entry_path);
            } else {
                total = total.saturating_add(entry_metadata.len());
            }
        }
    }
    Ok(total)
}

/// The `start_materialization` background worker. Spawned via the
/// application's own runtime handle; runs until the operation reaches a
/// terminal state (`Completed`, `Cancelled`, or `Failed`).
pub(crate) async fn run_materialize(
    inner: Arc<AppRuntime>,
    operation_id: OperationId,
    cancel: Arc<AtomicBool>,
    request: MaterializeRequest,
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
                unknown_session_error(request.session_id),
            )
            .await;
            return;
        }
    };

    let selection = match resolve_selection(&session, request.session_id, request.entry_id) {
        Ok(selection) => selection,
        Err(error) => {
            fail(&inner, operation_id, error).await;
            return;
        }
    };
    let files = selection.files();

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
    let local_path = selection.local_path(&dest_dir);

    let archive_name = session
        .source_path()
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| session.source_path().to_string_lossy().into_owned());
    let source_path = session.source_path().to_path_buf();
    let (backend, mut password) = {
        let archive = session.archive_arc();
        let guard = archive.lock();
        (
            guard.backend_arc(),
            guard.password_ref().map(str::to_string),
        )
    };
    let mut attempt: u32 = 1;

    'retry: loop {
        if inner.operations().is_cancelled(operation_id).await {
            return; // `reserved` drops here, cleaning up its own directory.
        }

        let Some(handle) = inner.tokio_handle() else {
            return;
        };

        let backend_for_spawn = backend.clone();
        let files_for_spawn = files.clone();
        let dest_for_spawn = dest_dir.clone();
        let source_for_spawn = source_path.clone();
        let password_for_spawn = password.clone();
        let cancel_for_spawn = cancel.clone();

        let spawn_result = handle
            .spawn_blocking(move || {
                backend_for_spawn.extract_files_with_progress(
                    &source_for_spawn,
                    &dest_for_spawn,
                    &files_for_spawn,
                    password_for_spawn.as_deref(),
                    None,
                    Some(&cancel_for_spawn),
                )
            })
            .await;

        match spawn_result {
            Ok(Ok(())) => break 'retry,
            Ok(Err(error)) => {
                let diagnostic = format!("{error:#}");
                if is_password_error(&diagnostic) {
                    match await_password_retry(&inner, operation_id, &archive_name, &mut attempt)
                        .await
                    {
                        Some(new_password) => {
                            password = Some(new_password);
                            continue 'retry;
                        }
                        None => return,
                    }
                } else {
                    fail(
                        &inner,
                        operation_id,
                        ApplicationError::new(
                            ApplicationErrorKind::Backend,
                            "failed to materialize the requested entry",
                        )
                        .with_diagnostic(diagnostic)
                        .with_recoverability(Recoverability::Retry)
                        .with_retryable(true),
                    )
                    .await;
                    return;
                }
            }
            Err(join_error) => {
                fail(
                    &inner,
                    operation_id,
                    ApplicationError::new(
                        ApplicationErrorKind::Internal,
                        "materialization worker failed",
                    )
                    .with_diagnostic(join_error.to_string()),
                )
                .await;
                return;
            }
        }
    }

    if inner.operations().is_cancelled(operation_id).await {
        return; // `reserved` drops here; extraction finished but a
                // concurrent cancel won the race for the registry.
    }

    let Some(handle) = inner.tokio_handle() else {
        return;
    };
    let size_path = local_path.clone();
    // A `Directory` selection with zero descendant files (an empty
    // archive folder) never gets its `local_path` created as a side
    // effect of extraction -- there is nothing for the backend to write.
    // `create_dir_all` is idempotent, so calling it unconditionally here
    // is harmless even when extraction already created the directory
    // (the common, non-empty case).
    let ensure_directory = matches!(selection, MaterializeSelection::Directory { .. });
    let size_result = handle
        .spawn_blocking(move || {
            if ensure_directory {
                std::fs::create_dir_all(&size_path).map_err(|error| fs_error(&size_path, error))?;
            }
            compute_total_size(&size_path)
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
                ApplicationError::new(
                    ApplicationErrorKind::Internal,
                    "materialization worker failed",
                )
                .with_diagnostic(join_error.to_string()),
            )
            .await;
            return;
        }
    };

    let lease = match inner
        .materialization()
        .commit(reserved, local_path, size, current_unix_ms())
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
