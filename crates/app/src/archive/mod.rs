//! Frontend-neutral archive read model: the DTOs a frontend uses to list
//! and display the contents of an open archive, independent of which
//! archive format or which UI toolkit is on the other end.

pub mod multipart;
mod product_metadata;
mod session;
mod store;

pub use multipart::{detect_multipart, MultiPartArchiveDto, MultiPartFormat};
pub use product_metadata::{product_metadata_from_document, ProductMetadataSummary, ScreenshotRef};
pub(crate) use session::{ArchiveSession, SessionEncryption};
pub(crate) use store::ArchiveSessionStore;

use crate::error::{ApplicationError, ApplicationErrorKind, Recoverability, SuggestedAction};
use crate::ids::{ArchiveSessionId, EntryId};

/// Reports whether `path`'s file name ends in an extension this
/// application recognizes as an archive.
///
/// Pure and synchronous -- no app handle, no I/O, no check that `path`
/// exists. Exposed here so a frontend deciding "is this dropped/extracted
/// file something we should open as an archive?" asks the application
/// rather than keeping its own copy of the extension list: the list is a
/// single source of truth shared with the organization pipeline's own
/// nested-archive discovery, and a frontend-side copy would silently
/// diverge from it as formats are added.
pub fn is_archive_extension(path: &std::path::Path) -> bool {
    arclain_core::features::organization::flatten::is_archive_extension(path)
}

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
///
/// `Hash` (alongside the existing `Eq`) is needed so `EntryKind` can be
/// part of `crate::archive::session::EntryIdAssigner`'s map key -- see
/// its own doc comment for why an entry's identity is keyed on kind as
/// well as path.
#[derive(Clone, Debug, Eq, Hash, PartialEq, serde::Deserialize, serde::Serialize)]
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
///
/// `encrypted`/`headers_encrypted`/`encryption_method` are the archive-
/// level encryption facts the backend reported when the session was
/// opened (per-entry encryption is on each [`ArchiveEntryDto`] instead).
/// They describe the *open-time* listing and are not re-derived on later
/// reindexes: a mutation that changed them would need a reopen to be
/// reflected, matching how the pre-facade UI treated the same three
/// values.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct ArchiveSnapshot {
    pub session_id: ArchiveSessionId,
    pub revision: u64,
    pub source_path: std::path::PathBuf,
    pub archive_type: String,
    pub entry_count: u64,
    pub total_uncompressed_size: u64,
    pub encrypted: bool,
    pub headers_encrypted: bool,
    pub encryption_method: Option<String>,
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

/// How many entries a directory-scoped listing asks for when the caller
/// wants the whole directory rather than one window of it. A directory's
/// own entry count has no upper bound worth naming more precisely than
/// "effectively unbounded" -- see [`ListEntriesRequest::whole_directory`].
pub const ALL_ENTRIES_IN_ONE_DIRECTORY: u32 = u32::MAX;

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

impl ListEntriesRequest {
    /// The request for everything in one directory, in the read model's
    /// baseline order: name-ascending, unfiltered, from the first row,
    /// with no cap.
    ///
    /// The one place that shape is written down, for *every* frontend.
    /// Each caller that resolves a selection of paths back to
    /// [`EntryId`]s needs exactly this request (extraction,
    /// add/replace-text, delete, the CLI's path-to-id walk), and before
    /// it existed each spelled the shape out itself -- including one that
    /// capped `limit` at a literal `100_000` and therefore silently
    /// dropped every selected row past that in a larger directory.
    pub fn whole_directory(directory: ArchivePath) -> Self {
        Self {
            directory,
            sort_key: EntrySortKey::Name,
            sort_direction: SortDirection::Ascending,
            name_filter: None,
            offset: 0,
            limit: ALL_ENTRIES_IN_ONE_DIRECTORY,
        }
    }
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

/// Every entry of an open archive session -- files and directories, at
/// every depth -- alongside the `revision` the list was computed against.
///
/// The whole-archive counterpart to [`EntryPage`]: where a page answers
/// one directory of the tree, this is the tree itself, for the consumers
/// that genuinely need all of it at once (a folder-tree panel's directory
/// set, whole-archive aggregate totals, a drag-out's recursive folder
/// expansion, a plugin event's entry snapshot). Entries are in
/// depth-first tree order: each directory's children in the same
/// name-sorted order [`crate::ArclainApp::list_entries`] uses as its
/// baseline, parents before their contents.
///
/// Every row's [`EntryId`] is minted and owned by the session -- a
/// frontend must never fabricate one, and ids taken from a superseded
/// `revision` resolve to nothing rather than to some other entry.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct ArchiveInventory {
    pub session_id: ArchiveSessionId,
    pub revision: u64,
    pub entries: Vec<ArchiveEntryDto>,
}

/// What one [`crate::ArclainApp::backfill_encrypted_crcs`] call did and
/// found -- everything a frontend's post-open choreography needs in one
/// answer.
///
/// * `computed` -- how many file rows had a CRC-32 filled in. Non-zero
///   means the session's rows changed and `revision` was bumped, so a
///   frontend should refetch whatever listing it holds.
/// * `revision` -- the session revision the answer describes (bumped
///   when `computed > 0`, unchanged otherwise).
/// * `password_available` -- whether any password was in hand: the one
///   the session was opened with, or one a stored password rule matched
///   (against the archive's name or its own entry paths). `false` with
///   `any_encrypted` true is the state a `prompt_on_open` policy turns
///   into a password prompt.
/// * `any_encrypted` -- whether any entry in the session is flagged
///   encrypted at all.
#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct EncryptedCrcBackfill {
    pub computed: u64,
    pub revision: u64,
    pub password_available: bool,
    pub any_encrypted: bool,
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
