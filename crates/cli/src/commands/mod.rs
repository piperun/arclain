//! The CLI's argument surface (`Cli`/`Command`) and the shared
//! archive-open-and-wait helper every read command that opens an archive
//! (`inspect`, `list`) builds on, plus the entry-path resolution and
//! path-absolutizing helpers every mutation command shares.

pub mod archive;
pub mod convert;
pub mod extract;
pub mod inspect;
pub mod list;
pub mod organize;
pub mod pipeline;
pub mod plugins;
pub mod profiles;
pub mod settings;

use std::path::{Path, PathBuf};

use arclain_app::archive::{
    ArchivePath, ArchiveSnapshot, EntrySortKey, ListEntriesRequest, OpenArchiveRequest,
    SortDirection,
};
use arclain_app::error::{ApplicationError, ApplicationErrorKind, Recoverability};
use arclain_app::event::{OperationEvent, OperationResult, OperationState};
use arclain_app::ids::{ArchiveSessionId, EntryId};
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
    /// Extract entries (or the whole archive) to a destination directory.
    Extract(extract::ExtractArgs),
    /// Convert a batch of archives to a target format.
    Convert(convert::ConvertArgs),
    /// Organize a batch of archives under one rule and output profile.
    Organize(organize::OrganizeArgs),
    /// Add or delete entries in an open archive.
    Archive {
        #[command(subcommand)]
        command: archive::ArchiveCommand,
    },
    /// Run a saved processing pipeline preset over a batch of inputs.
    Pipeline {
        #[command(subcommand)]
        command: pipeline::PipelineCommand,
    },
    /// Inspect and control the plugin runtime.
    Plugins {
        #[command(subcommand)]
        command: plugins::PluginsCommand,
    },
    /// Inspect and mutate application settings.
    Settings {
        #[command(subcommand)]
        command: settings::SettingsCommand,
    },
}

/// Runs the parsed `command` against a bootstrapped `app`, returning this
/// process's exit code.
pub async fn dispatch(app: &ArclainApp, command: &Command, json: bool) -> i32 {
    match command {
        Command::Inspect(args) => inspect::run(app, args, json).await,
        Command::List(args) => list::run(app, args, json).await,
        Command::Profiles { command } => profiles::dispatch(app, command, json).await,
        Command::Extract(args) => extract::run(app, args, json).await,
        Command::Convert(args) => convert::run(app, args, json).await,
        Command::Organize(args) => organize::run(app, args, json).await,
        Command::Archive { command } => archive::dispatch(app, command, json).await,
        Command::Pipeline { command } => pipeline::dispatch(app, command, json).await,
        Command::Plugins { command } => plugins::dispatch(app, command, json).await,
        Command::Settings { command } => settings::dispatch(app, command, json).await,
    }
}

/// Validates `archive_path` locally, then starts opening it as a facade
/// operation and waits for a terminal state -- the shared first step
/// every command that needs an open session (`inspect`/`list`, and every
/// mutation command that opens an archive first: `extract`, `archive
/// add`/`delete`) builds on.
///
/// Subscribes to the operation-event stream *before* calling
/// `start_open_archive`, matching `arclain_app`'s own integration-test
/// convention (see `crates/app/tests/archive_sessions.rs`): subscribing
/// after the call could race the operation's own `Accepted` event.
///
/// A `Challenge::Password` is answered via `interactive` (the only
/// variant actually reachable for an archive-open operation, but this
/// function still routes every variant through the same
/// `crate::events::handle_challenge`, matching that function's own
/// exhaustive, forward-compatible handling of all five), refused with
/// `exit_code::USER_ACTION_REQUIRED` when `interactive` reports no real
/// controlling terminal -- see that function's own doc comment.
///
/// Returns the opened session's snapshot on success, or the process exit
/// code to use on any failure path (already printed to stderr).
pub(crate) async fn open_archive_and_wait(
    app: &ArclainApp,
    archive_path: &Path,
    interactive: &dyn crate::events::Interactive,
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
            OperationState::Challenge { ref challenge } => {
                // A response was submitted and accepted -- keep waiting
                // for this same operation's next event. `?` propagates
                // `handle_challenge`'s own `Err(code)` directly: both
                // functions share the same `Result<_, i32>` error type.
                crate::events::handle_challenge(app, operation_id, challenge, interactive).await?;
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

/// Resolves one archive-relative path string (as a user types it on the
/// command line, e.g. `game/data/save.dat`) to the [`EntryId`] naming it
/// in `session_id`'s *current* index.
///
/// There is no facade method that resolves a path directly to an id --
/// `ArclainApp::list_entries` only ever returns one directory's own
/// direct children -- so this walks the path one segment at a time from
/// the root, looking up each segment's matching child before descending
/// into it. `O(depth)` facade calls per path; acceptable for the small,
/// human-typed paths a CLI invocation's own argument list can realistically
/// carry.
///
/// Known, documented limitation: if a directory contains more than one
/// entry with the same `name` (the facade's own index explicitly allows
/// this for a malformed/adversarial archive -- see `arclain_app`'s own
/// `EntryIndex` doc comment on "duplicate paths preserved as distinct
/// entries"), this resolves to whichever one `list_entries` returns
/// first for that name; there is no syntax in this CLI for addressing a
/// second, third, ... duplicate at the same path independently. A real
/// fix needs its own selection syntax (an ordinal suffix, for example)
/// and is out of this task's scope.
pub(crate) async fn resolve_entry_id(
    app: &ArclainApp,
    session_id: ArchiveSessionId,
    path: &str,
) -> Result<EntryId, ApplicationError> {
    let target = ArchivePath::parse(path.to_string())?;
    if target == ArchivePath::root() {
        return Err(cannot_select_root_error());
    }

    let mut directory = ArchivePath::root();
    let mut resolved: Option<EntryId> = None;
    for segment in target.as_str().split('/') {
        let page = app
            .list_entries(
                session_id,
                ListEntriesRequest {
                    directory: directory.clone(),
                    sort_key: EntrySortKey::Name,
                    sort_direction: SortDirection::Ascending,
                    name_filter: None,
                    offset: 0,
                    limit: u32::MAX,
                },
            )
            .await?;
        let found = page
            .entries
            .iter()
            .find(|entry| entry.name == segment)
            .ok_or_else(|| unknown_entry_path_error(path))?;
        resolved = Some(found.id);
        directory = found.path.clone();
    }
    resolved.ok_or_else(|| unknown_entry_path_error(path))
}

/// Resolves every one of `paths` via [`resolve_entry_id`], in order,
/// stopping at (and returning) the first one that fails to resolve.
pub(crate) async fn resolve_entry_ids(
    app: &ArclainApp,
    session_id: ArchiveSessionId,
    paths: &[String],
) -> Result<Vec<EntryId>, ApplicationError> {
    let mut ids = Vec::with_capacity(paths.len());
    for path in paths {
        ids.push(resolve_entry_id(app, session_id, path).await?);
    }
    Ok(ids)
}

fn cannot_select_root_error() -> ApplicationError {
    ApplicationError::new(
        ApplicationErrorKind::InvalidInput,
        "the archive root cannot be selected as a single entry",
    )
    .with_recoverability(Recoverability::UserAction)
    .with_field("entry")
}

fn unknown_entry_path_error(path: &str) -> ApplicationError {
    ApplicationError::new(
        ApplicationErrorKind::NotFound,
        "no such entry exists in this archive",
    )
    .with_diagnostic(format!("path: {path}"))
    .with_recoverability(Recoverability::UserAction)
    .with_field("entry")
}

/// Resolves `path` against this process's current working directory if
/// it is relative, using [`std::path::absolute`] -- purely lexical
/// (prepends the CWD and normalizes `.`/`..` segments; never requires
/// `path` to exist, never resolves a symlink). Every destination/source
/// path this crate's mutation commands accept goes through this: a user
/// typing a relative path on the command line expects it resolved
/// against the shell's own CWD, matching every ordinary CLI tool's
/// convention -- and several facade request types (`ExtractRequest::
/// destination` in particular) reject a relative path outright as
/// `InvalidInput`.
pub(crate) fn absolutize(path: &Path) -> Result<PathBuf, i32> {
    std::path::absolute(path).map_err(|error| {
        print_plain_error(&format!(
            "failed to resolve {} to an absolute path: {error}",
            path.display()
        ));
        exit_code::INTERNAL_FAILURE
    })
}

/// Tracks the last `OperationState::Progress` message observed across a
/// `crate::events::drive_operation` run, via that function's own
/// `on_event` callback -- shared by every mutation command
/// (`extract`, `convert`, `organize`, `archive add`/`delete`,
/// `pipeline run`) that surfaces it as `crate::output::MutationOutcome::summary`.
/// See that field's own doc comment for why this is captured at all:
/// `Convert`/`Organize`/`Pipeline` complete with `OperationResult::None`
/// even when every input failed, so this free-text tally message is the
/// only signal a caller has beyond a bare `{"status": "completed"}`.
#[derive(Default)]
pub(crate) struct LastProgressMessage(Option<String>);

impl LastProgressMessage {
    pub(crate) fn observe(&mut self, event: &OperationEvent) {
        if let OperationState::Progress {
            message: Some(message),
            ..
        } = &event.state
        {
            self.0 = Some(message.clone());
        }
    }

    pub(crate) fn into_inner(self) -> Option<String> {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let interactive = crate::events::std_interactive();
        let result = open_archive_and_wait(&app, &missing, &interactive).await;

        assert_eq!(result.err(), Some(exit_code::UNSUPPORTED_INPUT));

        let _ = app.shutdown().await;
    }

    #[test]
    fn absolutize_leaves_an_already_absolute_path_unchanged() {
        let absolute = if cfg!(windows) {
            PathBuf::from(r"C:\already\absolute")
        } else {
            PathBuf::from("/already/absolute")
        };
        assert_eq!(absolutize(&absolute).unwrap(), absolute);
    }

    #[test]
    fn absolutize_prefixes_a_relative_path_with_the_current_directory() {
        let relative = Path::new("relative/child.txt");
        let resolved = absolutize(relative).unwrap();
        assert!(resolved.is_absolute());
        assert!(
            resolved.ends_with("relative/child.txt") || resolved.ends_with("relative\\child.txt")
        );
    }
}
