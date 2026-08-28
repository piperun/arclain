//! The shared error envelope every fallible facade method returns.

use crate::ids::{ArchiveSessionId, CorrelationId, EntryId, OperationId};

/// Coarse-grained classification of what went wrong. Later tasks match on
/// this to decide how a frontend should react -- for example, prompting
/// for a password on `PasswordRequired` rather than showing a generic
/// failure dialog.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ApplicationErrorKind {
    Backend,
    Busy,
    Cancelled,
    Conflict,
    ExternalToolMissing,
    Internal,
    InvalidInput,
    NotFound,
    PasswordRequired,
    PermissionDenied,
    Persistence,
    Plugin,
    Unsupported,
}

/// Whether, and how, an operation might succeed if attempted again.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Recoverability {
    Retry,
    UserAction,
    Fatal,
}

/// A hint a frontend can render directly ("Choose a different
/// destination", "Supply a password") without parsing `diagnostic` prose.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SuggestedAction {
    CheckPermissions,
    ChooseDestination,
    InstallExternalTool,
    Retry,
    SupplyPassword,
}

/// The single error type every facade method returns.
///
/// All fields are public: later tasks and call sites across the facade
/// build these directly as well as through the `with_*` helpers below.
/// Construction still goes through [`ApplicationError::new`] plus the
/// `with_*` methods whenever the value needs anything more than a plain
/// literal, because that is where the invariants documented per field are
/// actually enforced (bounding `diagnostic`, generating a fresh
/// `correlation_id`, and so on).
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct ApplicationError {
    pub kind: ApplicationErrorKind,
    pub summary: String,
    pub diagnostic: Option<String>,
    pub recoverability: Recoverability,
    pub retryable: bool,
    pub suggested_action: Option<SuggestedAction>,
    pub correlation_id: CorrelationId,
    pub operation_id: Option<OperationId>,
    pub archive_session_id: Option<ArchiveSessionId>,
    pub entry_id: Option<EntryId>,
    pub path: Option<std::path::PathBuf>,
    pub field: Option<String>,
}

/// `diagnostic` never exceeds this many bytes once set through
/// [`ApplicationError::with_diagnostic`]. Diagnostics are meant for logs
/// and bug reports, not unbounded dumps of a backend error chain.
const MAX_DIAGNOSTIC_BYTES: usize = 4096;
const TRUNCATION_MARKER: &str = "... [truncated]";

/// Truncates `text` to at most [`MAX_DIAGNOSTIC_BYTES`] bytes, cutting on a
/// UTF-8 char boundary and leaving room for [`TRUNCATION_MARKER`] so the
/// returned string is visibly incomplete rather than silently cut off.
fn truncate_diagnostic(text: String) -> String {
    if text.len() <= MAX_DIAGNOSTIC_BYTES {
        return text;
    }
    let budget = MAX_DIAGNOSTIC_BYTES.saturating_sub(TRUNCATION_MARKER.len());
    let mut end = budget.min(text.len());
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    let mut truncated = String::with_capacity(end + TRUNCATION_MARKER.len());
    truncated.push_str(&text[..end]);
    truncated.push_str(TRUNCATION_MARKER);
    truncated
}

/// Best-effort redaction of filesystem-path-shaped tokens from free text.
/// Backend errors (I/O failures, external tool stderr) often embed a raw
/// OS path in their `Display` output; the dedicated, opt-in `path` field
/// is the vetted channel for exposing a path deliberately, so a free-text
/// diagnostic must not leak one a second time un-vetted -- including one
/// nested several `caused by:` levels deep in a wrapped backend error
/// chain.
///
/// This is a heuristic, not a guarantee: any whitespace-delimited token
/// containing `/` or `\` is replaced wholesale, which can occasionally
/// redact a non-path token (a fraction, a URL). That trade-off is
/// intentional -- over-redacting a diagnostic string is cheaper than
/// under-redacting one that leaks a username or directory layout.
fn redact_path_like_tokens(text: &str) -> String {
    text.split_whitespace()
        .map(|token| {
            if token.contains('/') || token.contains('\\') {
                "<redacted-path>"
            } else {
                token
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

impl ApplicationError {
    /// Starts a new error with conservative defaults: not retryable, not
    /// recoverable (`Fatal`), no suggested action, no attached context, and
    /// a fresh, process-unique `correlation_id`. Callers narrow these with
    /// the `with_*` methods.
    pub fn new(kind: ApplicationErrorKind, summary: impl Into<String>) -> Self {
        Self {
            kind,
            summary: summary.into(),
            diagnostic: None,
            recoverability: Recoverability::Fatal,
            retryable: false,
            suggested_action: None,
            correlation_id: CorrelationId::generate(),
            operation_id: None,
            archive_session_id: None,
            entry_id: None,
            path: None,
            field: None,
        }
    }

    /// Sets `diagnostic`, redacting path-like tokens (see
    /// [`redact_path_like_tokens`]) and truncating to 4 KiB (see
    /// [`truncate_diagnostic`]). Safe to call with a full backend error
    /// chain joined into one string.
    pub fn with_diagnostic(mut self, diagnostic: impl Into<String>) -> Self {
        let redacted = redact_path_like_tokens(&diagnostic.into());
        self.diagnostic = Some(truncate_diagnostic(redacted));
        self
    }

    /// Sets `recoverability`, and `retryable` with it.
    ///
    /// The two answer the same question, so they are one decision rather
    /// than two: `retryable` is true exactly when retrying on its own may
    /// succeed, which is what [`Recoverability::Retry`] means.
    /// [`Recoverability::UserAction`] is false because the caller has to
    /// do something first, and [`Recoverability::Fatal`] because nothing
    /// helps.
    ///
    /// They used to be set independently, and drifted apart at a quarter
    /// of the sites that set either: an envelope would tell a caller to
    /// retry through one field and not to through the other. The frontend
    /// that reads them worked around it by computing its own answer from
    /// `recoverability` and ignoring `retryable` entirely.
    pub fn with_recoverability(mut self, recoverability: Recoverability) -> Self {
        self.retryable = recoverability == Recoverability::Retry;
        self.recoverability = recoverability;
        self
    }

    pub fn with_suggested_action(mut self, suggested_action: SuggestedAction) -> Self {
        self.suggested_action = Some(suggested_action);
        self
    }

    pub fn with_operation_id(mut self, operation_id: OperationId) -> Self {
        self.operation_id = Some(operation_id);
        self
    }

    pub fn with_archive_session_id(mut self, archive_session_id: ArchiveSessionId) -> Self {
        self.archive_session_id = Some(archive_session_id);
        self
    }

    pub fn with_entry_id(mut self, entry_id: EntryId) -> Self {
        self.entry_id = Some(entry_id);
        self
    }

    /// Attaches a path. Callers are the safety boundary here: only call
    /// this with a path that is safe to display (for example, a
    /// user-chosen source or destination), never with an incidental
    /// internal path that might reveal more of the local filesystem than
    /// intended -- that is what `path` staying unset by default guards
    /// against.
    pub fn with_path(mut self, path: impl Into<std::path::PathBuf>) -> Self {
        self.path = Some(path.into());
        self
    }

    pub fn with_field(mut self, field: impl Into<String>) -> Self {
        self.field = Some(field.into());
        self
    }
}
