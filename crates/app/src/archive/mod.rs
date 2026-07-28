//! Frontend-neutral archive read model: the DTOs a frontend uses to list
//! and display the contents of an open archive, independent of which
//! archive format or which UI toolkit is on the other end.

mod session;
mod store;

pub(crate) use session::ArchiveSession;
pub(crate) use store::ArchiveSessionStore;

use crate::error::{ApplicationError, ApplicationErrorKind, Recoverability, SuggestedAction};
use crate::ids::{ArchiveSessionId, EntryId};

/// A slash-separated, archive-relative path. Always relative (never an
/// absolute filesystem path), always forward-slash-normalized regardless
/// of how it was typed, and never contains a `..` parent-traversal
/// segment: [`ArchivePath::parse`] enforces all three so a path taken from
/// an archive entry can be joined onto an extraction destination without
/// separately re-validating it for zip-slip-style escapes.
#[derive(
    Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, serde::Deserialize, serde::Serialize,
)]
#[serde(transparent)]
pub struct ArchivePath(String);

/// Builds the `ApplicationError` for a rejected [`ArchivePath::parse`]
/// input. All three rejection reasons share the same classification: the
/// caller passed a structurally invalid path string, and supplying a
/// different, valid one would succeed.
fn invalid_path_error(reason: &str) -> ApplicationError {
    ApplicationError::new(ApplicationErrorKind::InvalidInput, reason)
        .with_recoverability(Recoverability::UserAction)
        .with_suggested_action(SuggestedAction::ChooseDestination)
        .with_field("path")
}

/// Reports whether a backslash-normalized path string is absolute: a
/// leading `/` (also catches a normalized UNC path, since `\\server` would
/// have already become `//server`), or a Windows drive-letter prefix like
/// `C:`.
fn is_absolute(normalized: &str) -> bool {
    if normalized.starts_with('/') {
        return true;
    }
    let mut chars = normalized.chars();
    matches!(
        (chars.next(), chars.next()),
        (Some(drive), Some(':')) if drive.is_ascii_alphabetic()
    )
}

impl ArchivePath {
    /// The archive root: the empty path.
    pub fn root() -> Self {
        Self(String::new())
    }

    /// Parses and validates an archive-relative path.
    ///
    /// Backslashes are normalized to forward slashes so archive paths stay
    /// platform-neutral regardless of which OS produced the input. Rejects
    /// (as [`ApplicationErrorKind::InvalidInput`]): NUL bytes, absolute
    /// paths (Unix-style or a Windows drive letter), and any `..`
    /// parent-traversal segment. An empty string is accepted and equals
    /// [`ArchivePath::root`].
    pub fn parse(value: impl Into<String>) -> Result<Self, ApplicationError> {
        let raw = value.into();
        if raw.contains('\0') {
            return Err(invalid_path_error(
                "archive path must not contain NUL bytes",
            ));
        }
        let normalized = raw.replace('\\', "/");
        if is_absolute(&normalized) {
            return Err(invalid_path_error("archive path must be relative"));
        }
        if normalized.split('/').any(|segment| segment == "..") {
            return Err(invalid_path_error(
                "archive path must not contain parent traversal segments",
            ));
        }
        Ok(Self(normalized))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// What kind of filesystem object an archive entry represents.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryKind {
    Directory,
    File,
    Symlink,
}

/// One entry (file, directory, or symlink) inside an open archive.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct ArchiveEntryDto {
    pub id: EntryId,
    pub path: ArchivePath,
    pub name: String,
    pub kind: EntryKind,
    pub compressed_size: Option<u64>,
    pub uncompressed_size: u64,
    pub modified_at_unix_ms: Option<i64>,
    pub encrypted: bool,
    pub crc32: Option<String>,
}

/// A point-in-time summary of an open archive session. `revision`
/// increments every time the archive's contents change, so a frontend can
/// tell a cached [`EntryPage`] apart from one that reflects a newer state.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct ArchiveSnapshot {
    pub session_id: ArchiveSessionId,
    pub revision: u64,
    pub source_path: std::path::PathBuf,
    pub archive_type: String,
    pub entry_count: u64,
    pub total_uncompressed_size: u64,
    pub comment: Option<String>,
    pub metadata: Option<serde_json::Value>,
}

/// Which field [`ListEntriesRequest`] sorts by.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EntrySortKey {
    Compressed,
    Crc32,
    Encrypted,
    Kind,
    Modified,
    Name,
    Ratio,
    Size,
}

/// Ascending or descending, applied to whichever [`EntrySortKey`] is
/// active.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SortDirection {
    Ascending,
    Descending,
}

/// A request for one page of entries within a single directory of an open
/// archive.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct ListEntriesRequest {
    pub directory: ArchivePath,
    pub sort_key: EntrySortKey,
    pub sort_direction: SortDirection,
    pub name_filter: Option<String>,
    pub offset: u64,
    pub limit: u32,
}

/// One page of [`ArchiveEntryDto`] results for a [`ListEntriesRequest`],
/// alongside the `revision` it was computed against and the `total` count
/// matching the request (before paging).
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct EntryPage {
    pub session_id: ArchiveSessionId,
    pub revision: u64,
    pub directory: ArchivePath,
    pub total: u64,
    pub entries: Vec<ArchiveEntryDto>,
}

/// A request to open and index an archive, the argument to
/// [`crate::ArclainApp::start_open_archive`]. Not `Clone`/`Serialize`/
/// `Deserialize`: `password`, when present, carries a live
/// [`crate::challenge::SecretInput`], and those restrictions are
/// contagious by design (see `SecretInput`'s own doc comment).
#[derive(Debug)]
pub struct OpenArchiveRequest {
    pub source_path: std::path::PathBuf,
    pub password: Option<crate::challenge::SecretInput>,
}
