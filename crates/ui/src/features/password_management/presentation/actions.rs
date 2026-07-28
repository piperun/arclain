use std::path::PathBuf;

use crate::core::tabs::TabId;

/// A password submitted for a facade operation's pending `Challenge::
/// Password` (archive-open or extraction alike -- both share this
/// dialog, see `crate::core::tabs::PendingChallenge`'s own doc comment),
/// a cancellation of that same operation, or a password submitted for
/// the older "a single-file extraction needs a password `start_open_archive`
/// never asked for" trigger (`process_extraction_progress`'s own
/// password-dialog show, which sets `PasswordDialog::target_path` rather
/// than a `pending_challenge`) -- that one re-opens the archive with the
/// given password rather than answering an in-flight challenge.
pub enum PasswordFeatureAction {
    None,
    PasswordSubmitted {
        operation_id: arclain_app::ids::OperationId,
        challenge_id: arclain_app::ids::ChallengeId,
        password: String,
    },
    Cancelled {
        operation_id: arclain_app::ids::OperationId,
    },
    PasswordSubmittedForReopen {
        tab_id: TabId,
        path: PathBuf,
        password: String,
    },
}
