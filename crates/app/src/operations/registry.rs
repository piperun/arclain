//! The cancellable, event-broadcasting registry every in-flight
//! application operation is tracked through.
//!
//! `events` is the single fan-out channel every [`OperationEvent`] is
//! published on; `records` is the point-in-time store `operation`- and
//! `recent`-style lookups read from. The two are intentionally separate:
//! a slow subscriber can lag or miss events on `events` (Tokio reports
//! this explicitly rather than silently dropping them, see
//! [`OperationRegistry::subscribe`]), but `records` always reflects the
//! true last-known state of an operation that has not yet been evicted by
//! [`MAX_TERMINAL_HISTORY`]-style bounding, independent of whether every
//! intermediate event reached every subscriber.
//!
//! Not wired to anything yet: the facade (`ArclainApp`, added by a later
//! task) is the only intended caller, and it does not exist yet either.
//! Nothing outside this crate's own tests constructs an `OperationRegistry`
//! for the moment, hence the blanket `#[allow(dead_code)]` below -- the
//! same pattern already used for other not-yet-wired-up `pub(crate)` types
//! in this workspace (see `crates/data/src/features/resolver/memory.rs`).

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use tokio::sync::{broadcast, RwLock};

use crate::error::{ApplicationError, ApplicationErrorKind, Recoverability};
use crate::event::{OperationEvent, OperationKind, OperationSnapshot, OperationState};
use crate::ids::{ChallengeId, OperationId};

/// How many events the broadcast channel buffers before a subscriber that
/// has not called `recv` yet starts lagging. `tokio::sync::broadcast`
/// rounds the requested capacity up to the next power of two internally,
/// so this is deliberately already a power of two to make that actual
/// capacity exact rather than a surprise. Large enough under `cfg(test)`
/// that the concurrent-ordering test's several hundred transitions on one
/// operation all fit without lagging (lag is exercised deliberately, and
/// separately, by looping past this bound in
/// `a_lagging_subscriber_receives_tokios_explicit_lag_error`).
#[cfg(not(test))]
const EVENT_CHANNEL_CAPACITY: usize = 256;
#[cfg(test)]
const EVENT_CHANNEL_CAPACITY: usize = 1024;

/// How many *terminal* operations (`Completed`, `Cancelled`, `Failed`) the
/// registry keeps once they are no longer active before evicting the
/// oldest. Active operations are never evicted, however many there are.
/// Small under `cfg(test)` for the same reason as
/// [`EVENT_CHANNEL_CAPACITY`].
#[cfg(not(test))]
const MAX_TERMINAL_HISTORY: usize = 256;
#[cfg(test)]
const MAX_TERMINAL_HISTORY: usize = 2;

/// One tracked operation: its kind, the sequence number of the last event
/// broadcast for it, its current state, a cooperative-cancellation flag,
/// and -- while `state` is `OperationState::Challenge` -- the id of the
/// challenge a response must reference.
#[allow(dead_code)]
pub(crate) struct OperationRecord {
    kind: OperationKind,
    last_sequence: u64,
    state: OperationState,
    cancel: Arc<AtomicBool>,
    pending_challenge: Option<ChallengeId>,
}

impl OperationRecord {
    fn snapshot(&self, operation_id: OperationId) -> OperationSnapshot {
        OperationSnapshot {
            operation_id,
            kind: self.kind.clone(),
            last_sequence: self.last_sequence,
            state: self.state.clone(),
        }
    }
}

/// Whether `state` is one an operation never leaves: no further
/// transition, cancellation, or challenge resolution can change it.
fn is_terminal(state: &OperationState) -> bool {
    matches!(
        state,
        OperationState::Completed { .. }
            | OperationState::Cancelled
            | OperationState::Failed { .. }
    )
}

/// Mints a fresh, process-wide-unique `OperationId`. Mirrors
/// `CorrelationId::generate`'s exact pattern in `crate::ids` (a
/// function-local atomic counter, not a store) -- `crate::ids`'s own doc
/// comment reserves that free-standing style for `CorrelationId` only,
/// so this lives here instead: the operation *store* (this registry)
/// mints operation ids, which is the general rule the doc comment
/// describes for every id type other than `CorrelationId`.
fn next_operation_id() -> OperationId {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    OperationId::from_raw(NEXT.fetch_add(1, Ordering::Relaxed))
}

fn unknown_operation_error(operation_id: OperationId) -> ApplicationError {
    ApplicationError::new(ApplicationErrorKind::NotFound, "no such operation")
        .with_recoverability(Recoverability::Fatal)
        .with_operation_id(operation_id)
}

/// The cancellable, broadcasting registry of in-flight (and recently
/// finished) application operations. See the module doc comment for how
/// `events` and `records` divide responsibility.
#[allow(dead_code)]
pub(crate) struct OperationRegistry {
    events: broadcast::Sender<OperationEvent>,
    records: RwLock<HashMap<OperationId, OperationRecord>>,
}

#[allow(dead_code)]
impl OperationRegistry {
    pub(crate) fn new() -> Self {
        let (events, _receiver) = broadcast::channel(EVENT_CHANNEL_CAPACITY);
        Self {
            events,
            records: RwLock::new(HashMap::new()),
        }
    }

    /// Subscribes to the operation-event stream. Every subscriber
    /// receives every event published after it subscribes, independent of
    /// every other subscriber -- the fan-out `tokio::sync::broadcast`
    /// provides. A subscriber that does not keep up receives
    /// [`broadcast::error::RecvError::Lagged`] from `recv` instead of
    /// silently missing events.
    pub(crate) fn subscribe(&self) -> broadcast::Receiver<OperationEvent> {
        self.events.subscribe()
    }

    /// Registers a new operation in the `Accepted` state at sequence 1,
    /// broadcasts its first event, and returns the fresh id together with
    /// the cancellation flag a spawned worker should poll.
    pub(crate) async fn begin(&self, kind: OperationKind) -> (OperationId, Arc<AtomicBool>) {
        let operation_id = next_operation_id();
        let cancel = Arc::new(AtomicBool::new(false));
        let record = OperationRecord {
            kind: kind.clone(),
            last_sequence: 1,
            state: OperationState::Accepted,
            cancel: Arc::clone(&cancel),
            pending_challenge: None,
        };
        {
            let mut records = self.records.write().await;
            records.insert(operation_id, record);
            // See `transition` for why publishing happens before the write
            // guard is released. Not strictly load-bearing here (a freshly
            // minted `operation_id` cannot yet race with anything else --
            // nobody has it until this call returns), but kept consistent
            // so `begin` does not silently diverge from the rule the rest
            // of the registry follows.
            self.publish(operation_id, kind, 1, OperationState::Accepted);
        }
        (operation_id, cancel)
    }

    /// Broadcasts one event. A broadcast channel with zero subscribers
    /// returns `Err` on send; that is a normal, harmless outcome (nobody
    /// is listening yet), not a registry failure, so it is deliberately
    /// ignored here.
    fn publish(
        &self,
        operation_id: OperationId,
        kind: OperationKind,
        sequence: u64,
        state: OperationState,
    ) {
        // Re-asserts, at the point an error actually leaves the application,
        // the 4 KiB bound `ApplicationError::with_diagnostic` enforces at
        // construction. `diagnostic` is a plain public field (the contract
        // mandates that), so nothing stops a future call site from building
        // an `ApplicationError` with a struct literal instead of the bounded
        // `with_*` helpers -- this catches that in debug builds before an
        // unbounded diagnostic reaches a subscriber.
        if let OperationState::Failed { ref error } = state {
            debug_assert!(
                error.diagnostic.as_deref().map_or(true, |diagnostic| diagnostic.len() <= 4096),
                "OperationState::Failed diagnostic exceeds the 4 KiB bound ApplicationError::with_diagnostic enforces"
            );
        }
        let _ = self.events.send(OperationEvent {
            operation_id,
            sequence,
            kind,
            state,
        });
    }

    /// Applies `state` to `operation_id`, bumping its sequence and
    /// broadcasting the resulting event. Entering `OperationState::Challenge`
    /// records its challenge id as the operation's pending challenge;
    /// entering any other state clears it. A no-op success once the
    /// operation has already reached a terminal state: additional
    /// transitions never overwrite recorded history.
    ///
    /// Mutating the record and publishing its event happen under the same
    /// write-lock guard, deliberately. `broadcast::Sender::send` never
    /// awaits, so holding the lock across it costs nothing, and it is what
    /// actually guarantees the ordering `OperationEvent` documents: without
    /// it, two concurrent `transition` calls on the same `operation_id`
    /// could each bump `last_sequence` and release the lock before either
    /// one published, letting whichever thread reached `send` first
    /// deliver its (higher- or lower-numbered) event first -- a subscriber
    /// could then observe sequence 4 before sequence 3 for one operation.
    /// Holding the lock across the send serializes "bump the sequence" and
    /// "publish the resulting event" into one atomic step per operation, so
    /// the order events are sent in always matches the order their
    /// sequence numbers were assigned in.
    pub(crate) async fn transition(
        &self,
        operation_id: OperationId,
        state: OperationState,
    ) -> Result<(), ApplicationError> {
        let mut records = self.records.write().await;
        let Some(record) = records.get_mut(&operation_id) else {
            return Err(unknown_operation_error(operation_id));
        };
        if is_terminal(&record.state) {
            return Ok(());
        }
        record.pending_challenge = match &state {
            OperationState::Challenge { challenge } => Some(challenge.id()),
            _ => None,
        };
        record.last_sequence += 1;
        record.state = state.clone();
        let kind = record.kind.clone();
        let sequence = record.last_sequence;
        self.publish(operation_id, kind, sequence, state);
        drop(records);
        self.evict_excess_history().await;
        Ok(())
    }

    /// Reads a point-in-time snapshot of one operation.
    pub(crate) async fn operation(&self, operation_id: OperationId) -> Option<OperationSnapshot> {
        self.records
            .read()
            .await
            .get(&operation_id)
            .map(|record| record.snapshot(operation_id))
    }

    /// Reads up to `limit` operations, most-recently-created first.
    pub(crate) async fn recent(&self, limit: usize) -> Vec<OperationSnapshot> {
        let records = self.records.read().await;
        let mut snapshots: Vec<OperationSnapshot> = records
            .iter()
            .map(|(operation_id, record)| record.snapshot(*operation_id))
            .collect();
        snapshots.sort_by_key(|snapshot| std::cmp::Reverse(snapshot.operation_id.into_raw()));
        snapshots.truncate(limit);
        snapshots
    }

    /// Cooperatively cancels `operation_id`: sets its cancellation flag
    /// and, unless already terminal, transitions it to `Cancelled`.
    /// Idempotent -- cancelling an already-cancelled or otherwise-terminal
    /// operation is a no-op success, not an error.
    pub(crate) async fn cancel(&self, operation_id: OperationId) -> Result<(), ApplicationError> {
        let already_terminal = {
            let records = self.records.read().await;
            let Some(record) = records.get(&operation_id) else {
                return Err(unknown_operation_error(operation_id));
            };
            record.cancel.store(true, Ordering::SeqCst);
            is_terminal(&record.state)
        };
        if already_terminal {
            return Ok(());
        }
        self.transition(operation_id, OperationState::Cancelled)
            .await
    }

    /// True once `operation_id` has been flagged for cancellation.
    pub(crate) async fn is_cancelled(&self, operation_id: OperationId) -> bool {
        self.records
            .read()
            .await
            .get(&operation_id)
            .map(|record| record.cancel.load(Ordering::SeqCst))
            .unwrap_or(false)
    }

    /// Awaits until `operation_id` is flagged for cancellation. Polls
    /// [`Self::is_cancelled`] on a fixed interval rather than a push-based
    /// notification -- cancellation is rare and not latency-critical, so
    /// a lightweight poll avoids a second notification primitive per
    /// operation alongside the broadcast channel and challenge waiter
    /// this registry already has. Shared by every operation kind that
    /// raises a challenge and must stop waiting for a response once
    /// cancelled (`crate::runtime::archive_ops`,
    /// `crate::operations::extract`) -- both used to keep their own
    /// private copy of this exact loop before a second challenge-raising
    /// operation kind existed to justify centralizing it.
    pub(crate) async fn wait_until_cancelled(&self, operation_id: OperationId) {
        loop {
            if self.is_cancelled(operation_id).await {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
    }

    /// Resolves the pending challenge on `operation_id`, provided
    /// `challenge_id` matches the challenge that operation is actually
    /// waiting on. Rejects a mismatched challenge id, a challenge id
    /// belonging to a different operation, or an operation with no
    /// pending challenge at all -- all three collapse to the same
    /// "does not match" condition by construction, since `pending_challenge`
    /// is stored per-operation.
    pub(crate) async fn resolve_challenge(
        &self,
        operation_id: OperationId,
        challenge_id: ChallengeId,
    ) -> Result<(), ApplicationError> {
        let mut records = self.records.write().await;
        let Some(record) = records.get_mut(&operation_id) else {
            return Err(unknown_operation_error(operation_id));
        };
        if record.pending_challenge != Some(challenge_id) {
            return Err(ApplicationError::new(
                ApplicationErrorKind::Conflict,
                "challenge response does not match the operation's pending challenge",
            )
            .with_recoverability(Recoverability::UserAction)
            .with_operation_id(operation_id));
        }
        record.pending_challenge = None;
        Ok(())
    }

    /// Evicts the oldest terminal operations once the registry holds more
    /// than [`MAX_TERMINAL_HISTORY`] of them. Active (non-terminal)
    /// operations are never eviction candidates.
    async fn evict_excess_history(&self) {
        let mut records = self.records.write().await;
        let terminal_count = records
            .values()
            .filter(|record| is_terminal(&record.state))
            .count();
        if terminal_count <= MAX_TERMINAL_HISTORY {
            return;
        }
        let mut terminal_ids: Vec<OperationId> = records
            .iter()
            .filter(|(_, record)| is_terminal(&record.state))
            .map(|(operation_id, _)| *operation_id)
            .collect();
        terminal_ids.sort_by_key(|id| id.into_raw());
        for operation_id in terminal_ids
            .into_iter()
            .take(terminal_count - MAX_TERMINAL_HISTORY)
        {
            records.remove(&operation_id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::challenge::{Challenge, ChallengeResponse, SecretInput};
    use crate::event::OperationResult;

    #[tokio::test]
    async fn sequences_increase_monotonically_per_operation() {
        let registry = OperationRegistry::new();
        let (id, _cancel) = registry.begin(OperationKind::Extract).await;
        registry
            .transition(id, OperationState::Started)
            .await
            .unwrap();
        registry
            .transition(
                id,
                OperationState::Progress {
                    completed_units: 1,
                    total_units: Some(10),
                    message: None,
                },
            )
            .await
            .unwrap();

        let snapshot = registry.operation(id).await.unwrap();
        assert_eq!(snapshot.last_sequence, 3);
    }

    #[tokio::test]
    async fn each_operation_has_its_own_independent_sequence() {
        let registry = OperationRegistry::new();
        let (id_a, _cancel_a) = registry.begin(OperationKind::Extract).await;
        let (id_b, _cancel_b) = registry.begin(OperationKind::Convert).await;
        registry
            .transition(id_a, OperationState::Started)
            .await
            .unwrap();

        assert_eq!(registry.operation(id_a).await.unwrap().last_sequence, 2);
        assert_eq!(registry.operation(id_b).await.unwrap().last_sequence, 1);
    }

    #[tokio::test]
    async fn subscribers_fan_out_the_same_event() {
        let registry = OperationRegistry::new();
        let mut subscriber_a = registry.subscribe();
        let mut subscriber_b = registry.subscribe();

        let (id, _cancel) = registry.begin(OperationKind::Extract).await;

        let event_a = subscriber_a.recv().await.unwrap();
        let event_b = subscriber_b.recv().await.unwrap();
        assert_eq!(event_a.operation_id, id);
        assert_eq!(event_a, event_b);
    }

    #[tokio::test]
    async fn cancel_sets_the_operations_cancellation_flag() {
        let registry = OperationRegistry::new();
        let (id, cancel) = registry.begin(OperationKind::Extract).await;
        assert!(!cancel.load(Ordering::SeqCst));

        registry.cancel(id).await.unwrap();

        assert!(cancel.load(Ordering::SeqCst));
        assert!(registry.is_cancelled(id).await);
    }

    #[tokio::test]
    async fn cancel_transitions_a_non_terminal_operation_to_cancelled() {
        let registry = OperationRegistry::new();
        let (id, _cancel) = registry.begin(OperationKind::Extract).await;

        registry.cancel(id).await.unwrap();

        let snapshot = registry.operation(id).await.unwrap();
        assert_eq!(snapshot.state, OperationState::Cancelled);
    }

    #[tokio::test]
    async fn terminal_state_ignores_further_transitions() {
        let registry = OperationRegistry::new();
        let (id, _cancel) = registry.begin(OperationKind::Extract).await;
        registry
            .transition(
                id,
                OperationState::Completed {
                    result: OperationResult::None,
                },
            )
            .await
            .unwrap();
        let sequence_after_completion = registry.operation(id).await.unwrap().last_sequence;

        // Attempting to move a completed operation to `Cancelled` must not
        // overwrite the recorded terminal state or bump its sequence.
        registry
            .transition(id, OperationState::Cancelled)
            .await
            .unwrap();

        let snapshot = registry.operation(id).await.unwrap();
        assert_eq!(
            snapshot.state,
            OperationState::Completed {
                result: OperationResult::None
            }
        );
        assert_eq!(snapshot.last_sequence, sequence_after_completion);
    }

    /// The mechanism `archive_ops::run_open_archive`'s post-insertion
    /// cancellation guard depends on: if a concurrent `cancel` reaches
    /// `Cancelled` first, a worker's own later attempt to transition the
    /// *same* operation to `Completed` must silently no-op (already
    /// covered by `terminal_state_ignores_further_transitions`, the
    /// reverse ordering) -- and, critically, a caller that did not
    /// itself observe which side won can read the record back afterward
    /// and get a definitive, race-free answer: whichever terminal state
    /// is actually recorded is the one that stuck, because a terminal
    /// state never changes once set.
    #[tokio::test]
    async fn cancelling_before_a_racing_completion_attempt_leaves_the_operation_cancelled() {
        let registry = OperationRegistry::new();
        let (id, _cancel) = registry.begin(OperationKind::OpenArchive).await;

        registry.cancel(id).await.unwrap();

        // Simulates a worker that had already passed its own last
        // cancellation check, performed the (in this case, archive-open)
        // work, and only now attempts to record `Completed` -- unaware
        // that a `cancel` call already won the race.
        registry
            .transition(
                id,
                OperationState::Completed {
                    result: OperationResult::None,
                },
            )
            .await
            .unwrap();

        let snapshot = registry.operation(id).await.unwrap();
        assert_eq!(
            snapshot.state,
            OperationState::Cancelled,
            "a worker's own Completed transition must not overwrite a Cancelled that already won"
        );
    }

    #[tokio::test]
    async fn cancelling_an_already_terminal_operation_is_an_idempotent_no_op() {
        let registry = OperationRegistry::new();
        let (id, _cancel) = registry.begin(OperationKind::Extract).await;
        registry
            .transition(
                id,
                OperationState::Completed {
                    result: OperationResult::None,
                },
            )
            .await
            .unwrap();

        // Must succeed rather than error, and must not disturb the
        // recorded `Completed` state.
        registry.cancel(id).await.unwrap();

        let snapshot = registry.operation(id).await.unwrap();
        assert_eq!(
            snapshot.state,
            OperationState::Completed {
                result: OperationResult::None
            }
        );
    }

    #[tokio::test]
    async fn challenge_transition_records_the_pending_challenge() {
        let registry = OperationRegistry::new();
        let (id, _cancel) = registry.begin(OperationKind::Extract).await;
        let challenge_id = ChallengeId::from_raw(42);
        let challenge = Challenge::Password {
            id: challenge_id,
            archive_name: "archive.zip".to_string(),
            attempt: 1,
        };

        registry
            .transition(id, OperationState::Challenge { challenge })
            .await
            .unwrap();

        registry.resolve_challenge(id, challenge_id).await.unwrap();
    }

    #[tokio::test]
    async fn resolving_a_challenge_with_the_wrong_id_is_rejected() {
        let registry = OperationRegistry::new();
        let (id, _cancel) = registry.begin(OperationKind::Extract).await;
        let challenge = Challenge::Password {
            id: ChallengeId::from_raw(1),
            archive_name: "archive.zip".to_string(),
            attempt: 1,
        };
        registry
            .transition(id, OperationState::Challenge { challenge })
            .await
            .unwrap();

        let err = registry
            .resolve_challenge(id, ChallengeId::from_raw(999))
            .await
            .unwrap_err();
        assert_eq!(err.kind, ApplicationErrorKind::Conflict);
    }

    #[tokio::test]
    async fn resolving_a_challenge_against_the_wrong_operation_is_rejected() {
        let registry = OperationRegistry::new();
        let (id_a, _cancel_a) = registry.begin(OperationKind::Extract).await;
        let (id_b, _cancel_b) = registry.begin(OperationKind::Convert).await;
        let challenge_id = ChallengeId::from_raw(7);
        let challenge = Challenge::Password {
            id: challenge_id,
            archive_name: "archive.zip".to_string(),
            attempt: 1,
        };
        registry
            .transition(id_a, OperationState::Challenge { challenge })
            .await
            .unwrap();

        // `id_b` has no pending challenge at all, let alone this one.
        let err = registry
            .resolve_challenge(id_b, challenge_id)
            .await
            .unwrap_err();
        assert_eq!(err.kind, ApplicationErrorKind::Conflict);
    }

    /// Finding: `serializing_every_operation_state_cannot_contain_the_supplied_password`
    /// in `tests/operation_registry.rs` defined a marker password but never
    /// routed it anywhere, so `!json.contains(MARKER)` held unconditionally
    /// -- the test could not fail regardless of the registry's behavior.
    /// This is the real replacement: it drives a full flow where a
    /// `Password` challenge is actually answered with a `ChallengeResponse`
    /// carrying the marker as its secret, captures every event published
    /// across the whole flow (subscribing before anything starts), every
    /// snapshot reachable afterward, and the response's own `Debug`
    /// rendering, and asserts the marker appears in none of it. A future
    /// change that echoed the response's secret into a `Challenge` field,
    /// an `OperationRecord`/`OperationSnapshot`, or a diagnostic would make
    /// this test fail; the previous version could not have caught that.
    #[tokio::test]
    async fn resolving_a_password_challenge_never_leaks_the_secret_into_events_or_snapshots() {
        const MARKER_PASSWORD: &str = "correct-horse-battery-staple";

        let registry = OperationRegistry::new();
        let mut subscriber = registry.subscribe(); // subscribed before anything starts

        let (id, _cancel) = registry.begin(OperationKind::OpenArchive).await;
        let challenge_id = ChallengeId::from_raw(1);
        let challenge = Challenge::Password {
            id: challenge_id,
            archive_name: "archive.zip".to_string(),
            attempt: 1,
        };
        registry
            .transition(id, OperationState::Challenge { challenge })
            .await
            .unwrap();

        // The caller's response carries the real secret. `resolve_challenge`
        // only checks the response's id against the pending challenge --
        // it never threads the response's payload into a `Challenge`, an
        // `OperationEvent`, or an `OperationSnapshot`.
        let response = ChallengeResponse::Password {
            id: challenge_id,
            value: SecretInput::new(MARKER_PASSWORD.to_string()),
        };
        let response_debug = format!("{response:?}");
        registry.resolve_challenge(id, response.id()).await.unwrap();

        registry
            .transition(id, OperationState::Started)
            .await
            .unwrap();
        registry
            .transition(
                id,
                OperationState::Completed {
                    result: OperationResult::None,
                },
            )
            .await
            .unwrap();

        // Every event actually published across the whole flow.
        let mut serialized_events = Vec::new();
        loop {
            match subscriber.try_recv() {
                Ok(event) => serialized_events.push(serde_json::to_string(&event).unwrap()),
                Err(broadcast::error::TryRecvError::Lagged(_)) => continue,
                Err(
                    broadcast::error::TryRecvError::Empty | broadcast::error::TryRecvError::Closed,
                ) => break,
            }
        }
        assert_eq!(serialized_events.len(), 4); // Accepted, Challenge, Started, Completed

        // Every snapshot reachable after the flow completes.
        let serialized_snapshot =
            serde_json::to_string(&registry.operation(id).await.unwrap()).unwrap();
        let serialized_recent = serde_json::to_string(&registry.recent(10).await).unwrap();

        for serialized_event in &serialized_events {
            assert!(
                !serialized_event.contains(MARKER_PASSWORD),
                "supplied password leaked into a published event: {serialized_event}"
            );
        }
        assert!(
            !serialized_snapshot.contains(MARKER_PASSWORD),
            "supplied password leaked into an operation() snapshot: {serialized_snapshot}"
        );
        assert!(
            !serialized_recent.contains(MARKER_PASSWORD),
            "supplied password leaked into a recent() snapshot: {serialized_recent}"
        );
        assert!(
            !response_debug.contains(MARKER_PASSWORD),
            "supplied password leaked into ChallengeResponse's Debug output: {response_debug}"
        );
    }

    #[tokio::test]
    async fn operation_lookup_before_during_and_after_the_none_terminal_result() {
        let registry = OperationRegistry::new();
        let (id, _cancel) = registry.begin(OperationKind::Extract).await;

        // Before: freshly accepted.
        assert_eq!(
            registry.operation(id).await.unwrap().state,
            OperationState::Accepted
        );

        // During: an in-progress state.
        registry
            .transition(id, OperationState::Started)
            .await
            .unwrap();
        assert_eq!(
            registry.operation(id).await.unwrap().state,
            OperationState::Started
        );

        // After: the only `OperationResult` variant that exists yet.
        registry
            .transition(
                id,
                OperationState::Completed {
                    result: OperationResult::None,
                },
            )
            .await
            .unwrap();
        assert_eq!(
            registry.operation(id).await.unwrap().state,
            OperationState::Completed {
                result: OperationResult::None
            }
        );
    }

    #[tokio::test]
    async fn unknown_operation_id_is_not_found() {
        let registry = OperationRegistry::new();
        assert!(registry
            .operation(OperationId::from_raw(999))
            .await
            .is_none());

        let err = registry
            .cancel(OperationId::from_raw(999))
            .await
            .unwrap_err();
        assert_eq!(err.kind, ApplicationErrorKind::NotFound);

        let err = registry
            .resolve_challenge(OperationId::from_raw(999), ChallengeId::from_raw(1))
            .await
            .unwrap_err();
        assert_eq!(err.kind, ApplicationErrorKind::NotFound);
    }

    #[tokio::test]
    async fn bounded_history_evicts_the_oldest_terminal_operations_only() {
        // MAX_TERMINAL_HISTORY == 2 under cfg(test).
        let registry = OperationRegistry::new();
        let (id_1, _cancel_1) = registry.begin(OperationKind::Extract).await;
        let (id_2, _cancel_2) = registry.begin(OperationKind::Extract).await;
        let (id_3, _cancel_3) = registry.begin(OperationKind::Extract).await;

        for id in [id_1, id_2, id_3] {
            registry
                .transition(
                    id,
                    OperationState::Completed {
                        result: OperationResult::None,
                    },
                )
                .await
                .unwrap();
        }

        // Oldest terminal operation evicted once the bound (2) is exceeded.
        assert!(registry.operation(id_1).await.is_none());
        assert!(registry.operation(id_2).await.is_some());
        assert!(registry.operation(id_3).await.is_some());
    }

    #[tokio::test]
    async fn active_operations_are_never_evicted_by_bounded_history() {
        // MAX_TERMINAL_HISTORY == 2 under cfg(test).
        let registry = OperationRegistry::new();
        let (active_id, _cancel) = registry.begin(OperationKind::Extract).await;

        // Push three terminal operations through -- more than
        // MAX_TERMINAL_HISTORY -- while `active_id` stays non-terminal the
        // whole time.
        for _ in 0..3 {
            let (id, _cancel) = registry.begin(OperationKind::Extract).await;
            registry
                .transition(
                    id,
                    OperationState::Completed {
                        result: OperationResult::None,
                    },
                )
                .await
                .unwrap();
        }

        assert!(registry.operation(active_id).await.is_some());
    }

    #[tokio::test]
    async fn recent_operations_returns_at_most_the_requested_limit() {
        let registry = OperationRegistry::new();
        for _ in 0..3 {
            registry.begin(OperationKind::Extract).await;
        }

        let recent = registry.recent(2).await;
        assert_eq!(recent.len(), 2);
    }

    #[tokio::test]
    async fn recent_operations_orders_most_recently_created_first() {
        let registry = OperationRegistry::new();
        let (_id_1, _cancel_1) = registry.begin(OperationKind::Extract).await;
        let (id_2, _cancel_2) = registry.begin(OperationKind::Convert).await;

        let recent = registry.recent(1).await;
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].operation_id, id_2);
    }

    #[tokio::test]
    async fn a_lagging_subscriber_receives_tokios_explicit_lag_error() {
        let registry = OperationRegistry::new();
        let mut subscriber = registry.subscribe();

        let (id, _cancel) = registry.begin(OperationKind::Extract).await; // sequence 1

        // Publish enough events, without the subscriber ever reading, to
        // overflow EVENT_CHANNEL_CAPACITY (600 under cfg(test)) and force a
        // lag.
        let overflowing_transitions = EVENT_CHANNEL_CAPACITY + 10;
        for completed_units in 0..overflowing_transitions {
            registry
                .transition(
                    id,
                    OperationState::Progress {
                        completed_units: completed_units as u64,
                        total_units: None,
                        message: None,
                    },
                )
                .await
                .unwrap();
        }

        let result = subscriber.recv().await;
        assert!(matches!(
            result,
            Err(broadcast::error::RecvError::Lagged(_))
        ));

        // Even though the subscriber lagged, the registry's own record was
        // never touched by channel capacity -- it still reflects the true
        // current state.
        let snapshot = registry.operation(id).await.unwrap();
        let expected_last_sequence = 1 + overflowing_transitions as u64;
        assert_eq!(snapshot.last_sequence, expected_last_sequence);
        assert_eq!(
            snapshot.state,
            OperationState::Progress {
                completed_units: (overflowing_transitions - 1) as u64,
                total_units: None,
                message: None,
            }
        );
    }

    /// Finding: concurrent `transition` calls on the *same* `operation_id`
    /// could previously deliver their broadcasts out of order relative to
    /// their assigned sequence numbers, because the write-lock guard was
    /// dropped before `publish` ran -- two transitions could each finish
    /// mutating the record (and releasing the lock) before either one
    /// reached `self.events.send`, so whichever thread's `send` happened to
    /// run first "won", independent of which one actually incremented
    /// `last_sequence` first. A subscriber could then observe, for example,
    /// sequence 4 before sequence 3 for one operation, contradicting the
    /// ordering `OperationEvent` documents.
    ///
    /// This test spawns many concurrent tasks hammering `transition` on one
    /// `operation_id` from a real multi-thread runtime and asserts every
    /// sequence number the subscriber observes for that operation strictly
    /// increases. It is deterministic-by-construction post-fix (the write
    /// lock now serializes "mutate" and "publish" into one atomic step, so
    /// there is no window left for reordering), not merely probable: see
    /// the fix report for confirmation this test reliably FAILS against the
    /// pre-fix code (lock released before `publish`) and reliably PASSES
    /// against the fixed code, across repeated runs.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_transitions_on_one_operation_never_reorder_sequences_for_subscribers() {
        const CONCURRENT_TASKS: usize = 8;
        const TRANSITIONS_PER_TASK: usize = 60;

        let registry = Arc::new(OperationRegistry::new());
        let mut subscriber = registry.subscribe();
        let (id, _cancel) = registry.begin(OperationKind::Extract).await;

        let mut handles = Vec::with_capacity(CONCURRENT_TASKS);
        for task_index in 0..CONCURRENT_TASKS {
            let registry = Arc::clone(&registry);
            handles.push(tokio::spawn(async move {
                for step in 0..TRANSITIONS_PER_TASK {
                    registry
                        .transition(
                            id,
                            OperationState::Progress {
                                completed_units: (task_index * TRANSITIONS_PER_TASK + step) as u64,
                                total_units: None,
                                message: None,
                            },
                        )
                        .await
                        .unwrap();
                }
            }));
        }
        for handle in handles {
            handle.await.unwrap();
        }

        let mut observed_sequences = Vec::new();
        loop {
            match subscriber.try_recv() {
                Ok(event) => {
                    assert_eq!(event.operation_id, id);
                    observed_sequences.push(event.sequence);
                }
                Err(broadcast::error::TryRecvError::Lagged(_)) => continue,
                Err(
                    broadcast::error::TryRecvError::Empty | broadcast::error::TryRecvError::Closed,
                ) => break,
            }
        }

        // EVENT_CHANNEL_CAPACITY (600) comfortably covers the 1 + 8*60 = 481
        // events this test publishes, so no lag is expected here at all --
        // if this fails, the capacity assumption above needs revisiting.
        assert_eq!(
            observed_sequences.len(),
            1 + CONCURRENT_TASKS * TRANSITIONS_PER_TASK
        );

        for pair in observed_sequences.windows(2) {
            assert!(
                pair[0] < pair[1],
                "sequences observed out of order for one operation_id: {observed_sequences:?}"
            );
        }
    }
}
