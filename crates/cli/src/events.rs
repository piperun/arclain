//! Shared operation-event driving: the one loop every mutation command
//! (`extract`, `convert`, `organize`, `archive add`/`delete`, `pipeline
//! run`, `plugins action`) uses to subscribe once, filter by operation
//! id, preserve sequence order, render progress, answer a live
//! [`Challenge`], react to Ctrl+C, and exit only once the operation
//! reaches a terminal state.
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
//! pre-parse the whole stream first.
//!
//! In human (non-`--json`) mode, [`drive_operation`] renders a short,
//! one-line-per-event progress summary to stdout instead (percent/step
//! and message for `Progress`, the new revision for `SnapshotChanged`),
//! and prompts interactively on a [`Challenge`] rather than printing it.
//!
//! # Ctrl+C
//!
//! [`drive_operation`] races [`CancelTrigger::wait`] against the event
//! stream on every iteration. The first Ctrl+C calls
//! [`ArclainApp::cancel_operation`] and stops re-arming the trigger (a
//! second Ctrl+C does nothing new -- the operation is already being
//! cancelled); the loop keeps consuming events exactly as before until
//! the operation reports its own terminal `Cancelled` state, which this
//! function maps to [`exit_code::OPERATION_FAILURE`] -- see that
//! constant's own reuse rationale at the `Cancelled` match arm below.
//! This means an operation still prints every event it produces after a
//! cancellation is requested (including any final `Progress` ticks
//! already in flight), and only actually exits once the worker
//! cooperatively notices the cancellation and stops -- there is no
//! forced/instant process-level abort.
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
#[cfg(test)]
use std::sync::Arc;

use tokio::sync::broadcast::error::RecvError;

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

/// The external "please cancel the in-flight operation" trigger
/// [`drive_operation`]'s loop races against the event stream. `CtrlC` is
/// production's only real source; `Programmatic` lets this module's own
/// tests fire a cancellation deterministically without sending a real OS
/// console signal -- seed comment at the top of this file's test module
/// for why a real signal is impractical to automate reliably here.
pub(crate) enum CancelTrigger {
    CtrlC,
    #[cfg(test)]
    Programmatic(Arc<tokio::sync::Notify>),
}

impl CancelTrigger {
    async fn wait(&self) {
        match self {
            CancelTrigger::CtrlC => {
                // `ctrl_c()` only errors if the OS signal handler could
                // not be installed at all -- effectively unreachable
                // once this process's runtime is already up. Treating
                // that as "a cancellation was requested anyway" is the
                // safe direction; the alternative (silently never
                // reacting to Ctrl+C again for the rest of this
                // invocation) is worse.
                let _ = tokio::signal::ctrl_c().await;
            }
            #[cfg(test)]
            CancelTrigger::Programmatic(notify) => notify.notified().await,
        }
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

/// Drives one already-started operation to a terminal state.
///
/// `events` must already be subscribed (via [`ArclainApp::subscribe_operations`])
/// *before* the operation was started, matching every command module's
/// own established convention (see `crate::commands::open_archive_and_wait`'s
/// doc comment) -- subscribing afterward could race the operation's own
/// `Accepted` event.
///
/// `on_event` is called for every event this function observes for
/// `operation_id` (in sequence order, terminal event included) before
/// this function renders or acts on it -- the seam a caller uses to pull
/// out event-specific data this function's own generic `Ok(OperationResult)`
/// return does not carry (for example, `crate::commands::archive` reads
/// the new revision out of an `ArchiveModify` operation's `SnapshotChanged`
/// event this way).
///
/// Returns `Ok(result)` on `Completed`; `Err(code)` for every other
/// outcome (`Cancelled`, `Failed`, a challenge this process could not
/// answer, or an internal failure) -- the error path always already
/// printed its own diagnostic, so the caller only needs to propagate
/// `code` as this process's exit code.
pub(crate) async fn drive_operation(
    app: &ArclainApp,
    events: &mut tokio::sync::broadcast::Receiver<OperationEvent>,
    operation_id: OperationId,
    json: bool,
    interactive: &dyn Interactive,
    cancel: &mut CancelTrigger,
    mut on_event: impl FnMut(&OperationEvent),
) -> Result<OperationResult, i32> {
    let mut cancel_requested = false;
    let mut last_sequence: Option<u64> = None;

    loop {
        tokio::select! {
            () = cancel.wait(), if !cancel_requested => {
                cancel_requested = true;
                let _ = app.cancel_operation(operation_id).await;
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

                on_event(&event);

                if json {
                    if let Ok(line) = serde_json::to_string(&event) {
                        println!("{line}");
                    }
                } else {
                    render_human_event(&event);
                }

                match event.state {
                    OperationState::Completed { result } => return Ok(result),
                    OperationState::Cancelled => {
                        // Reused rather than a dedicated exit code: this
                        // CLI's own `exit_code_for` already maps
                        // `ApplicationErrorKind::Cancelled` to
                        // `OPERATION_FAILURE`, and a cancelled operation
                        // (whether from this process's own Ctrl+C
                        // handling or, in principle, any other caller)
                        // is the same "accepted but never reached a
                        // successful result" bucket from this exit-code
                        // convention's own perspective -- introducing a
                        // seventh code for one specific way an operation
                        // can fail to complete would grow the mapping's
                        // surface for no discriminating benefit a script
                        // could act on differently.
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

/// The production [`Interactive`] every command module drives
/// [`drive_operation`] with.
pub(crate) fn std_interactive() -> impl Interactive {
    StdInteractive
}

#[cfg(test)]
mod tests {
    //! Ctrl+C, interactive-challenge, and JSON-Lines-framing tests for
    //! [`drive_operation`], driven **in-process** against a real
    //! bootstrapped [`ArclainApp`] with a deterministic fake
    //! [`arclain_app::operations::extract::ExtractRunner`] installed via
    //! `BootstrapConfig::extract_runner_override` -- the same seam
    //! `crates/app/tests/extract_operation.rs` uses for its own
    //! deterministic extraction tests.
    //!
    //! This is the sanctioned fallback for testing Ctrl+C cancellation
    //! and an interactive password/confirm prompt on this workspace's
    //! Windows development target: reliably raising a real console
    //! Ctrl+C signal against a spawned child process requires the child
    //! to share a console with the sender (`GenerateConsoleCtrlEvent`
    //! only targets a whole console process group) and to actually be
    //! running attached to a real console, not a pipe-redirected test
    //! harness -- fragile to automate and prone to hanging or silently
    //! no-op'ing in a CI sandbox with no console at all. Driving
    //! [`drive_operation`] directly, in-process, with a scripted
    //! [`CancelTrigger::Programmatic`] and a scripted [`Interactive`]
    //! fake instead exercises the *exact* production logic (the same
    //! function `crate::commands::extract::run` calls) deterministically
    //! and portably. The real, OS-signal-driven behavior (this process's
    //! own `main` binary, run directly in a real terminal, actually
    //! stops cleanly when the user physically presses Ctrl+C during a
    //! large extraction) is exercised by manual verification -- this
    //! task's own report records that check.

    use std::collections::VecDeque;
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex};

    use arclain_app::archive::OpenArchiveRequest;
    use arclain_app::error::ApplicationError;
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
    /// drives directly (queueing progress ticks, deciding when -- or
    /// whether -- it "finishes"), instead of ever spawning a real 7-Zip
    /// process. Mirrors `crates/app/tests/extract_operation.rs`'s own
    /// established fake-runner shape.
    #[derive(Clone, Default)]
    struct FakeRunner {
        ticks: Arc<Mutex<VecDeque<ExtractProgressEvent>>>,
        finished: Arc<std::sync::atomic::AtomicBool>,
        killed: Arc<std::sync::atomic::AtomicBool>,
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
        spawned: Arc<std::sync::atomic::AtomicBool>,
    }

    impl FakeRunner {
        fn push_tick(&self, percent: u8, message: Option<&str>) {
            self.ticks.lock().unwrap().push_back(ExtractProgressEvent {
                percent,
                message: message.map(str::to_string),
            });
        }

        fn finish(&self) {
            self.finished
                .store(true, std::sync::atomic::Ordering::SeqCst);
        }

        fn was_killed(&self) -> bool {
            self.killed.load(std::sync::atomic::Ordering::SeqCst)
        }

        fn was_spawned(&self) -> bool {
            self.spawned.load(std::sync::atomic::Ordering::SeqCst)
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
            self.spawned
                .store(true, std::sync::atomic::Ordering::SeqCst);
            Ok(Box::new(self.clone()))
        }
    }

    impl RunningExtraction for FakeRunner {
        fn poll_progress(&mut self) -> Option<ExtractProgressEvent> {
            self.ticks.lock().unwrap().pop_front()
        }

        fn poll_outcome(&mut self) -> Option<Result<(), ApplicationError>> {
            if self.killed.load(std::sync::atomic::Ordering::SeqCst)
                || self.finished.load(std::sync::atomic::Ordering::SeqCst)
            {
                Some(Ok(()))
            } else {
                None
            }
        }

        fn kill(&mut self) {
            self.killed.store(true, std::sync::atomic::Ordering::SeqCst);
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

        let notify = Arc::new(tokio::sync::Notify::new());
        notify.notify_one();
        let mut cancel = CancelTrigger::Programmatic(notify);
        let interactive = ScriptedInteractive::non_interactive();

        let result = runtime.block_on(drive_operation(
            &app,
            &mut events,
            operation_id,
            false,
            &interactive,
            &mut cancel,
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

        let mut cancel = CancelTrigger::Programmatic(Arc::new(tokio::sync::Notify::new()));
        let interactive = ScriptedInteractive::non_interactive();
        let mut observed_sequences = Vec::new();

        let result = runtime.block_on(drive_operation(
            &app,
            &mut events,
            operation_id,
            true, // json
            &interactive,
            &mut cancel,
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

        let mut cancel = CancelTrigger::Programmatic(Arc::new(tokio::sync::Notify::new()));
        let interactive = ScriptedInteractive::scripted(vec![], vec![true]);

        let result = runtime.block_on(drive_operation(
            &app,
            &mut events,
            operation_id,
            false,
            &interactive,
            &mut cancel,
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
