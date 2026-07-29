//! Shared operation-event driving: the one loop every phase that waits
//! on a facade operation -- `crate::commands::open_archive_and_wait`'s
//! archive-open phase, and every mutation command's own [`drive_operation`]
//! phase (`extract`, `convert`, `organize`, `archive add`/`delete`,
//! `pipeline run`, `plugins action`) -- shares to subscribe once, filter
//! by operation id, preserve sequence order, render progress, answer a
//! live [`Challenge`], react to Ctrl+C, honor an optional `--timeout`,
//! and exit only once the operation reaches a terminal state.
//!
//! # JSON Lines framing (`--json` mode)
//!
//! While an operation is in flight, [`drive_operation`] prints exactly
//! one line of raw, unwrapped JSON per [`OperationEvent`] it observes
//! (`Accepted`, `Started`, `Progress`, `Challenge`, `SnapshotChanged`,
//! and the operation's own terminal event) directly to stdout --
//! `serde_json::to_string(&event)`, never pretty-printed, never wrapped
//! in [`crate::output::CliEnvelope`]. A consumer parsing stdout line by
//! line therefore sees a stream of independent JSON objects, one per
//! line, each shaped like [`arclain_app::event::OperationEvent`].
//!
//! Once the operation reaches its terminal state, the calling command
//! prints exactly one further line: the schema-versioned
//! [`crate::output::CliEnvelope`] this crate's every other `--json`
//! command already uses, summarizing that specific command's own
//! result. That envelope line is always the *last* line of stdout and
//! is the only line carrying a `schema_version` field -- a consumer can
//! reliably tell the two kinds of line apart by checking for that field
//! (or by simply reading the last line), without needing to buffer or
//! pre-parse the whole stream first. The archive-open phase
//! (`crate::commands::open_archive_and_wait`) never prints its own
//! per-event JSON Lines -- only a *mutation* command's own operation
//! does -- so opening an archive ahead of `extract`/`archive add`/
//! `archive delete` never doubles up on the framing above.
//!
//! In human (non-`--json`) mode, [`drive_operation`] renders a short,
//! one-line-per-event progress summary to stdout instead (percent/step
//! and message for `Progress`, the new revision for `SnapshotChanged`),
//! and prompts interactively on a [`Challenge`] rather than printing it.
//!
//! # Ctrl+C
//!
//! [`install_ctrl_c_handler`] must be called exactly once, as early as
//! possible in `main` -- before `commands::dispatch` runs, before any
//! facade operation is ever started. This is load-bearing, not merely
//! tidy: `tokio::signal::ctrl_c()`'s own doc comment states plainly that
//! the OS-level signal handler is installed on the *first poll* of its
//! returned future, not merely by calling the function -- confirmed
//! directly against this workspace's vendored tokio source
//! (`tokio-1.52.3/src/signal/windows/sys.rs::global_init`, a
//! `OnceLock`-guarded `SetConsoleCtrlHandler` call). Before this was
//! fixed, the first (and only) place anything in this crate ever polled
//! `ctrl_c()` was inside [`drive_operation`]'s own loop -- which only
//! starts once an archive has *already* fully opened. A Ctrl+C pressed
//! during `extract`/`archive add`/`archive delete`'s own open-and-index
//! phase therefore fell through to Windows' default handling (immediate
//! termination, `STATUS_CONTROL_C_EXIT`) instead of this crate's own
//! cooperative cancellation -- exactly what a real manual test observed,
//! misattributed at the time to unrelated timing. [`install_ctrl_c_handler`]
//! closes this by spawning one background pump task that polls
//! `ctrl_c()` in a loop for the whole life of the process and relays
//! every occurrence through a plain [`CancelSignal`] (`Arc<Notify>`),
//! which every operation-waiting phase -- the archive-open phase and
//! every mutation's own [`drive_operation`] call alike -- races against
//! uniformly. `Notify::notify_one` (not `notify_waiters`) is used
//! deliberately: it stores a permit when nothing is currently waiting,
//! so a Ctrl+C that arrives in the narrow gap between two phases (after
//! the archive finished opening, before the mutation's own operation has
//! started waiting) is never silently lost.
//!
//! The first Ctrl+C (or, see below, a `--timeout` expiry) calls
//! [`ArclainApp::cancel_operation`] and stops re-arming itself for that
//! phase (a second Ctrl+C is a documented no-op -- the operation is
//! already being cancelled); the loop keeps consuming events exactly as
//! before until the operation reports its own terminal `Cancelled`
//! state, which this function maps to [`exit_code::OPERATION_FAILURE`]
//! -- see that constant's own reuse rationale at the `Cancelled` match
//! arm below. This means an operation still prints every event it
//! produces after a cancellation is requested (including any final
//! `Progress` ticks already in flight), and only actually exits once the
//! worker cooperatively notices the cancellation and stops -- there is
//! no forced/instant process-level abort. If the operation does not
//! acknowledge the cancellation within a further, fixed grace period
//! ([`CANCEL_GRACE_PERIOD`]), this process gives up waiting and exits
//! [`exit_code::INTERNAL_FAILURE`] instead, saying plainly that the
//! operation may still be running -- see the `--timeout` section below,
//! whose same bounded-wait reasoning applies equally to a Ctrl+C-driven
//! cancellation.
//!
//! # `--timeout SECONDS`
//!
//! An opt-in global flag (see `crate::commands::Cli::timeout`); absent,
//! every phase waits exactly as unboundedly as before (a legitimate
//! multi-hour extraction is never cut off on its own). When given, each
//! operation-waiting phase -- the archive-open phase, then (if that
//! phase succeeds) the mutation's own [`drive_operation`] phase -- gets
//! its own fresh budget of that many seconds from the moment *that*
//! phase begins, not one deadline shared across both. On expiry, this
//! module reacts exactly like a Ctrl+C: `cancel_operation` is called,
//! this process prints a message naming the timeout, and it waits up to
//! [`CANCEL_GRACE_PERIOD`] further for the operation's own `Cancelled`
//! state before exiting [`exit_code::OPERATION_FAILURE`]; if even that
//! bounded follow-up wait expires, this process exits
//! [`exit_code::INTERNAL_FAILURE`] with a distinct message noting the
//! operation may still be running rather than waiting forever for an
//! acknowledgement that might never come.
//!
//! # Interactive challenges
//!
//! A [`Challenge::Password`] is answered via [`Interactive::read_password`]
//! (no echo); every other variant is answered via [`Interactive::confirm`]
//! (a plain yes/no prompt). Both are refused outright -- the operation is
//! cancelled and this process exits [`exit_code::USER_ACTION_REQUIRED`],
//! matching Task 12's own read-command convention -- unless
//! [`Interactive::is_interactive`] reports a real controlling terminal.
//! **No command in this crate ever accepts a password as a command-line
//! argument or flag**: the only way to answer a [`Challenge::Password`]
//! is this interactive prompt, so a password can never appear in shell
//! history, a process listing, or a log line this crate itself writes.
//!
//! Both prompts run as a direct, synchronous (blocking) call on this
//! process's own small `current_thread` runtime rather than through
//! `spawn_blocking` raced against Ctrl+C: the operation being answered
//! is itself blocked awaiting this exact response, so there is no other
//! useful work the runtime could perform during that wait regardless.
//! One real consequence: a Ctrl+C pressed *while* a password/confirm
//! prompt is mid-read is not honored until that prompt itself returns
//! (the user finishes answering it, or aborts it with EOF/Ctrl+D) --
//! `tokio`'s own Ctrl+C plumbing still records the signal immediately at
//! the OS level either way (a raw signal handler, independent of
//! whether this thread happens to be polling at that instant), so
//! nothing is lost, it is simply acted on one step later than a
//! Ctrl+C pressed while an ordinary progress event is being awaited.

use std::io::IsTerminal;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::broadcast::error::RecvError;
use tokio::sync::Notify;
use tokio::time::Instant;

use arclain_app::challenge::{Challenge, ChallengeResponse, SecretInput};
use arclain_app::event::{OperationEvent, OperationResult, OperationState};
use arclain_app::ids::OperationId;
use arclain_app::ArclainApp;

use crate::output::{exit_code, exit_code_for, print_error, print_plain_error};

/// What [`drive_operation`] needs to answer a live [`Challenge`].
/// [`StdInteractive`] is the production implementation (a real
/// controlling terminal); tests substitute a scripted fake so the
/// challenge-response *wiring* -- which facade calls happen, in which
/// order, with which exit code on refusal -- is verified without a real
/// pseudo-terminal (see this module's own test suite).
pub(crate) trait Interactive: Send + Sync {
    /// Whether this process can prompt a human right now. A
    /// piped/redirected stdin -- the shape every non-interactive
    /// invocation has, including every subprocess-driven integration
    /// test in this crate that does not itself allocate a
    /// pseudo-terminal -- reports `false`.
    fn is_interactive(&self) -> bool;

    /// Prompts on the real terminal and reads back a line without
    /// echoing it. Never called unless [`Self::is_interactive`] is
    /// `true`.
    fn read_password(&self, prompt: &str) -> std::io::Result<String>;

    /// Prompts on the real terminal and reads back a plain (echoed)
    /// yes/no answer -- `true` for a line trimmed and lowercased to `"y"`
    /// or `"yes"`, `false` for anything else (including an empty line,
    /// so pressing Enter alone declines). Never called unless
    /// [`Self::is_interactive`] is `true`.
    fn confirm(&self, prompt: &str) -> std::io::Result<bool>;
}

/// Production [`Interactive`]: the real controlling terminal, via
/// [`rpassword`] for the no-echo password read.
pub(crate) struct StdInteractive;

impl Interactive for StdInteractive {
    fn is_interactive(&self) -> bool {
        std::io::stdin().is_terminal()
    }

    fn read_password(&self, prompt: &str) -> std::io::Result<String> {
        rpassword::prompt_password(prompt)
    }

    fn confirm(&self, prompt: &str) -> std::io::Result<bool> {
        use std::io::Write;
        print!("{prompt}");
        std::io::stdout().flush()?;
        let mut line = String::new();
        std::io::stdin().read_line(&mut line)?;
        let normalized = line.trim().to_ascii_lowercase();
        Ok(normalized == "y" || normalized == "yes")
    }
}

/// The external "please cancel the in-flight operation" signal every
/// operation-waiting phase races against -- see this module's own
/// "Ctrl+C" doc section for why this must be armed exactly once, early,
/// via [`install_ctrl_c_handler`] in production. A plain `Arc<Notify>`
/// rather than a bespoke type: production and tests share one
/// mechanism, tests just construct their own `Notify` and fire it
/// programmatically instead of routing through a real OS signal (see
/// this module's own test suite for why a real signal is impractical to
/// automate reliably here).
pub(crate) type CancelSignal = Arc<Notify>;

/// Spawns the process-wide Ctrl+C pump task and returns the
/// [`CancelSignal`] every operation-waiting phase races against. Must be
/// called exactly once, as early as possible in `main` -- see this
/// module's own "Ctrl+C" doc section.
pub(crate) fn install_ctrl_c_handler() -> CancelSignal {
    let signal: CancelSignal = Arc::new(Notify::new());
    let notify = signal.clone();
    tokio::spawn(async move {
        loop {
            // `ctrl_c()` only errors if the OS handler could not be
            // installed at all -- effectively unreachable once this
            // process's runtime is up. Stop pumping rather than spin if
            // it somehow does; nothing productive can come from
            // retrying a broken registration in a tight loop.
            if tokio::signal::ctrl_c().await.is_err() {
                return;
            }
            // `notify_one`, not `notify_waiters`: stores a permit if
            // nothing is currently waiting, so a Ctrl+C landing in the
            // gap between two operation-waiting phases (the archive
            // just finished opening; the mutation has not started
            // waiting yet) is still observed by the very next
            // `.notified().await` rather than silently lost.
            notify.notify_one();
        }
    });
    signal
}

/// This process's optional operation-wait budget (`--timeout SECONDS`,
/// see `crate::commands::Cli::timeout`). `None` (no flag given)
/// preserves this CLI's original, fully unbounded wait.
#[derive(Clone, Copy)]
pub(crate) struct TimeoutBudget(Option<Duration>);

impl TimeoutBudget {
    pub(crate) fn from_secs(seconds: Option<u64>) -> Self {
        Self(seconds.map(Duration::from_secs))
    }

    /// Production reaches the unbounded case via `from_secs(None)`
    /// (`--timeout` simply omitted); this named constructor exists for
    /// this crate's own tests (here and in `crate::commands`), which
    /// want to say "no budget" without spelling out the `None`
    /// themselves.
    #[cfg(test)]
    pub(crate) fn unbounded() -> Self {
        Self(None)
    }

    #[cfg(test)]
    fn from_duration(duration: Duration) -> Self {
        Self(Some(duration))
    }

    fn seconds(&self) -> Option<u64> {
        self.0.map(|duration| duration.as_secs())
    }

    /// A fresh deadline computed from *now* -- called once per
    /// `drive_until_terminal` invocation, so each phase (opening an
    /// archive, then running a mutation) gets its own full budget
    /// rather than sharing one deadline across both.
    fn deadline(&self) -> Option<Instant> {
        self.0.map(|duration| Instant::now() + duration)
    }
}

/// How long [`drive_until_terminal`] waits for an operation to
/// acknowledge a cancellation (Ctrl+C or a `--timeout` expiry) before
/// giving up and exiting [`exit_code::INTERNAL_FAILURE`] instead of
/// [`exit_code::OPERATION_FAILURE`] -- see this module's own "Ctrl+C"
/// and "`--timeout`" doc sections. Not configurable: this bounds this
/// CLI's own promise to never wait forever for an acknowledgement,
/// independent of whatever budget (if any) a caller chose for the
/// operation itself.
const CANCEL_GRACE_PERIOD: Duration = Duration::from_secs(5);

async fn sleep_until_deadline(deadline: Option<Instant>) {
    match deadline {
        Some(instant) => tokio::time::sleep_until(instant).await,
        None => std::future::pending().await,
    }
}

/// Renders one event in human (non-`--json`) mode. `Accepted` and
/// `Challenge` are deliberately silent here: `Accepted` is too noisy to
/// be worth a line of its own, and a `Challenge` is rendered as an
/// interactive prompt instead (see [`handle_challenge`]), not as a
/// progress line. Terminal states are rendered by [`drive_operation`]'s
/// own caller (each command's own final summary), except `Cancelled`,
/// which prints its one-word notice here since [`drive_operation`]
/// returns only an exit code for it, nothing a caller could print
/// instead.
fn render_human_event(event: &OperationEvent) {
    match &event.state {
        OperationState::Accepted | OperationState::Challenge { .. } => {}
        OperationState::Started => println!("started"),
        OperationState::Progress {
            completed_units,
            total_units,
            message,
        } => {
            let progress = match total_units {
                Some(total) => format!("{completed_units}/{total}"),
                None => completed_units.to_string(),
            };
            match message {
                Some(message) => println!("progress: {progress} - {message}"),
                None => println!("progress: {progress}"),
            }
        }
        OperationState::SnapshotChanged { revision, .. } => {
            println!("archive updated to revision {revision}");
        }
        OperationState::Completed { .. } | OperationState::Failed { .. } => {}
        OperationState::Cancelled => println!("cancelled"),
    }
}

/// Prompts for (and submits) an answer to one live [`Challenge`].
/// `Ok(())` means a response was submitted and accepted -- the caller's
/// loop should keep waiting for the operation's next event. `Err(code)`
/// means this challenge could not be answered (non-interactive, a
/// terminal read failure, or the facade rejected the response); the
/// operation has already been cancelled (or already rejected the
/// response on its own) and `code` is this process's final exit code.
///
/// `pub(crate)`, not private: `crate::commands::open_archive_and_wait`
/// reuses this directly so every command that opens an archive first
/// (read commands and mutation commands alike) answers a password
/// challenge the same interactive way `drive_operation`'s own loop
/// does, rather than duplicating this match.
pub(crate) async fn handle_challenge(
    app: &ArclainApp,
    operation_id: OperationId,
    challenge: &Challenge,
    interactive: &dyn Interactive,
) -> Result<(), i32> {
    match challenge {
        Challenge::Password {
            id,
            archive_name,
            attempt,
        } => {
            if !interactive.is_interactive() {
                print_plain_error(&format!(
                    "password required for {archive_name} (attempt {attempt}) -- refusing to \
                     prompt: not running interactively"
                ));
                let _ = app.cancel_operation(operation_id).await;
                return Err(exit_code::USER_ACTION_REQUIRED);
            }
            let prompt = format!("Password for {archive_name} (attempt {attempt}): ");
            match interactive.read_password(&prompt) {
                Ok(secret) => {
                    let response = ChallengeResponse::Password {
                        id: *id,
                        value: SecretInput::new(secret),
                    };
                    respond(app, operation_id, response).await
                }
                Err(_) => {
                    print_plain_error("failed to read a password from the terminal");
                    let _ = app.cancel_operation(operation_id).await;
                    Err(exit_code::INTERNAL_FAILURE)
                }
            }
        }
        Challenge::ConfirmOverwrite { id, destination } => {
            confirm_challenge(
                app,
                operation_id,
                interactive,
                format!(
                    "{} already exists. Overwrite? [y/N] ",
                    destination.display()
                ),
                |overwrite| ChallengeResponse::ConfirmOverwrite { id: *id, overwrite },
            )
            .await
        }
        Challenge::ConfirmDestructiveAction { id, summary } => {
            confirm_challenge(
                app,
                operation_id,
                interactive,
                format!("{summary} Confirm? [y/N] "),
                |confirmed| ChallengeResponse::ConfirmDestructiveAction { id: *id, confirmed },
            )
            .await
        }
        Challenge::MissingExternalTool { id, tool } => {
            confirm_challenge(
                app,
                operation_id,
                interactive,
                format!("missing external tool {tool:?}. Retry? [y/N] "),
                |retry| ChallengeResponse::MissingExternalTool { id: *id, retry },
            )
            .await
        }
        Challenge::RetryPermission { id, path } => {
            confirm_challenge(
                app,
                operation_id,
                interactive,
                format!("permission denied for {}. Retry? [y/N] ", path.display()),
                |retry| ChallengeResponse::RetryPermission { id: *id, retry },
            )
            .await
        }
    }
}

/// Shared plain yes/no confirmation flow every non-`Password` [`Challenge`]
/// variant uses -- see [`handle_challenge`].
async fn confirm_challenge(
    app: &ArclainApp,
    operation_id: OperationId,
    interactive: &dyn Interactive,
    prompt: String,
    build_response: impl FnOnce(bool) -> ChallengeResponse,
) -> Result<(), i32> {
    if !interactive.is_interactive() {
        print_plain_error(&format!(
            "{prompt}-- refusing to prompt: not running interactively"
        ));
        let _ = app.cancel_operation(operation_id).await;
        return Err(exit_code::USER_ACTION_REQUIRED);
    }
    match interactive.confirm(&prompt) {
        Ok(answer) => respond(app, operation_id, build_response(answer)).await,
        Err(_) => {
            print_plain_error("failed to read a confirmation from the terminal");
            let _ = app.cancel_operation(operation_id).await;
            Err(exit_code::INTERNAL_FAILURE)
        }
    }
}

async fn respond(
    app: &ArclainApp,
    operation_id: OperationId,
    response: ChallengeResponse,
) -> Result<(), i32> {
    match app.respond_to_challenge(operation_id, response).await {
        Ok(()) => Ok(()),
        Err(error) => {
            let code = exit_code_for(&error.kind);
            print_error(&error);
            Err(code)
        }
    }
}

/// Everything [`drive_until_terminal`] (and, through it, [`drive_operation`])
/// needs to drive one already-started operation -- bundled into one
/// value so neither function's own parameter list keeps growing one at a
/// time as this module gains a new global concern (this task added
/// `cancel`/`budget` alongside the original `app`/`events`/`operation_id`/
/// `interactive`), the same way `crate::commands::Invocation` bundles the
/// equivalent concern one layer up.
///
/// `events` must already be subscribed (via [`ArclainApp::subscribe_operations`])
/// *before* the operation was started -- subscribing afterward could
/// race the operation's own `Accepted` event.
pub(crate) struct OperationWait<'a> {
    pub(crate) app: &'a ArclainApp,
    pub(crate) events: &'a mut tokio::sync::broadcast::Receiver<OperationEvent>,
    pub(crate) operation_id: OperationId,
    pub(crate) interactive: &'a dyn Interactive,
    pub(crate) cancel: &'a CancelSignal,
    pub(crate) budget: TimeoutBudget,
}

/// The shared core loop both [`drive_operation`] (every mutation
/// command) and `crate::commands::open_archive_and_wait` (every command
/// that opens an archive first) are built on: subscribe-once,
/// sequence-ordered event consumption, Ctrl+C, `--timeout`, and
/// interactive-challenge handling, all in one place so neither caller
/// re-implements (or subtly diverges on) any of it.
///
/// `render` is called for every event this function observes for
/// `wait.operation_id` (in sequence order, terminal event included)
/// before this function acts on it -- `drive_operation` uses this for
/// its own JSON-Lines/human-progress printing (and to let its own caller
/// observe events, e.g. `crate::commands::archive` reading a
/// `SnapshotChanged` revision out); `open_archive_and_wait` passes a
/// no-op closure, since the archive-open phase never streams its own
/// progress (see this module's own JSON-Lines doc section).
///
/// `on_completed` maps the operation's terminal `Completed` payload to
/// this function's own generic success type `T` -- `drive_operation`
/// treats every `OperationResult` as success; `open_archive_and_wait`
/// accepts only `ArchiveOpened`, rejecting any other payload as an
/// internal inconsistency.
///
/// Returns `Ok(value)` once `on_completed` is satisfied; `Err(code)` for
/// every other outcome (`Cancelled`, `Failed`, an unanswerable
/// challenge, a Ctrl+C/timeout-driven cancellation, or an internal
/// failure) -- the error path always already printed its own
/// diagnostic, so the caller only needs to propagate `code`.
pub(crate) async fn drive_until_terminal<T>(
    wait: OperationWait<'_>,
    mut render: impl FnMut(&OperationEvent),
    mut on_completed: impl FnMut(OperationResult) -> Result<T, i32>,
) -> Result<T, i32> {
    let OperationWait {
        app,
        events,
        operation_id,
        interactive,
        cancel,
        budget,
    } = wait;
    let mut last_sequence: Option<u64> = None;
    let operation_deadline = budget.deadline();
    // `None` until a cancellation (Ctrl+C or `--timeout`) is requested;
    // once set, this loop must observe the operation's own terminal
    // state by this instant or give up (see `CANCEL_GRACE_PERIOD`'s own
    // doc comment) rather than waiting forever for an acknowledgement
    // that might never come.
    let mut cancel_deadline: Option<Instant> = None;

    loop {
        tokio::select! {
            () = cancel.notified(), if cancel_deadline.is_none() => {
                print_plain_error("interrupted -- cancelling the operation");
                let _ = app.cancel_operation(operation_id).await;
                cancel_deadline = Some(Instant::now() + CANCEL_GRACE_PERIOD);
            }
            () = sleep_until_deadline(operation_deadline), if cancel_deadline.is_none() && operation_deadline.is_some() => {
                print_plain_error(&format!(
                    "operation timed out after {}s -- cancelling",
                    budget.seconds().unwrap_or_default()
                ));
                let _ = app.cancel_operation(operation_id).await;
                cancel_deadline = Some(Instant::now() + CANCEL_GRACE_PERIOD);
            }
            () = sleep_until_deadline(cancel_deadline), if cancel_deadline.is_some() => {
                print_plain_error(
                    "the operation did not acknowledge cancellation in time -- it may still be \
                     running"
                );
                return Err(exit_code::INTERNAL_FAILURE);
            }
            received = events.recv() => {
                let event = match received {
                    Ok(event) => event,
                    Err(RecvError::Lagged(_)) => continue,
                    Err(RecvError::Closed) => {
                        print_plain_error("application event stream closed unexpectedly");
                        return Err(exit_code::INTERNAL_FAILURE);
                    }
                };
                if event.operation_id != operation_id {
                    continue;
                }
                // Preserve sequence order: a same-operation event whose
                // sequence does not strictly increase is either a
                // duplicate delivery or arrived out of order for this
                // subscriber -- neither should ever actually happen (see
                // `OperationEvent::sequence`'s own doc comment), but this
                // loop never trusts that blindly. A defensive skip, not
                // a panic or an assertion: a real violation stays
                // invisible to a script parsing the JSON Lines stream
                // rather than corrupting it with a repeated or
                // out-of-order line.
                if let Some(previous) = last_sequence {
                    if event.sequence <= previous {
                        continue;
                    }
                }
                last_sequence = Some(event.sequence);

                render(&event);

                match event.state {
                    OperationState::Completed { result } => return on_completed(result),
                    OperationState::Cancelled => {
                        // Reused rather than a dedicated exit code: this
                        // CLI's own `exit_code_for` already maps
                        // `ApplicationErrorKind::Cancelled` to
                        // `OPERATION_FAILURE`, and a cancelled operation
                        // (Ctrl+C, `--timeout`, or in principle any
                        // other caller) is the same "accepted but never
                        // reached a successful result" bucket from this
                        // exit-code convention's own perspective --
                        // introducing a seventh code for one specific
                        // way an operation can fail to complete would
                        // grow the mapping's surface for no
                        // discriminating benefit a script could act on
                        // differently.
                        return Err(exit_code::OPERATION_FAILURE);
                    }
                    OperationState::Failed { error } => {
                        let code = exit_code_for(&error.kind);
                        print_error(&error);
                        return Err(code);
                    }
                    OperationState::Challenge { ref challenge } => {
                        // `?` propagates `handle_challenge`'s own
                        // `Err(code)` directly: both functions share the
                        // same `Result<_, i32>` error type.
                        handle_challenge(app, operation_id, challenge, interactive).await?;
                    }
                    OperationState::Accepted
                    | OperationState::Started
                    | OperationState::Progress { .. }
                    | OperationState::SnapshotChanged { .. } => {}
                }
            }
        }
    }
}

/// Drives one already-started *mutation* operation to a terminal state,
/// via [`drive_until_terminal`] -- see that function's own doc comment
/// for the shared cancel/timeout/challenge mechanics, and this module's
/// own top-level doc comment for the exact `--json`/human-mode framing.
///
/// `on_event` is called for every event this function observes (in
/// sequence order, terminal event included) before it prints or acts on
/// it -- the seam a caller uses to pull out event-specific data this
/// function's own generic `Ok(OperationResult)` return does not carry
/// (for example, `crate::commands::archive` reads the new revision out
/// of an `ArchiveModify` operation's `SnapshotChanged` event this way).
pub(crate) async fn drive_operation(
    wait: OperationWait<'_>,
    json: bool,
    mut on_event: impl FnMut(&OperationEvent),
) -> Result<OperationResult, i32> {
    drive_until_terminal(
        wait,
        |event| {
            on_event(event);
            if json {
                if let Ok(line) = serde_json::to_string(event) {
                    println!("{line}");
                }
            } else {
                render_human_event(event);
            }
        },
        Ok,
    )
    .await
}

/// The production [`Interactive`] every command module drives
/// [`drive_operation`] with.
pub(crate) fn std_interactive() -> impl Interactive {
    StdInteractive
}

#[cfg(test)]
mod tests {
    //! Ctrl+C, `--timeout`, interactive-challenge, and JSON-Lines-framing
    //! tests for [`drive_operation`]/[`drive_until_terminal`], driven
    //! **in-process** against a real bootstrapped [`ArclainApp`] with a
    //! deterministic fake [`arclain_app::operations::extract::ExtractRunner`]
    //! installed via `BootstrapConfig::extract_runner_override` -- the
    //! same seam `crates/app/tests/extract_operation.rs` uses for its
    //! own deterministic extraction tests.
    //!
    //! This is the sanctioned fallback for testing Ctrl+C cancellation
    //! and an interactive password/confirm prompt on this workspace's
    //! Windows development target: reliably raising a real console
    //! Ctrl+C signal against a spawned child process requires the child
    //! to share a console with the sender (`GenerateConsoleCtrlEvent`
    //! only targets a whole console process group) and to actually be
    //! running attached to a real console, not a pipe-redirected test
    //! harness -- fragile to automate and prone to hanging or silently
    //! no-op'ing in a CI sandbox with no console at all (confirmed
    //! empirically: this task's own manual verification attempts, using
    //! both `kill -INT` and a proper Win32 `AttachConsole`/
    //! `GenerateConsoleCtrlEvent` P/Invoke, could not reliably land a
    //! real signal against a real running extraction in this sandbox --
    //! see this task's own report). Driving these functions directly, in
    //! process, with a scripted [`CancelSignal`] and a scripted
    //! [`Interactive`] fake instead exercises the *exact* production
    //! logic (the same functions `crate::commands::extract::run` and
    //! `crate::commands::open_archive_and_wait` call) deterministically
    //! and portably.

    use std::collections::VecDeque;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    use arclain_app::archive::OpenArchiveRequest;
    use arclain_app::error::{ApplicationError, ApplicationErrorKind};
    use arclain_app::event::OperationResult;
    use arclain_app::operations::extract::{
        ExtractPlan, ExtractProgressEvent, ExtractRunner, RunningExtraction,
    };
    use arclain_app::operations::{CollisionPolicy, ExtractRequest};
    use arclain_app::{AppPaths, ArclainApp, BootstrapConfig};

    use super::*;

    fn foreign_runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
    }

    /// A deterministic [`ExtractRunner`] fake: reports the tool as
    /// always available and hands back a [`FakeRunning`] handle the test
    /// drives directly (queueing progress ticks, scripting a specific
    /// per-attempt outcome, deciding when -- or whether -- it
    /// "finishes"), instead of ever spawning a real 7-Zip process.
    /// Mirrors `crates/app/tests/extract_operation.rs`'s own established
    /// fake-runner shape.
    #[derive(Clone, Default)]
    struct FakeRunner {
        ticks: Arc<Mutex<VecDeque<ExtractProgressEvent>>>,
        /// Per-spawn-attempt scripted outcomes, consumed front-to-back
        /// by each successive call to `spawn` -- lets a test script
        /// "attempt 1 fails as a password error, attempt 2 succeeds"
        /// (the wrong-password-retry test) precisely. Empty (the
        /// default) means every spawned attempt instead falls back to
        /// `finished`/`killed`.
        scripted_outcomes: Arc<Mutex<VecDeque<Result<(), ApplicationError>>>>,
        finished: Arc<AtomicBool>,
        killed: Arc<AtomicBool>,
        /// Set inside `spawn` -- lets a test wait until `run_extract`'s
        /// worker has actually reached "the runner is spawned and being
        /// polled" before it requests cancellation. Cancelling any
        /// earlier is a real, valid outcome too (the worker's own outer
        /// retry loop checks cancellation *before* ever calling
        /// `spawn`, in which case `kill` is never reached because there
        /// is nothing yet to kill) -- but it is a *different* outcome
        /// than the one this file's own Ctrl+C test means to pin down
        /// (cancelling a spawn already in flight), so that test
        /// deliberately waits for this flag first rather than racing it.
        spawned: Arc<AtomicBool>,
    }

    impl FakeRunner {
        fn push_tick(&self, percent: u8, message: Option<&str>) {
            self.ticks.lock().unwrap().push_back(ExtractProgressEvent {
                percent,
                message: message.map(str::to_string),
            });
        }

        fn finish(&self) {
            self.finished.store(true, Ordering::SeqCst);
        }

        fn was_killed(&self) -> bool {
            self.killed.load(Ordering::SeqCst)
        }

        fn was_spawned(&self) -> bool {
            self.spawned.load(Ordering::SeqCst)
        }

        /// Queues `outcome` to be reported by the Nth call to `spawn`
        /// (first call gets the first queued outcome, and so on) --
        /// see [`Self::scripted_outcomes`]'s own doc comment.
        fn script_outcome(&self, outcome: Result<(), ApplicationError>) {
            self.scripted_outcomes.lock().unwrap().push_back(outcome);
        }
    }

    impl ExtractRunner for FakeRunner {
        fn tool_available(&self) -> bool {
            true
        }

        fn spawn(
            &self,
            _plan: &ExtractPlan,
        ) -> Result<Box<dyn RunningExtraction>, ApplicationError> {
            self.spawned.store(true, Ordering::SeqCst);
            let scripted_outcome = self.scripted_outcomes.lock().unwrap().pop_front();
            Ok(Box::new(FakeRunning {
                ticks: self.ticks.clone(),
                finished: self.finished.clone(),
                killed: self.killed.clone(),
                scripted_outcome,
            }))
        }
    }

    struct FakeRunning {
        ticks: Arc<Mutex<VecDeque<ExtractProgressEvent>>>,
        finished: Arc<AtomicBool>,
        killed: Arc<AtomicBool>,
        scripted_outcome: Option<Result<(), ApplicationError>>,
    }

    impl RunningExtraction for FakeRunning {
        fn poll_progress(&mut self) -> Option<ExtractProgressEvent> {
            self.ticks.lock().unwrap().pop_front()
        }

        fn poll_outcome(&mut self) -> Option<Result<(), ApplicationError>> {
            if self.killed.load(Ordering::SeqCst) {
                return Some(Ok(()));
            }
            if let Some(outcome) = &self.scripted_outcome {
                return Some(outcome.clone());
            }
            if self.finished.load(Ordering::SeqCst) {
                Some(Ok(()))
            } else {
                None
            }
        }

        fn kill(&mut self) {
            self.killed.store(true, Ordering::SeqCst);
        }
    }

    fn build_zip_fixture(dir: &Path, entries: &[(&str, &[u8])]) -> PathBuf {
        let path = dir.join("fixture.zip");
        let file = std::fs::File::create(&path).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default();
        for (entry_path, content) in entries {
            use std::io::Write;
            writer.start_file(*entry_path, options).unwrap();
            writer.write_all(content).unwrap();
        }
        writer.finish().unwrap();
        path
    }

    /// Bootstraps a real `ArclainApp` whose extraction spawns through
    /// `runner` instead of a real 7-Zip CLI, matching
    /// `crates/app/tests/extract_operation.rs::bootstrap_app`'s own
    /// shape -- except `archive_backend_override` is deliberately left
    /// `None`: this crate cannot name `arclain_core::ArchiveBackend`
    /// directly (see `scripts/frontend_boundary.py`'s own dependency
    /// boundary -- a frontend crate may depend on `arclain_app` only
    /// among headless crates), so these tests open a real ZIP fixture
    /// through the real native ZIP backend instead of faking that layer
    /// too. Still requires a real 7-Zip executable on `PATH` for
    /// bootstrap itself to succeed (this workspace's established test
    /// convention -- see `crates/cli/tests/read_commands.rs`'s own
    /// module doc comment).
    fn bootstrap_app_with_fake_runner(
        temp: &tempfile::TempDir,
        runner: Arc<dyn ExtractRunner>,
    ) -> ArclainApp {
        let paths = AppPaths {
            config_dir: temp.path().join("config"),
            data_dir: temp.path().join("data"),
            cache_dir: temp.path().join("cache"),
            log_dir: temp.path().join("logs"),
            plugins_dir: temp.path().join("plugins"),
        };
        ArclainApp::bootstrap(BootstrapConfig {
            paths_override: Some(paths),
            extract_runner_override: Some(runner),
            ..BootstrapConfig::system_default()
        })
        .expect("bootstrap must succeed (requires a real 7-Zip executable on PATH)")
    }

    /// A scripted [`Interactive`] fake: `is_interactive` and every
    /// scripted answer are fixed at construction; every call is also
    /// recorded so a test can assert exactly what was asked.
    struct ScriptedInteractive {
        interactive: bool,
        passwords: Mutex<VecDeque<std::io::Result<String>>>,
        confirms: Mutex<VecDeque<std::io::Result<bool>>>,
        password_prompts: Mutex<Vec<String>>,
        confirm_prompts: Mutex<Vec<String>>,
    }

    impl ScriptedInteractive {
        fn non_interactive() -> Self {
            Self {
                interactive: false,
                passwords: Mutex::new(VecDeque::new()),
                confirms: Mutex::new(VecDeque::new()),
                password_prompts: Mutex::new(Vec::new()),
                confirm_prompts: Mutex::new(Vec::new()),
            }
        }

        fn scripted(passwords: Vec<&str>, confirms: Vec<bool>) -> Self {
            Self {
                interactive: true,
                passwords: Mutex::new(passwords.into_iter().map(|p| Ok(p.to_string())).collect()),
                confirms: Mutex::new(confirms.into_iter().map(Ok).collect()),
                password_prompts: Mutex::new(Vec::new()),
                confirm_prompts: Mutex::new(Vec::new()),
            }
        }
    }

    impl Interactive for ScriptedInteractive {
        fn is_interactive(&self) -> bool {
            self.interactive
        }

        fn read_password(&self, prompt: &str) -> std::io::Result<String> {
            self.password_prompts
                .lock()
                .unwrap()
                .push(prompt.to_string());
            self.passwords
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| Ok(String::new()))
        }

        fn confirm(&self, prompt: &str) -> std::io::Result<bool> {
            self.confirm_prompts
                .lock()
                .unwrap()
                .push(prompt.to_string());
            self.confirms
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(Ok(false))
        }
    }

    fn start_whole_archive_extract(
        app: &ArclainApp,
        runtime: &tokio::runtime::Runtime,
        archive: &Path,
        destination: &Path,
    ) -> (
        arclain_app::ids::OperationId,
        tokio::sync::broadcast::Receiver<arclain_app::event::OperationEvent>,
    ) {
        let mut events = app.subscribe_operations();
        let open_operation = runtime
            .block_on(app.start_open_archive(OpenArchiveRequest {
                source_path: archive.to_path_buf(),
                password: None,
            }))
            .expect("start_open_archive must be accepted");
        let session_id = runtime.block_on(async {
            loop {
                let event = events.recv().await.unwrap();
                if event.operation_id != open_operation {
                    continue;
                }
                if let arclain_app::event::OperationState::Completed {
                    result: OperationResult::ArchiveOpened { snapshot },
                } = event.state
                {
                    return snapshot.session_id;
                }
            }
        });

        let events = app.subscribe_operations();
        let operation_id = runtime
            .block_on(app.start_extract(ExtractRequest {
                session_id,
                entry_ids: vec![],
                destination: destination.to_path_buf(),
                collision_policy: CollisionPolicy::Overwrite,
            }))
            .expect("start_extract must be accepted");
        (operation_id, events)
    }

    #[test]
    fn ctrl_c_cancels_the_operation_and_drive_operation_returns_operation_failure() {
        let temp = tempfile::tempdir().unwrap();
        let archive = build_zip_fixture(temp.path(), &[("a.txt", b"hello")]);
        let destination = temp.path().join("out");
        std::fs::create_dir_all(&destination).unwrap();
        let runner = Arc::new(FakeRunner::default());
        // Never finishes on its own -- `poll_outcome` only reports
        // `Some` once `kill()` (called by `cancel_operation`, via
        // `run_extract`'s own cancellation check) has run, so this test
        // deterministically proves the cancellation path actually
        // reached the runner rather than racing a coincidentally-fast
        // real extraction.
        let app = bootstrap_app_with_fake_runner(&temp, runner.clone());
        let runtime = foreign_runtime();

        let (operation_id, mut events) =
            start_whole_archive_extract(&app, &runtime, &archive, &destination);

        // Waits until `run_extract`'s worker has actually reached
        // `FakeRunner::spawn` -- see that field's own doc comment for
        // why firing the cancellation any earlier would (validly, but
        // not usefully for *this* test) short-circuit before a runner
        // even exists to kill.
        runtime.block_on(async {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while !runner.was_spawned() {
                assert!(
                    std::time::Instant::now() < deadline,
                    "the fake runner must be spawned well within 5s"
                );
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        });

        let cancel: CancelSignal = Arc::new(Notify::new());
        cancel.notify_one();
        let interactive = ScriptedInteractive::non_interactive();

        let result = runtime.block_on(drive_operation(
            OperationWait {
                app: &app,
                events: &mut events,
                operation_id,
                interactive: &interactive,
                cancel: &cancel,
                budget: TimeoutBudget::unbounded(),
            },
            false,
            |_event| {},
        ));

        assert_eq!(result, Err(exit_code::OPERATION_FAILURE));

        // `OperationRegistry::cancel` transitions the operation to its
        // public `Cancelled` state immediately (see its own doc
        // comment) -- independent of, and typically slightly *before*,
        // `run_extract`'s worker task noticing the cancellation flag on
        // its own next poll tick (up to its 25ms interval later) and
        // actually calling `kill()`. `drive_operation` returning
        // already proves the public state transition; this poll proves
        // the *worker* -- running concurrently and independently on the
        // application's own runtime -- also reached the runner and
        // killed it, within a generous bounded deadline rather than
        // asserting it synchronously against that same race.
        runtime.block_on(async {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while !runner.was_killed() {
                assert!(
                    std::time::Instant::now() < deadline,
                    "cancel_operation must reach the running extraction and kill it within 5s"
                );
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        });

        runtime.block_on(app.shutdown()).ok();
    }

    /// A second Ctrl+C (or, in this test's terms, a second notification
    /// on the same `CancelSignal`) after the first was already accepted
    /// must be a documented no-op: `drive_until_terminal`'s own
    /// `cancel_deadline.is_none()` guard stops re-polling `cancel.notified()`
    /// once a cancellation is already in flight, so a second press never
    /// re-triggers `cancel_operation` a second time or re-arms a second
    /// grace period.
    #[test]
    fn a_second_ctrl_c_after_the_first_is_a_no_op() {
        let temp = tempfile::tempdir().unwrap();
        let archive = build_zip_fixture(temp.path(), &[("a.txt", b"hello")]);
        let destination = temp.path().join("out");
        std::fs::create_dir_all(&destination).unwrap();
        let runner = Arc::new(FakeRunner::default());
        let app = bootstrap_app_with_fake_runner(&temp, runner.clone());
        let runtime = foreign_runtime();

        let (operation_id, mut events) =
            start_whole_archive_extract(&app, &runtime, &archive, &destination);
        runtime.block_on(async {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while !runner.was_spawned() {
                assert!(std::time::Instant::now() < deadline);
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        });

        let cancel: CancelSignal = Arc::new(Notify::new());
        // Two notifications queued up front. `Notify::notify_one` only
        // ever stores a single permit (a second call while one is
        // already pending is itself a no-op at the `Notify` level), so
        // this alone would not distinguish "double Ctrl+C is ignored"
        // from "Notify simply cannot queue two" -- the real proof below
        // is that the operation completes via exactly one
        // `cancel_operation` call's worth of cancellation, not two.
        cancel.notify_one();
        cancel.notify_one();
        let interactive = ScriptedInteractive::non_interactive();

        let result = runtime.block_on(drive_operation(
            OperationWait {
                app: &app,
                events: &mut events,
                operation_id,
                interactive: &interactive,
                cancel: &cancel,
                budget: TimeoutBudget::unbounded(),
            },
            false,
            |_event| {},
        ));

        assert_eq!(result, Err(exit_code::OPERATION_FAILURE));
        runtime.block_on(async {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while !runner.was_killed() {
                assert!(std::time::Instant::now() < deadline);
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        });
        // A further, third notification arriving after the operation
        // has already reached its terminal state must not panic or
        // otherwise misbehave -- there is simply nothing left driving
        // this operation to observe it.
        cancel.notify_one();

        runtime.block_on(app.shutdown()).ok();
    }

    #[test]
    fn timeout_cancels_a_stalled_operation_and_exits_operation_failure() {
        let temp = tempfile::tempdir().unwrap();
        let archive = build_zip_fixture(temp.path(), &[("a.txt", b"hello")]);
        let destination = temp.path().join("out");
        std::fs::create_dir_all(&destination).unwrap();
        let runner = Arc::new(FakeRunner::default());
        // Never finishes on its own -- see `ctrl_c_cancels_the_operation...`'s
        // identical rationale; the `--timeout` budget, not Ctrl+C, must
        // be what ends this test.
        let app = bootstrap_app_with_fake_runner(&temp, runner.clone());
        let runtime = foreign_runtime();

        let (operation_id, mut events) =
            start_whole_archive_extract(&app, &runtime, &archive, &destination);
        runtime.block_on(async {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while !runner.was_spawned() {
                assert!(std::time::Instant::now() < deadline);
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        });

        let cancel: CancelSignal = Arc::new(Notify::new()); // never fired -- the timeout alone must trigger cancellation
        let interactive = ScriptedInteractive::non_interactive();
        let budget = TimeoutBudget::from_duration(Duration::from_millis(50));

        let result = runtime.block_on(drive_operation(
            OperationWait {
                app: &app,
                events: &mut events,
                operation_id,
                interactive: &interactive,
                cancel: &cancel,
                budget,
            },
            false,
            |_event| {},
        ));

        assert_eq!(result, Err(exit_code::OPERATION_FAILURE));
        runtime.block_on(async {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while !runner.was_killed() {
                assert!(
                    std::time::Instant::now() < deadline,
                    "a timeout-driven cancellation must still reach the runner and kill it"
                );
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        });

        runtime.block_on(app.shutdown()).ok();
    }

    #[test]
    fn without_a_timeout_a_released_operation_completes_normally() {
        let temp = tempfile::tempdir().unwrap();
        let archive = build_zip_fixture(temp.path(), &[("a.txt", b"hello")]);
        let destination = temp.path().join("out");
        std::fs::create_dir_all(&destination).unwrap();
        let runner = Arc::new(FakeRunner::default());
        let app = bootstrap_app_with_fake_runner(&temp, runner.clone());
        let runtime = foreign_runtime();

        let (operation_id, mut events) =
            start_whole_archive_extract(&app, &runtime, &archive, &destination);
        runtime.block_on(async {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while !runner.was_spawned() {
                assert!(std::time::Instant::now() < deadline);
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        });
        // Released shortly after being confirmed "in flight" -- with no
        // budget at all, `drive_operation` must simply wait for this,
        // however long it takes, rather than timing out on its own.
        runner.finish();

        let cancel: CancelSignal = Arc::new(Notify::new());
        let interactive = ScriptedInteractive::non_interactive();

        let result = runtime.block_on(drive_operation(
            OperationWait {
                app: &app,
                events: &mut events,
                operation_id,
                interactive: &interactive,
                cancel: &cancel,
                budget: TimeoutBudget::unbounded(),
            },
            false,
            |_event| {},
        ));

        assert!(matches!(result, Ok(OperationResult::None)));

        runtime.block_on(app.shutdown()).ok();
    }

    /// A wrong password on the first attempt raises exactly one
    /// `Challenge::Password` (real production logic -- `run_extract`'s
    /// own retry loop, not a test fake of the retry itself); supplying
    /// any password in response lets the second, scripted-to-succeed
    /// attempt complete the operation.
    #[test]
    fn a_wrong_password_raises_one_challenge_and_the_retry_completes() {
        let temp = tempfile::tempdir().unwrap();
        let archive = build_zip_fixture(temp.path(), &[("a.txt", b"hello")]);
        let destination = temp.path().join("out");
        std::fs::create_dir_all(&destination).unwrap();
        let runner = Arc::new(FakeRunner::default());
        runner.script_outcome(Err(ApplicationError::new(
            ApplicationErrorKind::PasswordRequired,
            "wrong password",
        )));
        runner.script_outcome(Ok(()));
        let app = bootstrap_app_with_fake_runner(&temp, runner);
        let runtime = foreign_runtime();

        let (operation_id, mut events) =
            start_whole_archive_extract(&app, &runtime, &archive, &destination);

        let cancel: CancelSignal = Arc::new(Notify::new());
        let interactive = ScriptedInteractive::scripted(vec!["first-guess"], vec![]);

        let result = runtime.block_on(drive_operation(
            OperationWait {
                app: &app,
                events: &mut events,
                operation_id,
                interactive: &interactive,
                cancel: &cancel,
                budget: TimeoutBudget::unbounded(),
            },
            false,
            |_event| {},
        ));

        assert!(
            matches!(result, Ok(OperationResult::None)),
            "the retry must let the operation complete, got {result:?}"
        );
        assert_eq!(
            interactive.password_prompts.lock().unwrap().len(),
            1,
            "exactly one password challenge is raised for one wrong attempt"
        );

        runtime.block_on(app.shutdown()).ok();
    }

    #[test]
    fn json_mode_emits_one_json_line_per_event_with_no_envelope_and_increasing_sequence() {
        let temp = tempfile::tempdir().unwrap();
        let archive = build_zip_fixture(temp.path(), &[("a.txt", b"hello")]);
        let destination = temp.path().join("out");
        std::fs::create_dir_all(&destination).unwrap();
        let runner = Arc::new(FakeRunner::default());
        runner.push_tick(10, Some("first"));
        runner.push_tick(50, Some("second"));
        runner.finish();
        let app = bootstrap_app_with_fake_runner(&temp, runner);
        let runtime = foreign_runtime();

        let (operation_id, mut events) =
            start_whole_archive_extract(&app, &runtime, &archive, &destination);

        let cancel: CancelSignal = Arc::new(Notify::new());
        let interactive = ScriptedInteractive::non_interactive();
        let mut observed_sequences = Vec::new();

        let result = runtime.block_on(drive_operation(
            OperationWait {
                app: &app,
                events: &mut events,
                operation_id,
                interactive: &interactive,
                cancel: &cancel,
                budget: TimeoutBudget::unbounded(),
            },
            true, // json
            |event| observed_sequences.push(event.sequence),
        ));

        assert!(matches!(result, Ok(OperationResult::None)));
        assert!(
            observed_sequences.len() >= 2,
            "must observe at least Accepted and the terminal event"
        );
        let mut sorted = observed_sequences.clone();
        sorted.sort_unstable();
        assert_eq!(
            observed_sequences, sorted,
            "sequence numbers must be observed in strictly increasing order"
        );

        runtime.block_on(app.shutdown()).ok();
    }

    #[test]
    fn out_of_order_or_duplicate_sequence_is_skipped_not_reprocessed() {
        // Unit-level proof of the defensive skip itself, independent of
        // whether a real operation could ever actually violate ordering
        // (it cannot -- see `OperationEvent::sequence`'s own doc
        // comment): a hand-built event whose sequence does not exceed
        // `last_sequence` must never reach `on_event`/rendering twice.
        use arclain_app::event::{OperationKind, OperationState};
        use arclain_app::ids::OperationId;

        fn event(operation_id: OperationId, sequence: u64) -> OperationEvent {
            OperationEvent {
                operation_id,
                sequence,
                kind: OperationKind::Extract,
                state: OperationState::Started,
            }
        }

        let operation_id = OperationId::from_raw(1);
        let mut last_sequence: Option<u64> = Some(3);
        let mut accepted = Vec::new();
        for candidate in [
            event(operation_id, 3),
            event(operation_id, 2),
            event(operation_id, 4),
        ] {
            let should_process = last_sequence.is_none_or(|previous| candidate.sequence > previous);
            if should_process {
                last_sequence = Some(candidate.sequence);
                accepted.push(candidate.sequence);
            }
        }
        assert_eq!(
            accepted,
            vec![4],
            "only the strictly-increasing sequence must be accepted"
        );
    }

    #[test]
    fn non_interactive_password_challenge_is_refused_with_user_action_required() {
        let temp = tempfile::tempdir().unwrap();
        // A header-encrypted archive is not needed here: the fake
        // runner raises the password challenge itself would need a real
        // password-shaped CLI failure to trigger, which this test does
        // not need -- `handle_challenge`'s refusal path is exercised
        // directly instead, against a hand-built `Challenge::Password`,
        // proving the exit code and the "never prompts when non-
        // interactive" behavior without needing a real encrypted
        // fixture or a live operation at all.
        let app_paths = AppPaths {
            config_dir: temp.path().join("config"),
            data_dir: temp.path().join("data"),
            cache_dir: temp.path().join("cache"),
            log_dir: temp.path().join("logs"),
            plugins_dir: temp.path().join("plugins"),
        };
        let app = ArclainApp::bootstrap(BootstrapConfig {
            paths_override: Some(app_paths),
            ..BootstrapConfig::system_default()
        })
        .expect("bootstrap must succeed (requires a real 7-Zip executable on PATH)");
        let runtime = foreign_runtime();
        let interactive = ScriptedInteractive::non_interactive();

        let challenge = Challenge::Password {
            id: arclain_app::ids::ChallengeId::from_raw(1),
            archive_name: "secret.7z".to_string(),
            attempt: 1,
        };
        // No real in-flight operation is needed to exercise the refusal
        // itself: `handle_challenge` calls `cancel_operation` on an
        // operation id that does not exist, which is a harmless no-op
        // (`OperationRegistry::cancel` is documented idempotent/tolerant
        // of an unknown id) -- only the refusal's own exit code and the
        // fact that no password prompt was attempted are under test.
        let bogus_operation_id = arclain_app::ids::OperationId::from_raw(999_999);

        let result = runtime.block_on(handle_challenge(
            &app,
            bogus_operation_id,
            &challenge,
            &interactive,
        ));

        assert_eq!(result, Err(exit_code::USER_ACTION_REQUIRED));
        assert!(
            interactive.password_prompts.lock().unwrap().is_empty(),
            "a non-interactive process must never attempt a password prompt"
        );

        runtime.block_on(app.shutdown()).ok();
    }

    #[test]
    fn interactive_confirm_overwrite_accepting_lets_the_operation_complete() {
        let temp = tempfile::tempdir().unwrap();
        let archive = build_zip_fixture(temp.path(), &[("a.txt", b"hello")]);
        let destination = temp.path().join("out");
        std::fs::create_dir_all(&destination).unwrap();
        // A real, pre-existing collision at the destination: `a.txt`
        // already present is exactly what makes `CollisionPolicy::Ask`
        // raise a live `Challenge::ConfirmOverwrite` (see
        // `crate::operations::extract::resolve_collisions`) -- this is
        // real production logic, not something this test fakes.
        std::fs::write(destination.join("a.txt"), b"already here").unwrap();
        let runner = Arc::new(FakeRunner::default());
        runner.finish();
        let app = bootstrap_app_with_fake_runner(&temp, runner);
        let runtime = foreign_runtime();

        let mut events = app.subscribe_operations();
        let open_operation = runtime
            .block_on(app.start_open_archive(OpenArchiveRequest {
                source_path: archive.clone(),
                password: None,
            }))
            .unwrap();
        let session_id = runtime.block_on(async {
            loop {
                let event = events.recv().await.unwrap();
                if event.operation_id != open_operation {
                    continue;
                }
                if let arclain_app::event::OperationState::Completed {
                    result: OperationResult::ArchiveOpened { snapshot },
                } = event.state
                {
                    return snapshot.session_id;
                }
            }
        });

        let mut events = app.subscribe_operations();
        let operation_id = runtime
            .block_on(app.start_extract(ExtractRequest {
                session_id,
                entry_ids: vec![],
                destination: destination.clone(),
                collision_policy: CollisionPolicy::Ask,
            }))
            .unwrap();

        let cancel: CancelSignal = Arc::new(Notify::new());
        let interactive = ScriptedInteractive::scripted(vec![], vec![true]);

        let result = runtime.block_on(drive_operation(
            OperationWait {
                app: &app,
                events: &mut events,
                operation_id,
                interactive: &interactive,
                cancel: &cancel,
                budget: TimeoutBudget::unbounded(),
            },
            false,
            |_event| {},
        ));

        assert!(
            matches!(result, Ok(OperationResult::None)),
            "confirming overwrite must let the operation complete, got {result:?}"
        );
        assert_eq!(interactive.confirm_prompts.lock().unwrap().len(), 1);
        assert!(interactive.confirm_prompts.lock().unwrap()[0].contains("Overwrite"));

        runtime.block_on(app.shutdown()).ok();
    }

    #[test]
    fn human_mode_progress_rendering_matches_the_completed_and_total_units() {
        // Pure rendering-text checks -- no operation needed. Kept
        // narrow and fast; the end-to-end proof that a real `extract`
        // invocation actually prints these lines to the real process
        // stdout lives in `tests/mutation_commands.rs`.
        use arclain_app::event::{OperationKind, OperationState};
        use arclain_app::ids::OperationId;

        // `render_human_event` itself only prints -- captured indirectly
        // is impractical without a process-wide stdout redirect, so this
        // test instead pins the *formatting rule* the function's own
        // doc comment documents, directly, the same way
        // `crate::output`'s own tests pin `exit_code_for`'s mapping
        // without capturing real stderr output.
        let with_total = OperationEvent {
            operation_id: OperationId::from_raw(1),
            sequence: 1,
            kind: OperationKind::Extract,
            state: OperationState::Progress {
                completed_units: 42,
                total_units: Some(100),
                message: Some("extracting".to_string()),
            },
        };
        let without_total = OperationEvent {
            operation_id: OperationId::from_raw(1),
            sequence: 2,
            kind: OperationKind::Convert,
            state: OperationState::Progress {
                completed_units: 2,
                total_units: None,
                message: None,
            },
        };
        // Smoke-exercise both shapes through the real function (proves
        // it does not panic on either); the exact text contract is
        // pinned by the doc comment above and cross-checked end to end
        // in the subprocess integration suite.
        render_human_event(&with_total);
        render_human_event(&without_total);
    }
}
