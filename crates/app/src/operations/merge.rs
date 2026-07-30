//! Merging a split multi-part archive set into one archive, as a
//! cancellable, event-broadcasting application operation.
//!
//! # Characterization: what this replaces
//!
//! Pre-facade, `crates/ui/src/core/arclain_app/dialog_handler.rs`'s
//! merge-dialog handler did all of this inline on a bare
//! `runtime.spawn`:
//!
//! - Built an `arclain_core::services::MergeOptions` from the dialog's
//!   own state and called `MergeService::merge(&mut multipart, options,
//!   None, None)` -- **both** the progress callback and the cancellation
//!   token passed as `None`.
//! - Because the progress callback was `None`, nothing ever reported
//!   progress: the handler opened the per-tab extraction progress dialog
//!   at `percent: 0` with `can_cancel: false` and left it there until the
//!   whole merge returned, then hid it and wrote one status-bar line.
//! - Because the cancellation token was `None`, the merge could not be
//!   cancelled at all; the dialog's cancel button was disabled to match.
//! - The password came from `MergeDialogState::password`, a field no
//!   widget in `merge_dialog.rs` ever wrote to. It was therefore always
//!   empty, i.e. always `None`: a passworded set failed with 7-Zip's own
//!   "wrong password" text in the status bar and no way to supply one.
//! - Nothing tracked the merge as an operation, so two merges (or a merge
//!   and an extraction) could drive the same per-tab progress dialog at
//!   once, each overwriting the other's fields.
//!
//! # What this operation changes
//!
//! - **Progress is real, on the standard stream.** The merge runs with a
//!   live `MergeProgressCallback` whose updates are forwarded as
//!   `OperationState::Progress` (percent out of 100, plus the phase
//!   message verbatim), so a frontend renders merge progress through
//!   exactly the machinery every other operation already uses.
//! - **Cancellation works.** The operation's own cancel flag *is* an
//!   `arclain_core::archive::CancellationToken` (both are
//!   `Arc<AtomicBool>`), so it is handed straight to `MergeService::
//!   merge`. See [`run_merge`]'s doc comment for precisely which
//!   checkpoints observe it and what is left on disk at each.
//! - **Passworded sets are answerable.** A password-shaped failure raises
//!   `Challenge::Password` and retries the whole merge with the supplied
//!   secret, the same ladder `crate::operations::extract` and
//!   `crate::runtime::processing_ops`'s organize use. The secret travels
//!   as [`crate::challenge::SecretInput`] and never reaches an
//!   `ApplicationError`.
//! - **The set is re-confirmed on disk before anything runs.** A
//!   [`MergeRequest`] carries a [`MultiPartArchiveDto`] the caller
//!   obtained from [`crate::archive::detect_multipart`], possibly frames
//!   or minutes earlier. Detection re-runs against
//!   `archive.first_part` and must agree on the set's identity, so a set
//!   that changed underneath the dialog is a structured error rather than
//!   a merge of something else.
//! - **The caller's part list cannot direct file deletion.** Neither the
//!   core `MultiPartArchive` handed to `MergeService` (always built with
//!   an *empty* `all_parts`, forcing `validate()` to re-enumerate from
//!   disk) nor this operation's own cleanup step ever reads
//!   `MergeRequest::archive::parts`. Both work from a disk-derived
//!   enumeration, because honoring a caller-supplied list would turn a
//!   request field into an arbitrary-file-deletion primitive.
//! - **`delete_originals` is sequenced against the terminal state**, not
//!   against core's own last cancellation check -- so a cancelled merge
//!   provably keeps its parts. See [`run_merge`]'s doc comment.
//!
//! # Preserved: merging an encrypted set produces an *unencrypted* archive
//!
//! `arclain_core::services::MergeService` uses the request's password to
//! *extract* the parts and never passes one to the compression step that
//! writes the result (`create_archive_with_options` builds its `7z a`
//! command with no `-p` at all). Merging an encrypted split set therefore
//! writes a plaintext archive beside it, and with `delete_originals` set,
//! deletes the encrypted parts afterwards.
//!
//! This operation preserves that unchanged and pins it with a test
//! (`crates/app/tests/merge_operation.rs`), rather than quietly starting
//! to encrypt outputs: which password to use, and whether a merged
//! archive should be encrypted at all, are product decisions this
//! extraction has no mandate to make. It is called out here because it is
//! a confidentiality downgrade a caller cannot see from the request
//! shape.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use arclain_core::services::{MergeOptions, MergeProgress, MergeService};

use crate::archive::multipart::{detect_multipart, MultiPartArchiveDto};
use crate::challenge::{next_challenge_id, Challenge, ChallengeResponse, SecretInput};
use crate::error::{ApplicationError, ApplicationErrorKind, Recoverability, SuggestedAction};
use crate::event::{OperationResult, OperationState};
use crate::ids::OperationId;
use crate::operations::OperationRegistry;
use crate::runtime::AppRuntime;

/// Which container a merge writes its single output archive as. Mirrors
/// `arclain_core::services::OutputFormat` variant-for-variant.
#[derive(
    Clone, Copy, Debug, Default, Eq, Hash, PartialEq, serde::Deserialize, serde::Serialize,
)]
#[serde(rename_all = "snake_case")]
pub enum MergeOutputFormat {
    #[default]
    SevenZip,
    Zip,
}

impl MergeOutputFormat {
    /// The output file's extension, without a leading dot. Kept identical
    /// to core's own so a merge writes the same file name it did
    /// pre-facade.
    pub fn extension(self) -> &'static str {
        self.to_core().extension()
    }

    /// The label a format picker shows, identical to core's.
    pub fn display_name(self) -> &'static str {
        self.to_core().display_name()
    }

    /// Every format a merge can write, in the order a picker lists them.
    pub fn all() -> &'static [MergeOutputFormat] {
        &[MergeOutputFormat::SevenZip, MergeOutputFormat::Zip]
    }

    fn to_core(self) -> arclain_core::services::OutputFormat {
        match self {
            Self::SevenZip => arclain_core::services::OutputFormat::SevenZip,
            Self::Zip => arclain_core::services::OutputFormat::Zip,
        }
    }
}

/// How hard a merge compresses its output. Mirrors
/// `arclain_core::services::CompressionLevel` variant-for-variant.
#[derive(
    Clone, Copy, Debug, Default, Eq, Hash, PartialEq, serde::Deserialize, serde::Serialize,
)]
#[serde(rename_all = "snake_case")]
pub enum MergeCompressionLevel {
    Store,
    Fastest,
    Fast,
    #[default]
    Normal,
    Maximum,
    Ultra,
}

impl MergeCompressionLevel {
    /// The label a compression picker shows, identical to core's.
    pub fn display_name(self) -> &'static str {
        self.to_core().display_name()
    }

    /// Every level, weakest first -- the order a picker lists them.
    pub fn all() -> &'static [MergeCompressionLevel] {
        &[
            MergeCompressionLevel::Store,
            MergeCompressionLevel::Fastest,
            MergeCompressionLevel::Fast,
            MergeCompressionLevel::Normal,
            MergeCompressionLevel::Maximum,
            MergeCompressionLevel::Ultra,
        ]
    }

    fn to_core(self) -> arclain_core::services::CompressionLevel {
        match self {
            Self::Store => arclain_core::services::CompressionLevel::Store,
            Self::Fastest => arclain_core::services::CompressionLevel::Fastest,
            Self::Fast => arclain_core::services::CompressionLevel::Fast,
            Self::Normal => arclain_core::services::CompressionLevel::Normal,
            Self::Maximum => arclain_core::services::CompressionLevel::Maximum,
            Self::Ultra => arclain_core::services::CompressionLevel::Ultra,
        }
    }
}

/// A request to merge one detected multi-part set into a single archive,
/// the argument to [`crate::ArclainApp::start_merge`].
///
/// Not `Clone`/`Serialize`/`Deserialize`: `password`, when present,
/// carries a live [`SecretInput`], and those restrictions are contagious
/// by design (see `SecretInput`'s own doc comment) -- the same reason
/// `crate::archive::OpenArchiveRequest` carries none of them either.
#[derive(Debug)]
pub struct MergeRequest {
    /// The set to merge, as [`crate::archive::detect_multipart`]
    /// described it. Only its *identity* (`first_part`, `base_name`,
    /// `format`) is authoritative, and even that is re-confirmed against
    /// the filesystem before the merge runs; `parts` is informational
    /// (the merge always re-enumerates -- see this module's doc comment
    /// for why that is load-bearing rather than merely tidy).
    pub archive: MultiPartArchiveDto,
    pub output_format: MergeOutputFormat,
    pub compression_level: MergeCompressionLevel,
    /// Where to write the merged archive. `None` writes
    /// `<base_name>.<extension>` beside the set's first part -- byte for
    /// byte the path the pre-facade merge dialog computed for itself, and
    /// the same default `arclain_core::services::MergeService` applies
    /// when given none.
    pub output_path: Option<PathBuf>,
    /// Deletes every part the merge enumerated, after the output archive
    /// has been written *and* this operation's terminal state has been
    /// observed to be `Completed { Merged }` -- so a cancelled or failed
    /// merge never deletes anything (see [`run_merge`]'s doc comment for
    /// why that sequencing is enforced here rather than delegated).
    ///
    /// Cleanup is best-effort throughout, and always trails the terminal
    /// event rather than preceding it. A part that cannot be removed is
    /// logged and skipped (core's own behaviour), not treated as a merge
    /// failure; and a process death between the `Completed { Merged }` event
    /// and the end of the delete loop leaves the remaining parts on disk
    /// beside the finished archive. Every way this can go wrong therefore
    /// leaks parts rather than losing data -- the content is already in the
    /// output archive by the time any of it runs.
    pub delete_originals: bool,
    /// A password to try first, for a set whose contents are encrypted.
    /// `None` starts with no password and lets the operation raise
    /// `Challenge::Password` if one turns out to be needed.
    pub password: Option<SecretInput>,
}

impl MergeRequest {
    /// Validates the parts of this request that are purely structural --
    /// discoverable with no filesystem access at all -- so
    /// [`crate::ArclainApp::start_merge`] can reject a malformed request
    /// before registering an operation, leaving no phantom
    /// `OperationId` behind. Mirrors `ConvertRequest::validate`/
    /// `OrganizeRequest::validate`'s role.
    ///
    /// Whether the set still exists, still matches the named convention,
    /// and has a complete run of parts are all filesystem questions;
    /// [`run_merge`] answers those after the operation is registered, so
    /// they surface as `OperationState::Failed` with the operation's own
    /// id attached rather than as a bare rejection.
    pub(crate) fn validate(&self) -> Result<(), ApplicationError> {
        if self.archive.first_part.as_os_str().is_empty() {
            return Err(invalid_request_error(
                "the multi-part archive's first part has no path",
                "archive.first_part",
            ));
        }
        if self.archive.first_part.file_name().is_none() {
            return Err(invalid_request_error(
                "the multi-part archive's first part is not a file path",
                "archive.first_part",
            ));
        }
        if self.archive.base_name.trim().is_empty() {
            return Err(invalid_request_error(
                "the multi-part archive has no base name",
                "archive.base_name",
            ));
        }
        // An empty `output_path` is not "the current directory": it would
        // be handed to the packer as a bare argument and resolved against
        // whatever the process working directory happens to be. Same
        // boundary `OrganizeRequest::validate` draws for its own
        // destination, for the same reason.
        if let Some(output_path) = &self.output_path {
            if output_path.as_os_str().is_empty() || output_path.file_name().is_none() {
                return Err(invalid_request_error(
                    "the merge output path is not a file path",
                    "output_path",
                ));
            }
        }
        Ok(())
    }

    /// Where this request's merged archive will be written: its explicit
    /// [`Self::output_path`], or `<base_name>.<extension>` beside the
    /// set's first part.
    ///
    /// Resolved here, once, and always passed to
    /// `arclain_core::services::MergeOptions` explicitly, so exactly one
    /// copy of this rule decides the output name -- core's identical
    /// default branch is then never taken. The rule itself is unchanged
    /// from the pre-facade dialog handler's own computation (`first_part.
    /// parent().join(format!("{base_name}.{ext}"))`); a `first_part` with
    /// no parent falls back to `.`, matching core.
    fn resolved_output_path(&self) -> PathBuf {
        if let Some(output_path) = &self.output_path {
            return output_path.clone();
        }
        self.archive
            .first_part
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(format!(
                "{}.{}",
                self.archive.base_name,
                self.output_format.extension()
            ))
    }
}

fn invalid_request_error(summary: &str, field: &'static str) -> ApplicationError {
    ApplicationError::new(ApplicationErrorKind::InvalidInput, summary)
        .with_recoverability(Recoverability::UserAction)
        .with_field(field)
}

/// The set named by the request is no longer recognizable on disk -- its
/// first part is gone, or the sibling that made a bare `.rar`/`.zip`
/// count as a set member is. Retrying will not help without the files
/// coming back, so this is `Fatal` rather than `Retry`.
fn set_no_longer_detected_error(first_part: &Path) -> ApplicationError {
    ApplicationError::new(
        ApplicationErrorKind::NotFound,
        "this split archive is no longer recognizable on disk -- its parts may have been moved, \
         renamed, or deleted",
    )
    .with_recoverability(Recoverability::Fatal)
    .with_path(first_part.to_path_buf())
}

/// The set is still a set, but not the same one the request described:
/// re-detection settled on a different convention or base name. Treated
/// as a `Conflict` (the request was prepared against a state that has
/// since changed) rather than `InvalidInput`, because a caller that
/// re-detects and rebuilds its request can succeed.
fn set_changed_error(first_part: &Path) -> ApplicationError {
    ApplicationError::new(
        ApplicationErrorKind::Conflict,
        "this split archive changed since the merge was prepared -- re-detect it and try again",
    )
    .with_recoverability(Recoverability::Retry)
    .with_retryable(true)
    .with_path(first_part.to_path_buf())
}

/// Enumerating the set from its start found no members at all. Distinct
/// from [`set_no_longer_detected_error`]: the naming convention still
/// matches (so this *is* a member of some set), but the run of parts a
/// merge must read does not begin where the convention requires.
///
/// Worded without naming *which* member is missing on purpose: which one
/// that is depends on the convention (a `.partN.rar` set starts at
/// `part1`, a split ZIP's enumeration starts at `.z01` even though its
/// entry point is the `.zip`), and guessing wrong in a user-facing string
/// is worse than staying general.
fn incomplete_set_error(first_part: &Path) -> ApplicationError {
    ApplicationError::new(
        ApplicationErrorKind::NotFound,
        "this split archive is incomplete -- none of its parts could be found, so there is \
         nothing to merge from",
    )
    .with_recoverability(Recoverability::Fatal)
    .with_path(first_part.to_path_buf())
}

/// The output archive already exists. Checked before any extraction so a
/// long merge never runs only to be refused at the end; core performs the
/// identical check itself, but only as prose inside an `anyhow` chain,
/// which a frontend cannot act on.
fn output_exists_error(output_path: &Path) -> ApplicationError {
    ApplicationError::new(
        ApplicationErrorKind::Conflict,
        "a file already exists where the merged archive would be written",
    )
    .with_recoverability(Recoverability::UserAction)
    .with_suggested_action(SuggestedAction::ChooseDestination)
    .with_path(output_path.to_path_buf())
    .with_field("output_path")
}

fn merge_backend_error(error: anyhow::Error) -> ApplicationError {
    ApplicationError::new(
        ApplicationErrorKind::Backend,
        "failed to merge the split archive",
    )
    .with_diagnostic(format!("{error:#}"))
    .with_recoverability(Recoverability::Retry)
    .with_retryable(true)
}

fn internal_join_error(join_error: tokio::task::JoinError) -> ApplicationError {
    ApplicationError::new(ApplicationErrorKind::Internal, "merge worker failed")
        .with_diagnostic(join_error.to_string())
}

async fn fail(inner: &Arc<AppRuntime>, operation_id: OperationId, error: ApplicationError) {
    let _ = inner
        .operations()
        .transition(operation_id, OperationState::Failed { error })
        .await;
}

/// Whether two detections describe the same set. Compared field by field
/// rather than with a derived `PartialEq` on the whole DTO on purpose:
/// `parts` is expected to differ (the caller's list is a snapshot; the
/// re-detected one is current), and only the identity fields a merge
/// actually acts on must agree.
fn describes_the_same_set(request: &MultiPartArchiveDto, current: &MultiPartArchiveDto) -> bool {
    request.first_part == current.first_part
        && request.base_name == current.base_name
        && request.format == current.format
}

/// One attempt's outcome, classified before the caller decides whether to
/// prompt for a password and retry.
enum MergeAttempt {
    Merged(PathBuf),
    /// The failure looked password-shaped (see
    /// `crate::runtime::archive_ops::is_password_error`), so a fresh
    /// password may make the same attempt succeed.
    PasswordRequired,
    Failed(anyhow::Error),
}

/// Runs one whole merge attempt on a blocking thread.
///
/// `multipart` is rebuilt from `archive`'s identity on every attempt with
/// an empty `all_parts`, so `MergeService`'s own `validate()` always
/// re-enumerates the set from disk -- see this module's doc comment for
/// why a caller-supplied part list must never reach the code that honors
/// `delete_originals`.
///
/// Retrying after a password failure is safe with respect to the output
/// file: extraction happens before compression, so a password failure
/// leaves nothing written at `output_path` for the next attempt to
/// collide with.
fn merge_attempt(
    backend_selector: arclain_core::backends::BackendSelector,
    archive: &MultiPartArchiveDto,
    options: MergeOptions,
    progress: Option<tokio::sync::mpsc::UnboundedSender<MergeProgress>>,
    cancel: Arc<AtomicBool>,
) -> MergeAttempt {
    let mut multipart = arclain_core::archive::MultiPartArchive {
        first_part: archive.first_part.clone(),
        all_parts: Vec::new(),
        format: archive.format.to_core(),
        base_name: archive.base_name.clone(),
    };
    let progress_callback: Option<arclain_core::services::MergeProgressCallback> =
        progress.map(|sender| {
            Box::new(move |update: MergeProgress| {
                // A closed receiver means the forwarding task is already
                // gone (the operation reached a terminal state); dropping
                // the update is correct, not an error worth surfacing.
                let _ = sender.send(update);
            }) as arclain_core::services::MergeProgressCallback
        });
    match MergeService::new(backend_selector).merge(
        &mut multipart,
        options,
        progress_callback,
        Some(cancel),
    ) {
        Ok(output_path) => MergeAttempt::Merged(output_path),
        Err(error) => {
            if crate::runtime::archive_ops::is_password_error(&format!("{error:#}")) {
                MergeAttempt::PasswordRequired
            } else {
                MergeAttempt::Failed(error)
            }
        }
    }
}

/// What [`run_merge`] does once one attempt's outcome is known.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PostAttempt {
    /// Publish `Completed { Merged }`, then honour `delete_originals`.
    Complete,
    /// Stop without publishing anything: cancellation was requested and
    /// this attempt did not succeed, so the registry's own `Cancelled`
    /// (published by `cancel_operation` itself) is the whole story.
    Stop,
    /// Raise another password challenge and retry the attempt.
    Prompt,
    /// Publish `Failed`.
    Fail,
}

/// Decides what to do with one attempt's outcome.
///
/// Split out of [`run_merge`] because the one rule here that is easy to
/// get wrong is impossible to test by racing a real merge: **a merge that
/// actually succeeded must be reported as a success even when
/// cancellation was requested.** By the time core returns `Ok`, the
/// output archive is written; suppressing `Completed { Merged }` at that
/// point would throw away the only value that tells a frontend where the
/// result is, in exchange for nothing. Cancellation only decides the fate
/// of an attempt that did *not* finish.
fn decide_post_attempt(outcome: &MergeAttempt, cancellation_requested: bool) -> PostAttempt {
    match outcome {
        MergeAttempt::Merged(_) => PostAttempt::Complete,
        MergeAttempt::PasswordRequired if cancellation_requested => PostAttempt::Stop,
        MergeAttempt::PasswordRequired => PostAttempt::Prompt,
        MergeAttempt::Failed(_) if cancellation_requested => PostAttempt::Stop,
        MergeAttempt::Failed(_) => PostAttempt::Fail,
    }
}

/// Publishes a successful merge's terminal state and then, only if that
/// state is what actually landed, deletes the original parts.
///
/// The read-back is the load-bearing part. `OperationRegistry::cancel`
/// sets the cancel flag *and* transitions to `Cancelled` in one call, so
/// a cancellation racing this function can win the registry's write lock
/// and make `Cancelled` terminal — at which point `transition` no-ops and
/// this operation must not delete anything. Reading the state back after
/// the transition settles that unambiguously: `Completed` is terminal, so
/// once observed it can never be superseded, and any other observed state
/// means the merge is not the operation's outcome and the parts stay.
///
/// `parts` is `None` when the request did not ask for deletion. When set,
/// it is always the disk-derived enumeration taken before the attempt ran
/// — the same source `MergeService`'s own cleanup step used. For a
/// single-attempt merge (the common case) that is also the same *moment*
/// core would have enumerated at; across a password-parked retry whose
/// directory changed underneath, a part that appeared mid-parking is
/// merged by core's own re-enumeration but not deleted here. Leak-only,
/// never a deletion of something outside the set, and never anything the
/// caller supplied.
///
/// Deletion is best-effort and deliberately *after* the terminal event:
/// a process death in that gap leaves the parts on disk with the merged
/// archive already written — the content is never lost, only the cleanup
/// is skipped. The `"Deleting original parts..."` progress message can
/// likewise precede a veto (it has to be emitted before the terminal
/// transition to be seen at all), in which case it announced an intent
/// the terminal `Cancelled` immediately corrects.
///
/// Takes the registry rather than the whole [`AppRuntime`] because that is
/// all it needs — and because the read-back gate is the single line
/// standing between a cancelled merge and an irreversible deletion, so it
/// must be directly constructible in a test (see this module's
/// `the_read_back_gate_*` tests) rather than only reachable through a
/// bootstrapped application and a real 7-Zip.
async fn finish_successful_merge(
    operations: &OperationRegistry,
    operation_id: OperationId,
    output_path: PathBuf,
    parts: Option<&[PathBuf]>,
) {
    // Emitted before the terminal transition, since a `Progress` event
    // published after one is silently dropped. Reported at 100 rather
    // than core's own 90: core has already emitted its own 100
    // ("Merge complete") by now, and a progress bar must not run
    // backwards.
    if parts.is_some() {
        let _ = operations
            .transition(
                operation_id,
                OperationState::Progress {
                    completed_units: 100,
                    total_units: Some(100),
                    message: Some("Deleting original parts...".to_string()),
                },
            )
            .await;
    }

    let result = OperationResult::Merged {
        output_path: output_path.clone(),
    };
    if operations
        .transition(operation_id, OperationState::Completed { result })
        .await
        .is_err()
    {
        return;
    }

    let Some(parts) = parts else {
        return;
    };
    match operations.operation(operation_id).await {
        Some(snapshot) => {
            if !matches!(
                snapshot.state,
                OperationState::Completed {
                    result: OperationResult::Merged { .. }
                }
            ) {
                // A cancellation won the registry's write lock, so
                // `Cancelled` is terminal and the merge is not this
                // operation's outcome. Vetoing the deletion here is the
                // whole reason this operation owns it.
                tracing::info!(
                    "skipping split-archive cleanup: the operation ended as something other \
                     than a completed merge"
                );
                return;
            }
        }
        None => {
            // The record was evicted between the transition above and this
            // read (the registry bounds its terminal history), so the
            // gate cannot be satisfied. Skipping is the fail-safe
            // direction -- the parts leak rather than being deleted on an
            // unverified outcome -- but it is silent otherwise, and a
            // caller who asked for cleanup deserves to know it did not
            // happen.
            tracing::warn!(
                "skipping split-archive cleanup: the operation's own record was no longer \
                 available to confirm it completed"
            );
            return;
        }
    }
    delete_original_parts(parts);
}

/// Removes every part of a merged set, mirroring `MergeService`'s own
/// cleanup step (which [`run_merge`] asks core to skip). A part that
/// cannot be removed is logged and skipped rather than failing the merge
/// — core's behaviour, and the right one: the output archive already
/// holds the content, so a leftover part is untidy, not a failure.
///
/// Only the part's file name is logged, never its directory: a warning
/// line is not the vetted channel for exposing a user's filesystem
/// layout (the same reason `ApplicationError::path` is opt-in).
fn delete_original_parts(parts: &[PathBuf]) {
    for part in parts {
        let name = part
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "<unnamed>".to_string());
        match std::fs::remove_file(part) {
            Ok(()) => tracing::debug!("deleted merged split-archive part {name}"),
            Err(error) => {
                tracing::warn!("failed to delete merged split-archive part {name}: {error}");
            }
        }
    }
}

/// Raises a `Challenge::Password` on `operation_id`, awaits the caller's
/// response, and returns the freshly supplied password. `None` means the
/// operation was cancelled (or the challenge channel closed) while
/// waiting. Mirrors `crate::operations::extract::await_password_retry`
/// and `crate::materialization::await_password_retry` exactly, so a
/// passworded merge prompts the same way every other operation does.
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

/// The `start_merge` background worker. Spawned via the application's own
/// runtime handle; runs until the operation reaches a terminal state
/// (`Completed`, `Cancelled`, or `Failed`).
///
/// # Cancellation and what is left on disk
///
/// Unlike every other operation kind in this crate, `cancel` is used
/// *directly* here: `arclain_core::archive::CancellationToken` is the
/// same `Arc<AtomicBool>` the registry hands out, so it is passed to
/// `MergeService::merge` unchanged and the merge itself observes
/// cancellation at core's own checkpoints. Cancellation therefore remains
/// checkpoint-based -- neither the extraction nor the compression child
/// process is killed mid-run (there is no lower-level hook for either,
/// the same documented limitation `crate::operations::archive_mutation`
/// has for its backend calls) -- and what survives depends on which
/// checkpoint observed it:
///
/// - **Before/at validation** (the common case: cancelling while the
///   dialog is still up, or immediately after starting): nothing has been
///   created. No temporary directory, no output archive.
/// - **After extraction, before compression**: the extracted content
///   lived in a `tempfile::TempDir` that core drops on the way out, so it
///   is removed. Still no output archive.
/// - **After compression**: the output archive is *complete and left in
///   place*. That window is core's own, pre-existing shape, preserved
///   rather than "fixed" by deleting a finished archive out from under a
///   user -- so a frontend must not read `OperationState::Cancelled` as
///   "nothing was written".
/// - **While parked on a password challenge**: extraction has already
///   failed, so the temporary directory is gone and nothing was written.
///
/// ## Two guarantees that hold at the *composition* level, not core's
///
/// The cancel flag can be set at any instant, including after core's own
/// last check has already passed. Two properties therefore cannot be
/// delegated to `MergeService` and are enforced here instead:
///
/// 1. **A merge that finished always reports that it finished.** The
///    cancel flag is never allowed to suppress a successful attempt --
///    see [`decide_post_attempt`]. Once core returns `Ok`, the output
///    archive exists, and withholding `Completed { Merged }` would
///    discard the only value telling a frontend where it is. (Whether
///    that publish or a concurrently-arriving `Cancelled` becomes the
///    terminal state is the registry's call; both are honest, and
///    guarantee 2 covers either.)
/// 2. **`Cancelled` always implies the original parts survived.** This is
///    why `delete_originals` is *not* delegated to core: core's final
///    `check_cancelled()` sits before its own deletion loop, so a cancel
///    landing in the window `[core's check passes] -> [N remove_file] ->
///    [core returns Ok] -> [this worker resumes]` would have deleted the
///    parts while the operation reported `Cancelled` -- the parts gone
///    and the `Merged { output_path }` needed to find the result never
///    emitted. Instead core is always called with `delete_originals:
///    false`, and [`finish_successful_merge`] deletes only after reading
///    this operation's own terminal state back as `Completed { Merged }`.
///    `Completed` is terminal and so cannot be superseded once observed,
///    which makes the implication exact rather than probable.
///
/// The parts deleted are always the disk-derived enumeration taken before
/// the attempt ran -- the same source, at the same point in the sequence,
/// core's own cleanup step used -- never anything the caller supplied.
pub(crate) async fn run_merge(
    inner: Arc<AppRuntime>,
    operation_id: OperationId,
    cancel: Arc<AtomicBool>,
    request: MergeRequest,
) {
    if inner
        .operations()
        .transition(operation_id, OperationState::Started)
        .await
        .is_err()
    {
        return;
    }

    // The request's DTO may be minutes old (a merge dialog left open).
    // Re-detect and require agreement on the set's identity before any
    // work runs: the alternative is merging whatever now happens to sit
    // at those paths.
    let Some(current) = detect_multipart(&request.archive.first_part) else {
        fail(
            &inner,
            operation_id,
            set_no_longer_detected_error(&request.archive.first_part),
        )
        .await;
        return;
    };
    if !describes_the_same_set(&request.archive, &current) {
        fail(
            &inner,
            operation_id,
            set_changed_error(&request.archive.first_part),
        )
        .await;
        return;
    }
    if current.parts.is_empty() {
        fail(
            &inner,
            operation_id,
            incomplete_set_error(&request.archive.first_part),
        )
        .await;
        return;
    }

    let output_path = request.resolved_output_path();
    if output_path.exists() {
        fail(&inner, operation_id, output_exists_error(&output_path)).await;
        return;
    }

    if inner.operations().is_cancelled(operation_id).await {
        return;
    }

    let archive_name = request
        .archive
        .first_part
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| request.archive.base_name.clone());

    // Held as a plain `String` across retries rather than as a
    // `SecretInput`: `SecretInput` is deliberately not `Clone`, and the
    // blocking merge attempt needs an owned copy each time. Same
    // trade-off `crate::operations::extract` and
    // `crate::runtime::processing_ops` already make for their own
    // password ladders.
    let mut current_password = request
        .password
        .as_ref()
        .map(|secret| secret.expose_secret().to_string());
    let mut attempt: u32 = 1;

    loop {
        if inner.operations().is_cancelled(operation_id).await {
            return;
        }
        let Some(handle) = inner.tokio_handle() else {
            return;
        };

        let options = MergeOptions {
            output_format: request.output_format.to_core(),
            output_path: Some(output_path.clone()),
            compression_level: request.compression_level.to_core(),
            // Always `false`, whatever the request asked for: this
            // operation performs the deletion itself, after publishing
            // its own terminal state. See `run_merge`'s doc comment for
            // why that sequencing -- not core's -- is what makes
            // "`Cancelled` implies the parts survived" true.
            delete_originals: false,
            password: current_password.clone(),
        };

        // Progress arrives on a blocking thread and must reach the
        // registry from an async task, so it is funnelled through an
        // unbounded channel with a dedicated forwarder. The sender is
        // moved into the blocking closure, so it drops the moment the
        // attempt returns, which is what ends the forwarder -- awaiting
        // that join below is what guarantees no `Progress` event is ever
        // published after this operation's terminal transition.
        let (progress_tx, mut progress_rx) =
            tokio::sync::mpsc::unbounded_channel::<MergeProgress>();
        let forwarder = {
            let inner = inner.clone();
            handle.spawn(async move {
                while let Some(update) = progress_rx.recv().await {
                    let _ = inner
                        .operations()
                        .transition(
                            operation_id,
                            OperationState::Progress {
                                completed_units: u64::from(update.percent),
                                total_units: Some(100),
                                message: Some(update.message),
                            },
                        )
                        .await;
                }
            })
        };

        let backend_selector = inner.backend_selector();
        let archive_for_attempt = request.archive.clone();
        let cancel_for_attempt = cancel.clone();
        let attempt_result = handle
            .spawn_blocking(move || {
                merge_attempt(
                    backend_selector,
                    &archive_for_attempt,
                    options,
                    Some(progress_tx),
                    cancel_for_attempt,
                )
            })
            .await;
        let _ = forwarder.await;

        let outcome = match attempt_result {
            Ok(outcome) => outcome,
            Err(join_error) => {
                fail(&inner, operation_id, internal_join_error(join_error)).await;
                return;
            }
        };

        // Deliberately consulted *after* the outcome is known, and never
        // allowed to suppress a success -- see `decide_post_attempt`.
        let cancellation_requested = inner.operations().is_cancelled(operation_id).await;

        match decide_post_attempt(&outcome, cancellation_requested) {
            PostAttempt::Complete => {
                let written_path = match outcome {
                    MergeAttempt::Merged(written_path) => written_path,
                    // Unreachable: `decide_post_attempt` only answers
                    // `Complete` for `Merged`. Handled rather than
                    // `unreachable!()` so a future variant cannot turn a
                    // classification mistake into a panic on a
                    // destructive path.
                    _ => return,
                };
                finish_successful_merge(
                    inner.operations(),
                    operation_id,
                    written_path,
                    request.delete_originals.then_some(current.parts.as_slice()),
                )
                .await;
                return;
            }
            PostAttempt::Stop => return,
            PostAttempt::Fail => {
                let MergeAttempt::Failed(error) = outcome else {
                    return;
                };
                fail(&inner, operation_id, merge_backend_error(error)).await;
                return;
            }
            PostAttempt::Prompt => {
                match await_password_retry(&inner, operation_id, &archive_name, &mut attempt).await
                {
                    Some(password) => current_password = Some(password),
                    None => return,
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive::multipart::MultiPartFormat;
    use crate::event::OperationKind;

    fn dto(first_part: &str, base_name: &str) -> MultiPartArchiveDto {
        MultiPartArchiveDto {
            first_part: PathBuf::from(first_part),
            base_name: base_name.to_string(),
            format: MultiPartFormat::RarPart,
            parts: vec![PathBuf::from(first_part)],
        }
    }

    fn request(archive: MultiPartArchiveDto) -> MergeRequest {
        MergeRequest {
            archive,
            output_format: MergeOutputFormat::SevenZip,
            compression_level: MergeCompressionLevel::Normal,
            output_path: None,
            delete_originals: false,
            password: None,
        }
    }

    #[test]
    fn output_format_and_compression_mirror_cores_own_labels() {
        for format in MergeOutputFormat::all() {
            assert_eq!(format.extension(), format.to_core().extension());
            assert_eq!(format.display_name(), format.to_core().display_name());
        }
        assert_eq!(
            MergeOutputFormat::all().len(),
            arclain_core::services::OutputFormat::all().len(),
            "every core output format must be reachable through the facade"
        );
        for level in MergeCompressionLevel::all() {
            assert_eq!(level.display_name(), level.to_core().display_name());
        }
        assert_eq!(
            MergeCompressionLevel::all().len(),
            arclain_core::services::CompressionLevel::all().len(),
            "every core compression level must be reachable through the facade"
        );
    }

    #[test]
    fn format_and_level_defaults_match_cores_own_defaults() {
        assert_eq!(
            MergeOutputFormat::default().to_core(),
            arclain_core::services::OutputFormat::default()
        );
        assert_eq!(
            MergeCompressionLevel::default().to_core(),
            arclain_core::services::CompressionLevel::default()
        );
    }

    #[test]
    fn format_and_level_serialize_snake_case_and_round_trip() {
        for (format, expected) in [
            (MergeOutputFormat::SevenZip, "seven_zip"),
            (MergeOutputFormat::Zip, "zip"),
        ] {
            let value = serde_json::to_value(format).expect("serialize format");
            assert_eq!(value, serde_json::json!(expected));
            let round_tripped: MergeOutputFormat =
                serde_json::from_value(value).expect("deserialize format");
            assert_eq!(round_tripped, format);
        }
        for (level, expected) in [
            (MergeCompressionLevel::Store, "store"),
            (MergeCompressionLevel::Fastest, "fastest"),
            (MergeCompressionLevel::Fast, "fast"),
            (MergeCompressionLevel::Normal, "normal"),
            (MergeCompressionLevel::Maximum, "maximum"),
            (MergeCompressionLevel::Ultra, "ultra"),
        ] {
            let value = serde_json::to_value(level).expect("serialize level");
            assert_eq!(value, serde_json::json!(expected));
            let round_tripped: MergeCompressionLevel =
                serde_json::from_value(value).expect("deserialize level");
            assert_eq!(round_tripped, level);
        }
    }

    #[test]
    fn a_well_formed_request_validates() {
        request(dto("/sets/RJ123456.part1.rar", "rj123456"))
            .validate()
            .expect("a well-formed request must be accepted");
    }

    #[test]
    fn an_empty_first_part_is_rejected() {
        let error = request(dto("", "rj123456")).validate().unwrap_err();
        assert_eq!(error.kind, ApplicationErrorKind::InvalidInput);
        assert_eq!(error.field.as_deref(), Some("archive.first_part"));
    }

    #[test]
    fn a_first_part_that_is_not_a_file_path_is_rejected() {
        let error = request(dto("/sets/..", "rj123456")).validate().unwrap_err();
        assert_eq!(error.kind, ApplicationErrorKind::InvalidInput);
        assert_eq!(error.field.as_deref(), Some("archive.first_part"));
    }

    #[test]
    fn a_blank_base_name_is_rejected() {
        let error = request(dto("/sets/RJ123456.part1.rar", "   "))
            .validate()
            .unwrap_err();
        assert_eq!(error.kind, ApplicationErrorKind::InvalidInput);
        assert_eq!(error.field.as_deref(), Some("archive.base_name"));
    }

    #[test]
    fn an_empty_explicit_output_path_is_rejected() {
        let mut request = request(dto("/sets/RJ123456.part1.rar", "rj123456"));
        request.output_path = Some(PathBuf::new());
        let error = request.validate().unwrap_err();
        assert_eq!(error.kind, ApplicationErrorKind::InvalidInput);
        assert_eq!(error.field.as_deref(), Some("output_path"));
    }

    /// The default output path is the one the pre-facade merge dialog
    /// computed: the set's base name plus the chosen format's extension,
    /// in the first part's own directory.
    #[test]
    fn the_default_output_path_sits_beside_the_first_part() {
        let request = request(dto("/sets/RJ123456.part1.rar", "rj123456"));
        assert_eq!(
            request.resolved_output_path(),
            PathBuf::from("/sets").join("rj123456.7z")
        );

        let mut zip = request;
        zip.output_format = MergeOutputFormat::Zip;
        assert_eq!(
            zip.resolved_output_path(),
            PathBuf::from("/sets").join("rj123456.zip")
        );
    }

    #[test]
    fn an_explicit_output_path_wins_over_the_default() {
        let mut request = request(dto("/sets/RJ123456.part1.rar", "rj123456"));
        request.output_path = Some(PathBuf::from("/elsewhere/merged.7z"));
        assert_eq!(
            request.resolved_output_path(),
            PathBuf::from("/elsewhere/merged.7z")
        );
    }

    /// The identity comparison must ignore `parts`: a caller's snapshot
    /// legitimately differs from a fresh enumeration (a part finished
    /// downloading in between), and that alone must not abort a merge.
    #[test]
    fn a_differing_part_list_alone_still_describes_the_same_set() {
        let requested = dto("/sets/RJ123456.part1.rar", "rj123456");
        let mut current = requested.clone();
        current
            .parts
            .push(PathBuf::from("/sets/RJ123456.part2.rar"));
        assert!(describes_the_same_set(&requested, &current));
    }

    #[test]
    fn a_differing_identity_field_is_a_different_set() {
        let requested = dto("/sets/RJ123456.part1.rar", "rj123456");

        let mut other_base = requested.clone();
        other_base.base_name = "rj999999".to_string();
        assert!(!describes_the_same_set(&requested, &other_base));

        let mut other_format = requested.clone();
        other_format.format = MultiPartFormat::Generic001;
        assert!(!describes_the_same_set(&requested, &other_format));

        let mut other_first = requested.clone();
        other_first.first_part = PathBuf::from("/sets/RJ999999.part1.rar");
        assert!(!describes_the_same_set(&requested, &other_first));
    }

    /// The regression this pins: a cancel landing after core has already
    /// written the output must not suppress `Completed { Merged }`. An
    /// earlier revision consulted the cancel flag before matching the
    /// outcome *at all*, so a merge that had genuinely finished reported
    /// only `Cancelled` — and the caller lost the one value telling it
    /// where the result is. Deterministic by construction: the flag is
    /// passed in rather than raced.
    #[test]
    fn a_successful_attempt_is_completed_even_when_cancellation_was_requested() {
        let merged = MergeAttempt::Merged(PathBuf::from("/sets/rj123456.7z"));
        assert_eq!(
            decide_post_attempt(&merged, true),
            PostAttempt::Complete,
            "a merge that finished must report that it finished"
        );
        assert_eq!(decide_post_attempt(&merged, false), PostAttempt::Complete);
    }

    /// The other half of the same rule: cancellation *does* decide the
    /// fate of an attempt that did not finish, and it must not turn into a
    /// password prompt or a `Failed` event for an operation the registry
    /// has already reported as `Cancelled`.
    #[test]
    fn an_unfinished_attempt_stops_when_cancellation_was_requested() {
        assert_eq!(
            decide_post_attempt(&MergeAttempt::PasswordRequired, true),
            PostAttempt::Stop
        );
        assert_eq!(
            decide_post_attempt(&MergeAttempt::PasswordRequired, false),
            PostAttempt::Prompt
        );

        let failed = || MergeAttempt::Failed(anyhow::anyhow!("7z failed (code Some(1)): nope"));
        assert_eq!(decide_post_attempt(&failed(), true), PostAttempt::Stop);
        assert_eq!(decide_post_attempt(&failed(), false), PostAttempt::Fail);
    }

    /// Writes `count` placeholder part files into a fresh temp directory,
    /// returning it alongside their paths. Real files, so the assertions
    /// below observe real deletions rather than a mock's record of them.
    fn parts_on_disk(count: usize) -> (tempfile::TempDir, Vec<PathBuf>) {
        let temp = tempfile::tempdir().expect("create tempdir");
        let parts = (1..=count)
            .map(|index| {
                let part = temp.path().join(format!("rj123456.7z.{index:03}"));
                std::fs::write(&part, b"placeholder part").expect("write part");
                part
            })
            .collect();
        (temp, parts)
    }

    /// The gate this whole fix hangs on, in the direction that loses data
    /// if it regresses: the operation's record is already terminal
    /// `Cancelled` when the merge finishes, so `finish_successful_merge`
    /// must read that back and delete nothing.
    ///
    /// Deterministic on purpose. The end-to-end test races a real
    /// cancellation against a real merge and then inspects the disk, which
    /// cannot reliably observe a *post-terminal* deletion at all -- the
    /// regression class this gate exists for. Here the interleaving is
    /// constructed rather than raced, and the deletion (or its absence) has
    /// finished by the time the assertion runs.
    #[tokio::test]
    async fn the_read_back_gate_refuses_to_delete_when_the_operation_was_cancelled() {
        let operations = OperationRegistry::new();
        let (operation_id, _cancel) = operations.begin(OperationKind::Merge).await;
        operations
            .cancel(operation_id)
            .await
            .expect("cancelling a live operation must be accepted");

        let (temp, parts) = parts_on_disk(3);
        let output_path = temp.path().join("rj123456.7z");
        std::fs::write(&output_path, b"the merged archive").expect("write output");

        finish_successful_merge(&operations, operation_id, output_path.clone(), Some(&parts)).await;

        for part in &parts {
            assert!(
                part.exists(),
                "a merge whose operation is already Cancelled must delete nothing: {part:?}"
            );
        }
        assert!(
            output_path.exists(),
            "the archive core already wrote stays; only the cleanup is vetoed"
        );
        assert!(
            matches!(
                operations.operation(operation_id).await.map(|s| s.state),
                Some(OperationState::Cancelled)
            ),
            "the terminal state must still be Cancelled -- Completed cannot overwrite it"
        );
    }

    /// The same gate in the direction that must still do the work: nothing
    /// cancelled the operation, `Completed { Merged }` lands, and the parts
    /// go. Without this half, the test above could be satisfied by never
    /// deleting anything at all.
    #[tokio::test]
    async fn the_read_back_gate_deletes_when_the_merge_is_the_operations_outcome() {
        let operations = OperationRegistry::new();
        let (operation_id, _cancel) = operations.begin(OperationKind::Merge).await;

        let (temp, parts) = parts_on_disk(3);
        let output_path = temp.path().join("rj123456.7z");
        std::fs::write(&output_path, b"the merged archive").expect("write output");

        finish_successful_merge(&operations, operation_id, output_path.clone(), Some(&parts)).await;

        for part in &parts {
            assert!(
                !part.exists(),
                "a completed merge that asked for cleanup must remove every part: {part:?}"
            );
        }
        assert!(output_path.exists());
        assert!(
            matches!(
                operations.operation(operation_id).await.map(|s| s.state),
                Some(OperationState::Completed {
                    result: OperationResult::Merged { .. }
                })
            ),
            "the terminal state must be the merge's own Completed"
        );
    }

    /// `parts: None` (the request did not ask for cleanup) must publish the
    /// same terminal state and touch nothing -- and must not emit the
    /// "Deleting original parts..." progress message either.
    #[tokio::test]
    async fn a_merge_that_did_not_ask_for_cleanup_deletes_nothing() {
        let operations = OperationRegistry::new();
        let mut events = operations.subscribe();
        let (operation_id, _cancel) = operations.begin(OperationKind::Merge).await;

        let (temp, parts) = parts_on_disk(2);
        let output_path = temp.path().join("rj123456.7z");

        finish_successful_merge(&operations, operation_id, output_path.clone(), None).await;

        for part in &parts {
            assert!(part.exists(), "nothing was requested, so nothing goes");
        }
        assert!(matches!(
            operations.operation(operation_id).await.map(|s| s.state),
            Some(OperationState::Completed {
                result: OperationResult::Merged { .. }
            })
        ));

        let mut saw_cleanup_message = false;
        while let Ok(event) = events.try_recv() {
            if let OperationState::Progress {
                message: Some(message),
                ..
            } = &event.state
            {
                saw_cleanup_message |= message.contains("Deleting original parts");
            }
        }
        assert!(
            !saw_cleanup_message,
            "a merge with no cleanup requested must not announce one"
        );
    }

    /// `delete_original_parts` is the only code in this crate that removes
    /// a user's files, and it must remove exactly what it is handed and
    /// tolerate a member that is already gone (core logs and skips one it
    /// cannot remove; so does this).
    #[test]
    fn deleting_original_parts_removes_each_one_and_tolerates_a_missing_member() {
        let temp = tempfile::tempdir().expect("create tempdir");
        let dir = temp.path();
        let present = dir.join("rj123456.part1.rar");
        let also_present = dir.join("rj123456.part2.rar");
        let never_existed = dir.join("rj123456.part3.rar");
        std::fs::write(&present, b"").expect("write part");
        std::fs::write(&also_present, b"").expect("write part");
        let bystander = dir.join("keep-me.bin");
        std::fs::write(&bystander, b"keep").expect("write bystander");

        delete_original_parts(&[present.clone(), also_present.clone(), never_existed.clone()]);

        assert!(!present.exists());
        assert!(!also_present.exists());
        assert!(!never_existed.exists());
        assert!(
            bystander.exists(),
            "only the parts handed in may be removed"
        );
    }

    #[test]
    fn a_request_never_debug_prints_its_password() {
        let mut request = request(dto("/sets/RJ123456.part1.rar", "rj123456"));
        request.password = Some(SecretInput::new("hunter2".to_string()));
        let rendered = format!("{request:?}");
        assert!(
            !rendered.contains("hunter2"),
            "MergeRequest's Debug output must never carry the password: {rendered}"
        );
        assert!(rendered.contains("REDACTED"));
    }

    /// The one field a caller could otherwise use to direct deletion: the
    /// core value handed to `MergeService` must always start with an
    /// empty `all_parts` so `validate()` re-enumerates from disk and
    /// `delete_originals` can only ever remove real members of the set.
    #[test]
    fn the_core_value_never_inherits_a_caller_supplied_part_list() {
        let mut archive = dto("/sets/RJ123456.part1.rar", "rj123456");
        archive.parts = vec![
            PathBuf::from("/home/someone/taxes.pdf"),
            PathBuf::from("/etc/passwd"),
        ];
        let multipart = arclain_core::archive::MultiPartArchive {
            first_part: archive.first_part.clone(),
            all_parts: Vec::new(),
            format: archive.format.to_core(),
            base_name: archive.base_name.clone(),
        };
        assert!(
            multipart.all_parts.is_empty(),
            "the part list a merge deletes from must come from disk, never from the request"
        );
        assert_eq!(multipart.first_part, archive.first_part);
        assert_eq!(multipart.base_name, archive.base_name);
    }
}
