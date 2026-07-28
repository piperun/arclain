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
    Extract,
    Materialize,
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
    ArchiveOpened { snapshot: ArchiveSnapshot },
    Materialized { lease: MaterializationLease },
    PluginUiUpdated { update: PluginUiUpdate },
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
