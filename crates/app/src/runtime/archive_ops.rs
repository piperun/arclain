//! The `start_open_archive` background worker: resolves a backend, lists
//! the archive (trying an auto-detected password first, then raising an
//! interactive [`Challenge::Password`] on failure), builds the
//! [`ArchiveSession`], and dispatches the archive-open plugin event --
//! all the steps `crates/ui`'s `AppState::list_archive`/`list_with_password`
//! used to perform directly against a single shared, mutable `AppState`.
//!
//! Ported to run against this crate's multi-session-aware store instead:
//! every step here is scoped to the one [`OperationId`] and eventual
//! [`ArchiveSessionId`] it produces, so concurrent opens (multiple tabs,
//! multiple archives) never share or clobber each other's in-flight state
//! the way the pre-facade single-`AppState` design's `last_entries`/
//! `current_password` fields could.

use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use arclain_core::archive::{ArchiveKind, MultiPartArchive};
use arclain_core::backends::BackendSelector;
use arclain_core::utilities::{auto_password_for, PassRule};
use arclain_core::{ArchiveBackend, ArchiveInfo};
use arclain_plugins::PluginEvent;

use crate::archive::OpenArchiveRequest;
use crate::challenge::{next_challenge_id, Challenge, ChallengeResponse};
use crate::error::{ApplicationError, ApplicationErrorKind, Recoverability};
use crate::event::{OperationResult, OperationState};
use crate::ids::OperationId;

use super::AppRuntime;

/// Detects whether a backend error message indicates a password failure.
///
/// Ported verbatim from `crates/ui/src/core/operations/archive.rs::
/// is_password_error` (which stays in place there for its own remaining
/// caller, the extraction-progress handler -- extraction is not part of
/// this task's scope). Cannot be shared with `crates/ui` via a
/// dependency in either direction (`arclain_app` must not depend on
/// `arclain_ui`, and the reverse already holds); duplicated from there
/// rather than left unreachable from this crate. A shared
/// `is_password_error`-style classifier in `arclain_core::utilities`
/// would remove that cross-crate duplication, but moving it touches
/// extraction's call site too, which is out of scope here.
///
/// `pub(crate)`, reused by `processing_ops::list_attempt_initial`/
/// `list_attempt_with_password` and by `crate::operations::merge`: unlike
/// the cross-crate case above, all three live in `arclain_app`, so there
/// is no reason to *duplicate* this classifier again within one crate --
/// only to share it. (Merge reads exactly the same text: its extraction
/// step reaches the same `SevenZipCli::run_status` this classifier was
/// written against.)
pub(crate) fn is_password_error(err_msg: &str) -> bool {
    err_msg.contains("Wrong password")
        || err_msg.contains("Incorrect password")
        || err_msg.contains("Password for encrypted archive not specified")
        || err_msg.contains("Cannot open encrypted")
        || err_msg.contains("Can not open encrypted")
        || err_msg.contains("Enter password")
        || err_msg.contains("code Some(2)")
        || err_msg.contains("code Some(11)")
        || err_msg.contains("code Some(255)")
}

fn archive_kind_to_type_string(kind: &ArchiveKind) -> String {
    match kind {
        ArchiveKind::Zip => "zip".to_string(),
        ArchiveKind::SevenZ => "7z".to_string(),
        ArchiveKind::Rar => "rar".to_string(),
        ArchiveKind::Unknown(other) => other.clone(),
    }
}

fn backend_error(source_path: &Path, error: anyhow::Error) -> ApplicationError {
    ApplicationError::new(ApplicationErrorKind::Backend, "failed to open archive")
        .with_diagnostic(format!("{error:#}"))
        .with_recoverability(Recoverability::Retry)
        .with_retryable(true)
        .with_path(source_path.to_path_buf())
}

/// What one blocking attempt (inside `spawn_blocking`) to open the archive
/// produced.
enum AttemptOutcome {
    Success {
        backend: Arc<dyn ArchiveBackend>,
        info: ArchiveInfo,
        password_used: Option<String>,
    },
    PasswordRequired,
    Failed(ApplicationError),
}

/// Resolves the backend for `path` and immediately calls `list()` with no
/// password, then follows the pre-facade UI's exact two-branch auto-
/// password characterization (see `AppState::list_archive`):
///
/// - `list(path, None)` succeeds but reports `headers_encrypted`: try the
///   auto-detected password once; on success use the re-listed info, on
///   failure or no match **keep the original (possibly incomplete)
///   listing** rather than failing -- the original code never raises an
///   error on this path, matching how a header-encrypted 7z archive can
///   still return a placeholder listing.
/// - `list(path, None)` fails outright: try the auto-detected password
///   once; on success use it. On failure, classify: a password-shaped
///   failure raises `PasswordRequired` (whether that's the very first
///   failure, or the auto-guessed password's own retry also failing --
///   the second case is a small, deliberate improvement over the
///   pre-facade code, which propagated that specific failure as a raw
///   error instead of still offering a password prompt); any other
///   failure is `Failed`.
///
/// Deliberately NOT reproduced: the pre-facade code additionally matched
/// auto-password rules against the *previous* archive's entry-path list
/// (`AppState::last_entries`, carried over from whatever archive had been
/// open before this one on the same shared `AppState`). That cross-archive
/// coupling has no equivalent in a multi-session-aware store where two
/// archives can legitimately be open at once with no ordering relationship
/// -- matching only the archive's own filename is the behavior this task
/// keeps.
fn attempt_initial(
    backend: &Arc<dyn ArchiveBackend>,
    path: &Path,
    pass_rules: &[PassRule],
) -> AttemptOutcome {
    let archive_name = path.to_str();
    match backend.list(path, None) {
        Ok(info) => {
            if info.headers_encrypted {
                if let Some(password) = auto_password_for(pass_rules, archive_name, &[]) {
                    match backend.list(path, Some(&password)) {
                        Ok(unlocked) => AttemptOutcome::Success {
                            backend: backend.clone(),
                            info: unlocked,
                            password_used: Some(password),
                        },
                        Err(_) => AttemptOutcome::Success {
                            backend: backend.clone(),
                            info,
                            password_used: None,
                        },
                    }
                } else {
                    AttemptOutcome::Success {
                        backend: backend.clone(),
                        info,
                        password_used: None,
                    }
                }
            } else {
                AttemptOutcome::Success {
                    backend: backend.clone(),
                    info,
                    password_used: None,
                }
            }
        }
        Err(error) => {
            if let Some(password) = auto_password_for(pass_rules, archive_name, &[]) {
                match backend.list(path, Some(&password)) {
                    Ok(info) => AttemptOutcome::Success {
                        backend: backend.clone(),
                        info,
                        password_used: Some(password),
                    },
                    Err(retry_error) => {
                        if is_password_error(&format!("{retry_error:#}")) {
                            AttemptOutcome::PasswordRequired
                        } else {
                            AttemptOutcome::Failed(backend_error(path, retry_error))
                        }
                    }
                }
            } else if is_password_error(&format!("{error:#}")) {
                AttemptOutcome::PasswordRequired
            } else {
                AttemptOutcome::Failed(backend_error(path, error))
            }
        }
    }
}

/// Tries one explicit password directly (no auto-detection, no
/// None-first attempt) -- matches `AppState::list_with_password`'s
/// one-shot semantics. A failure is classified the same way
/// [`attempt_initial`]'s inner retry is: password-shaped failures raise
/// another `PasswordRequired` (so the caller can prompt again with an
/// incremented attempt counter) instead of looping forever on a
/// non-password backend error.
fn attempt_with_password(
    backend: &Arc<dyn ArchiveBackend>,
    path: &Path,
    password: &str,
) -> AttemptOutcome {
    match backend.list(path, Some(password)) {
        Ok(info) => AttemptOutcome::Success {
            backend: backend.clone(),
            info,
            password_used: Some(password.to_string()),
        },
        Err(error) => {
            if is_password_error(&format!("{error:#}")) {
                AttemptOutcome::PasswordRequired
            } else {
                AttemptOutcome::Failed(backend_error(path, error))
            }
        }
    }
}

/// Resolves the backend for one open attempt: an
/// [`AppRuntime::archive_backend_override`] (test seam) wins
/// unconditionally when set; otherwise the real, extension-based
/// `BackendSelector::select`.
fn resolve_backend(
    path: &Path,
    backend_selector: &BackendSelector,
    override_backend: Option<&Arc<dyn ArchiveBackend>>,
) -> Result<Arc<dyn ArchiveBackend>, ApplicationError> {
    if let Some(backend) = override_backend {
        return Ok(backend.clone());
    }
    backend_selector.select(path).map_err(|error| {
        ApplicationError::new(
            ApplicationErrorKind::Backend,
            "failed to select an archive backend",
        )
        .with_diagnostic(format!("{error:#}"))
        .with_recoverability(Recoverability::Fatal)
        .with_path(path.to_path_buf())
    })
}

fn multipart_error(path: &Path, multipart: &MultiPartArchive) -> ApplicationError {
    ApplicationError::new(
        ApplicationErrorKind::Unsupported,
        "multi-part archives must be merged before they can be opened",
    )
    .with_diagnostic(format!(
        "detected {} ({})",
        multipart.format.description(),
        multipart.base_name
    ))
    .with_recoverability(Recoverability::UserAction)
    .with_path(path.to_path_buf())
}

async fn fail(inner: &Arc<AppRuntime>, operation_id: OperationId, error: ApplicationError) {
    let _ = inner
        .operations()
        .transition(operation_id, OperationState::Failed { error })
        .await;
}

/// Builds the archive-open plugin event. Pure and separate from actually
/// scheduling it so this task's requirement -- the event now carries the
/// application-owned `archive_session_id` rather than a UI-signal handle
/// (see `PluginEvent::OnArchiveOpen`'s own doc comment) -- is directly
/// unit-testable without needing a running `PluginEventScheduler`.
fn build_archive_opened_event(
    source_path: &Path,
    kind: ArchiveKind,
    password: Option<String>,
    entries: Arc<Vec<arclain_core::ArchiveEntry>>,
    archive_session_id: u64,
) -> PluginEvent {
    PluginEvent::OnArchiveOpen {
        path: source_path.to_string_lossy().into_owned(),
        kind,
        password,
        entries,
        archive_session_id,
    }
}

/// Dispatches the archive-open plugin event. Best-effort: a full or
/// disconnected scheduler channel is logged and otherwise ignored --
/// matching the bounded, best-effort delivery every other plugin-event
/// call site in this workspace already accepts (see
/// `crates/ui/src/core/state/archive_ops.rs`'s own `try_schedule` use,
/// which this replaces as the one place `OnArchiveOpen` is now fired
/// from).
fn dispatch_archive_opened_event(
    inner: &AppRuntime,
    source_path: &Path,
    kind: ArchiveKind,
    password: Option<String>,
    entries: Arc<Vec<arclain_core::ArchiveEntry>>,
    archive_session_id: u64,
) {
    let Some(scheduler) = inner.plugin_event_scheduler() else {
        return;
    };
    let event =
        build_archive_opened_event(source_path, kind, password, entries, archive_session_id);
    match scheduler.try_schedule(event) {
        Ok(()) => {}
        Err(error) => {
            tracing::warn!(
                "Dropping OnArchiveOpen plugin event for session {archive_session_id}: {error:?}"
            );
        }
    }
}

/// The `start_open_archive` background worker. Spawned via the
/// application's own runtime handle; runs until the operation reaches a
/// terminal state (`Completed`, `Cancelled`, or `Failed`).
pub(super) async fn run_open_archive(
    inner: Arc<AppRuntime>,
    operation_id: OperationId,
    _cancel: Arc<AtomicBool>,
    request: OpenArchiveRequest,
) {
    if inner
        .operations()
        .transition(operation_id, OperationState::Started)
        .await
        .is_err()
    {
        return;
    }

    let source_path = request.source_path.clone();
    let mut current_password = request
        .password
        .as_ref()
        .map(|p| p.expose_secret().to_string());
    let mut attempt: u32 = 1;

    loop {
        if inner.operations().is_cancelled(operation_id).await {
            return;
        }

        let backend_selector = inner.backend_selector();
        let backend_override = inner.archive_backend_override();
        let pass_rules = inner.pass_rules();
        let attempt_path = source_path.clone();
        let attempt_password = current_password.clone();

        // `tokio_handle()` returning `None` here would mean the app's
        // runtime tore down while this very task -- itself running on
        // that runtime -- was executing, which `AppRuntime::tokio_handle`'s
        // doc comment explains is not reachable in practice. Handled
        // defensively anyway: stop rather than panic: there is nothing
        // left to spawn work onto.
        let Some(handle) = inner.tokio_handle() else {
            return;
        };
        let outcome = match handle
            .spawn_blocking(move || {
                if let Some(multipart) = MultiPartArchive::detect(&attempt_path) {
                    return AttemptOutcome::Failed(multipart_error(&attempt_path, &multipart));
                }
                let backend = match resolve_backend(
                    &attempt_path,
                    &backend_selector,
                    backend_override.as_ref(),
                ) {
                    Ok(backend) => backend,
                    Err(error) => return AttemptOutcome::Failed(error),
                };
                match &attempt_password {
                    Some(password) => attempt_with_password(&backend, &attempt_path, password),
                    None => attempt_initial(&backend, &attempt_path, &pass_rules),
                }
            })
            .await
        {
            Ok(outcome) => outcome,
            Err(join_error) => {
                fail(
                    &inner,
                    operation_id,
                    ApplicationError::new(
                        ApplicationErrorKind::Internal,
                        "archive open worker failed",
                    )
                    .with_diagnostic(join_error.to_string()),
                )
                .await;
                return;
            }
        };

        match outcome {
            AttemptOutcome::Success {
                backend,
                info,
                password_used,
            } => {
                // The blocking `list()` call just above can run for a long
                // time on a large archive; cancellation is otherwise
                // checked only at the top of this loop and while parked on
                // a challenge (see `wait_until_cancelled`), so a cancel
                // requested while that call was in flight would otherwise
                // go unnoticed until after a session was inserted below
                // and a plugin told about an open its caller believes was
                // cancelled. There is no session eviction, so a session
                // left behind here would be unreachable forever.
                if inner.operations().is_cancelled(operation_id).await {
                    return;
                }

                let archive = match password_used.clone() {
                    Some(password) => {
                        arclain_core::Archive::with_password(backend, source_path.clone(), password)
                    }
                    None => arclain_core::Archive::new(backend, source_path.clone()),
                };
                let archive_type = archive_kind_to_type_string(&info.archive_kind);
                let encryption = crate::archive::SessionEncryption::from_listing(&info);
                let entries = Arc::new(info.entries);
                let session = match inner
                    .archive_sessions()
                    .open(
                        source_path.clone(),
                        archive_type,
                        archive,
                        entries.clone(),
                        encryption,
                        &handle,
                    )
                    .await
                {
                    Ok(session) => session,
                    Err(error) => {
                        fail(&inner, operation_id, error).await;
                        return;
                    }
                };

                // `open` above is itself async and can yield (indexing runs
                // on a blocking-pool thread it awaits the result of -- see
                // its own doc comment), so a cancellation can still land in
                // the gap between the check above and the session now
                // existing in the store. Re-check before telling any
                // plugin about a session id a cancelled operation's own
                // caller will never learn about.
                if inner.operations().is_cancelled(operation_id).await {
                    let _ = inner.archive_sessions().close(session.id()).await;
                    return;
                }

                let snapshot = session.snapshot();

                let _ = inner
                    .operations()
                    .transition(
                        operation_id,
                        OperationState::Completed {
                            result: OperationResult::ArchiveOpened { snapshot },
                        },
                    )
                    .await;

                // `transition` silently no-ops once a record is already
                // terminal -- including when a concurrent
                // `cancel_operation` call's own transition to `Cancelled`
                // won the race for the registry's write lock in the gap
                // between the check above and this call. Read the record
                // back to find out which transition actually stuck: this
                // operation's `Completed` result can only ever be produced
                // by this worker's own call just above, so seeing anything
                // else here means that call lost the race -- the session
                // just inserted is now unreachable through this
                // operation's result and must be closed rather than left
                // leaked.
                let we_completed_it = matches!(
                    inner.operations().operation(operation_id).await,
                    Some(snapshot) if matches!(snapshot.state, OperationState::Completed { .. })
                );
                if !we_completed_it {
                    let _ = inner.archive_sessions().close(session.id()).await;
                    return;
                }

                // Only now, once the operation is confirmed `Completed`
                // (not lost to a concurrent cancel just above), tell any
                // plugin about the new session. Ordering this after the
                // transition -- and gating it on `we_completed_it` --
                // means a plugin's `OnArchiveOpen` handler can never
                // observe a session for an operation whose caller
                // believes was cancelled: dispatching first (a previous
                // version of this function did) could tell a plugin
                // about a session in the same instant it was being torn
                // down as unreachable.
                dispatch_archive_opened_event(
                    &inner,
                    &source_path,
                    info.archive_kind,
                    password_used,
                    entries,
                    session.id().into_raw(),
                );
                return;
            }
            AttemptOutcome::PasswordRequired => {
                let challenge_id = next_challenge_id();
                let archive_name = source_path
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_else(|| source_path.to_string_lossy().into_owned());
                let receiver = inner.challenges().register(operation_id);
                if inner
                    .operations()
                    .transition(
                        operation_id,
                        OperationState::Challenge {
                            challenge: Challenge::Password {
                                id: challenge_id,
                                archive_name,
                                attempt,
                            },
                        },
                    )
                    .await
                    .is_err()
                {
                    inner.challenges().cancel(operation_id);
                    return;
                }

                let response = tokio::select! {
                    response = receiver => response,
                    () = inner.operations().wait_until_cancelled(operation_id) => {
                        inner.challenges().cancel(operation_id);
                        return;
                    }
                };

                match response {
                    Ok(ChallengeResponse::Password { value, .. }) => {
                        current_password = Some(value.expose_secret().to_string());
                        attempt += 1;
                    }
                    Ok(_) => {
                        fail(
                            &inner,
                            operation_id,
                            ApplicationError::new(
                                ApplicationErrorKind::Internal,
                                "expected a password response to a password challenge",
                            ),
                        )
                        .await;
                        return;
                    }
                    Err(_) => {
                        // The sender was dropped without a response (the
                        // application is shutting down, or an unexpected
                        // internal error tore down the waiter). Nothing
                        // left to do but stop; the operation's own record
                        // (if not already terminal) is left in `Challenge`
                        // rather than papering over the gap with a
                        // fabricated `Failed`.
                        return;
                    }
                }
            }
            AttemptOutcome::Failed(error) => {
                fail(&inner, operation_id, error).await;
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Deterministic fake backend for the password/encryption
    /// characterization tests below: `list`'s behavior for both the
    /// no-password and explicit-password cases is fully configurable, so
    /// these tests exercise `attempt_initial`/`attempt_with_password`'s
    /// branching without depending on a real encrypted archive fixture --
    /// real header-encryption behavior varies by archive format and by the
    /// installed 7-Zip/UnRAR version, which would make tests built on a
    /// real encrypted fixture flaky across machines.
    #[derive(Clone, Copy)]
    enum ListBehavior {
        /// `list(path, None)` succeeds, reporting `headers_encrypted`.
        HeadersEncryptedPlaceholder,
        /// `list(path, None)` fails with a password-shaped error.
        FailsOutright,
        /// `list` fails with a non-password-shaped error regardless of
        /// whether a (correct or incorrect) password is supplied --
        /// models a genuinely broken/corrupt archive, distinct from a
        /// merely-encrypted one.
        AlwaysFailsWithNonPasswordError,
    }

    struct FakeBackend {
        correct_password: String,
        behavior: ListBehavior,
    }

    impl FakeBackend {
        fn headers_encrypted_placeholder(correct_password: &str) -> Self {
            Self {
                correct_password: correct_password.to_string(),
                behavior: ListBehavior::HeadersEncryptedPlaceholder,
            }
        }

        fn fails_outright_without_password(correct_password: &str) -> Self {
            Self {
                correct_password: correct_password.to_string(),
                behavior: ListBehavior::FailsOutright,
            }
        }

        fn fails_with_a_non_password_error(correct_password: &str) -> Self {
            Self {
                correct_password: correct_password.to_string(),
                behavior: ListBehavior::AlwaysFailsWithNonPasswordError,
            }
        }
    }

    fn fake_info(headers_encrypted: bool) -> ArchiveInfo {
        ArchiveInfo {
            archive_path: PathBuf::new(),
            archive_kind: ArchiveKind::Zip,
            entries: Vec::new(),
            encrypted: true,
            headers_encrypted,
            encryption_method: Some("fake".to_string()),
        }
    }

    impl ArchiveBackend for FakeBackend {
        fn name(&self) -> &str {
            "fake"
        }
        fn capabilities(&self) -> arclain_core::archive::BackendCapabilities {
            arclain_core::archive::BackendCapabilities::read_only()
        }
        fn identify(&self, _path: &Path) -> anyhow::Result<ArchiveKind> {
            Ok(ArchiveKind::Zip)
        }
        fn list(&self, _path: &Path, password: Option<&str>) -> anyhow::Result<ArchiveInfo> {
            if matches!(self.behavior, ListBehavior::AlwaysFailsWithNonPasswordError) {
                return Err(anyhow::anyhow!("disk read error: I/O failure"));
            }
            match password {
                Some(candidate) if candidate == self.correct_password => Ok(fake_info(false)),
                Some(_) => Err(anyhow::anyhow!("Wrong password for archive")),
                None => match self.behavior {
                    ListBehavior::HeadersEncryptedPlaceholder => Ok(fake_info(true)),
                    ListBehavior::FailsOutright => {
                        Err(anyhow::anyhow!("Wrong password for archive"))
                    }
                    ListBehavior::AlwaysFailsWithNonPasswordError => unreachable!("handled above"),
                },
            }
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
        fn add_files(&self, _archive: &Path, _files: &[PathBuf]) -> anyhow::Result<()> {
            unimplemented!()
        }
        fn create_archive(
            &self,
            _dest: &Path,
            _files: &[PathBuf],
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

    fn matching_rule(archive_name: &str, password: &str) -> PassRule {
        // Not `regex::escape`d: every character in the test fixture names
        // used below is regex-literal-safe as-is (no `regex` dependency
        // needed just for this test helper).
        PassRule {
            name: "test".to_string(),
            pattern: archive_name.to_string(),
            password: password.to_string(),
            priority: 10,
            enabled: true,
        }
    }

    fn as_backend(fake: FakeBackend) -> Arc<dyn ArchiveBackend> {
        Arc::new(fake)
    }

    #[test]
    fn is_password_error_recognizes_known_backend_error_shapes() {
        for message in [
            "Wrong password for this archive",
            "Incorrect password supplied",
            "Password for encrypted archive not specified",
            "Cannot open encrypted archive",
            "Can not open encrypted archive",
            "Enter password to continue",
            "process exited with code Some(2)",
            "process exited with code Some(11)",
            "process exited with code Some(255)",
        ] {
            assert!(
                is_password_error(message),
                "expected {message:?} to be recognized"
            );
        }
    }

    #[test]
    fn is_password_error_rejects_unrelated_backend_errors() {
        for message in [
            "disk read error: I/O failure",
            "archive is corrupt",
            "file not found",
        ] {
            assert!(
                !is_password_error(message),
                "did not expect {message:?} to be recognized"
            );
        }
    }

    #[test]
    fn archive_kind_to_type_string_maps_every_known_kind() {
        assert_eq!(archive_kind_to_type_string(&ArchiveKind::Zip), "zip");
        assert_eq!(archive_kind_to_type_string(&ArchiveKind::SevenZ), "7z");
        assert_eq!(archive_kind_to_type_string(&ArchiveKind::Rar), "rar");
        assert_eq!(
            archive_kind_to_type_string(&ArchiveKind::Unknown("tar".to_string())),
            "tar"
        );
    }

    #[test]
    fn headers_encrypted_with_matching_auto_password_uses_the_unlocked_listing() {
        let backend = as_backend(FakeBackend::headers_encrypted_placeholder("secret"));
        let rules = vec![matching_rule("archive.zip", "secret")];

        let outcome = attempt_initial(&backend, Path::new("archive.zip"), &rules);

        match outcome {
            AttemptOutcome::Success {
                info,
                password_used,
                ..
            } => {
                assert_eq!(password_used.as_deref(), Some("secret"));
                assert!(
                    !info.headers_encrypted,
                    "unlocked listing must replace the placeholder"
                );
            }
            _ => panic!("expected Success"),
        }
    }

    #[test]
    fn headers_encrypted_with_no_matching_auto_password_keeps_the_placeholder_listing_without_error(
    ) {
        let backend = as_backend(FakeBackend::headers_encrypted_placeholder("secret"));

        let outcome = attempt_initial(&backend, Path::new("archive.zip"), &[]);

        match outcome {
            AttemptOutcome::Success {
                info,
                password_used,
                ..
            } => {
                assert_eq!(password_used, None);
                assert!(
                    info.headers_encrypted,
                    "original placeholder listing is kept, not an error"
                );
            }
            _ => panic!("expected Success, matching the pre-facade UI's tolerant behavior"),
        }
    }

    #[test]
    fn outright_failure_with_matching_auto_password_succeeds() {
        let backend = as_backend(FakeBackend::fails_outright_without_password("secret"));
        let rules = vec![matching_rule("archive.zip", "secret")];

        let outcome = attempt_initial(&backend, Path::new("archive.zip"), &rules);

        match outcome {
            AttemptOutcome::Success { password_used, .. } => {
                assert_eq!(password_used.as_deref(), Some("secret"));
            }
            _ => panic!("expected Success"),
        }
    }

    #[test]
    fn outright_failure_with_no_auto_password_raises_password_required() {
        let backend = as_backend(FakeBackend::fails_outright_without_password("secret"));

        let outcome = attempt_initial(&backend, Path::new("archive.zip"), &[]);

        assert!(matches!(outcome, AttemptOutcome::PasswordRequired));
    }

    #[test]
    fn outright_failure_with_wrong_auto_password_still_raises_password_required() {
        // Deliberate improvement over the pre-facade code (see
        // `attempt_initial`'s doc comment): a wrong auto-guessed password
        // still offers an interactive prompt instead of hard-failing.
        let backend = as_backend(FakeBackend::fails_outright_without_password("secret"));
        let rules = vec![matching_rule("archive.zip", "wrong-guess")];

        let outcome = attempt_initial(&backend, Path::new("archive.zip"), &rules);

        assert!(matches!(outcome, AttemptOutcome::PasswordRequired));
    }

    #[test]
    fn non_password_failure_with_no_auto_password_fails_outright() {
        let backend = as_backend(FakeBackend::fails_with_a_non_password_error("secret"));

        let outcome = attempt_initial(&backend, Path::new("archive.zip"), &[]);

        match outcome {
            AttemptOutcome::Failed(error) => assert_eq!(error.kind, ApplicationErrorKind::Backend),
            _ => panic!("expected Failed for a non-password backend error"),
        }
    }

    #[test]
    fn attempt_with_password_succeeds_on_the_correct_password() {
        let backend = as_backend(FakeBackend::fails_outright_without_password("secret"));

        let outcome = attempt_with_password(&backend, Path::new("archive.zip"), "secret");

        assert!(matches!(outcome, AttemptOutcome::Success { .. }));
    }

    #[test]
    fn attempt_with_password_raises_another_challenge_on_a_wrong_password() {
        let backend = as_backend(FakeBackend::fails_outright_without_password("secret"));

        let outcome = attempt_with_password(&backend, Path::new("archive.zip"), "not-it");

        assert!(matches!(outcome, AttemptOutcome::PasswordRequired));
    }

    #[test]
    fn attempt_with_password_fails_outright_on_a_non_password_error() {
        let backend = as_backend(FakeBackend::fails_with_a_non_password_error("secret"));

        // Any password is wrong here because the backend fails with a
        // non-password error regardless of what's supplied.
        let outcome = attempt_with_password(&backend, Path::new("archive.zip"), "secret");

        match outcome {
            AttemptOutcome::Failed(error) => assert_eq!(error.kind, ApplicationErrorKind::Backend),
            _ => panic!("expected Failed, not an endless reprompt loop"),
        }
    }

    #[test]
    fn build_archive_opened_event_carries_the_given_session_id() {
        let event = build_archive_opened_event(
            Path::new("archive.zip"),
            ArchiveKind::Zip,
            Some("secret".to_string()),
            Arc::new(Vec::new()),
            42,
        );

        match event {
            PluginEvent::OnArchiveOpen {
                archive_session_id,
                path,
                ..
            } => {
                assert_eq!(archive_session_id, 42);
                assert_eq!(path, "archive.zip");
            }
        }
    }

    #[test]
    fn detected_multipart_archive_is_unsupported_not_a_backend_failure() {
        let path = PathBuf::from("game.part1.rar");
        let multipart = MultiPartArchive::detect(&path).expect("part1.rar must be detected");

        let error = multipart_error(&path, &multipart);

        assert_eq!(error.kind, ApplicationErrorKind::Unsupported);
    }

    #[test]
    fn list_call_count_is_bounded_to_at_most_two_attempts_per_call() {
        // Regression guard for the retry logic: `attempt_initial` must
        // call `list` at most twice (an initial no-password attempt, then
        // at most one auto-password retry) -- never loop internally.
        let calls = Arc::new(AtomicUsize::new(0));
        struct CountingBackend {
            calls: Arc<AtomicUsize>,
        }
        impl ArchiveBackend for CountingBackend {
            fn name(&self) -> &str {
                "counting"
            }
            fn capabilities(&self) -> arclain_core::archive::BackendCapabilities {
                arclain_core::archive::BackendCapabilities::read_only()
            }
            fn identify(&self, _path: &Path) -> anyhow::Result<ArchiveKind> {
                Ok(ArchiveKind::Zip)
            }
            fn list(&self, _path: &Path, _password: Option<&str>) -> anyhow::Result<ArchiveInfo> {
                self.calls.fetch_add(1, Ordering::SeqCst);
                Err(anyhow::anyhow!("Wrong password for archive"))
            }
            fn extract_all(&self, _p: &Path, _d: &Path, _pw: Option<&str>) -> anyhow::Result<()> {
                unimplemented!()
            }
            fn extract_files(
                &self,
                _p: &Path,
                _d: &Path,
                _f: &[String],
                _pw: Option<&str>,
            ) -> anyhow::Result<()> {
                unimplemented!()
            }
            fn extract_directory(
                &self,
                _p: &Path,
                _d: &Path,
                _dp: &str,
                _pw: Option<&str>,
            ) -> anyhow::Result<()> {
                unimplemented!()
            }
            fn recompress_7z(&self, _s: &Path, _d: &Path) -> anyhow::Result<()> {
                unimplemented!()
            }
            fn add_files(&self, _a: &Path, _f: &[PathBuf]) -> anyhow::Result<()> {
                unimplemented!()
            }
            fn create_archive(&self, _d: &Path, _f: &[PathBuf], _fmt: &str) -> anyhow::Result<()> {
                unimplemented!()
            }
            fn read_text_file(
                &self,
                _a: &Path,
                _p: &str,
                _pw: Option<&str>,
            ) -> anyhow::Result<String> {
                unimplemented!()
            }
            fn delete_files(&self, _a: &Path, _f: &[String]) -> anyhow::Result<()> {
                unimplemented!()
            }
            fn add_or_update_file_from_str(
                &self,
                _a: &Path,
                _p: &str,
                _c: &str,
            ) -> anyhow::Result<()> {
                unimplemented!()
            }
            fn convert_to_7z(
                &self,
                _s: &arclain_core::Archive,
                _d: &Path,
                _t: &Path,
            ) -> anyhow::Result<()> {
                unimplemented!()
            }
            fn crc32_of_entry(
                &self,
                _a: &Path,
                _p: &str,
                _pw: Option<&str>,
            ) -> anyhow::Result<String> {
                unimplemented!()
            }
        }
        let backend: Arc<dyn ArchiveBackend> = Arc::new(CountingBackend {
            calls: calls.clone(),
        });
        let rules = vec![matching_rule("archive.zip", "guess")];

        let outcome = attempt_initial(&backend, Path::new("archive.zip"), &rules);

        assert!(matches!(outcome, AttemptOutcome::PasswordRequired));
        assert_eq!(
            calls.load(Ordering::SeqCst),
            2,
            "no-password attempt + one auto-password retry"
        );
    }
}
