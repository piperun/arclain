//! The CLI's argument surface (`Cli`/`Command`) and the shared
//! archive-open-and-wait helper every read command that opens an archive
//! (`inspect`, `list`) builds on.

pub mod inspect;
pub mod list;
pub mod profiles;

use std::path::Path;

use arclain_app::archive::{ArchiveSnapshot, OpenArchiveRequest};
use arclain_app::challenge::Challenge;
use arclain_app::event::{OperationResult, OperationState};
use arclain_app::ArclainApp;
use clap::{Parser, Subcommand};

use crate::output::{exit_code, exit_code_for, print_error, print_plain_error};

#[derive(Debug, Parser)]
#[command(
    name = "arclain-cli",
    version,
    about = "Read-only inspection of Arclain archives and organization profiles"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,

    /// Emit machine-readable JSON (a versioned `{schema_version, data}`
    /// envelope) instead of human-readable text. Valid for every
    /// subcommand; JSON always goes to stdout, diagnostics always go to
    /// stderr, in both modes.
    #[arg(long, global = true)]
    pub json: bool,

    /// Overrides every OS-conventional application directory with fresh
    /// subdirectories of DIR instead of the real user profile. Intended
    /// for tests and isolated/portable installs; the real profile is used
    /// when omitted.
    #[arg(long, global = true, value_name = "DIR")]
    pub config_dir: Option<std::path::PathBuf>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Show a summary of an archive: type, entry count, total size.
    Inspect(inspect::InspectArgs),
    /// List the entries of one directory within an archive.
    List(list::ListArgs),
    /// Inspect configured organization/output profiles.
    Profiles {
        #[command(subcommand)]
        command: profiles::ProfilesCommand,
    },
}

/// Runs the parsed `command` against a bootstrapped `app`, returning this
/// process's exit code.
pub async fn dispatch(app: &ArclainApp, command: &Command, json: bool) -> i32 {
    match command {
        Command::Inspect(args) => inspect::run(app, args, json).await,
        Command::List(args) => list::run(app, args, json).await,
        Command::Profiles { command } => profiles::dispatch(app, command, json).await,
    }
}

/// Validates `archive_path` locally, then starts opening it as a facade
/// operation and waits for a terminal state -- the shared first step
/// both `inspect` and `list` need before they can serve anything from the
/// resulting session.
///
/// Subscribes to the operation-event stream *before* calling
/// `start_open_archive`, matching `arclain_app`'s own integration-test
/// convention (see `crates/app/tests/archive_sessions.rs`): subscribing
/// after the call could race the operation's own `Accepted` event.
///
/// A challenge of any kind (only `Password` is reachable in practice for
/// an archive-open operation, but every variant is handled) cannot be
/// answered by this task's read commands -- interactive input is a later
/// task's scope -- so it cancels the operation and returns
/// `exit_code::USER_ACTION_REQUIRED` with a clear stderr message rather
/// than hanging or silently failing.
///
/// Returns the opened session's snapshot on success, or the process exit
/// code to use on any failure path (already printed to stderr).
pub(crate) async fn open_archive_and_wait(
    app: &ArclainApp,
    archive_path: &Path,
) -> Result<ArchiveSnapshot, i32> {
    if !archive_path.is_file() {
        print_plain_error(&format!("archive not found: {}", archive_path.display()));
        return Err(exit_code::UNSUPPORTED_INPUT);
    }

    let mut events = app.subscribe_operations();
    let operation_id = match app
        .start_open_archive(OpenArchiveRequest {
            source_path: archive_path.to_path_buf(),
            password: None,
        })
        .await
    {
        Ok(operation_id) => operation_id,
        Err(error) => {
            let code = exit_code_for(&error.kind);
            print_error(&error);
            return Err(code);
        }
    };

    loop {
        let event = match events.recv().await {
            Ok(event) => event,
            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                // Missed some intermediate events (Accepted/Started/
                // Progress) -- harmless for this loop, which only acts on
                // a terminal state or a Challenge. Keep reading.
                continue;
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                print_plain_error("application event stream closed unexpectedly");
                return Err(exit_code::INTERNAL_FAILURE);
            }
        };
        if event.operation_id != operation_id {
            continue;
        }
        match event.state {
            OperationState::Completed {
                result: OperationResult::ArchiveOpened { snapshot },
            } => return Ok(snapshot),
            OperationState::Completed { .. } => {
                print_plain_error("unexpected result for an archive-open operation");
                return Err(exit_code::INTERNAL_FAILURE);
            }
            OperationState::Challenge { challenge } => {
                print_plain_error(&format!(
                    "{} -- not supported by this command (interactive input is not yet \
                     implemented)",
                    describe_challenge(&challenge)
                ));
                let _ = app.cancel_operation(operation_id).await;
                return Err(exit_code::USER_ACTION_REQUIRED);
            }
            OperationState::Failed { error } => {
                let code = exit_code_for(&error.kind);
                print_error(&error);
                return Err(code);
            }
            OperationState::Cancelled => {
                print_plain_error("archive open was unexpectedly cancelled");
                return Err(exit_code::INTERNAL_FAILURE);
            }
            OperationState::Accepted
            | OperationState::Started
            | OperationState::Progress { .. }
            | OperationState::SnapshotChanged { .. } => {
                // Not part of this task's characterization -- keep
                // waiting for a terminal state or a challenge.
            }
        }
    }
}

/// A human-readable, one-line description of `challenge`, for the stderr
/// message `open_archive_and_wait` prints when it cannot answer one.
fn describe_challenge(challenge: &Challenge) -> String {
    match challenge {
        Challenge::Password {
            archive_name,
            attempt,
            ..
        } => format!("password required for {archive_name} (attempt {attempt})"),
        Challenge::ConfirmOverwrite { destination, .. } => {
            format!(
                "confirmation required to overwrite {}",
                destination.display()
            )
        }
        Challenge::ConfirmDestructiveAction { summary, .. } => {
            format!("confirmation required: {summary}")
        }
        Challenge::MissingExternalTool { tool, .. } => {
            format!("missing external tool: {tool}")
        }
        Challenge::RetryPermission { path, .. } => {
            format!("permission retry required for {}", path.display())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arclain_app::ids::ChallengeId;

    #[test]
    fn describe_challenge_password_mentions_password_and_attempt() {
        let challenge = Challenge::Password {
            id: ChallengeId::from_raw(1),
            archive_name: "secret.zip".to_string(),
            attempt: 2,
        };
        let description = describe_challenge(&challenge);
        assert!(description.contains("password"));
        assert!(description.contains("secret.zip"));
        assert!(description.contains('2'));
    }

    #[test]
    fn describe_challenge_covers_every_variant_without_panicking() {
        let challenges = [
            Challenge::Password {
                id: ChallengeId::from_raw(1),
                archive_name: "a.zip".to_string(),
                attempt: 1,
            },
            Challenge::ConfirmOverwrite {
                id: ChallengeId::from_raw(2),
                destination: std::path::PathBuf::from("out/file.txt"),
            },
            Challenge::ConfirmDestructiveAction {
                id: ChallengeId::from_raw(3),
                summary: "delete everything".to_string(),
            },
            Challenge::MissingExternalTool {
                id: ChallengeId::from_raw(4),
                tool: "7z".to_string(),
            },
            Challenge::RetryPermission {
                id: ChallengeId::from_raw(5),
                path: std::path::PathBuf::from("locked.txt"),
            },
        ];
        for challenge in &challenges {
            assert!(!describe_challenge(challenge).is_empty());
        }
    }

    /// A real (if minimal) `ArclainApp` bootstrap is enough to exercise
    /// `open_archive_and_wait`'s local existence check without ever
    /// reaching the facade's archive-open operation -- constructible
    /// here using only `arclain_app` types (`AppPaths`/`BootstrapConfig`),
    /// matching this crate's own dependency boundary (no `arclain_core`
    /// needed). Relies on a real 7-Zip executable on `PATH`, matching
    /// this workspace's established test convention (see
    /// `crates/app/tests/bootstrap.rs`'s own module doc comment).
    #[tokio::test]
    async fn open_archive_and_wait_rejects_a_nonexistent_source_path() {
        let temp = tempfile::tempdir().unwrap();
        let paths = arclain_app::AppPaths {
            config_dir: temp.path().join("config"),
            data_dir: temp.path().join("data"),
            cache_dir: temp.path().join("cache"),
            log_dir: temp.path().join("logs"),
            plugins_dir: temp.path().join("plugins"),
        };
        let app = arclain_app::ArclainApp::bootstrap(arclain_app::BootstrapConfig {
            paths_override: Some(paths),
            ..arclain_app::BootstrapConfig::system_default()
        })
        .expect("bootstrap must succeed (requires a real 7-Zip executable on PATH)");

        let missing = temp.path().join("does-not-exist.zip");
        let result = open_archive_and_wait(&app, &missing).await;

        assert_eq!(result.err(), Some(exit_code::UNSUPPORTED_INPUT));

        let _ = app.shutdown().await;
    }
}
