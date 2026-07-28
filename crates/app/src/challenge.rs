//! The password/confirmation prompts an in-flight operation can raise, the
//! caller's answer to one, and the secret-carrying container that answer
//! is built from.

use std::sync::atomic::{AtomicU64, Ordering};

use crate::ids::ChallengeId;

/// Mints a fresh, process-wide-unique [`ChallengeId`] for a challenge an
/// in-flight operation raises. The one minting point for every
/// `ChallengeId`, shared by every challenge-raising operation kind
/// (`crate::runtime::archive_ops`, `crate::operations::extract`) --
/// mirrors `crate::operations::registry::next_operation_id`'s exact
/// pattern (a function-local atomic counter, not a store), centralized
/// here once a second challenge-raising operation kind actually existed
/// to justify it (see `archive_ops::next_challenge_id`'s predecessor
/// comment, which anticipated exactly this).
pub(crate) fn next_challenge_id() -> ChallengeId {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    ChallengeId::from_raw(NEXT.fetch_add(1, Ordering::Relaxed))
}

/// A single secret value (an archive or vault password) supplied by a
/// caller, held only long enough to hand to the backend that needs it.
///
/// Deliberately not `Clone`, not `serde::Serialize`, and not
/// `serde::Deserialize` (proven by the compile-fail cases under
/// `crates/app/tests/ui/secret_input_*.rs`): nothing should be able to
/// duplicate this value into a second heap allocation zeroize does not
/// know about, persist it to disk, or serialize it into a bridge/log
/// payload by accident. The only way to read the value back out is
/// [`SecretInput::expose_secret`], a deliberate, grep-able call site.
pub struct SecretInput(zeroize::Zeroizing<String>);

impl SecretInput {
    /// Takes ownership of `value`; the backing buffer is zeroized on drop.
    pub fn new(value: String) -> Self {
        Self(zeroize::Zeroizing::new(value))
    }

    /// Reads the secret. Named so every call site reads as a deliberate
    /// decision to handle a live secret, not an incidental field access.
    pub fn expose_secret(&self) -> &str {
        &self.0
    }
}

/// Hand-written rather than `#[derive(Debug)]`: `zeroize::Zeroizing<Z>`
/// derives `Debug` by forwarding to `Z`'s own impl, which for `String`
/// prints the plaintext. Deriving here would print the real secret the
/// first time a containing struct is logged with `{:?}` (for example, an
/// operation request logged for diagnostics), silently defeating the rest
/// of this type's protections.
impl std::fmt::Debug for SecretInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("SecretInput").field(&"[REDACTED]").finish()
    }
}

/// A prompt an in-flight operation raises when it needs input from a human
/// (or a caller acting on one's behalf) before it can continue. Carried by
/// [`crate::event::OperationState::Challenge`]; safe to log or send across
/// a bridge in full, since no variant carries a secret value -- only
/// [`ChallengeResponse::Password`] does, and that type stays off the event
/// stream entirely.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Challenge {
    Password {
        id: ChallengeId,
        archive_name: String,
        attempt: u32,
    },
    ConfirmOverwrite {
        id: ChallengeId,
        destination: std::path::PathBuf,
    },
    ConfirmDestructiveAction {
        id: ChallengeId,
        summary: String,
    },
    MissingExternalTool {
        id: ChallengeId,
        tool: String,
    },
    RetryPermission {
        id: ChallengeId,
        path: std::path::PathBuf,
    },
}

impl Challenge {
    /// The id every variant carries, so a caller that only needs to
    /// correlate a challenge with its eventual response does not have to
    /// match on all five variants itself.
    pub fn id(&self) -> ChallengeId {
        match self {
            Challenge::Password { id, .. }
            | Challenge::ConfirmOverwrite { id, .. }
            | Challenge::ConfirmDestructiveAction { id, .. }
            | Challenge::MissingExternalTool { id, .. }
            | Challenge::RetryPermission { id, .. } => *id,
        }
    }
}

/// A caller's answer to a [`Challenge`]. Not `Clone`, `Serialize`, or
/// `Deserialize`: the `Password` variant carries a live [`SecretInput`],
/// and those restrictions are contagious on purpose -- a `ChallengeResponse`
/// must be consumed immediately by whichever facade method receives it,
/// never queued, cloned, logged, or persisted.
#[derive(Debug)]
pub enum ChallengeResponse {
    Password { id: ChallengeId, value: SecretInput },
    ConfirmOverwrite { id: ChallengeId, overwrite: bool },
    ConfirmDestructiveAction { id: ChallengeId, confirmed: bool },
    MissingExternalTool { id: ChallengeId, retry: bool },
    RetryPermission { id: ChallengeId, retry: bool },
}

impl ChallengeResponse {
    /// The id every variant carries; mirrors [`Challenge::id`] so a caller
    /// can check a response matches the challenge it is answering without
    /// matching on all five variants itself.
    pub fn id(&self) -> ChallengeId {
        match self {
            ChallengeResponse::Password { id, .. }
            | ChallengeResponse::ConfirmOverwrite { id, .. }
            | ChallengeResponse::ConfirmDestructiveAction { id, .. }
            | ChallengeResponse::MissingExternalTool { id, .. }
            | ChallengeResponse::RetryPermission { id, .. } => *id,
        }
    }
}
