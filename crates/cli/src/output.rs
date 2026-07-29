//! Shared CLI output plumbing: the versioned JSON envelope every `--json`
//! command wraps its payload in, this CLI's process exit-code convention,
//! and the small printing helpers every command module shares.
//!
//! JSON always goes to stdout via [`print_json`]; diagnostics always go
//! to stderr via [`print_error`]/`eprintln!`. Neither path ever emits an
//! ANSI escape sequence -- this module (and every command module) prints
//! with plain `println!`/`eprintln!` only, never a color/styling crate,
//! so `--json` output is always safe to pipe into a JSON parser.

use arclain_app::error::{ApplicationError, ApplicationErrorKind};
use serde::Serialize;

/// Schema version for every `--json` payload this CLI emits. Bump only
/// when an existing field's meaning changes incompatibly; adding a new
/// optional field is additive and does not require a bump.
pub const CLI_SCHEMA_VERSION: u32 = 1;

/// The versioned envelope every `--json` command wraps its payload in.
#[derive(Serialize)]
pub struct CliEnvelope<T> {
    pub schema_version: u32,
    pub data: T,
}

impl<T> CliEnvelope<T> {
    pub fn new(data: T) -> Self {
        Self {
            schema_version: CLI_SCHEMA_VERSION,
            data,
        }
    }
}

/// The `--json` payload every mutation command that drives
/// `crate::events::drive_operation` (`extract`, `convert`, `organize`,
/// `archive add`/`delete`, `pipeline run`, `plugins action`'s own
/// `Completed` fallback path) reports on success, alongside their own
/// facade-defined result where one exists.
///
/// `summary` carries the last `OperationState::Progress` message this
/// command's own event loop observed, when one was reported --
/// deliberately surfaced rather than silently discarded: `Convert`/
/// `Organize`/`Pipeline` operations complete with `OperationResult::None`
/// (no structured per-file success/failure count of their own) **even
/// when every single input failed** -- `crate::runtime::processing_ops`'s
/// own doc comment documents this as intentional, matching
/// `execute_pipeline`'s "keep going, tally the outcome" semantics rather
/// than turning the whole operation `Failed` for a per-file problem.
/// That is workable for a GUI showing a live, always-visible progress
/// list, but a one-shot CLI process has no such fallback: without this
/// field, a caller (human or script) parsing only `{"status":"completed"}`
/// would see success even for a run that silently converted zero files.
/// This is *not* a full fix -- there is still no *structured*
/// success/failure count to build a reliable non-zero exit code from,
/// only this free-text tally message -- see this task's own report for
/// the architectural gap it surfaces.
///
/// `new_revision` is `archive add`/`archive delete`'s own counterpart:
/// `Some` when the mutation actually changed the archive (an
/// `OperationState::SnapshotChanged` was observed), `None` for a
/// structurally-empty mutation that completed as a harmless no-op.
/// Every other command leaves this `None` -- there is no archive
/// session, and therefore no revision, to report.
#[derive(Serialize)]
pub struct MutationOutcome {
    pub status: &'static str,
    pub summary: Option<String>,
    pub new_revision: Option<u64>,
}

impl MutationOutcome {
    pub fn completed(summary: Option<String>) -> Self {
        Self {
            status: "completed",
            summary,
            new_revision: None,
        }
    }

    pub fn completed_with_revision(new_revision: Option<u64>) -> Self {
        Self {
            status: "completed",
            summary: None,
            new_revision,
        }
    }
}

/// This CLI's process exit codes. `SUCCESS`/`INVOCATION_ERROR` intentionally
/// match clap's own convention (`0` for a normal exit, including `--help`/
/// `--version`; `2` for a usage error) -- `clap::Parser`'s generated
/// argument parsing already exits with these two codes on its own before
/// any of this crate's own code runs, so every other code here only needs
/// to cover what happens *after* arguments parsed successfully.
pub mod exit_code {
    /// The command completed successfully.
    pub const SUCCESS: i32 = 0;
    /// Argument parsing failed (matches clap's own usage-error exit
    /// code). Never actually returned by this crate's own code -- clap's
    /// generated parsing calls `error.exit()` directly (which never
    /// returns) before reaching any of it -- so this is dead code in the
    /// plain (non-test) build by design; it exists purely so
    /// `invocation_error_matches_claps_own_usage_error_exit_code` (in
    /// this module's own test suite) can pin the equivalence as an
    /// executable fact instead of an unchecked comment.
    #[allow(dead_code)]
    pub const INVOCATION_ERROR: i32 = 2;
    /// The command cannot proceed without input this task's read surface
    /// has no way to supply interactively (a password challenge, or any
    /// other in-flight challenge kind).
    pub const USER_ACTION_REQUIRED: i32 = 3;
    /// The user-supplied input is invalid, unsupported, or does not
    /// resolve to anything (an archive path that does not exist, an
    /// invalid in-archive path, an unknown profile id).
    pub const UNSUPPORTED_INPUT: i32 = 4;
    /// The requested operation was accepted but failed (a backend error,
    /// a conflicting revision, a missing external tool, and so on).
    pub const OPERATION_FAILURE: i32 = 5;
    /// An internal failure this CLI cannot attribute to the user's input
    /// or to a well-understood operation failure.
    pub const INTERNAL_FAILURE: i32 = 70;
}

/// Maps a facade error's [`ApplicationErrorKind`] to this CLI's exit-code
/// convention. Exhaustive over every kind so a future addition to the enum
/// fails this crate's own build (not silently falls through to a default)
/// until this mapping is updated to cover it.
pub fn exit_code_for(kind: &ApplicationErrorKind) -> i32 {
    match kind {
        ApplicationErrorKind::PasswordRequired => exit_code::USER_ACTION_REQUIRED,
        ApplicationErrorKind::InvalidInput
        | ApplicationErrorKind::Unsupported
        | ApplicationErrorKind::NotFound => exit_code::UNSUPPORTED_INPUT,
        ApplicationErrorKind::Internal => exit_code::INTERNAL_FAILURE,
        ApplicationErrorKind::Backend
        | ApplicationErrorKind::Busy
        | ApplicationErrorKind::Cancelled
        | ApplicationErrorKind::Conflict
        | ApplicationErrorKind::ExternalToolMissing
        | ApplicationErrorKind::PermissionDenied
        | ApplicationErrorKind::Persistence
        | ApplicationErrorKind::Plugin => exit_code::OPERATION_FAILURE,
    }
}

/// Prints a facade error to stderr as plain, ANSI-free diagnostic text.
pub fn print_error(error: &ApplicationError) {
    eprintln!("error: {}", error.summary);
    if let Some(diagnostic) = &error.diagnostic {
        eprintln!("  diagnostic: {diagnostic}");
    }
}

/// Prints a plain one-line error to stderr (no [`ApplicationError`]
/// available -- a purely local/CLI-level rejection).
pub fn print_plain_error(message: &str) {
    eprintln!("error: {message}");
}

/// Serializes `data` into a schema-versioned [`CliEnvelope`] and prints it
/// as a single, compact (non-pretty-printed) JSON line to stdout.
///
/// Every mutation command that first streams `crate::events::drive_operation`'s
/// own JSON Lines events uses this, never [`print_json`], for its final
/// envelope: `events`'s own module doc comment documents that whole
/// stream (progress events *and* this final line alike) as one JSON
/// object per line, and a pretty-printed (multi-line) envelope would
/// break that contract for exactly the one line meant to close it out.
pub fn print_json_line<T: Serialize>(data: &T) {
    let envelope = CliEnvelope::new(data);
    match serde_json::to_string(&envelope) {
        Ok(text) => println!("{text}"),
        Err(error) => print_plain_error(&format!("failed to serialize JSON output: {error}")),
    }
}

/// Serializes `data` into a schema-versioned [`CliEnvelope`] and prints it
/// as pretty JSON to stdout. Used by every command that prints exactly
/// one JSON object with nothing streamed before it (`inspect`, `list`,
/// `profiles`, `plugins list`, `settings show`) -- see [`print_json_line`]
/// for the mutation commands' own, JSON-Lines-safe counterpart.
pub fn print_json<T: Serialize>(data: &T) {
    let envelope = CliEnvelope::new(data);
    match serde_json::to_string_pretty(&envelope) {
        Ok(text) => println!("{text}"),
        Err(error) => print_plain_error(&format!("failed to serialize JSON output: {error}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arclain_app::error::{ApplicationErrorKind as Kind, Recoverability};

    #[test]
    fn schema_version_constant_is_one() {
        assert_eq!(CLI_SCHEMA_VERSION, 1);
    }

    #[test]
    fn envelope_serializes_schema_version_and_data() {
        let envelope = CliEnvelope::new(42);
        let value = serde_json::to_value(&envelope).unwrap();
        assert_eq!(value["schema_version"], 1);
        assert_eq!(value["data"], 42);
    }

    #[test]
    fn every_error_kind_maps_to_a_defined_exit_code() {
        let cases = [
            (Kind::Backend, exit_code::OPERATION_FAILURE),
            (Kind::Busy, exit_code::OPERATION_FAILURE),
            (Kind::Cancelled, exit_code::OPERATION_FAILURE),
            (Kind::Conflict, exit_code::OPERATION_FAILURE),
            (Kind::ExternalToolMissing, exit_code::OPERATION_FAILURE),
            (Kind::Internal, exit_code::INTERNAL_FAILURE),
            (Kind::InvalidInput, exit_code::UNSUPPORTED_INPUT),
            (Kind::NotFound, exit_code::UNSUPPORTED_INPUT),
            (Kind::PasswordRequired, exit_code::USER_ACTION_REQUIRED),
            (Kind::PermissionDenied, exit_code::OPERATION_FAILURE),
            (Kind::Persistence, exit_code::OPERATION_FAILURE),
            (Kind::Plugin, exit_code::OPERATION_FAILURE),
            (Kind::Unsupported, exit_code::UNSUPPORTED_INPUT),
        ];
        for (kind, expected) in cases {
            assert_eq!(
                exit_code_for(&kind),
                expected,
                "unexpected exit code for {kind:?}"
            );
        }
    }

    /// Pins the assumption `exit_code::INVOCATION_ERROR`'s own doc
    /// comment states as fact: clap's generated argument parsing already
    /// exits with code `2` for a real usage error before any of this
    /// crate's own dispatch code runs, so this module's exit codes only
    /// need to cover what happens afterward. Verified directly against a
    /// real clap parse failure (no subcommand at all) rather than left as
    /// an unchecked assumption in a comment.
    #[test]
    fn invocation_error_matches_claps_own_usage_error_exit_code() {
        use clap::Parser;
        let error = crate::commands::Cli::try_parse_from(["arclain-cli"]).unwrap_err();
        assert_eq!(error.exit_code(), exit_code::INVOCATION_ERROR);
    }

    #[test]
    fn print_error_includes_summary_and_diagnostic() {
        // Smoke test only: `print_error` writes to the real process
        // stderr, so this just proves it does not panic for both the
        // with- and without-diagnostic shapes.
        print_error(&ApplicationError::new(Kind::Backend, "summary only"));
        print_error(
            &ApplicationError::new(Kind::Backend, "summary")
                .with_diagnostic("diagnostic")
                .with_recoverability(Recoverability::Retry),
        );
    }
}
