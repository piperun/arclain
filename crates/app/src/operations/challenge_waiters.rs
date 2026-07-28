//! Delivers a [`ChallengeResponse`]'s live payload (a password, a yes/no
//! confirmation) to whichever spawned operation task is waiting on it.
//!
//! [`OperationRegistry`](crate::operations::OperationRegistry) deliberately
//! tracks only *state* (see its module doc comment) -- it validates that a
//! `ChallengeResponse` answers the operation's actual pending challenge,
//! but never carries the response's payload itself (a `ChallengeResponse`
//! is not `Clone`/`Serialize` on purpose; nothing should be able to smuggle
//! a live secret into a broadcast event or a stored record). This module
//! is the other half: a `oneshot` channel per in-flight challenge, keyed by
//! the [`OperationId`] that raised it, so the task awaiting a password can
//! resume with the actual value the caller supplied.

use std::collections::HashMap;

use tokio::sync::oneshot;

use crate::challenge::ChallengeResponse;
use crate::error::{ApplicationError, ApplicationErrorKind, Recoverability};
use crate::ids::OperationId;

/// Registry of `oneshot` senders for operations currently parked on
/// [`ChallengeWaiters::wait`], keyed by the operation that raised the
/// challenge. At most one challenge is ever pending per operation (the
/// registry enforces that), so one slot per `OperationId` is sufficient.
#[derive(Default)]
pub(crate) struct ChallengeWaiters {
    senders: parking_lot::Mutex<HashMap<OperationId, oneshot::Sender<ChallengeResponse>>>,
}

fn no_pending_challenge_error(operation_id: OperationId) -> ApplicationError {
    ApplicationError::new(
        ApplicationErrorKind::Conflict,
        "operation has no pending challenge to respond to",
    )
    .with_recoverability(Recoverability::UserAction)
    .with_operation_id(operation_id)
}

impl ChallengeWaiters {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Registers a wait slot for `operation_id` and returns the receiver
    /// half. Called by the spawned operation task immediately before it
    /// transitions the operation to `OperationState::Challenge` (so a
    /// response can never arrive before a waiter exists for it).
    pub(crate) fn register(
        &self,
        operation_id: OperationId,
    ) -> oneshot::Receiver<ChallengeResponse> {
        let (sender, receiver) = oneshot::channel();
        self.senders.lock().insert(operation_id, sender);
        receiver
    }

    /// Delivers `response` to the task waiting on `operation_id`.
    /// Rejects with `Conflict` if no task is currently waiting (the
    /// operation never raised a challenge, already resolved one, or
    /// already finished) -- mirrors `OperationRegistry::resolve_challenge`'s
    /// own rejection shape for the equivalent "nothing to answer" case.
    pub(crate) fn respond(
        &self,
        operation_id: OperationId,
        response: ChallengeResponse,
    ) -> Result<(), ApplicationError> {
        let sender = self
            .senders
            .lock()
            .remove(&operation_id)
            .ok_or_else(|| no_pending_challenge_error(operation_id))?;
        // The receiver may already be gone if the waiting task was
        // cancelled between raising the challenge and this call -- that is
        // a race, not a bug, and is not this caller's problem to report:
        // the operation's own state (already `Cancelled`) is the source of
        // truth the caller should be consulting instead.
        let _ = sender.send(response);
        Ok(())
    }

    /// Removes any wait slot for `operation_id` without delivering a
    /// response -- called when a spawned task stops waiting for another
    /// reason (cancellation, an unrelated fatal error) so a later,
    /// unrelated `respond` call cannot resurrect a slot nobody is
    /// actually reading from anymore.
    pub(crate) fn cancel(&self, operation_id: OperationId) {
        self.senders.lock().remove(&operation_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::challenge::SecretInput;
    use crate::ids::ChallengeId;

    #[tokio::test]
    async fn a_registered_waiter_receives_the_delivered_response() {
        let waiters = ChallengeWaiters::new();
        let operation_id = OperationId::from_raw(1);
        let receiver = waiters.register(operation_id);

        waiters
            .respond(
                operation_id,
                ChallengeResponse::Password {
                    id: ChallengeId::from_raw(1),
                    value: SecretInput::new("hunter2".to_string()),
                },
            )
            .unwrap();

        let response = receiver.await.unwrap();
        match response {
            ChallengeResponse::Password { value, .. } => {
                assert_eq!(value.expose_secret(), "hunter2");
            }
            other => panic!("unexpected response variant: {other:?}"),
        }
    }

    #[tokio::test]
    async fn responding_with_no_registered_waiter_is_rejected() {
        let waiters = ChallengeWaiters::new();
        let error = waiters
            .respond(
                OperationId::from_raw(42),
                ChallengeResponse::Password {
                    id: ChallengeId::from_raw(1),
                    value: SecretInput::new("x".to_string()),
                },
            )
            .unwrap_err();
        assert_eq!(error.kind, ApplicationErrorKind::Conflict);
    }

    #[tokio::test]
    async fn responding_twice_to_the_same_operation_rejects_the_second_call() {
        let waiters = ChallengeWaiters::new();
        let operation_id = OperationId::from_raw(7);
        let _receiver = waiters.register(operation_id);

        waiters
            .respond(
                operation_id,
                ChallengeResponse::Password {
                    id: ChallengeId::from_raw(1),
                    value: SecretInput::new("first".to_string()),
                },
            )
            .unwrap();

        let second = waiters.respond(
            operation_id,
            ChallengeResponse::Password {
                id: ChallengeId::from_raw(2),
                value: SecretInput::new("second".to_string()),
            },
        );
        assert!(second.is_err());
    }

    #[test]
    fn cancel_removes_the_waiter_so_a_later_response_is_rejected() {
        let waiters = ChallengeWaiters::new();
        let operation_id = OperationId::from_raw(3);
        let _receiver = waiters.register(operation_id);

        waiters.cancel(operation_id);

        let error = waiters
            .respond(
                operation_id,
                ChallengeResponse::Password {
                    id: ChallengeId::from_raw(1),
                    value: SecretInput::new("x".to_string()),
                },
            )
            .unwrap_err();
        assert_eq!(error.kind, ApplicationErrorKind::Conflict);
    }
}
