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
    about = "Inspect, extract, convert, organize, and manage Arclain archives, plugins, and settings"
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

    /// Bounds how long this process waits on any single in-flight
    /// operation -- opening an archive, then (independently, with its own
    /// fresh budget) a mutation's own progress -- before giving up. On
    /// expiry, the operation is cancelled; this process then waits a
    /// further few seconds for that cancellation to actually take effect
    /// before exiting (exit code 5, or 70 if even that further wait
    /// expires). Omit for this CLI's original, fully unbounded wait --
    /// appropriate for a long-running interactive extraction;
    /// scripted/automated callers should opt in explicitly.
    #[arg(long, global = true, value_name = "SECONDS")]
    pub timeout: Option<u64>,
}

/// Everything every command's own `run`/`dispatch` function needs beyond
/// `app` and its own parsed arguments -- bundled into one value so a new
/// global concern (this task added `cancel`/`budget` alongside Task 12's
/// own `json`) does not keep growing every command function's own
/// parameter list one at a time.
#[derive(Clone)]
pub(crate) struct Invocation {
    pub(crate) json: bool,
    pub(crate) cancel: crate::events::CancelSignal,
    pub(crate) budget: crate::events::TimeoutBudget,
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
pub async fn dispatch(app: &ArclainApp, command: &Command, ctx: &Invocation) -> i32 {
    match command {
        Command::Inspect(args) => inspect::run(app, args, ctx).await,
        Command::List(args) => list::run(app, args, ctx).await,
        Command::Profiles { command } => profiles::dispatch(app, command, ctx.json).await,
        Command::Extract(args) => extract::run(app, args, ctx).await,
        Command::Convert(args) => convert::run(app, args, ctx).await,
        Command::Organize(args) => organize::run(app, args, ctx).await,
        Command::Archive { command } => archive::dispatch(app, command, ctx).await,
        Command::Pipeline { command } => pipeline::dispatch(app, command, ctx).await,
        Command::Plugins { command } => plugins::dispatch(app, command, ctx).await,
        Command::Settings { command } => settings::dispatch(app, command, ctx.json).await,
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
/// `cancel`/`budget` race this operation exactly like
/// [`crate::events::drive_operation`]'s own mutation-phase loop does --
/// via the same shared [`crate::events::drive_until_terminal`] -- so a
/// Ctrl+C or a `--timeout` expiry during this archive-open phase (the
/// whole point of opening a possibly-large archive before `extract`/
/// `archive add`/`archive delete` even reach their own mutation) is
/// cancelled and reported the same way, not left to fall through to the
/// OS's own default handling. This phase never itself prints per-event
/// JSON Lines/progress, unlike a mutation's own `drive_operation` call --
/// see `crate::events`'s own module doc comment for why.
///
/// Returns the opened session's snapshot on success, or the process exit
/// code to use on any failure path (already printed to stderr).
pub(crate) async fn open_archive_and_wait(
    app: &ArclainApp,
    archive_path: &Path,
    interactive: &dyn crate::events::Interactive,
    cancel: &crate::events::CancelSignal,
    budget: crate::events::TimeoutBudget,
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

    crate::events::drive_until_terminal(
        crate::events::OperationWait {
            app,
            events: &mut events,
            operation_id,
            interactive,
            cancel,
            budget,
        },
        |_event| {},
        |result| match result {
            OperationResult::ArchiveOpened { snapshot } => Ok(snapshot),
            _ => {
                print_plain_error("unexpected result for an archive-open operation");
                Err(exit_code::INTERNAL_FAILURE)
            }
        },
    )
    .await
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
    use std::sync::Arc;

    use arclain_app::{AppPaths, ArclainApp, BootstrapConfig};

    use crate::events::{CancelSignal, TimeoutBudget};

    use super::*;

    fn temp_app_paths(temp: &tempfile::TempDir) -> AppPaths {
        AppPaths {
            config_dir: temp.path().join("config"),
            data_dir: temp.path().join("data"),
            cache_dir: temp.path().join("cache"),
            log_dir: temp.path().join("logs"),
            plugins_dir: temp.path().join("plugins"),
        }
    }

    fn no_cancel() -> CancelSignal {
        Arc::new(tokio::sync::Notify::new())
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
        let app = ArclainApp::bootstrap(BootstrapConfig {
            paths_override: Some(temp_app_paths(&temp)),
            ..BootstrapConfig::system_default()
        })
        .expect("bootstrap must succeed (requires a real 7-Zip executable on PATH)");

        let missing = temp.path().join("does-not-exist.zip");
        let interactive = crate::events::std_interactive();
        let cancel = no_cancel();
        let result = open_archive_and_wait(
            &app,
            &missing,
            &interactive,
            &cancel,
            TimeoutBudget::unbounded(),
        )
        .await;

        assert_eq!(result.err(), Some(exit_code::UNSUPPORTED_INPUT));

        let _ = app.shutdown().await;
    }

    /// **Important-1 regression test**: a cancellation arriving *during*
    /// the archive-open phase (before `drive_operation` -- and, before
    /// this fix, before anything in this crate had ever polled
    /// `tokio::signal::ctrl_c()` at all -- ever runs) must still cancel
    /// the in-flight `start_open_archive` operation and exit
    /// `OPERATION_FAILURE`, not fall through to whatever the caller
    /// would otherwise observe.
    ///
    /// A real (if synthetic) archive with several thousand entries is
    /// used deliberately: `EntryIndex::build`'s own indexing work (see
    /// `arclain_app::archive::session`) is real, non-trivial CPU work
    /// proportional to entry count, giving the background thread below a
    /// realistic window to land its cancellation *during* the operation
    /// rather than racing a coincidentally-instant open. This crate
    /// cannot fake or otherwise slow down archive opening itself (no
    /// `archive_backend_override`-equivalent seam is reachable across
    /// this crate's own dependency boundary -- see
    /// `crate::events::tests`' own module doc comment on why extraction
    /// alone can be faked here), so unlike this file's sibling
    /// `crate::events` tests, this one's timing is real rather than
    /// barrier-controlled; the entry count and delay below are chosen
    /// generously and were verified stable across repeated runs.
    #[test]
    fn open_archive_and_wait_is_cancelled_by_a_signal_arriving_mid_open() {
        let temp = tempfile::tempdir().unwrap();
        let archive = temp.path().join("many-entries.zip");
        {
            let file = std::fs::File::create(&archive).unwrap();
            let mut writer = zip::ZipWriter::new(file);
            let options = zip::write::SimpleFileOptions::default();
            for i in 0..8000 {
                writer
                    .start_file(format!("dir{}/file_{i:05}.txt", i % 50), options)
                    .unwrap();
                use std::io::Write;
                writer.write_all(b"x").unwrap();
            }
            writer.finish().unwrap();
        }

        let app = ArclainApp::bootstrap(BootstrapConfig {
            paths_override: Some(temp_app_paths(&temp)),
            ..BootstrapConfig::system_default()
        })
        .expect("bootstrap must succeed (requires a real 7-Zip executable on PATH)");
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        let cancel: CancelSignal = Arc::new(tokio::sync::Notify::new());
        // Fired from a plain OS thread -- deliberately independent of
        // this test's own runtime, exactly like a real Ctrl+C's OS-level
        // delivery is independent of whatever this process happens to
        // be doing when it arrives.
        let signal = cancel.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(5));
            signal.notify_one();
        });

        let interactive = crate::events::std_interactive();
        let result = runtime.block_on(open_archive_and_wait(
            &app,
            &archive,
            &interactive,
            &cancel,
            TimeoutBudget::unbounded(),
        ));

        assert_eq!(
            result.err(),
            Some(exit_code::OPERATION_FAILURE),
            "a cancellation during the open phase must be honored, not silently ignored"
        );

        runtime.block_on(app.shutdown()).ok();
    }

    /// Proves `resolve_entry_id`'s own segment-by-segment descent
    /// actually walks *multiple* directory levels correctly (`dir` ->
    /// `dir/sub` -> the file), not just a single-segment lookup -- the
    /// shallower existing coverage (`tests/mutation_commands.rs`'s own
    /// `extract_specific_entry_by_path_extracts_only_that_file`) only
    /// ever names a root-level entry.
    #[tokio::test]
    async fn resolve_entry_id_descends_through_multiple_directory_levels() {
        let temp = tempfile::tempdir().unwrap();
        let archive_path = temp.path().join("nested.zip");
        {
            let file = std::fs::File::create(&archive_path).unwrap();
            let mut writer = zip::ZipWriter::new(file);
            let options = zip::write::SimpleFileOptions::default();
            writer.start_file("dir/sub/file.txt", options).unwrap();
            use std::io::Write;
            writer.write_all(b"nested content").unwrap();
            writer.finish().unwrap();
        }

        let app = ArclainApp::bootstrap(BootstrapConfig {
            paths_override: Some(temp_app_paths(&temp)),
            ..BootstrapConfig::system_default()
        })
        .expect("bootstrap must succeed (requires a real 7-Zip executable on PATH)");
        let interactive = crate::events::std_interactive();
        let cancel = no_cancel();
        let snapshot = open_archive_and_wait(
            &app,
            &archive_path,
            &interactive,
            &cancel,
            TimeoutBudget::unbounded(),
        )
        .await
        .expect("opening the fixture must succeed");

        let resolved = resolve_entry_id(&app, snapshot.session_id, "dir/sub/file.txt")
            .await
            .expect("a real, multi-segment path must resolve");

        // Cross-checked against a direct listing of the file's own
        // parent directory, rather than merely asserting `resolve_entry_id`
        // returns *some* id without a panic.
        let listing = app
            .list_entries(
                snapshot.session_id,
                ListEntriesRequest {
                    directory: ArchivePath::parse("dir/sub".to_string()).unwrap(),
                    sort_key: EntrySortKey::Name,
                    sort_direction: SortDirection::Ascending,
                    name_filter: None,
                    offset: 0,
                    limit: 10,
                },
            )
            .await
            .expect("listing the resolved directory must succeed");
        let expected = listing
            .entries
            .iter()
            .find(|entry| entry.name == "file.txt")
            .expect("file.txt must be listed under dir/sub")
            .id;
        assert_eq!(resolved, expected);

        let missing = resolve_entry_id(&app, snapshot.session_id, "dir/sub/does-not-exist.txt")
            .await
            .unwrap_err();
        assert_eq!(missing.kind, ApplicationErrorKind::NotFound);

        let _ = app.close_archive(snapshot.session_id).await;
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
