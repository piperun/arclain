//! Extraction as a cancellable, event-broadcasting application operation.
//!
//! [`ExtractRequest`]/[`CollisionPolicy`] are the contract-mandated public
//! request types `ArclainApp::start_extract` (added in `crate::runtime`)
//! accepts. Everything else in this module is the machinery that request
//! drives: [`ExtractRunner`]/[`RunningExtraction`] are the seam production
//! 7-Zip-CLI spawning and a test's deterministic fake both implement (the
//! same role `BootstrapConfig::archive_backend_override` plays for
//! opening), and [`run_extract`] is the background worker
//! `ArclainApp::start_extract` spawns onto this app's own runtime.
//!
//! The facade owns process spawning and cancellation here exactly the
//! way `crate::runtime::archive_ops` already owns the archive-open
//! backend calls: egui used to hold a raw `std::process::Child` in
//! `ArchiveOperationsState::extraction_child` and poll an `mpsc::Receiver`
//! every frame; this operation instead spawns the same 7-Zip CLI command
//! (via [`SevenZipRunner`], which calls the exact same
//! `SevenZipCli::spawn_extract_*_with_progress` methods the pre-facade UI
//! called directly) from a task on this app's own runtime, and reports
//! progress/completion through the same [`crate::event::OperationEvent`]
//! stream every other operation uses.
//!
//! `CollisionPolicy::Rename` is the one policy the 7-Zip CLI cannot
//! express directly (its destination filename is always derived from the
//! archive-internal path, with no way to tell it "write this one under a
//! different name") -- see [`finalize_rename_policy`] for how this
//! extracts into a private staging directory first and then moves each
//! file into the real destination, renaming on any collision.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Duration;

use arclain_core::backends::sevenz_cli::SevenZipCli;
use arclain_core::utilities::rename_no_replace;

use crate::archive::ArchiveSession;
use crate::challenge::{next_challenge_id, Challenge, ChallengeResponse};
use crate::error::{ApplicationError, ApplicationErrorKind, Recoverability, SuggestedAction};
use crate::event::{OperationResult, OperationState};
use crate::ids::{ArchiveSessionId, EntryId, OperationId};
use crate::runtime::AppRuntime;

/// A request to extract entries from an open archive session, the
/// argument to `ArclainApp::start_extract`. An empty `entry_ids` means
/// "the whole archive" -- there is no separate boolean/variant for it,
/// mirroring how the pre-facade UI's distinct `extract_selected`/
/// `extract_all` functions collapse into one request shape here.
#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub struct ExtractRequest {
    pub session_id: ArchiveSessionId,
    pub entry_ids: Vec<EntryId>,
    pub destination: std::path::PathBuf,
    pub collision_policy: CollisionPolicy,
}

/// How an extraction should handle a destination file that already
/// exists. `Ask` defers the decision to a live [`Challenge::ConfirmOverwrite`]
/// raised only if a collision is actually found; the other three are
/// applied unconditionally with no prompt.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CollisionPolicy {
    Ask,
    Overwrite,
    Rename,
    Skip,
}

/// One progress tick a running extraction reports: percent complete and,
/// for the CLI runner, the same free-text log line the pre-facade UI
/// displayed in its "Details" log panel.
#[derive(Clone, Debug)]
pub struct ExtractProgressEvent {
    pub percent: u8,
    pub message: Option<String>,
}

/// What one CLI extraction attempt should extract: every entry in the
/// archive, or an explicit set of archive-relative file paths already
/// resolved from `ExtractRequest::entry_ids` (see
/// [`ArchiveSession::resolve_extractable_paths`]).
#[derive(Clone, Debug)]
pub enum ExtractSelection {
    WholeArchive,
    Files(Vec<String>),
}

/// Fully-resolved instructions for one CLI extraction attempt: every
/// application-level concern (entry-id resolution, collision-policy
/// filtering, password) is already settled by the time [`ExtractRunner::spawn`]
/// receives this. Accessor methods rather than public fields for the same
/// reason `arclain_core::Archive` exposes `password_ref()` instead of a
/// public field -- `password`, when present, is a live secret a runner
/// implementation must not accidentally log or persist via a derived
/// `Debug`/`Clone`.
pub struct ExtractPlan {
    source_path: PathBuf,
    destination: PathBuf,
    password: Option<String>,
    selection: ExtractSelection,
}

impl ExtractPlan {
    pub fn source_path(&self) -> &Path {
        &self.source_path
    }

    pub fn destination(&self) -> &Path {
        &self.destination
    }

    pub fn password(&self) -> Option<&str> {
        self.password.as_deref()
    }

    pub fn selection(&self) -> &ExtractSelection {
        &self.selection
    }
}

/// Hand-written rather than `#[derive(Debug)]`, matching
/// `arclain_core::Archive`'s own `Debug` impl: reports only whether a
/// password is set, never its value.
impl std::fmt::Debug for ExtractPlan {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExtractPlan")
            .field("source_path", &self.source_path)
            .field("destination", &self.destination)
            .field("password_is_set", &self.password.is_some())
            .field("selection", &self.selection)
            .finish()
    }
}

/// The seam production 7-Zip-CLI spawning and a test's deterministic
/// fake both implement. `BootstrapConfig::extract_runner_override`
/// installs a fake for tests (see `crates/app/tests/extract_operation.rs`);
/// production always uses the real [`SevenZipRunner`] built from the
/// bootstrapped 7-Zip CLI. Mirrors the role
/// `BootstrapConfig::archive_backend_override` plays for
/// `start_open_archive`.
pub trait ExtractRunner: Send + Sync {
    /// Whether the external tool this runner depends on is available
    /// right now. Checked immediately before every spawn attempt, not
    /// just once at bootstrap: a 7-Zip executable present at bootstrap
    /// time can still be deleted or moved before an extraction actually
    /// starts (the same "double-check at use time" `SessionStore::
    /// sevenzip_still_available` already performs for `capabilities()`/
    /// `health()`).
    fn tool_available(&self) -> bool;

    /// Starts one extraction attempt. Returns a handle the operation
    /// worker polls non-blockingly for progress and completion. This
    /// method itself may block -- production spawns a real child
    /// process; a deterministic test fake may synchronize on a barrier
    /// -- so callers must invoke it from a blocking-safe context
    /// (`spawn_blocking`), never directly from an async task.
    fn spawn(&self, plan: &ExtractPlan) -> Result<Box<dyn RunningExtraction>, ApplicationError>;
}

/// A running extraction, polled non-blockingly by the operation worker
/// loop. Every method here must return immediately regardless of whether
/// the underlying work has progressed -- the worker calls these from an
/// async task on a fixed poll interval, never wrapping them in
/// `spawn_blocking` themselves.
pub trait RunningExtraction: Send {
    /// The next buffered progress event, if any arrived since the last
    /// poll. Never blocks; returns `None` when nothing new is available
    /// yet.
    fn poll_progress(&mut self) -> Option<ExtractProgressEvent>;

    /// `Some` once the process has exited, carrying its outcome (a
    /// classified [`ApplicationError`] on failure -- in particular
    /// `ApplicationErrorKind::PasswordRequired` when the failure looks
    /// password-shaped, which [`run_extract`] treats as retryable via a
    /// [`Challenge::Password`] rather than terminal). Never blocks.
    fn poll_outcome(&mut self) -> Option<Result<(), ApplicationError>>;

    /// Forcefully terminates the underlying process. Called once the
    /// worker notices the operation was cancelled.
    fn kill(&mut self);
}

/// Detects whether a backend error message indicates a password failure.
/// Ported verbatim from `crate::runtime::archive_ops::is_password_error`
/// (itself ported from `crates/ui/src/core/operations/archive.rs`) --
/// duplicated rather than shared for the reason that function's own doc
/// comment gives: `arclain_app` cannot depend on `arclain_ui` or vice
/// versa, and centralizing this specific classifier into
/// `arclain_core::utilities` would touch call sites outside this task's
/// scope. This copy is intra-crate (both live in `arclain_app`), but kept
/// as its own private copy anyway: `archive_ops`'s classifier reads a
/// `list()` error chain, this one reads a CLI exit diagnostic -- same
/// patterns, different callers, and each is small enough that sharing a
/// single generic version would cost more indirection than it saves.
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

fn cli_spawn_error(error: anyhow::Error) -> ApplicationError {
    ApplicationError::new(ApplicationErrorKind::Backend, "failed to start extraction")
        .with_diagnostic(format!("{error:#}"))
        .with_recoverability(Recoverability::Retry)
        .with_retryable(true)
}

/// Classifies one finished CLI extraction attempt: `Some(percent)`
/// updates aside, a non-zero exit combined with recent output that looks
/// password-shaped becomes `PasswordRequired` (retryable via a
/// challenge); anything else becomes a plain `Backend` failure.
fn classify_cli_exit(
    status: std::process::ExitStatus,
    recent_output: &[String],
) -> ApplicationError {
    let diagnostic = format!(
        "extraction process exited with code {:?}; recent output: {}",
        status.code(),
        recent_output.join(" | ")
    );
    if is_password_error(&diagnostic) {
        ApplicationError::new(
            ApplicationErrorKind::PasswordRequired,
            "extraction failed: the archive's contents need a password",
        )
        .with_diagnostic(diagnostic)
        .with_recoverability(Recoverability::UserAction)
        .with_suggested_action(SuggestedAction::SupplyPassword)
    } else {
        ApplicationError::new(ApplicationErrorKind::Backend, "extraction process failed")
            .with_diagnostic(diagnostic)
            .with_recoverability(Recoverability::Retry)
            .with_retryable(true)
    }
}

/// How many of the CLI process's most recent log-line updates
/// [`SevenZipRunningExtraction`] retains purely to build a failure
/// diagnostic -- not shown to the user live (that already happens via
/// the forwarded [`ExtractProgressEvent`]s themselves), just enough
/// recent context for [`classify_cli_exit`] to recognize a password
/// failure whose text arrived on stdout rather than in the exit code.
const RECENT_OUTPUT_CAPACITY: usize = 20;

/// Production [`ExtractRunner`]: spawns the real 7-Zip CLI exactly the
/// way the pre-facade UI did (see the module doc comment).
pub(crate) struct SevenZipRunner {
    cli: SevenZipCli,
}

impl SevenZipRunner {
    pub(crate) fn new(cli: SevenZipCli) -> Self {
        Self { cli }
    }
}

impl ExtractRunner for SevenZipRunner {
    fn tool_available(&self) -> bool {
        self.cli.exe_path().exists()
    }

    fn spawn(&self, plan: &ExtractPlan) -> Result<Box<dyn RunningExtraction>, ApplicationError> {
        let handle = match plan.selection() {
            ExtractSelection::WholeArchive => self.cli.spawn_extract_all_with_progress(
                plan.source_path(),
                plan.destination(),
                plan.password(),
            ),
            ExtractSelection::Files(files) => self.cli.spawn_extract_files_with_progress(
                plan.source_path(),
                plan.destination(),
                files,
                plan.password(),
            ),
        }
        .map_err(cli_spawn_error)?;
        Ok(Box::new(SevenZipRunningExtraction {
            child: handle.child,
            rx: handle.rx,
            recent_output: Vec::new(),
        }))
    }
}

struct SevenZipRunningExtraction {
    child: std::process::Child,
    rx: std::sync::mpsc::Receiver<arclain_core::backends::sevenz_cli::ProgressUpdate>,
    recent_output: Vec<String>,
}

impl RunningExtraction for SevenZipRunningExtraction {
    fn poll_progress(&mut self) -> Option<ExtractProgressEvent> {
        match self.rx.try_recv() {
            Ok(update) => {
                if let Some(message) = &update.message {
                    if self.recent_output.len() >= RECENT_OUTPUT_CAPACITY {
                        self.recent_output.remove(0);
                    }
                    self.recent_output.push(message.clone());
                }
                Some(ExtractProgressEvent {
                    percent: update.percent,
                    message: update.message,
                })
            }
            Err(_) => None,
        }
    }

    fn poll_outcome(&mut self) -> Option<Result<(), ApplicationError>> {
        match self.child.try_wait() {
            Ok(Some(status)) => Some(if status.success() {
                Ok(())
            } else {
                Err(classify_cli_exit(status, &self.recent_output))
            }),
            Ok(None) => None,
            Err(error) => Some(Err(ApplicationError::new(
                ApplicationErrorKind::Internal,
                "failed to poll the extraction process",
            )
            .with_diagnostic(error.to_string()))),
        }
    }

    fn kill(&mut self) {
        let _ = self.child.kill();
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
        "one of the requested entries does not exist in this archive session",
    )
    .with_recoverability(Recoverability::Fatal)
    .with_archive_session_id(session_id)
    .with_entry_id(entry_id)
}

fn missing_tool_error() -> ApplicationError {
    ApplicationError::new(
        ApplicationErrorKind::ExternalToolMissing,
        "the CLI tool this extraction needs is not available",
    )
    .with_recoverability(Recoverability::UserAction)
    .with_suggested_action(SuggestedAction::InstallExternalTool)
}

/// Validates `destination` before anything else runs. Must be absolute
/// (a relative path is ambiguous once handed to a spawned CLI process
/// whose working directory a caller does not control) and, if it already
/// exists, must be a directory -- extracting into a path that is
/// actually a plain file can never succeed.
fn validate_destination(destination: &Path) -> Result<(), ApplicationError> {
    if !destination.is_absolute() {
        return Err(ApplicationError::new(
            ApplicationErrorKind::InvalidInput,
            "extraction destination must be an absolute path",
        )
        .with_recoverability(Recoverability::UserAction)
        .with_suggested_action(SuggestedAction::ChooseDestination)
        .with_field("destination"));
    }
    if destination.exists() && !destination.is_dir() {
        return Err(ApplicationError::new(
            ApplicationErrorKind::InvalidInput,
            "extraction destination exists and is not a directory",
        )
        .with_recoverability(Recoverability::UserAction)
        .with_suggested_action(SuggestedAction::ChooseDestination)
        .with_path(destination.to_path_buf())
        .with_field("destination"));
    }
    Ok(())
}

/// Resolves `request.entry_ids` against `session`'s current index. An
/// empty `entry_ids` means "the whole archive" (see [`ExtractRequest`]'s
/// own doc comment).
fn resolve_selection(
    session: &ArchiveSession,
    request: &ExtractRequest,
) -> Result<ExtractSelection, ApplicationError> {
    if request.entry_ids.is_empty() {
        return Ok(ExtractSelection::WholeArchive);
    }
    session
        .resolve_extractable_paths(&request.entry_ids)
        .map(ExtractSelection::Files)
        .map_err(|bad_id| unknown_entry_error(request.session_id, bad_id))
}

/// The concrete candidate file paths `selection` would write, used only
/// to pre-scan `destination` for collisions (`Skip`/`Ask` policies).
fn candidate_paths(session: &ArchiveSession, selection: &ExtractSelection) -> Vec<String> {
    match selection {
        ExtractSelection::WholeArchive => session.all_file_paths(),
        ExtractSelection::Files(files) => files.clone(),
    }
}

fn colliding_paths(destination: &Path, candidates: &[String]) -> Vec<String> {
    candidates
        .iter()
        .filter(|path| destination.join(path).exists())
        .cloned()
        .collect()
}

fn filter_out(candidates: Vec<String>, excluded: &HashSet<String>) -> Vec<String> {
    candidates
        .into_iter()
        .filter(|path| !excluded.contains(path))
        .collect()
}

/// Safety ceiling on one CLI invocation's total file-argument length,
/// comfortably under the ~32,767-character Windows command-line limit
/// (leaving headroom for the executable path, the fixed flags, the
/// archive path, and the destination path). Enforced on every platform,
/// not just Windows, so [`chunk_file_list`]'s behavior is deterministic
/// and testable everywhere: Unix's `ARG_MAX` is typically megabytes, so
/// this never binds there in practice, but nothing about the *behavior*
/// should depend on which OS happens to run a given test.
const MAX_CHUNK_ARGS_CHARS: usize = 28_000;

/// Splits `files` into chunks whose total argument length (each path's
/// length plus one separator) stays under [`MAX_CHUNK_ARGS_CHARS`].
///
/// A whole-archive `CollisionPolicy::Skip` (or a declined `Ask`) expands
/// to an explicit per-file argument list -- there is no "extract
/// everything except these" flag to hand the CLI instead -- and a large
/// archive's full (or nearly full) file list can otherwise exceed the
/// command-line length a spawned process is allowed. [`run_extract`]
/// runs each returned chunk as its own CLI invocation, in sequence,
/// treating the whole run as one logical operation.
///
/// A single pathologically long individual path (longer than the ceiling
/// by itself) still gets its own one-item chunk rather than being
/// dropped silently -- the CLI is left to fail that specific invocation
/// if the OS truly cannot accept it, which is at least a visible failure
/// rather than a silently incomplete extraction.
fn chunk_file_list(files: Vec<String>) -> Vec<Vec<String>> {
    if files.is_empty() {
        return Vec::new();
    }
    let mut chunks = Vec::new();
    let mut current = Vec::new();
    let mut current_len = 0usize;
    for file in files {
        let added_len = file.len() + 1;
        if !current.is_empty() && current_len + added_len > MAX_CHUNK_ARGS_CHARS {
            chunks.push(std::mem::take(&mut current));
            current_len = 0;
        }
        current_len += added_len;
        current.push(file);
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    chunks
}

/// What [`resolve_collisions`] decided the operation should actually do.
struct Resolution {
    selection: ExtractSelection,
    /// `true` only for `CollisionPolicy::Rename`: the CLI extracts into a
    /// private staging directory first, and [`finalize_rename_policy`]
    /// moves each file into the real destination afterward.
    needs_staging: bool,
}

/// Applies `request.collision_policy`, pre-scanning `destination` for
/// `Skip`/`Ask` and raising a live [`Challenge::ConfirmOverwrite`] for
/// `Ask` when a collision is actually found. `Ok(None)` means the
/// operation was cancelled while waiting on that challenge -- the
/// registry already recorded `Cancelled`, so the caller returns without
/// transitioning again, mirroring `archive_ops::run_open_archive`'s own
/// handling of the equivalent case.
async fn resolve_collisions(
    inner: &Arc<AppRuntime>,
    operation_id: OperationId,
    request: &ExtractRequest,
    session: &ArchiveSession,
    selection: ExtractSelection,
) -> Result<Option<Resolution>, ApplicationError> {
    match request.collision_policy {
        CollisionPolicy::Overwrite => Ok(Some(Resolution {
            selection,
            needs_staging: false,
        })),
        CollisionPolicy::Rename => Ok(Some(Resolution {
            selection,
            needs_staging: true,
        })),
        CollisionPolicy::Skip => {
            let candidates = candidate_paths(session, &selection);
            let colliding: HashSet<String> = colliding_paths(&request.destination, &candidates)
                .into_iter()
                .collect();
            Ok(Some(Resolution {
                selection: ExtractSelection::Files(filter_out(candidates, &colliding)),
                needs_staging: false,
            }))
        }
        CollisionPolicy::Ask => {
            let candidates = candidate_paths(session, &selection);
            let colliding = colliding_paths(&request.destination, &candidates);
            if colliding.is_empty() {
                return Ok(Some(Resolution {
                    selection,
                    needs_staging: false,
                }));
            }

            let challenge_id = next_challenge_id();
            let receiver = inner.challenges().register(operation_id);
            if inner
                .operations()
                .transition(
                    operation_id,
                    OperationState::Challenge {
                        challenge: Challenge::ConfirmOverwrite {
                            id: challenge_id,
                            destination: request.destination.clone(),
                        },
                    },
                )
                .await
                .is_err()
            {
                inner.challenges().cancel(operation_id);
                return Ok(None);
            }

            let response = tokio::select! {
                response = receiver => response,
                () = inner.operations().wait_until_cancelled(operation_id) => {
                    inner.challenges().cancel(operation_id);
                    return Ok(None);
                }
            };

            match response {
                Ok(ChallengeResponse::ConfirmOverwrite {
                    overwrite: true, ..
                }) => Ok(Some(Resolution {
                    selection,
                    needs_staging: false,
                })),
                Ok(ChallengeResponse::ConfirmOverwrite {
                    overwrite: false, ..
                }) => {
                    let colliding: HashSet<String> = colliding.into_iter().collect();
                    Ok(Some(Resolution {
                        selection: ExtractSelection::Files(filter_out(candidates, &colliding)),
                        needs_staging: false,
                    }))
                }
                Ok(_) => Err(ApplicationError::new(
                    ApplicationErrorKind::Internal,
                    "expected a collision response to a collision challenge",
                )),
                Err(_) => Ok(None),
            }
        }
    }
}

/// RAII guard that best-effort removes a staging directory on drop, so a
/// failure or cancellation partway through a `CollisionPolicy::Rename`
/// extraction never leaves an orphaned hidden folder under the user's
/// chosen destination. Idempotent: `finalize_rename_policy`'s own
/// successful-path cleanup running first just means this later removal
/// is a harmless no-op (the ignored `Result` covers that).
struct StagingDirGuard(PathBuf);

impl StagingDirGuard {
    fn create(path: PathBuf) -> std::io::Result<Self> {
        std::fs::create_dir_all(&path)?;
        Ok(Self(path))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for StagingDirGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn staging_dir_path(destination: &Path, operation_id: OperationId) -> PathBuf {
    // Nested inside `destination` itself (not the OS temp directory) so
    // the final move in `finalize_rename_policy` is a same-filesystem
    // rename -- `rename_no_replace`'s own contract requires that, and it
    // is what keeps the move a cheap metadata operation rather than a
    // byte-for-byte copy. Keyed by `operation_id` so two concurrent
    // extractions into the same destination never collide.
    destination.join(format!(".arclain-extract-{}", operation_id.into_raw()))
}

fn staging_io_error(path: &Path, error: std::io::Error) -> ApplicationError {
    ApplicationError::new(
        ApplicationErrorKind::Internal,
        "failed to finalize renamed extraction output",
    )
    .with_diagnostic(format!("{}: {error}", path.display()))
}

/// Moves `from` to `to`, renaming with an incrementing `" (n)"` suffix
/// if `to` already exists (or the platform's atomic no-replace primitive
/// is unavailable) rather than replacing it. Race-free: two concurrent
/// callers contending for the same candidate name both attempt
/// `rename_no_replace` and only one can win a given candidate, so the
/// loser simply advances to the next one instead of silently overwriting
/// the winner's file -- the same property `rename_no_replace`'s own doc
/// comment (and `impl_rename_archive`'s existing use of it) documents.
fn move_with_rename_on_collision(from: &Path, to: &Path) -> Result<(), ApplicationError> {
    if rename_no_replace(from, to).is_ok() {
        return Ok(());
    }
    let dir = to.parent().unwrap_or_else(|| Path::new("."));
    let stem = to
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    let extension = to.extension().map(|ext| ext.to_string_lossy().into_owned());
    for n in 1..1000u32 {
        let candidate_name = match &extension {
            Some(ext) => format!("{stem} ({n}).{ext}"),
            None => format!("{stem} ({n})"),
        };
        let candidate = dir.join(candidate_name);
        match rename_no_replace(from, &candidate) {
            Ok(()) => return Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(staging_io_error(&candidate, error)),
        }
    }
    Err(ApplicationError::new(
        ApplicationErrorKind::Internal,
        "could not find a free renamed filename after 999 attempts",
    ))
}

/// Walks `staging_root` recursively and moves every file into the same
/// relative path under `destination_root`, creating destination
/// directories as needed and applying [`move_with_rename_on_collision`]
/// to each. See the module doc comment for why `CollisionPolicy::Rename`
/// needs a staging pass at all.
fn finalize_rename_policy(
    staging_root: &Path,
    destination_root: &Path,
) -> Result<(), ApplicationError> {
    let mut stack = vec![staging_root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let read_dir = std::fs::read_dir(&dir).map_err(|error| staging_io_error(&dir, error))?;
        for entry in read_dir {
            let entry = entry.map_err(|error| staging_io_error(&dir, error))?;
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let relative = path
                .strip_prefix(staging_root)
                .expect("walked path must be under staging_root");
            let target = destination_root.join(relative);
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent).map_err(|error| staging_io_error(parent, error))?;
            }
            move_with_rename_on_collision(&path, &target)?;
        }
    }
    Ok(())
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
/// waiting -- the caller should stop without transitioning again, the
/// registry having already recorded `Cancelled`. Mirrors
/// `archive_ops::run_open_archive`'s own password-challenge handling
/// exactly, so both operation kinds behave identically from a
/// frontend's perspective.
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

/// The `start_extract` background worker. Spawned via the application's
/// own runtime handle; runs until the operation reaches a terminal state
/// (`Completed`, `Cancelled`, or `Failed`).
///
/// `_cancel` is unused directly: every cancellation check in this worker
/// goes through `inner.operations().is_cancelled(operation_id)` (the
/// registry's own record, which `ArclainApp::cancel_operation` sets
/// through the exact same path regardless of which flag a caller might
/// otherwise inspect), never this raw flag -- mirrors
/// `archive_ops::run_open_archive`'s identical parameter, kept for
/// signature symmetry with `OperationRegistry::begin`'s return value
/// rather than dropped, since a future direct-flag optimization (skipping
/// the registry lock on the hot poll path) would want it back in hand.
pub(crate) async fn run_extract(
    inner: Arc<AppRuntime>,
    operation_id: OperationId,
    _cancel: Arc<AtomicBool>,
    request: ExtractRequest,
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

    if let Err(error) = validate_destination(&request.destination) {
        fail(&inner, operation_id, error).await;
        return;
    }

    let selection = match resolve_selection(&session, &request) {
        Ok(selection) => selection,
        Err(error) => {
            fail(&inner, operation_id, error).await;
            return;
        }
    };

    let runner = inner.extract_runner();
    if !runner.tool_available() {
        fail(&inner, operation_id, missing_tool_error()).await;
        return;
    }

    let resolution =
        match resolve_collisions(&inner, operation_id, &request, &session, selection).await {
            Ok(Some(resolution)) => resolution,
            Ok(None) => return,
            Err(error) => {
                fail(&inner, operation_id, error).await;
                return;
            }
        };

    // An empty post-filter selection -- every candidate collided under
    // `Skip`, the user declined `Ask` and every remaining candidate was
    // itself colliding, or the caller selected only an empty directory --
    // is a real, valid "nothing to do" outcome. Completing immediately
    // without ever invoking the runner is load-bearing, not an
    // optimization: `SevenZipRunner::spawn` hands an empty file list
    // straight to the CLI as no file-list argument at all, which 7z reads
    // as "no filter -- extract everything" (see its own doc comment).
    // Without this check, `Skip`/a declined `Ask` would silently invert
    // into an unfiltered whole-archive extraction, overwriting (via the
    // unconditional `-y`) the very files those policies exist to protect.
    if let ExtractSelection::Files(files) = &resolution.selection {
        if files.is_empty() {
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
    }

    let staging_guard = if resolution.needs_staging {
        match StagingDirGuard::create(staging_dir_path(&request.destination, operation_id)) {
            Ok(guard) => Some(guard),
            Err(error) => {
                fail(
                    &inner,
                    operation_id,
                    staging_io_error(&request.destination, error),
                )
                .await;
                return;
            }
        }
    } else {
        None
    };
    let extract_destination = staging_guard
        .as_ref()
        .map(|guard| guard.path().to_path_buf())
        .unwrap_or_else(|| request.destination.clone());

    let archive_name = session
        .source_path()
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| session.source_path().to_string_lossy().into_owned());
    let mut password = {
        let archive = session.archive_arc();
        let guard = archive.lock();
        guard.password_ref().map(str::to_string)
    };

    // A whole-archive selection is always exactly one chunk: it never
    // carries an explicit file-list argument at all (see
    // `SevenZipRunner::spawn`), so there is no command-line-length
    // concern for it. A `Files` selection splits into as many chunks as
    // needed to keep every single CLI invocation's file-list argument
    // comfortably under the command-line length limit -- see
    // `chunk_file_list`'s own doc comment. Each chunk runs to completion
    // (with its own password-retry loop) before the next one starts; the
    // whole sequence is one logical operation from a caller's
    // perspective, reporting one continuous 0-100% progress and exactly
    // one terminal event.
    let chunks: Vec<ExtractSelection> = match &resolution.selection {
        ExtractSelection::WholeArchive => vec![ExtractSelection::WholeArchive],
        ExtractSelection::Files(files) => chunk_file_list(files.clone())
            .into_iter()
            .map(ExtractSelection::Files)
            .collect(),
    };
    let total_chunks = chunks.len().max(1) as u64;

    for (chunk_index, chunk_selection) in chunks.into_iter().enumerate() {
        let chunk_index = chunk_index as u64;
        let mut attempt: u32 = 1;

        'retry: loop {
            if inner.operations().is_cancelled(operation_id).await {
                return;
            }

            let plan = ExtractPlan {
                source_path: session.source_path().to_path_buf(),
                destination: extract_destination.clone(),
                password: password.clone(),
                selection: chunk_selection.clone(),
            };

            let Some(handle) = inner.tokio_handle() else {
                return;
            };
            let runner_for_spawn = runner.clone();
            let spawn_result = handle
                .spawn_blocking(move || runner_for_spawn.spawn(&plan))
                .await;
            let mut running = match spawn_result {
                Ok(Ok(running)) => running,
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
                            "extraction worker failed",
                        )
                        .with_diagnostic(join_error.to_string()),
                    )
                    .await;
                    return;
                }
            };

            let outcome = loop {
                if inner.operations().is_cancelled(operation_id).await {
                    running.kill();
                    return;
                }
                while let Some(progress) = running.poll_progress() {
                    let overall_percent =
                        (chunk_index * 100 + u64::from(progress.percent)) / total_chunks;
                    let _ = inner
                        .operations()
                        .transition(
                            operation_id,
                            OperationState::Progress {
                                completed_units: overall_percent,
                                total_units: Some(100),
                                message: progress.message,
                            },
                        )
                        .await;
                }
                if let Some(outcome) = running.poll_outcome() {
                    break outcome;
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            };

            match outcome {
                Ok(()) => break 'retry,
                Err(error) if error.kind == ApplicationErrorKind::PasswordRequired => {
                    match await_password_retry(&inner, operation_id, &archive_name, &mut attempt)
                        .await
                    {
                        Some(new_password) => {
                            password = Some(new_password);
                            continue 'retry;
                        }
                        None => return,
                    }
                }
                Err(error) => {
                    fail(&inner, operation_id, error).await;
                    return;
                }
            }
        }
    }

    if let Some(guard) = staging_guard {
        if let Err(error) = finalize_rename_policy(guard.path(), &request.destination) {
            fail(&inner, operation_id, error).await;
            return;
        }
    }

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collision_policy_serializes_snake_case() {
        assert_eq!(
            serde_json::to_value(CollisionPolicy::Ask).unwrap(),
            serde_json::json!("ask")
        );
        assert_eq!(
            serde_json::to_value(CollisionPolicy::Overwrite).unwrap(),
            serde_json::json!("overwrite")
        );
        assert_eq!(
            serde_json::to_value(CollisionPolicy::Rename).unwrap(),
            serde_json::json!("rename")
        );
        assert_eq!(
            serde_json::to_value(CollisionPolicy::Skip).unwrap(),
            serde_json::json!("skip")
        );
    }

    #[test]
    fn is_password_error_recognizes_known_shapes() {
        assert!(is_password_error("Wrong password for archive"));
        assert!(is_password_error("process exited with code Some(2)"));
        assert!(!is_password_error("disk read error"));
    }

    #[test]
    fn validate_destination_rejects_relative_paths() {
        let error = validate_destination(Path::new("relative/dest")).unwrap_err();
        assert_eq!(error.kind, ApplicationErrorKind::InvalidInput);
    }

    #[test]
    fn validate_destination_rejects_a_path_that_is_an_existing_file() {
        let temp = tempfile::NamedTempFile::new().unwrap();
        let error = validate_destination(temp.path()).unwrap_err();
        assert_eq!(error.kind, ApplicationErrorKind::InvalidInput);
    }

    #[test]
    fn validate_destination_accepts_an_absolute_directory_that_does_not_exist_yet() {
        let temp = tempfile::tempdir().unwrap();
        let not_yet_created = temp.path().join("nested").join("dest");
        assert!(validate_destination(&not_yet_created).is_ok());
    }

    #[test]
    fn move_with_rename_on_collision_renames_instead_of_replacing() {
        let temp = tempfile::tempdir().unwrap();
        let existing = temp.path().join("file.txt");
        std::fs::write(&existing, b"original").unwrap();
        let incoming = temp.path().join("incoming.txt");
        std::fs::write(&incoming, b"new content").unwrap();

        move_with_rename_on_collision(&incoming, &existing).unwrap();

        assert_eq!(std::fs::read(&existing).unwrap(), b"original");
        let renamed = temp.path().join("file (1).txt");
        assert_eq!(std::fs::read(&renamed).unwrap(), b"new content");
    }

    #[test]
    fn move_with_rename_on_collision_finds_the_next_free_suffix() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("file.txt"), b"a").unwrap();
        std::fs::write(temp.path().join("file (1).txt"), b"b").unwrap();
        let incoming = temp.path().join("incoming.txt");
        std::fs::write(&incoming, b"c").unwrap();

        move_with_rename_on_collision(&incoming, &temp.path().join("file.txt")).unwrap();

        assert_eq!(
            std::fs::read(temp.path().join("file (2).txt")).unwrap(),
            b"c"
        );
    }

    #[test]
    fn finalize_rename_policy_moves_nested_files_and_renames_on_collision() {
        let temp = tempfile::tempdir().unwrap();
        let staging = temp.path().join("staging");
        let destination = temp.path().join("dest");
        std::fs::create_dir_all(staging.join("nested")).unwrap();
        std::fs::create_dir_all(&destination).unwrap();
        std::fs::write(staging.join("top.txt"), b"top").unwrap();
        std::fs::write(staging.join("nested").join("inner.txt"), b"inner").unwrap();
        // Pre-existing collision at the destination.
        std::fs::write(destination.join("top.txt"), b"already here").unwrap();

        finalize_rename_policy(&staging, &destination).unwrap();

        assert_eq!(
            std::fs::read(destination.join("top.txt")).unwrap(),
            b"already here"
        );
        assert_eq!(
            std::fs::read(destination.join("top (1).txt")).unwrap(),
            b"top"
        );
        assert_eq!(
            std::fs::read(destination.join("nested").join("inner.txt")).unwrap(),
            b"inner"
        );
    }

    #[test]
    fn chunk_file_list_keeps_a_small_list_in_one_chunk() {
        let files = vec![
            "a.txt".to_string(),
            "b.txt".to_string(),
            "c.txt".to_string(),
        ];
        let chunks = chunk_file_list(files.clone());
        assert_eq!(chunks, vec![files]);
    }

    #[test]
    fn chunk_file_list_returns_no_chunks_for_an_empty_list() {
        assert_eq!(chunk_file_list(Vec::new()), Vec::<Vec<String>>::new());
    }

    #[test]
    fn chunk_file_list_splits_once_the_ceiling_is_exceeded() {
        // Each path is 1000 chars; 28 of them (28,028 with separators)
        // exceeds MAX_CHUNK_ARGS_CHARS (28,000), so the 28th must start a
        // new chunk rather than pushing the first over the ceiling.
        let files: Vec<String> = (0..40)
            .map(|i| format!("{i:04}-{}", "x".repeat(995)))
            .collect();
        let chunks = chunk_file_list(files.clone());

        assert!(chunks.len() > 1, "a list this large must split");
        for chunk in &chunks {
            let total: usize = chunk.iter().map(|f| f.len() + 1).sum();
            assert!(
                total <= MAX_CHUNK_ARGS_CHARS,
                "chunk total {total} exceeds the ceiling"
            );
        }
        // Every original file must appear exactly once across all chunks,
        // in original order, with none dropped or duplicated.
        let flattened: Vec<&String> = chunks.iter().flatten().collect();
        let original_refs: Vec<&String> = files.iter().collect();
        assert_eq!(flattened, original_refs);
    }

    #[test]
    fn chunk_file_list_gives_a_single_oversized_path_its_own_chunk() {
        let huge = "x".repeat(MAX_CHUNK_ARGS_CHARS + 500);
        let files = vec![
            "small.txt".to_string(),
            huge.clone(),
            "other.txt".to_string(),
        ];
        let chunks = chunk_file_list(files);

        assert!(
            chunks.iter().any(|chunk| chunk == &vec![huge.clone()]),
            "the oversized path must still be scheduled, alone, rather than dropped"
        );
    }

    #[test]
    fn candidate_and_colliding_paths_detect_only_real_collisions() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("exists.txt"), b"x").unwrap();
        let candidates = vec!["exists.txt".to_string(), "missing.txt".to_string()];

        let colliding = colliding_paths(temp.path(), &candidates);

        assert_eq!(colliding, vec!["exists.txt".to_string()]);
    }
}
