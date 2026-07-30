//! The event stream every asynchronous application operation reports its
//! progress through, and the point-in-time snapshot read back from it.
//!
//! first needed a terminal payload. `Materialized` is the terminal payload
//! `start_materialization`'s spawned operation produces on success.
//! `PluginUiUpdated` is the terminal payload `start_plugin_action`'s
//! spawned operation produces on success. Adding a variant is additive
//! and does not change anything defined earlier.

use crate::archive::ArchiveSnapshot;
use crate::challenge::Challenge;
use crate::error::ApplicationError;
use crate::ids::{ArchiveSessionId, OperationId};
use crate::materialization::MaterializationLease;
use crate::plugins::PluginUiUpdate;

/// Which kind of long-running, cancellable action an operation performs.
/// A frontend uses this to choose an icon/label without inspecting the
/// operation's current [`OperationState`].
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationKind {
    ArchiveModify,
    Convert,
    /// Staging archive content onto local disk for an OS drag-out --
    /// `crate::ArclainApp::start_drag_stage`. Deliberately its own kind
    /// rather than folded into `Materialize`: a drag stage takes a
    /// multi-entry selection (which `start_materialization` rejects),
    /// never raises a password `Challenge` (the OS shell is synchronously
    /// blocked waiting on it -- see `crate::materialization::drag_stage`'s
    /// module doc comment), and its lease's lifecycle belongs to the drag
    /// source that started it, not to whatever generic handling a
    /// frontend gives `Materialize` completions.
    DragStage,
    Extract,
    Materialize,
    /// Combining a split multi-part archive set into a single archive --
    /// `crate::ArclainApp::start_merge`. Deliberately its own kind rather
    /// than folded into `ArchiveModify` (which mutates one already-open
    /// session in place) or `Convert` (which reformats whole archives
    /// one-to-one): a merge reads a set of files nothing has open and
    /// writes one new archive beside them.
    Merge,
    OpenArchive,
    Organize,
    Pipeline,
    PluginAction,
}

/// The payload an operation's terminal `Completed` state carries.
///
/// Deliberately not `Eq`: `ArchiveSnapshot` carries a `serde_json::Value`
/// (`metadata`), which has no total order, so `Eq` is left off
/// `OperationResult` from the start rather than removed later as a
/// breaking change to every type that embeds this one transitively
/// (`OperationState`, `OperationEvent`, `OperationSnapshot`).
#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum OperationResult {
    None,
    ArchiveOpened {
        snapshot: ArchiveSnapshot,
    },
    Materialized {
        lease: MaterializationLease,
    },
    PluginUiUpdated {
        update: PluginUiUpdate,
    },
    /// The single archive `crate::ArclainApp::start_merge` wrote. Carried
    /// as a payload rather than left for the caller to re-derive: the
    /// output name depends on the request's format and on whether it
    /// named an explicit `output_path`, so only the operation itself
    /// knows which file now exists.
    Merged {
        output_path: std::path::PathBuf,
    },
}

/// The lifecycle state of one in-flight (or finished) operation. Carried,
/// together with a monotonic per-operation `sequence`, by every
/// [`OperationEvent`].
#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum OperationState {
    Accepted,
    Started,
    Progress {
        completed_units: u64,
        total_units: Option<u64>,
        message: Option<String>,
    },
    Challenge {
        challenge: Challenge,
    },
    SnapshotChanged {
        session_id: ArchiveSessionId,
        revision: u64,
    },
    Completed {
        result: OperationResult,
    },
    Cancelled,
    Failed {
        error: ApplicationError,
    },
}

/// One published transition of one operation. `sequence` starts at 1 and
/// increases by exactly 1 for every event a single `operation_id` ever
/// produces; every subscriber that observes a given `operation_id` sees
/// the same `sequence` values in the same order, whatever else it may
/// have missed (see the operation registry's own tests for the latter).
#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct OperationEvent {
    pub operation_id: OperationId,
    pub sequence: u64,
    pub kind: OperationKind,
    pub state: OperationState,
}

/// A point-in-time read of one operation's last known state, independent
/// of the event stream -- what an `operation(id)`-style lookup returns.
#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct OperationSnapshot {
    pub operation_id: OperationId,
    pub kind: OperationKind,
    pub last_sequence: u64,
    pub state: OperationState,
}

/// A session-scoped change that happens outside any operation -- the
/// only case so far is a plugin writing an archive session's metadata
/// (from an event-triggered fetch, or a panel-driven emit) through
/// `crate::plugins::ArchiveContextBridge`. Nothing about opening,
/// closing, or mutating an archive through `start_open_archive`/
/// `start_archive_mutation` flows through this: those already have their
/// own progress/completion story via [`OperationEvent`]/[`OperationState`].
///
/// Delivered through [`crate::ArclainApp::subscribe_session_events`]: a
/// bounded, best-effort broadcast with the same lag semantics as
/// [`OperationEvent`]'s own stream (see `crate::archive::ArchiveSessionStore`'s
/// own doc comment for the channel itself) -- a subscriber that falls
/// behind receives `RecvError::Lagged` rather than silently missing
/// events, and reconciles by re-fetching [`crate::ArclainApp::archive_snapshot`]
/// for whichever sessions it still cares about, rather than trusting it
/// never missed one.
///
/// Only one variant exists today (a `session_id` is enough to name what
/// changed and let a subscriber re-fetch the rest), but the type is
/// already an enum -- adding a second session-scoped change later needs
/// no brand-new broadcast stream and no new subscribe method, just a new
/// arm here and in whatever already matches on `MetadataChanged`
/// exhaustively.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SessionEvent {
    /// `session_id`'s metadata (or another session-visible field an
    /// `archive_snapshot` reconciliation would also pick up, such as its
    /// `source_path` after a plugin-triggered rename) changed. Carries no
    /// payload of its own by design -- a subscriber always re-fetches the
    /// authoritative current state via `archive_snapshot` rather than
    /// trusting a value that could itself be stale by the time it is
    /// read, which also keeps this event cheap to construct and free of
    /// its own lag-ordering concerns (unlike `OperationState::Progress`,
    /// there is no meaningful "intermediate" value to preserve here --
    /// only the latest snapshot ever matters).
    MetadataChanged { session_id: ArchiveSessionId },
}
